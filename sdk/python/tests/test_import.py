"""Basic import test for the wheelhouse package."""


def test_wheelhouse_import():
    """Verify the wheelhouse package can be imported."""
    import wheelhouse

    assert wheelhouse.__version__ == "0.1.0"
