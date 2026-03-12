"""Canonical type import path for Wheelhouse types (architecture decision).

Import types from here:
    from wheelhouse.types import TextMessage

Custom types registered via @register_type are also accessible here after registration.
"""

from __future__ import annotations

from typing import Any


class TypedMessage:
    """Received message — known type is deserialized, unknown type has raw bytes.

    Per AC #2: if the receiver knows the type, it gets a deserialized instance.
    If the receiver does not know the type, it gets raw bytes + type name string.
    Never a silent failure or crash.
    """

    def __init__(
        self,
        type_name: str,
        data: Any | None = None,
        raw_bytes: bytes | None = None,
        is_known: bool = False,
    ):
        self.type_name = type_name
        self.data = data
        self.raw_bytes = raw_bytes
        self.is_known = is_known

    @classmethod
    def known(cls, type_name: str, data: Any) -> "TypedMessage":
        """Create a message with a known, deserialized type."""
        return cls(type_name=type_name, data=data, is_known=True)

    @classmethod
    def unknown(cls, type_name: str, raw_bytes: bytes) -> "TypedMessage":
        """Create a message with an unknown type — raw bytes + type name (AC #2)."""
        return cls(type_name=type_name, raw_bytes=raw_bytes, is_known=False)
