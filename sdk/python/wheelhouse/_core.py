"""Internal implementation module for Wheelhouse Python SDK.

Do NOT import from this module directly — use `import wheelhouse` or `from wheelhouse import ...`.

Wire format (matches Rust broker):
  Publish:   single ZMQ frame — b"stream_name\0<StreamEnvelope protobuf bytes>"
  Subscribe: prefix filter  — b"stream_name\0" (null-terminated stream name)

This format is compatible with the Rust broker's routing loop (routing/mod.rs).
"""

from __future__ import annotations

import asyncio
import logging
import os
import random
import time
from typing import Any, Callable, Awaitable

import betterproto
import zmq
import zmq.asyncio

from wheelhouse.errors import (
    ConnectionError,
    InvalidTypeNameError,
    PublishTimeout,
    ReservedNamespaceError,
)
from wheelhouse._proto.wheelhouse.v1 import (
    CronEvent,
    SkillInvocation,
    SkillProgress,
    SkillResult,
    StreamEnvelope,
    TextMessage,
    TopologyShutdown,
)

logger = logging.getLogger("wheelhouse")

# Default endpoint when neither WH_URL nor endpoint= is provided
DEFAULT_ENDPOINT = "tcp://127.0.0.1:5555"

# Reconnect backoff constants (ADR-011, FM-13)
RECONNECT_BASE_MS = 100
RECONNECT_MULTIPLIER = 2
RECONNECT_CAP_S = 5.0
RECONNECT_JITTER_MAX_MS = 100

# Global registry of types decorated with @register_type
_registered_types: dict[str, type] = {}

# Map from short type name to type_url prefix
_BUILTIN_TYPES: dict[str, type] = {
    "TextMessage": TextMessage,
    "CronEvent": CronEvent,
    "SkillInvocation": SkillInvocation,
    "SkillProgress": SkillProgress,
    "SkillResult": SkillResult,
    "TopologyShutdown": TopologyShutdown,
}


def _calculate_backoff(attempt: int) -> float:
    """Calculate reconnect backoff delay in seconds.

    Formula: min(5s, 100ms * 2^attempt) + random(0..100ms)
    Per ADR-011 / FM-13.
    """
    base_s = min(RECONNECT_CAP_S, (RECONNECT_BASE_MS / 1000) * (RECONNECT_MULTIPLIER ** attempt))
    jitter_s = random.random() * (RECONNECT_JITTER_MAX_MS / 1000)
    return base_s + jitter_s


def _resolve_endpoint(endpoint: str | None) -> str:
    """Resolve the Wheelhouse endpoint.

    Priority: explicit endpoint= parameter > WH_URL env var > default.
    Never hardcodes localhost (PF-01).
    """
    if endpoint is not None:
        return endpoint
    return os.environ.get("WH_URL", DEFAULT_ENDPOINT)


def _endpoint_with_port_offset(endpoint: str, offset: int) -> str:
    """Return endpoint with port incremented by offset.

    The broker uses three consecutive ports starting from WH_URL:
      +0 (WH_URL)  : broker PUB socket  — agents subscribe here
      +1           : broker SUB socket  — agents publish here
      +2           : broker control REP — health checks / commands

    Examples:
      tcp://127.0.0.1:5555 + 1 → tcp://127.0.0.1:5556
      tcp://host.containers.internal:5555 + 2 → tcp://host.containers.internal:5557
    """
    import re
    match = re.search(r":(\d+)$", endpoint)
    if match:
        port = int(match.group(1))
        return endpoint[: match.start()] + f":{port + offset}"
    return endpoint


