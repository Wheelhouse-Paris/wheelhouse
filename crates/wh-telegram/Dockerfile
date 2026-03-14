# Minimal container image for wh-telegram surface runner (Story 9.2)
# ghcr.io/wheelhouse-paris/wh-telegram:<version>
#
# Build context expects pre-built static binary at ./wh-telegram
# Used by release workflow, not for local development.
# No secrets baked in — all config via environment variables.

FROM scratch

COPY wh-telegram /usr/local/bin/wh-telegram

ENTRYPOINT ["/usr/local/bin/wh-telegram"]
