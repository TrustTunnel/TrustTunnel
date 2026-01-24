#!/bin/bash
set -e

check_file() {
    local file="$1"
    if [ ! -f "$file" ]; then
        return 1
    fi
    return 0
}

verify_configs() {
    local missing=0
    check_file "vpn.toml" || missing=1
    return $missing
}

run_setup_wizard() {
    echo "Configuration files missing. Attempting auto-configuration..."

    if [ -z "$TT_HOSTNAME" ] || [ -z "$TT_CREDENTIALS" ]; then
        echo "Error: Missing required environment variables: TT_HOSTNAME, TT_CREDENTIALS"
        echo "Auto-configuration failed."
        return 1
    fi

    local args=(
        "-m" "non-interactive"
        "-n" "$TT_HOSTNAME"
        "-c" "$TT_CREDENTIALS"
        "--lib-settings" "vpn.toml"
        "--hosts-settings" "hosts.toml"
    )

    if [ -n "$TT_LISTEN_ADDRESS" ]; then
        args+=("-a" "$TT_LISTEN_ADDRESS")
    else
        args+=("-a" "0.0.0.0:443")
    fi

    if [ -n "$TT_CERT_TYPE" ]; then
        args+=("--cert-type" "$TT_CERT_TYPE")
        if [ "$TT_CERT_TYPE" = "letsencrypt" ]; then
             if [ -n "$TT_ACME_EMAIL" ]; then
                args+=("--acme-email" "$TT_ACME_EMAIL")
             else
                echo "Error: TT_ACME_EMAIL is required for letsencrypt"
                return 1
             fi
        fi
    fi
    
    # Optional Let's Encrypt Staging
    if [ "$TT_ACME_STAGING" = "true" ]; then
        args+=("--acme-staging")
    fi
    
    if [ -n "$TT_QUIC_RECV_UDP_PAYLOAD_SIZE" ]; then
        args+=("--quic-recv-udp-payload-size" "$TT_QUIC_RECV_UDP_PAYLOAD_SIZE")
    fi
    
    if [ -n "$TT_QUIC_SEND_UDP_PAYLOAD_SIZE" ]; then
        args+=("--quic-send-udp-payload-size" "$TT_QUIC_SEND_UDP_PAYLOAD_SIZE")
    fi

    echo "Running setup_wizard with: ${args[*]}"
    setup_wizard "${args[@]}"
}

apply_quic_payload_overrides() {
    local config_file="vpn.toml"
    local updated=0

    if [ ! -f "$config_file" ]; then
        return 0
    fi

    if [ -n "$TT_QUIC_RECV_UDP_PAYLOAD_SIZE" ]; then
        sed -i -E "s/^([[:space:]]*recv_udp_payload_size[[:space:]]*=[[:space:]]*).*/\\1${TT_QUIC_RECV_UDP_PAYLOAD_SIZE}/" "$config_file"
        updated=1
    fi

    if [ -n "$TT_QUIC_SEND_UDP_PAYLOAD_SIZE" ]; then
        sed -i -E "s/^([[:space:]]*send_udp_payload_size[[:space:]]*=[[:space:]]*).*/\\1${TT_QUIC_SEND_UDP_PAYLOAD_SIZE}/" "$config_file"
        updated=1
    fi

    if [ "$updated" -eq 1 ]; then
        echo "Applied QUIC UDP payload size overrides to $config_file"
    fi
}

apply_timeout_overrides() {
    local config_file="vpn.toml"
    local updated=0

    if [ ! -f "$config_file" ]; then
        return 0
    fi

    if [ -n "$TT_CLIENT_LISTENER_TIMEOUT_SECS" ]; then
        if grep -qE "^[[:space:]]*client_listener_timeout_secs[[:space:]]*=" "$config_file"; then
            sed -i -E "s/^([[:space:]]*client_listener_timeout_secs[[:space:]]*=[[:space:]]*).*/\\1${TT_CLIENT_LISTENER_TIMEOUT_SECS}/" "$config_file"
        else
            printf "\nclient_listener_timeout_secs = %s\n" "$TT_CLIENT_LISTENER_TIMEOUT_SECS" >> "$config_file"
        fi
        updated=1
    fi

    if [ -n "$TT_UDP_CONNECTIONS_TIMEOUT_SECS" ]; then
        if grep -qE "^[[:space:]]*udp_connections_timeout_secs[[:space:]]*=" "$config_file"; then
            sed -i -E "s/^([[:space:]]*udp_connections_timeout_secs[[:space:]]*=[[:space:]]*).*/\\1${TT_UDP_CONNECTIONS_TIMEOUT_SECS}/" "$config_file"
        else
            printf "\nudp_connections_timeout_secs = %s\n" "$TT_UDP_CONNECTIONS_TIMEOUT_SECS" >> "$config_file"
        fi
        updated=1
    fi

    if [ "$updated" -eq 1 ]; then
        echo "Applied timeout overrides to $config_file"
    fi
}

main() {
    if verify_configs; then
        echo "Configuration found. Starting TrustTunnel..."
    else
        if run_setup_wizard; then
            echo "Auto-configuration successful. Starting TrustTunnel..."
        else
            echo "Configuration missing and auto-configuration failed."
            if [ -t 0 ]; then
                 echo "Launching interactive setup wizard..."
                 exec setup_wizard
            else
                 echo "Please mount existing config files or provide required environment variables."
                 exit 1
            fi
        fi
    fi

    apply_quic_payload_overrides
    apply_timeout_overrides

    # Setup NAT/Masquerading
    echo "Setting up NAT..."
    iptables -t nat -A POSTROUTING -o eth0 -j MASQUERADE || echo "Warning: Failed to set iptables rule. Ensure privileges."
    ip6tables -t nat -A POSTROUTING -o eth0 -j MASQUERADE || echo "Warning: Failed to set ip6tables rule. Ensure privileges/IPv6 enabled."

    exec trusttunnel_endpoint vpn.toml hosts.toml
}

main
