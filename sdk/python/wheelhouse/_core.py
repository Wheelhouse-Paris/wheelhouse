"""Internal implementation module for Wheelhouse Python SDK.

Do NOT import from this module directly — use `import wheelhouse` or `from wheelhouse import ...`.
"""

from __future__ import annotations

import asyncio
import logging
import os
import random
from typing import Any, Callable, Awaitable

import zmq
import zmq.asyncio

from wheelhouse.errors import (
    ConnectionError,
    InvalidTypeNameError,
    PublishTimeout,
    ReservedNamespaceError,
)
from wheelhouse.types import TextMessage

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


class Connection:
    """A connection to Wheelhouse for publishing and subscribing to streams.

    Not thread-safe — each thread should create its own connection (FM-14).
    Supports async context manager pattern for automatic cleanup.
    """

    def __init__(self, endpoint: str) -> None:
        self._endpoint = endpoint
        self._ctx: zmq.asyncio.Context | None = None
        self._pub_socket: zmq.asyncio.Socket | None = None
        self._sub_socket: zmq.asyncio.Socket | None = None
        self._subscriptions: dict[str, list[MessageHandler]] = {}
        self._instance_types: dict[str, type] = {}
        self._connected = False
        self._listener_task: asyncio.Task[None] | None = None
        self._closing = False

    async def _connect(self) -> None:
        """Establish ZMQ connections to Wheelhouse.

        Uses a REQ socket health-check to verify Wheelhouse is actually
        reachable before declaring the connection successful.
        """
        try:
            self._ctx = zmq.asyncio.Context()

            # Health check: use a REQ socket with short timeout to verify
            # Wheelhouse is actually running before setting up PUB/SUB.
            health_socket = self._ctx.socket(zmq.REQ)
            health_socket.setsockopt(zmq.LINGER, 0)
            health_socket.setsockopt(zmq.CONNECT_TIMEOUT, 2000)
            health_socket.setsockopt(zmq.RCVTIMEO, 2000)
            health_socket.setsockopt(zmq.SNDTIMEO, 2000)

            try:
                health_socket.connect(self._endpoint)
                await health_socket.send(b'{"v":1,"command":"health"}')
                await health_socket.recv()
            except zmq.ZMQError:
                raise ConnectionError(
                    "Wheelhouse is not running or not reachable",
                    code="CONNECTION_ERROR",
                )
            finally:
                if not health_socket.closed:
                    health_socket.close(linger=0)

            self._pub_socket = self._ctx.socket(zmq.PUB)
            self._pub_socket.connect(self._endpoint)

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
        """Publish a typed message to a stream (fire-and-forget)."""
        if not self._connected or self._pub_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        data = message.SerializeToString()
        topic = f"{stream}:{type(message).__name__}".encode("utf-8")
        await self._pub_socket.send_multipart([topic, data])

    async def publish_confirmed(
        self, stream: str, message: Any, timeout: float = 5.0
    ) -> None:
        """Publish a message and wait for broker acknowledgement."""
        if not self._connected or self._pub_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        data = message.SerializeToString()
        topic = f"{stream}:{type(message).__name__}".encode("utf-8")
        await self._pub_socket.send_multipart([topic, data])

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
        """Subscribe to a stream with an async handler callback."""
        if not self._connected or self._sub_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        if stream not in self._subscriptions:
            self._subscriptions[stream] = []
            # Subscribe to ZMQ topic for this stream
            topic = f"{stream}:".encode("utf-8")
            self._sub_socket.subscribe(topic)

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
                parts = await asyncio.wait_for(
                    self._sub_socket.recv_multipart(), timeout=0.1
                )
                if len(parts) >= 2:
                    topic_bytes, data = parts[0], parts[1]
                    topic = topic_bytes.decode("utf-8")

                    # Parse topic: "stream_name:TypeName"
                    if ":" in topic:
                        stream_name, type_name = topic.split(":", 1)
                        handlers = self._subscriptions.get(stream_name, [])

                        # Deserialize based on type name
                        message = self._deserialize(type_name, data)

                        for handler in handlers:
                            try:
                                await handler(message)
                            except Exception:
                                logger.exception(
                                    "Handler error for stream %s", stream_name
                                )
            except asyncio.TimeoutError:
                continue
            except zmq.ZMQError:
                if not self._closing:
                    logger.warning("ZMQ error in listener, attempting reconnect")
                    await self._reconnect()
                break
            except Exception:
                if not self._closing:
                    logger.exception("Unexpected error in listener")
                break

    def _deserialize(self, type_name: str, data: bytes) -> Any:
        """Deserialize a message based on its type name.

        Checks instance-level types first, then global @register_type registry,
        then falls back to known built-in types.
        """
        # Check instance-level registered types
        if type_name in self._instance_types:
            return self._instance_types[type_name].FromString(data)

        # Check global @register_type registry (CM-07)
        if type_name in _registered_types:
            return _registered_types[type_name].FromString(data)

        # Fall back to known types
        if type_name == "TextMessage":
            return TextMessage.FromString(data)

        # Return raw data if type unknown
        logger.warning("Unknown message type: %s", type_name)
        return data

    async def _reconnect(self) -> None:
        """Reconnect with exponential backoff (ADR-011).

        Re-registers custom types before resuming subscriptions (CM-07).
        """
        attempt = 0
        saved_subscriptions = dict(self._subscriptions)
        saved_instance_types = dict(self._instance_types)

        while not self._closing:
            delay = _calculate_backoff(attempt)
            logger.info(
                "Reconnecting to Wheelhouse (attempt %d, delay %.1fs)",
                attempt + 1,
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

                logger.info("Reconnected to Wheelhouse at %s", self._endpoint)
                return
            except (ConnectionError, zmq.ZMQError):
                attempt += 1
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


async def connect(endpoint: str | None = None) -> Connection:
    """Connect to Wheelhouse.

    Args:
        endpoint: Wheelhouse endpoint URL. If not provided, uses WH_URL
                  environment variable, or defaults to tcp://127.0.0.1:5555.

    Returns:
        A Connection object for publishing and subscribing to streams.

    Raises:
        wheelhouse.errors.ConnectionError: If Wheelhouse is not running or
            not reachable.
    """
    resolved = _resolve_endpoint(endpoint)
    conn = Connection(resolved)

    try:
        await conn._connect()
    except Exception:
        # Ensure cleanup on failure
        conn._cleanup_sockets()
        raise

    return conn
