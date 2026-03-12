"""Internal implementation module for Wheelhouse SDK (CF-04).

Do NOT import from this module directly — use `import wheelhouse` or `from wheelhouse import ...`.
"""

from __future__ import annotations

import base64
import logging
import os
import threading
import time
from typing import Any, Callable

from wheelhouse.errors import (
    ConnectionError,
    InvalidTypeNameError,
    RegistrationError,
    RegistryFullError,
    ReservedNamespaceError,
)

logger = logging.getLogger("wheelhouse")

# Global registry of types decorated with @register_type
_registered_types: dict[str, type] = {}

# Reconnect backoff constants (FM-13):
# backoff = min(5s, 100ms x 2^attempt) + random(0..100ms)
_RECONNECT_INITIAL_MS = 100
_RECONNECT_MULTIPLIER = 2
_RECONNECT_CAP_S = 5.0


def _validate_type_name(type_name: str) -> tuple[str, str]:
    """Validate and parse a fully-qualified type name.

    Returns (namespace, short_name) or raises.
    """
    if "." not in type_name:
        raise InvalidTypeNameError(
            f"Type name '{type_name}' must be in format '<namespace>.<TypeName>'",
            code="INVALID_TYPE_NAME",
        )

    dot_pos = type_name.index(".")
    namespace = type_name[:dot_pos]
    short_name = type_name[dot_pos + 1 :]

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


def register_type(type_name: str) -> Callable:
    """Decorator to register a custom Protobuf type with a namespace.

    Usage:
        @wheelhouse.register_type("biotech.MoleculeObject")
        class MoleculeObject:
            ...

    The type is validated immediately. Registration with the running Wheelhouse
    instance happens on connect() and is re-done automatically on reconnect (CM-07).
    """
    # Validate format immediately (fail fast)
    _validate_type_name(type_name)

    def decorator(cls: type) -> type:
        _registered_types[type_name] = cls
        cls._wh_type_name = type_name  # type: ignore[attr-defined]
        return cls

    return decorator


def _map_error_response(code: str, message: str) -> RegistrationError:
    """Map a control socket error code to a typed SDK exception."""
    if code == "RESERVED_NAMESPACE":
        return ReservedNamespaceError(message, code=code)
    elif code == "INVALID_TYPE_NAME":
        return InvalidTypeNameError(message, code=code)
    elif code == "REGISTRY_FULL":
        return RegistryFullError(message, code=code)
    else:
        return RegistrationError(message, code=code)


class Surface:
    """Surface connection handle — stream participant only (FP-04).

    Manages connection to Wheelhouse, type registration, and reconnection.
    Not thread-safe (FM-14) — each thread must call connect() independently.
    """

    def __init__(self, endpoint: str, mock: bool = False):
        self._endpoint = endpoint
        self._mock = mock
        self._connected = False
        self._control_socket: Any = None
        self._reconnect_attempt = 0
        self._lock = threading.Lock()

    def _connect_control_socket(self) -> None:
        """Connect to the broker control socket (ZMQ REQ)."""
        if self._mock:
            self._connected = True
            return

        try:
            import zmq

            ctx = zmq.Context.instance()
            self._control_socket = ctx.socket(zmq.REQ)
            self._control_socket.connect(self._endpoint)
            self._control_socket.setsockopt(zmq.RCVTIMEO, 5000)  # 5s timeout (CF-02)
            self._connected = True
            self._reconnect_attempt = 0
        except ImportError:
            raise ConnectionError(
                "pyzmq is required: pip install pyzmq",
                code="MISSING_DEPENDENCY",
            )
        except Exception as e:
            raise ConnectionError(
                f"Wheelhouse is not running or unreachable at {self._endpoint}",
                code="CONNECTION_ERROR",
            ) from e

    def _send_control(self, command: str, data: dict | None = None) -> dict:
        """Send a command to the control socket and return response."""
        if self._mock:
            return {"v": 1, "status": "ok", "data": data or {}}

        if self._control_socket is None:
            raise ConnectionError(
                "Not connected to Wheelhouse",
                code="NOT_CONNECTED",
            )

        request = {"v": 1, "command": command}
        if data is not None:
            request["data"] = data

        try:
            self._control_socket.send_json(request)
            response = self._control_socket.recv_json()
            return response
        except Exception as e:
            raise ConnectionError(
                f"Wheelhouse is not running or unreachable",
                code="CONNECTION_ERROR",
            ) from e

    def _register_all_types(self) -> None:
        """Register all @register_type-decorated types with the broker.

        Called on connect and on reconnect (CM-07).
        """
        for type_name, cls in _registered_types.items():
            descriptor_bytes = None
            if hasattr(cls, "DESCRIPTOR"):
                descriptor_bytes = base64.b64encode(cls.DESCRIPTOR).decode("ascii")

            data = {"type_name": type_name}
            if descriptor_bytes:
                data["descriptor_bytes"] = descriptor_bytes

            response = self._send_control("register_type", data)
            if response.get("status") == "error":
                code = response.get("code", "UNKNOWN_ERROR")
                message = response.get("message", "Registration failed")
                raise _map_error_response(code, message)

    def _reconnect(self) -> None:
        """Reconnect with exponential backoff (FM-13) and re-register types (CM-07)."""
        import random

        backoff_ms = min(
            _RECONNECT_CAP_S * 1000,
            _RECONNECT_INITIAL_MS * (_RECONNECT_MULTIPLIER ** self._reconnect_attempt),
        )
        jitter_ms = random.uniform(0, 100)
        wait_s = (backoff_ms + jitter_ms) / 1000.0

        logger.info(
            "Reconnecting to Wheelhouse in %.2fs (attempt %d)",
            wait_s,
            self._reconnect_attempt + 1,
        )
        time.sleep(wait_s)
        self._reconnect_attempt += 1

        try:
            self._connect_control_socket()
            self._register_all_types()
            logger.info("Reconnected and re-registered %d types", len(_registered_types))
        except Exception:
            logger.warning("Reconnect attempt %d failed", self._reconnect_attempt)
            raise

    @property
    def connected(self) -> bool:
        return self._connected

    def disconnect(self) -> None:
        """Disconnect from Wheelhouse."""
        if self._control_socket is not None:
            self._control_socket.close()
            self._control_socket = None
        self._connected = False


def connect(
    endpoint: str | None = None,
    mock: bool = False,
) -> Surface:
    """Connect to a running Wheelhouse instance.

    Args:
        endpoint: Wheelhouse control socket URL. Defaults to WH_URL env var (PF-01).
        mock: If True, use mock mode without a real connection (NFR-D4).

    Returns:
        Surface connection handle.

    Raises:
        ConnectionError: If Wheelhouse is not running or unreachable.
    """
    if endpoint is None:
        endpoint = os.environ.get("WH_URL", "tcp://127.0.0.1:5556")

    surface = Surface(endpoint=endpoint, mock=mock)
    surface._connect_control_socket()
    surface._register_all_types()

    return surface
