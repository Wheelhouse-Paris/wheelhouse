"""Wheelhouse Python SDK — stream participant only (FP-04).

Public API:
    connect()    — connect to a running Wheelhouse instance
    Surface      — surface connection handle
    register_type — decorator to register custom Protobuf types with a namespace
"""

from wheelhouse._core import Surface, connect, register_type

__all__ = ["connect", "Surface", "register_type"]
