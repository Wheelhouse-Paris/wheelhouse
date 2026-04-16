"""Wheelhouse Librarian — autonomous knowledge persistence runtime.

Entry point: python -m wheelhouse.librarian

The librarian connects to the Wheelhouse broker, subscribes to end-of-turn
streams, and processes LibraryWriteEvent messages through the decision loop.
"""

__version__ = "0.1.0"
