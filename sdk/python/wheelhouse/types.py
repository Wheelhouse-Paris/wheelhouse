"""Canonical type import path for Wheelhouse Protobuf types.

Usage:
    from wheelhouse.types import TextMessage, TypedMessage

Note: Until proto/ files exist (Epic 1), TextMessage and FileMessage are stub
dataclass types that mirror the expected betterproto-generated interface.
TypedMessage is a permanent abstraction for received messages with unknown types.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class TextMessage:
    """A text message published to a stream.

    Mirrors the Protobuf TextMessage in wheelhouse.v1.core.
    """

    content: str = ""
    user_id: str = ""
    stream_name: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes.

        STUB: Uses simple encoding. Will be replaced by betterproto-generated
        binary Protobuf serialization once proto/ files exist (Epic 1).
        """
        import json
        return json.dumps({"content": self.content, "user_id": self.user_id, "stream_name": self.stream_name}).encode("utf-8")

    @classmethod
    def FromString(cls, data: bytes) -> TextMessage:
        """Deserialize from bytes.

        STUB: Uses simple encoding. Will be replaced by betterproto-generated
        binary Protobuf deserialization once proto/ files exist (Epic 1).
        """
        import json
        try:
            obj = json.loads(data.decode("utf-8"))
            return cls(
                content=obj.get("content", ""),
                user_id=obj.get("user_id", ""),
                stream_name=obj.get("stream_name", ""),
            )
        except (json.JSONDecodeError, UnicodeDecodeError):
            return cls(content=data.decode("utf-8", errors="replace"))


@dataclass
class FileMessage:
    """A file message published to a stream.

    Mirrors the Protobuf FileMessage in wheelhouse.v1.core.
    """

    filename: str = ""
    content: bytes = field(default_factory=bytes)
    mime_type: str = ""
    user_id: str = ""


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
