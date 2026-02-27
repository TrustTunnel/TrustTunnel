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

FROM gcr.io/distroless/cc-debian12 AS runtime

COPY --from=builder /usr/local/bin/trusttunnel-sidecar-agent /usr/local/bin/trusttunnel-sidecar-agent

ENTRYPOINT ["/usr/local/bin/trusttunnel-sidecar-agent"]
