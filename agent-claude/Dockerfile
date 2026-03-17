# agent-claude Dockerfile — Two-stage build per ADR-016
# Base image: python:3.12-slim (Debian Bookworm) per ADR-015

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

# Stage 2: runtime image
FROM python:3.12-slim

ARG GIT_SHA=unknown
ARG VERSION=0.1.0

# Install Node.js (required for Claude Code CLI)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        gnupg \
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
        | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" \
        > /etc/apt/sources.list.d/nodesource.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends nodejs \
    && apt-get purge -y gnupg \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI (uses Max subscription via CLAUDE_CODE_OAUTH_TOKEN)
RUN npm install -g @anthropic-ai/claude-code
ENV CLAUDE_DISABLE_AUTO_UPDATES=1

# Non-root user required by claude --dangerously-skip-permissions
RUN useradd --create-home --shell /bin/bash agent \
    && mkdir -p /home/agent/.claude /persona /context \
    && chown -R agent:agent /home/agent/.claude /persona /context

# Copy installed Python packages from builder
COPY --from=builder /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages

# Copy application code
COPY --chown=agent:agent agent-claude/agent_claude/ /app/agent_claude/

WORKDIR /app

# Inject build metadata into package (ADR-019)
RUN echo "__version__ = \"${VERSION}\"" > /app/agent_claude/_build_info.py && \
    echo "__git_sha__ = \"${GIT_SHA}\"" >> /app/agent_claude/_build_info.py && \
    chown agent:agent /app/agent_claude/_build_info.py

USER agent

# AC-10: log lines flush immediately for `wh logs` real-time tail
ENV PYTHONUNBUFFERED=1

# No CMD — ENTRYPOINT only (ADR-016)
ENTRYPOINT ["python", "-m", "agent_claude"]
