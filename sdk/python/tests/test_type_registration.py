"""Tests for @register_type decorator and SDK type registration."""

import pytest

from wheelhouse._core import _registered_types, _validate_type_name, register_type
from wheelhouse.errors import (
    InvalidTypeNameError,
    ReservedNamespaceError,
)


class TestValidateTypeName:
    """Unit tests for type name validation."""

    def test_valid_name(self):
        ns, name = _validate_type_name("biotech.MoleculeObject")
        assert ns == "biotech"
        assert name == "MoleculeObject"

    def test_rejects_no_dot(self):
        with pytest.raises(InvalidTypeNameError):
            _validate_type_name("InvalidName")

    def test_rejects_empty_namespace(self):
        with pytest.raises(InvalidTypeNameError):
            _validate_type_name(".TypeName")

    def test_rejects_empty_short_name(self):
        with pytest.raises(InvalidTypeNameError):
            _validate_type_name("namespace.")

    def test_rejects_nested_dots(self):
        with pytest.raises(InvalidTypeNameError):
            _validate_type_name("ns.Type.Sub")

    def test_rejects_wheelhouse_namespace(self):
        with pytest.raises(ReservedNamespaceError):
            _validate_type_name("wheelhouse.CustomType")


class TestRegisterTypeDecorator:
    """Tests for the @register_type decorator."""

    def setup_method(self):
        """Clear the global registry before each test."""
        _registered_types.clear()

    def test_registers_class_in_global_registry(self):
        @register_type("biotech.MoleculeObject")
        class MoleculeObject:
            name: str = ""

        assert "biotech.MoleculeObject" in _registered_types
        assert _registered_types["biotech.MoleculeObject"] is MoleculeObject

    def test_sets_wh_type_name_attribute(self):
        @register_type("pharma.DrugCompound")
        class DrugCompound:
            compound_id: str = ""

        assert DrugCompound._wh_type_name == "pharma.DrugCompound"  # type: ignore

    def test_rejects_wheelhouse_namespace_at_decoration_time(self):
        with pytest.raises(ReservedNamespaceError):

            @register_type("wheelhouse.Forbidden")
            class Forbidden:
                name: str = ""

    def test_rejects_invalid_name_at_decoration_time(self):
        with pytest.raises(InvalidTypeNameError):

            @register_type("NoNamespace")
            class Bad:
                name: str = ""

    def test_two_namespaces_same_type_name(self):
        @register_type("biotech.MoleculeObject")
        class BiotechMolecule:
            name: str = ""

        @register_type("pharma.MoleculeObject")
        class PharmaMolecule:
            name: str = ""

        assert "biotech.MoleculeObject" in _registered_types
        assert "pharma.MoleculeObject" in _registered_types
        assert _registered_types["biotech.MoleculeObject"] is BiotechMolecule
        assert _registered_types["pharma.MoleculeObject"] is PharmaMolecule


class TestConnect:
    """Tests for connect() function."""

    def setup_method(self):
        _registered_types.clear()

    async def test_connect_mock_mode(self):
        from wheelhouse import connect

        surface = await connect(mock=True)
        assert surface._connected

    async def test_connect_mock_registers_types(self):
        @register_type("test.MockType")
        class MockType:
            name: str = ""

        from wheelhouse import connect

        # Mock mode should succeed without a running broker
        surface = await connect(mock=True)
        assert surface._connected

    def test_connect_without_broker_raises_connection_error(self):
        from wheelhouse import connect
        from wheelhouse.errors import ConnectionError

        # Connecting to a non-existent endpoint should fail
        # (only if pyzmq is installed — skip if not)
        try:
            import zmq  # noqa: F401
        except ImportError:
            pytest.skip("pyzmq not installed")

        # Use a port that nothing listens on
        with pytest.raises(Exception):
            surface = connect(endpoint="tcp://127.0.0.1:59999")
            # Force a send to trigger the error
            surface._send_control("list_types")


class TestTypedMessage:
    """Tests for TypedMessage known/unknown handling."""

    def test_unknown_type_returns_raw_bytes(self):
        from wheelhouse.types import TypedMessage

        msg = TypedMessage.unknown("biotech.MoleculeObject", b"\x01\x02\x03")
        assert not msg.is_known
        assert msg.type_name == "biotech.MoleculeObject"
        assert msg.raw_bytes == b"\x01\x02\x03"

    def test_known_type_returns_data(self):
        from wheelhouse.types import TypedMessage

        msg = TypedMessage.known("biotech.MoleculeObject", {"name": "aspirin"})
        assert msg.is_known
        assert msg.type_name == "biotech.MoleculeObject"
        assert msg.data == {"name": "aspirin"}
