"""agent-claude — Wheelhouse Claude API agent container.

Connects to the Wheelhouse broker via the Python SDK, subscribes to
declared streams, and processes incoming messages via the Claude API.
"""

import sys

__version__ = "0.1.0"
__git_sha__ = "unknown"

# Override from Docker build-time injection (ADR-019)
try:
    from agent_claude._build_info import __version__ as _bv, __git_sha__ as _bs  # type: ignore[import-not-found]
    __version__ = _bv
    __git_sha__ = _bs
except ImportError:
    pass  # Not running in container — use defaults


def _version_string() -> str:
    """Build version string: agent-claude 0.1.0 (git: abc1234, python: 3.12.x)."""
    py_version = f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}"
    sha_short = __git_sha__[:7] if len(__git_sha__) > 7 else __git_sha__
    return f"agent-claude {__version__} (git: {sha_short}, python: {py_version})"
