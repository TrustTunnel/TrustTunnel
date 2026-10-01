# syntax=docker/dockerfile:1
FROM python:3.13-slim-bookworm AS build
ARG ENDPOINT_DIR_NAME="TrustTunnel"
ARG RUST_DEFAULT_VERSION="1.95"
WORKDIR /home
# Install needed packets
RUN apt update && \
    apt install -y build-essential cmake curl make git libclang-dev
# Install Rust and Cargo
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain $RUST_DEFAULT_VERSION -y
ENV PATH="/root/.cargo/bin:$PATH"
# Copy source files
WORKDIR $ENDPOINT_DIR_NAME
COPY deeplink/ ./deeplink
COPY endpoint/ ./endpoint
COPY lib/ ./lib
COPY macros/ ./macros
COPY tools/ ./tools
COPY Cargo.toml Cargo.lock rust-toolchain.toml Makefile ./
# Build
RUN make endpoint/build
RUN make endpoint/build-wizard

# Copy binaries
FROM debian:bookworm-slim AS trusttunnel-endpoint
ARG ENDPOINT_DIR_NAME="TrustTunnel"
ARG LOG_LEVEL="info"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates iproute2 && rm -rf /var/lib/apt/lists/*
COPY --from=build /home/$ENDPOINT_DIR_NAME/target/release/setup_wizard /bin/
COPY --from=build /home/$ENDPOINT_DIR_NAME/target/release/trusttunnel_endpoint /bin/
COPY --chmod=755  /docker-entrypoint.sh /scripts/
WORKDIR /trusttunnel_endpoint

# Persist endpoint state/configuration under this directory:
# - vpn.toml
# - hosts.toml
# - credentials.toml
# - rules.toml
# - certs/
VOLUME /trusttunnel_endpoint/
ENTRYPOINT ["/scripts/docker-entrypoint.sh"]

