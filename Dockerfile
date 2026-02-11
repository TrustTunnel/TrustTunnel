# syntax=docker/dockerfile:1
FROM rust:1.85-slim-bookworm AS build
ARG ENDPOINT_DIR_NAME="TrustTunnel"

WORKDIR /home

# Install needed build dependencies
RUN apt-get update && \
    apt-get install -y build-essential cmake libclang-dev pkg-config libssl-dev git perl golang-go mold clang && \
    rm -rf /var/lib/apt/lists/*

# Copy source files
WORKDIR /home/$ENDPOINT_DIR_NAME
# Copy only Cargo files first to cache dependencies
COPY Cargo.toml Cargo.lock rust-toolchain.toml Makefile ./
COPY endpoint/Cargo.toml ./endpoint/
COPY lib/Cargo.toml ./lib/
COPY macros/Cargo.toml ./macros/
COPY tools/Cargo.toml ./tools/

# Create dummy source files to build dependencies
RUN mkdir -p endpoint/src lib/src macros/src tools/setup_wizard && \
    mkdir -p .cargo && \
    echo '[target.x86_64-unknown-linux-gnu]' > .cargo/config.toml && \
    echo 'linker = "clang"' >> .cargo/config.toml && \
    echo 'rustflags = ["-C", "link-arg=-fuse-ld=mold"]' >> .cargo/config.toml && \
    echo "fn main() {}" > endpoint/src/main.rs && \
    echo "fn main() {}" > tools/setup_wizard/main.rs && \
    touch lib/src/lib.rs && \
    touch macros/src/lib.rs

# Build dependencies only
RUN cargo build --release --workspace

# Copy actual source files
COPY endpoint/ ./endpoint/
COPY lib/ ./lib/
COPY macros/ ./macros/
COPY tools/ ./tools/

# Force rebuild of local packages because modification time might not trigger it
RUN touch endpoint/src/main.rs tools/setup_wizard/main.rs lib/src/lib.rs macros/src/lib.rs

# Build the release binaries
RUN cargo build --release --bin setup_wizard && \
    cargo build --release --bin trusttunnel_endpoint

# Runtime stage
FROM debian:bookworm-slim AS trusttunnel-endpoint
ARG ENDPOINT_DIR_NAME="TrustTunnel"

# Install runtime dependencies (OpenSSL is usually needed)
RUN apt-get update && \
    apt-get install -y ca-certificates openssl curl iproute2 iptables && \
    rm -rf /var/lib/apt/lists/*

COPY --from=build /home/$ENDPOINT_DIR_NAME/target/release/setup_wizard /bin/
COPY --from=build /home/$ENDPOINT_DIR_NAME/target/release/trusttunnel_endpoint /bin/
COPY --chmod=755 docker-entrypoint.sh /scripts/

WORKDIR /trusttunnel_endpoint
VOLUME /trusttunnel_endpoint/

ENTRYPOINT ["/scripts/docker-entrypoint.sh"]