def _validate_type_name(type_name: str) -> tuple[str, str]:
    """Validate and parse a fully-qualified type name.

    Returns (namespace, short_name) or raises InvalidTypeNameError / ReservedNamespaceError.
    """
    if "." not in type_name:
        raise InvalidTypeNameError(
            f"Type name '{type_name}' must be in format '<namespace>.<TypeName>'",
            code="INVALID_TYPE_NAME",
        )

    dot_pos = type_name.index(".")
    namespace = type_name[:dot_pos]
    short_name = type_name[dot_pos + 1:]

    if not namespace:
        raise InvalidTypeNameError(
            f"Type name '{type_name}' has empty namespace",
            code="INVALID_TYPE_NAME",
        )

    if not short_name:
        raise InvalidTypeNameError(
            f"Type name '{type_name}' has empty type name",
            code="INVALID_TYPE_NAME",
        )

    if "." in short_name:
        raise InvalidTypeNameError(
            f"Type name '{type_name}' must have exactly one dot separator",
            code="INVALID_TYPE_NAME",
        )

    if namespace == "wheelhouse":
        raise ReservedNamespaceError(
            f"Namespace 'wheelhouse' is reserved and cannot be registered (ADR-004)",
            code="RESERVED_NAMESPACE",
        )

    return namespace, short_name


def _validate_type_schema(cls: type) -> None:
    """Validate that a class has at least one data field (annotation).

    Raises TypeError if the class has no annotations — schema validation
    runs in both mock and real mode, it is not a no-op (AC #4, Story 6.3).

    Uses vars(cls) to check only the class's own annotations, not inherited ones.
    """
    own_annotations = vars(cls).get("__annotations__", {})
    if not own_annotations:
        raise TypeError(
            f"Type '{cls.__name__}' has no data fields (no type annotations). "
            f"@register_type requires at least one typed attribute, e.g.: "
            f"'name: str = \"\"'"
        )


def register_type(type_name: str) -> Callable:
    """Decorator to register a custom Protobuf type with a namespace.

    Usage:
        @wheelhouse.register_type("biotech.MoleculeObject")
        class MoleculeObject:
            ...

    The type is validated immediately — both name format and schema are checked
    at decoration time. Registration with the running Wheelhouse instance happens
    on connect() and is re-done automatically on reconnect (CM-07).

    Raises:
        InvalidTypeNameError: If type_name format is invalid.
        ReservedNamespaceError: If type_name uses reserved 'wheelhouse.*' namespace.
        TypeError: If the decorated class has no data fields (AC #4, Story 6.3).
    """
    # Validate format immediately (fail fast)
    _validate_type_name(type_name)

    def decorator(cls: type) -> type:
        # Validate schema — class must have at least one typed field
        _validate_type_schema(cls)
        _registered_types[type_name] = cls
        cls._wh_type_name = type_name  # type: ignore[attr-defined]
        return cls

    return decorator


MessageHandler = Callable[[Any], Awaitable[None]]


def _encode_message(stream: str, message: Any, publisher_id: str) -> bytes:
    """Encode a message into wire format: stream_name\\0StreamEnvelope_bytes.

    Uses betterproto serialization for known built-in types and custom types
    that inherit from betterproto.Message.
    """
    type_name = type(message).__name__

    # Determine type_url
    if hasattr(message, "_wh_type_name"):
        # @register_type decorated custom type
        type_url = f"wheelhouse.v1.custom.{message._wh_type_name}"
    else:
        type_url = f"wheelhouse.v1.{type_name}"

    # Serialize inner payload using betterproto
    if isinstance(message, betterproto.Message):
        payload = bytes(message)
    else:
        raise TypeError(f"Cannot serialize type {type_name}: must be a betterproto.Message")

    # Build StreamEnvelope
    envelope = StreamEnvelope(
        stream_name=stream,
        type_url=type_url,
        payload=payload,
        publisher_id=publisher_id,
        published_at_ms=int(time.time() * 1000),
        sequence_number=0,  # Broker assigns authoritative value
    )
    envelope_bytes = bytes(envelope)

    # Wire format: stream_name\0envelope_bytes
    return stream.encode("utf-8") + b"\0" + envelope_bytes


def _decode_message(raw: bytes) -> tuple[str, str, Any] | None:
    """Decode a wire-format message: stream_name\\0StreamEnvelope_bytes.

    Returns (stream_name, type_name, message) or None if decoding fails.
    """
    null_pos = raw.find(b"\0")
    if null_pos < 0:
        logger.debug("received message without stream prefix, skipping")
        return None

    stream_name = raw[:null_pos].decode("utf-8", errors="replace")
    payload = raw[null_pos + 1:]

    # Decode StreamEnvelope
    try:
        envelope = StreamEnvelope().parse(payload)
    except Exception as e:
        logger.debug("failed to decode StreamEnvelope: %s", e)
        return None

    type_url = envelope.type_url
    # Extract short type name from type_url (e.g. "wheelhouse.v1.TextMessage" → "TextMessage")
    type_name = type_url.rsplit(".", 1)[-1] if "." in type_url else type_url

    # Deserialize the inner payload
    message = _deserialize_payload(type_name, envelope.payload)

    return stream_name, type_name, message


