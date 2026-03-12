"""
Acceptance tests for Story 6.1: Python SDK Custom Protobuf Type Registration

These tests verify the Python SDK @register_type decorator and auto-reconnect behavior.
All tests are expected to FAIL (RED phase) until implementation is complete.
"""

import pytest


class TestRegisterTypeDecorator:
    """AC #1: Custom type registration via Python SDK decorator."""

    def test_register_type_decorator_registers_on_connect(self):
        """Given I define a custom type with @wheelhouse.register_type("biotech.MoleculeObject")
        When my surface connects to the broker
        Then the type is registered in the broker's type registry under the "biotech" namespace.
        """
        # This will fail with ImportError until sdk/python/wheelhouse/ is created
        from wheelhouse import connect, register_type  # noqa: F401

        pytest.fail("SDK @register_type decorator not yet implemented")

    def test_reserved_namespace_rejected_at_registration(self):
        """Given I attempt @wheelhouse.register_type("wheelhouse.MyType")
        When the registration runs
        Then it is rejected with a clear error (ADR-004 security invariant).
        """
        from wheelhouse import register_type  # noqa: F401

        pytest.fail("Reserved namespace rejection not yet implemented in SDK")

    def test_namespace_format_enforced(self):
        """Given I use @wheelhouse.register_type("InvalidName")
        When the registration runs
        Then it is rejected with an error about invalid namespace format.
        """
        from wheelhouse import register_type  # noqa: F401

        pytest.fail("Namespace format validation not yet implemented in SDK")


class TestAutoReregistration:
    """AC #3: Automatic type re-registration on reconnect (CM-07)."""

    def test_reconnect_reregisters_types_automatically(self):
        """Given a surface registers a type and then disconnects and reconnects
        When the SDK reconnects
        Then it re-registers the custom type automatically without any developer action.
        """
        from wheelhouse import connect  # noqa: F401

        pytest.fail("Auto re-registration on reconnect not yet implemented")


class TestNamespaceIsolation:
    """AC #4: Multi-namespace coexistence without collision."""

    def test_different_namespaces_same_type_name(self):
        """Given two surfaces register types in different namespaces
        (biotech.MoleculeObject vs pharma.MoleculeObject)
        When both register successfully
        Then objects from each namespace are routed independently without collision.
        """
        from wheelhouse import register_type  # noqa: F401

        pytest.fail("Multi-namespace isolation not yet implemented")


class TestUnknownTypeHandling:
    """AC #2: Graceful handling of unknown types."""

    def test_unknown_type_returns_raw_bytes_and_type_name(self):
        """Given a receiver does not know the type
        When it receives an object of that type
        Then it receives the raw bytes plus the type name string — never a silent failure or crash.
        """
        from wheelhouse import connect  # noqa: F401

        pytest.fail("Unknown type fallback not yet implemented in SDK")

    def test_known_type_deserializes_correctly(self):
        """Given a receiver knows the type (via its own registration)
        When it receives an object of that type
        Then it receives a deserialized instance.
        """
        from wheelhouse import connect  # noqa: F401

        pytest.fail("Known type deserialization not yet implemented in SDK")


class TestSDKErrorHandling:
    """Error cases for type registration."""

    def test_register_type_connection_failure_gives_actionable_error(self):
        """Given the broker is not running
        When I connect and try to register a type
        Then I get an actionable error (not 'connection refused').
        """
        from wheelhouse import connect  # noqa: F401
        from wheelhouse.errors import ConnectionError as WhConnectionError  # noqa: F401

        pytest.fail("SDK error handling not yet implemented")
