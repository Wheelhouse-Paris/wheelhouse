# wh-librarian Dockerfile — Two-stage build per ADR-016
# Base image: python:3.12-slim (Debian Bookworm) per ADR-041
# Entry point: python -m wheelhouse.librarian

# Stage 1: dependency installer
FROM python:3.12-slim AS builder

ARG GIT_SHA=unknown
ARG VERSION=0.1.0

RUN pip install --no-cache-dir uv

WORKDIR /build

# Copy SDK (local dependency)
COPY sdk/python/ ./sdk/python/

# Install SDK (includes wheelhouse.librarian package)
RUN cd sdk/python && uv pip install --system --no-cache .

# Stage 2: runtime image
FROM python:3.12-slim

ARG GIT_SHA=unknown
ARG VERSION=0.1.0

# Install git (for LibrarySandbox git operations) and poppler-utils (PDF extraction)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        git \
        poppler-utils \
    && rm -rf /var/lib/apt/lists/*

# Non-root user
RUN useradd --create-home --shell /bin/bash librarian \
    && mkdir -p /workspace/.library \
    && chown -R librarian:librarian /workspace

# Copy installed Python packages from builder
COPY --from=builder /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages

# Copy SDK source (for wheelhouse.librarian module)
COPY --chown=librarian:librarian sdk/python/wheelhouse/ /usr/local/lib/python3.12/site-packages/wheelhouse/

WORKDIR /workspace

# Inject build metadata
RUN echo "__version__ = \"${VERSION}\"" > /tmp/_build_info.py && \
    echo "__git_sha__ = \"${GIT_SHA}\"" >> /tmp/_build_info.py

USER librarian

# AC-10: log lines flush immediately for `wh logs` real-time tail
ENV PYTHONUNBUFFERED=1

# No CMD — ENTRYPOINT only (ADR-016)
ENTRYPOINT ["python", "-m", "wheelhouse.librarian"]
