# syntax=docker/dockerfile:1.6
#
# Multi-stage build for http402-forge-api (mirrors mintforge/oracles layout).
#
#   docker build -f scripts/docker/Dockerfile \
#       -t forge-devnet:$(git rev-parse --short HEAD) .
#
# Production runs via systemd + forge-{devnet,mainnet}.service — not raw docker run.

ARG RUST_VERSION=1.92.0

FROM rust:${RUST_VERSION}-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl-dev pkg-config ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN echo "[build] http402-forge-api release binary" && \
    cargo build --release --bin http402-forge-api

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl3 ca-certificates ffmpeg poppler-utils && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
RUN mkdir -p /app/data/objects

COPY --from=builder /build/target/release/http402-forge-api /usr/local/bin/http402-forge-api

LABEL org.opencontainers.image.title="http402-forge-api" \
      org.opencontainers.image.source="https://github.com/miralandlabs/x402"

EXPOSE 8092 8093
ENTRYPOINT ["/usr/local/bin/http402-forge-api"]
