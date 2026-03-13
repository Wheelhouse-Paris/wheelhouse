"""Error types for agent-claude startup and runtime failures.

Exit-code contract:
  - AgentConfigError: exit code 1 (missing/invalid configuration)
  - ClaudeAuthError: exit code 1 (invalid API key, detected on first call)
"""


class AgentConfigError(Exception):
    """Raised when a required configuration value is missing or invalid.

    The container should log this error and exit with code 1.
    No retry, no crash loop (AC-01).
    """

    def __init__(self, message: str) -> None:
        super().__init__(message)


class ClaudeAuthError(Exception):
    """Raised when the Claude API rejects the provided API key.

    The container should log this error and exit with code 1.
    No retry with an invalid key (AC-02).
    """

    def __init__(self, message: str) -> None:
        super().__init__(message)
