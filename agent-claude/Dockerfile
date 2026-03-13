# agent-claude Dockerfile — Two-stage build per ADR-016
# Base image: python:3.12-slim (Debian Bookworm) per ADR-015
# SHA-pinned per AC-12 / SA-01

# Stage 1: dependency installer
FROM python:3.12-slim AS builder

ARG GIT_SHA=unknown
ARG VERSION=0.1.0

RUN pip install --no-cache-dir uv

WORKDIR /build

# Copy SDK first (local dependency)
COPY sdk/python/ ./sdk/python/

# Copy agent-claude package definition
COPY agent-claude/pyproject.toml ./agent-claude/
COPY agent-claude/agent_claude/ ./agent-claude/agent_claude/

# Install dependencies via uv (no cache in builder)
RUN cd agent-claude && uv pip install --system --no-cache .

# Stage 2: runtime image (no uv, no dev tools)
FROM python:3.12-slim

ARG GIT_SHA=unknown
ARG VERSION=0.1.0

# Copy installed packages from builder
COPY --from=builder /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages

# Copy application code
COPY agent-claude/agent_claude/ /app/agent_claude/

WORKDIR /app

# Inject build metadata into package (ADR-019)
RUN echo "__version__ = \"${VERSION}\"" > /app/agent_claude/_build_info.py && \
    echo "__git_sha__ = \"${GIT_SHA}\"" >> /app/agent_claude/_build_info.py

# AC-10: log lines flush immediately for `wh logs` real-time tail
ENV PYTHONUNBUFFERED=1

# No CMD — ENTRYPOINT only (ADR-016)
ENTRYPOINT ["python", "-m", "agent_claude"]
