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

    echo "Running setup_wizard with: ${args[*]}"
    setup_wizard "${args[@]}"
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

    # Setup NAT/Masquerading
    echo "Setting up NAT..."
    iptables -t nat -A POSTROUTING -o eth0 -j MASQUERADE || echo "Warning: Failed to set iptables rule. Ensure privileges."
    ip6tables -t nat -A POSTROUTING -o eth0 -j MASQUERADE || echo "Warning: Failed to set ip6tables rule. Ensure privileges/IPv6 enabled."

    exec trusttunnel_endpoint vpn.toml hosts.toml
}

main