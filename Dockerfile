# Minimal container image for wh-cli (SR-02)
# ghcr.io/wheelhouse-paris/wh-cli:<version>
#
# Build context expects pre-built static binary at ./wh
# Used by release.yml workflow, not for local development.

FROM scratch

COPY wh /usr/local/bin/wh

ENTRYPOINT ["/usr/local/bin/wh"]
