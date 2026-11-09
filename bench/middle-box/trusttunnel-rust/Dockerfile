# syntax=docker/dockerfile:1
FROM bench-common

ARG ENDPOINT_HOSTNAME="endpoint.bench"
ARG CONFIG_FILE="vpn.conf"
ARG TLS_HOSTS_SETTINGS_FILE="tls_hosts.conf"
ARG LOG_LEVEL="info"

WORKDIR /bench/endpoint

COPY . .
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/bench/endpoint/target \
    cargo build --release --bin trusttunnel_endpoint_setup_wizard --bin trusttunnel_endpoint && \
    cp ./target/release/trusttunnel_endpoint_setup_wizard ./target/release/trusttunnel_endpoint /bench/

WORKDIR /bench
RUN ./trusttunnel_endpoint_setup_wizard --mode non-interactive  \
    --address [::]:4433 \
    --creds premium:premium  \
    --hostname "$ENDPOINT_HOSTNAME"  \
    --lib-settings "$CONFIG_FILE"  \
    --hosts-settings "$TLS_HOSTS_SETTINGS_FILE"

ENV LOG_LEVEL=$LOG_LEVEL \
    CONFIG_FILE=$CONFIG_FILE \
    TLS_HOSTS_SETTINGS_FILE=$TLS_HOSTS_SETTINGS_FILE \
    RUST_BACKTRACE=1
ENTRYPOINT ./trusttunnel_endpoint -l "$LOG_LEVEL" "$CONFIG_FILE" "$TLS_HOSTS_SETTINGS_FILE" >>/tmp/vpn.log 2>&1
