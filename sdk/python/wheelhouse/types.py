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
    publisher: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes.

        STUB: Uses simple encoding. Will be replaced by betterproto-generated
        binary Protobuf serialization once proto/ files exist (Epic 1).
        """
        import json
        return json.dumps({
            "content": self.content,
            "user_id": self.user_id,
            "stream_name": self.stream_name,
            "publisher": self.publisher,
        }).encode("utf-8")

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
                publisher=obj.get("publisher", ""),
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


def _json_serialize(obj: dict[str, Any]) -> bytes:
    """Shared JSON serialization for stub types."""
    import json
    return json.dumps(obj).encode("utf-8")


def _json_deserialize(data: bytes) -> dict[str, Any]:
    """Shared JSON deserialization for stub types."""
    import json
    return json.loads(data.decode("utf-8"))


@dataclass
class CronEvent:
    """A cron job trigger event published to a stream.

    Mirrors the Protobuf CronEvent in wheelhouse.v1.core.
    """

    job_name: str = ""
    triggered_at: str = ""
    publisher: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes (STUB)."""
        return _json_serialize({
            "job_name": self.job_name,
            "triggered_at": self.triggered_at,
            "publisher": self.publisher,
        })

    @classmethod
    def FromString(cls, data: bytes) -> "CronEvent":
        """Deserialize from bytes (STUB)."""
        obj = _json_deserialize(data)
        return cls(
            job_name=obj.get("job_name", ""),
            triggered_at=obj.get("triggered_at", ""),
            publisher=obj.get("publisher", ""),
        )


@dataclass
class SkillInvocation:
    """A skill invocation request published to a stream.

    Mirrors the Protobuf SkillInvocation in wheelhouse.v1.core.
    """

    invocation_id: str = ""
    skill_name: str = ""
    input_payload: str = ""
    target_agent: str = ""
    publisher: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes (STUB)."""
        return _json_serialize({
            "invocation_id": self.invocation_id,
            "skill_name": self.skill_name,
            "input_payload": self.input_payload,
            "target_agent": self.target_agent,
            "publisher": self.publisher,
        })

    @classmethod
    def FromString(cls, data: bytes) -> "SkillInvocation":
        """Deserialize from bytes (STUB)."""
        obj = _json_deserialize(data)
        return cls(
            invocation_id=obj.get("invocation_id", ""),
            skill_name=obj.get("skill_name", ""),
            input_payload=obj.get("input_payload", ""),
            target_agent=obj.get("target_agent", ""),
            publisher=obj.get("publisher", ""),
        )


@dataclass
class SkillProgress:
    """A skill progress update published to a stream.

    Published within 2 seconds of receiving a SkillInvocation (AC-06).
    """

    invocation_id: str = ""
    status: str = ""
    message: str = ""
    publisher: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes (STUB)."""
        return _json_serialize({
            "invocation_id": self.invocation_id,
            "status": self.status,
            "message": self.message,
            "publisher": self.publisher,
        })

    @classmethod
    def FromString(cls, data: bytes) -> "SkillProgress":
        """Deserialize from bytes (STUB)."""
        obj = _json_deserialize(data)
        return cls(
            invocation_id=obj.get("invocation_id", ""),
            status=obj.get("status", ""),
            message=obj.get("message", ""),
            publisher=obj.get("publisher", ""),
        )


@dataclass
class SkillResult:
    """A skill result published to a stream after Claude API completion.

    Published after a SkillInvocation is processed (Story 8.3).
    """

    invocation_id: str = ""
    success: bool = False
    payload: str = ""
    publisher: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes (STUB)."""
        return _json_serialize({
            "invocation_id": self.invocation_id,
            "success": self.success,
            "payload": self.payload,
            "publisher": self.publisher,
        })

    @classmethod
    def FromString(cls, data: bytes) -> "SkillResult":
        """Deserialize from bytes (STUB)."""
        obj = _json_deserialize(data)
        return cls(
            invocation_id=obj.get("invocation_id", ""),
            success=obj.get("success", False),
            payload=obj.get("payload", ""),
            publisher=obj.get("publisher", ""),
        )


@dataclass
class TopologyShutdown:
    """A topology shutdown event.

    Signals the agent to drain in-flight calls and disconnect gracefully (ADR-020).
    """

    reason: str = ""

    def SerializeToString(self) -> bytes:
        """Serialize to bytes (STUB)."""
        return _json_serialize({"reason": self.reason})

    @classmethod
    def FromString(cls, data: bytes) -> "TopologyShutdown":
        """Deserialize from bytes (STUB)."""
        obj = _json_deserialize(data)
        return cls(reason=obj.get("reason", ""))
