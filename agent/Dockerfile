# syntax=docker/dockerfile:1

FROM rust:1.85.1-bookworm AS builder
WORKDIR /workspace

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        clang \
        cmake \
        libclang-dev \
        make \
        perl \
        pkg-config; \
    rm -rf /var/lib/apt/lists/*

COPY . .

RUN set -eux; \
    cargo build --manifest-path agent/Cargo.toml --release --bin trusttunnel_sidecar_agent; \
    install -D target/release/trusttunnel_sidecar_agent /usr/local/bin/trusttunnel-sidecar-agent

FROM alpine:3.21 AS runtime

RUN addgroup -S trusttunnel && adduser -S -G trusttunnel -u 10001 trusttunnel \
    && apk add --no-cache ca-certificates wget

COPY --from=builder /usr/local/bin/trusttunnel-sidecar-agent /usr/local/bin/trusttunnel-sidecar-agent

USER 10001:10001
EXPOSE 9105
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD wget -qO- http://127.0.0.1:${AGENT_PORT:-9105}/healthz >/dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/trusttunnel-sidecar-agent"]