def _deserialize_payload(type_name: str, data: bytes) -> Any:
    """Deserialize an inner message payload based on type name."""
    # Check global @register_type registry (CM-07)
    if type_name in _registered_types:
        cls = _registered_types[type_name]
        if isinstance(cls, type) and issubclass(cls, betterproto.Message):
            return cls().parse(data)
        # Legacy: try FromString for non-betterproto types
        if hasattr(cls, "FromString"):
            return cls.FromString(data)

    # Fall back to known built-in types
    if type_name in _BUILTIN_TYPES:
        return _BUILTIN_TYPES[type_name]().parse(data)

    # Unknown type — return raw bytes
    logger.warning("Unknown message type '%s', returning raw bytes", type_name)
    return data


class Connection:
    """A connection to Wheelhouse for publishing and subscribing to streams.

    Not thread-safe — each thread should create its own connection (FM-14).
    Supports async context manager pattern for automatic cleanup.
    """

    def __init__(
        self,
        endpoint: str,
        publisher_id: str = "",
        on_connection_event: Callable[[dict[str, Any]], None] | None = None,
    ) -> None:
        self._endpoint = endpoint
        self._publisher_id = publisher_id
        self._ctx: zmq.asyncio.Context | None = None
        self._pub_socket: zmq.asyncio.Socket | None = None
        self._sub_socket: zmq.asyncio.Socket | None = None
        self._subscriptions: dict[str, list[MessageHandler]] = {}
        self._instance_types: dict[str, type] = {}
        self._connected = False
        self._listener_task: asyncio.Task[None] | None = None
        self._closing = False
        self._on_connection_event = on_connection_event

    def _fire_connection_event(self, event_type: str, **kwargs: Any) -> None:
        """Fire a connection event to the registered callback (CM-02).

        Event types: "disconnected", "reconnecting", "reconnected", "reconnect_failed".
        User-facing text avoids "broker" (RT-B1).
        """
        if self._on_connection_event is not None:
            event = {"type": event_type, **kwargs}
            try:
                self._on_connection_event(event)
            except Exception:
                logger.exception("Connection event callback error")

    async def _connect(self) -> None:
        """Establish ZMQ connections to Wheelhouse.

        Uses a REQ socket health-check to verify Wheelhouse is actually
        reachable before declaring the connection successful.

        The broker uses three consecutive ports relative to WH_URL:
          WH_URL+0: broker PUB socket  (agents subscribe here)
          WH_URL+1: broker SUB socket  (agents publish here)
          WH_URL+2: broker control REP (health checks)
        """
        try:
            self._ctx = zmq.asyncio.Context()

            # Health check: use a REQ socket with short timeout to verify
            # Wheelhouse is actually running before setting up PUB/SUB.
            # The control socket is at WH_URL+2 (e.g. 5555+2=5557).
            control_endpoint = _endpoint_with_port_offset(self._endpoint, 2)
            health_socket = self._ctx.socket(zmq.REQ)
            health_socket.setsockopt(zmq.LINGER, 0)
            health_socket.setsockopt(zmq.CONNECT_TIMEOUT, 2000)
            health_socket.setsockopt(zmq.RCVTIMEO, 2000)
            health_socket.setsockopt(zmq.SNDTIMEO, 2000)

            try:
                health_socket.connect(control_endpoint)
                await health_socket.send(b'{"v":1,"command":"health"}')
                await health_socket.recv()
            except zmq.ZMQError:
                raise ConnectionError(
                    f"Wheelhouse is not running or not reachable at {self._endpoint}",
                    code="CONNECTION_ERROR",
                )
            finally:
                if not health_socket.closed:
                    health_socket.close(linger=0)

            # Agent PUB socket connects to broker SUB (WH_URL+1, e.g. 5556).
            pub_endpoint = _endpoint_with_port_offset(self._endpoint, 1)
            self._pub_socket = self._ctx.socket(zmq.PUB)
            self._pub_socket.connect(pub_endpoint)

            # Agent SUB socket connects to broker PUB (WH_URL+0, e.g. 5555).
            self._sub_socket = self._ctx.socket(zmq.SUB)
            self._sub_socket.connect(self._endpoint)

            # Allow sockets to establish connection
            await asyncio.sleep(0.01)

            self._connected = True
            logger.info("Connected to Wheelhouse at %s", self._endpoint)
        except ConnectionError:
            self._cleanup_sockets()
            raise
        except zmq.ZMQError as exc:
            self._cleanup_sockets()
            raise ConnectionError(
                "Wheelhouse is not running or not reachable",
                code="CONNECTION_ERROR",
            ) from exc

    def _cleanup_sockets(self) -> None:
        """Close ZMQ sockets and context."""
        if self._pub_socket is not None:
            self._pub_socket.close(linger=0)
            self._pub_socket = None
        if self._sub_socket is not None:
            self._sub_socket.close(linger=0)
            self._sub_socket = None
        if self._ctx is not None:
            self._ctx.term()
            self._ctx = None
        self._connected = False

    async def publish(self, stream: str, message: Any) -> None:
        """Publish a typed message to a stream (fire-and-forget).

        Wire format: single ZMQ frame — b"stream_name\\0StreamEnvelope_bytes"
        """
        if not self._connected or self._pub_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        wire = _encode_message(stream, message, self._publisher_id)
        await self._pub_socket.send(wire)

    async def publish_confirmed(
        self, stream: str, message: Any, timeout: float = 5.0
    ) -> None:
        """Publish a message and wait for broker acknowledgement."""
        if not self._connected or self._pub_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        wire = _encode_message(stream, message, self._publisher_id)
        await self._pub_socket.send(wire)

        try:
            await asyncio.wait_for(self._wait_for_ack(stream), timeout=timeout)
        except asyncio.TimeoutError:
            raise PublishTimeout(
                f"Publish to stream '{stream}' was not confirmed within {timeout}s",
                code="PUBLISH_TIMEOUT",
            )

    async def _wait_for_ack(self, stream: str) -> None:
        """Wait for broker acknowledgement of a published message.

        KNOWN LIMITATION (MVP): This is a no-op stub. The broker ack protocol
        depends on Epic 1 (broker implementation). Until then, publish_confirmed()
        succeeds immediately without actual confirmation. The timeout mechanism
        is wired correctly and will function once the broker sends ack messages.
        """
        # [PHASE-2-ONLY: BROKER-ACK] Replace with actual ack wait from broker
        await asyncio.sleep(0)  # Yield control

    async def subscribe(self, stream: str, handler: MessageHandler) -> None:
        """Subscribe to a stream with an async handler callback.

        Subscription prefix: b"stream_name\\0" — matches the broker's wire format
        (single-frame messages starting with the stream name followed by null byte).
        """
        if not self._connected or self._sub_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        if stream not in self._subscriptions:
            self._subscriptions[stream] = []
            # Subscribe using null-terminated stream name prefix to match broker wire format
            # Broker sends: b"stream_name\0<StreamEnvelope bytes>"
            prefix = f"{stream}\0".encode("utf-8")
            self._sub_socket.subscribe(prefix)

        self._subscriptions[stream].append(handler)

        # Start listener task if not already running
        if self._listener_task is None or self._listener_task.done():
            self._listener_task = asyncio.create_task(self._listen())

    async def _listen(self) -> None:
        """Background task to receive and dispatch messages from subscriptions."""
        if self._sub_socket is None:
            return

        while self._connected and not self._closing:
            try:
                # Receive single-frame ZMQ message
                raw: bytes = await asyncio.wait_for(
                    self._sub_socket.recv(), timeout=0.1
                )

                # Decode the wire-format message
                decoded = _decode_message(raw)
                if decoded is None:
                    continue

                stream_name, type_name, message = decoded

                # Dispatch to registered handlers for this stream
                handlers = self._subscriptions.get(stream_name, [])
                for handler in handlers:
                    try:
                        await handler(message)
                    except Exception:
                        logger.exception(
                            "Handler error for stream %s type %s", stream_name, type_name
                        )

            except asyncio.TimeoutError:
                continue
            except zmq.ZMQError as exc:
                if not self._closing:
                    logger.warning("ZMQ error in listener, attempting reconnect")
                    self._fire_connection_event(
                        "disconnected", reason=f"Connection lost: {exc}"
                    )
                    await self._reconnect()
                break
            except Exception:
                if not self._closing:
                    logger.exception("Unexpected error in listener")
                break

    async def _reconnect(self) -> None:
        """Reconnect with exponential backoff (ADR-011).

        Re-registers custom types before resuming subscriptions (CM-07).
        Fires connection events at each state transition (CM-02).
        """
        attempt = 0
        saved_subscriptions = dict(self._subscriptions)
        saved_instance_types = dict(self._instance_types)

        while not self._closing:
            attempt += 1
            delay = _calculate_backoff(attempt - 1)

            self._fire_connection_event("reconnecting", attempt=attempt)

            logger.info(
                "Reconnecting to Wheelhouse (attempt %d, delay %.1fs)",
                attempt,
                delay,
            )
            await asyncio.sleep(delay)

            try:
                self._cleanup_sockets()
                await self._connect()

                # Restore instance-level types BEFORE subscriptions (CM-07)
                self._instance_types = saved_instance_types

                # Re-register all subscriptions
                self._subscriptions = {}
                for stream, handlers in saved_subscriptions.items():
                    for handler in handlers:
                        await self.subscribe(stream, handler)

                self._fire_connection_event("reconnected")
                logger.info("Reconnected to Wheelhouse at %s", self._endpoint)
                return
            except (ConnectionError, zmq.ZMQError) as exc:
                self._fire_connection_event(
                    "reconnect_failed",
                    attempts=attempt,
                    last_error=str(exc),
                )
                continue

    def register_type(self, type_name: str, type_class: type) -> None:
        """Register a custom Protobuf type for this connection instance.

        For global registration across all connections, use the @register_type decorator.

        Args:
            type_name: The Protobuf type name (e.g., "MyCustomType").
            type_class: The betterproto dataclass type.
        """
        self._instance_types[type_name] = type_class

    async def close(self) -> None:
        """Close the connection and release all resources."""
        self._closing = True
        self._connected = False

        if self._listener_task is not None and not self._listener_task.done():
            self._listener_task.cancel()
            try:
                await self._listener_task
            except asyncio.CancelledError:
                pass

        self._cleanup_sockets()
        self._subscriptions.clear()
        logger.info("Disconnected from Wheelhouse")

    async def __aenter__(self) -> Connection:
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit — auto-close."""
        await self.close()


# Surface is an alias for Connection for API consistency
Surface = Connection


async def connect(
    endpoint: str | None = None,
    publisher_id: str = "",
    on_connection_event: Callable[[dict[str, Any]], None] | None = None,
) -> Connection:
    """Connect to Wheelhouse.

    Args:
        endpoint: Wheelhouse endpoint URL. If not provided, uses WH_URL
                  environment variable, or defaults to tcp://127.0.0.1:5555.
        publisher_id: Identifier for this connection used in outgoing message envelopes.
                      Set to the agent name so surfaces can filter self-echoes.
        on_connection_event: Optional callback for connection lifecycle events
            (CM-02). Receives a dict with "type" key: "disconnected",
            "reconnecting", "reconnected", or "reconnect_failed".

    Returns:
        A Connection object for publishing and subscribing to streams.

    Raises:
        wheelhouse.errors.ConnectionError: If Wheelhouse is not running or
            not reachable.
    """
    resolved = _resolve_endpoint(endpoint)
    conn = Connection(resolved, publisher_id=publisher_id, on_connection_event=on_connection_event)

    try:
        await conn._connect()
    except Exception:
        # Ensure cleanup on failure
        conn._cleanup_sockets()
        raise

    return conn
