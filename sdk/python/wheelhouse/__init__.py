"""Wheelhouse Python SDK — stream participant library.

This package provides the Python interface for publishing and subscribing
to Wheelhouse streams. It requires a running Wheelhouse instance.

Usage::

    import wheelhouse

    conn = wheelhouse.connect()
    conn.publish("my-stream", {"content": "hello"})
"""

__version__ = "0.1.0"

try:
    from wheelhouse._proto import wheelhouse as _proto_types  # noqa: F401
except ImportError:
    import warnings

    warnings.warn(
        "Wheelhouse message types not found. "
        "Run 'make proto-python' from the sdk/python directory to generate them. "
        "See CONTRIBUTING.md for setup instructions.",
        ImportWarning,
        stacklevel=2,
    )
