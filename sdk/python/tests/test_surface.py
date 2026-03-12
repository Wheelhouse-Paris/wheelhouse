"""Acceptance tests for Surface base class.

These tests are in RED phase: they MUST fail until implementation exists.
"""

import pytest


class TestSurface:
    """Surface wraps a connection with publish/subscribe helpers."""

    def test_surface_importable_from_init(self):
        """Given the wheelhouse package,
        When I import Surface from wheelhouse,
        Then the import succeeds.
        """
        from wheelhouse import Surface

        assert Surface is not None

    def test_surface_is_class(self):
        """Given Surface,
        Then it is a class that can be subclassed.
        """
        from wheelhouse import Surface

        class MySurface(Surface):
            pass

        assert issubclass(MySurface, Surface)

    @pytest.mark.asyncio
    async def test_surface_has_publish_confirmed(self):
        """Given a Surface instance,
        Then it has publish_confirmed() method (WW-02).
        """
        from wheelhouse import Surface

        surface = Surface.__new__(Surface)
        assert hasattr(surface, "publish_confirmed")
        assert callable(surface.publish_confirmed)


class TestModuleExports:
    """Verify thin __init__.py exports only connect() and Surface."""

    def test_init_exports_connect(self):
        """wheelhouse.__init__ exports connect()."""
        import wheelhouse

        assert hasattr(wheelhouse, "connect")
        assert callable(wheelhouse.connect)

    def test_init_exports_surface(self):
        """wheelhouse.__init__ exports Surface."""
        import wheelhouse

        assert hasattr(wheelhouse, "Surface")

    def test_init_does_not_export_errors(self):
        """wheelhouse.__init__ does NOT re-export errors (MA-02)."""
        import wheelhouse

        assert not hasattr(wheelhouse, "ConnectionError")
        assert not hasattr(wheelhouse, "PublishTimeout")
        assert not hasattr(wheelhouse, "StreamNotFound")

    def test_init_does_not_export_internals(self):
        """wheelhouse.__all__ does NOT include internal modules."""
        import wheelhouse

        assert "_core" not in wheelhouse.__all__
        assert "_proto" not in wheelhouse.__all__


class TestErrorImports:
    """Errors imported directly from wheelhouse.errors (MA-02)."""

    def test_import_connection_error(self):
        from wheelhouse.errors import ConnectionError as WhConnectionError

        assert WhConnectionError is not None

    def test_import_publish_timeout(self):
        from wheelhouse.errors import PublishTimeout

        assert PublishTimeout is not None

    def test_import_stream_not_found(self):
        from wheelhouse.errors import StreamNotFound

        assert StreamNotFound is not None


class TestTypesImport:
    """Types imported from wheelhouse.types."""

    def test_import_text_message(self):
        from wheelhouse.types import TextMessage

        assert TextMessage is not None


class TestTestingImportGuard:
    """testing.py has import guard — no ZMQ at module level."""

    def test_testing_module_importable_without_zmq(self):
        """Given the testing module,
        When imported,
        Then it does NOT import ZMQ at module level.
        """
        import importlib
        import sys

        # Remove zmq from sys.modules if present
        zmq_modules = [k for k in sys.modules if k.startswith("zmq")]
        saved = {}
        for k in zmq_modules:
            saved[k] = sys.modules.pop(k)

        try:
            if "wheelhouse.testing" in sys.modules:
                del sys.modules["wheelhouse.testing"]
            importlib.import_module("wheelhouse.testing")
            # zmq should NOT have been imported
            assert "zmq" not in sys.modules
        finally:
            sys.modules.update(saved)

    def test_mock_connection_available(self):
        """MockConnection is available from wheelhouse.testing."""
        from wheelhouse.testing import MockConnection

        assert MockConnection is not None
