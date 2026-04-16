"""LibraryWriteEvent and ConversationMessage betterproto definitions.

Re-exports from the canonical generated bindings at
``wheelhouse._proto.wheelhouse.librarian.v1`` so that all code
referencing ``wheelhouse.librarian.proto.LibraryWriteEvent`` gets the
same class identity as ``wheelhouse.types.LibraryWriteEvent``.
"""

from wheelhouse._proto.wheelhouse.librarian.v1 import (
    ConversationMessage,
    LibraryWriteEvent,
)

__all__ = ["ConversationMessage", "LibraryWriteEvent"]
