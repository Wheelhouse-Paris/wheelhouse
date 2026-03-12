"""Canonical type import path for Wheelhouse Protobuf types.

Usage:
    from wheelhouse.types import TextMessage

Note: Until proto/ files exist (Epic 1), this module provides stub
dataclass types that mirror the expected betterproto-generated interface.
Once proto generation is set up, these stubs will be replaced by
generated types from wheelhouse._proto.
"""

from __future__ import annotations

from dataclasses import dataclass, field


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
        # Temporary: JSON-like encoding. Real betterproto uses binary protobuf.
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
