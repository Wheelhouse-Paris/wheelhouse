# Container image for wh-broker.
# Multi-stage build — compiles inside the container.
# ghcr.io/wheelhouse-paris/wh-broker:<version>
#
# Build context: workspace root (so Cargo workspace resolves).
# Usage: docker buildx build -f crates/wh-broker/Dockerfile -t wh-broker .

FROM rust:1.85-slim-bookworm AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
RUN cargo build --release -p wh-broker

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/wh-broker /usr/local/bin/wh-broker
ENTRYPOINT ["/usr/local/bin/wh-broker"]
