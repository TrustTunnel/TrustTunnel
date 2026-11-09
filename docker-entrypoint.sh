#!/bin/bash

set -e

check_file() {
    local file="$1"
    if [ ! -f "$file" ]; then
        echo "Configuration file '$file' not found"
        return 1
    fi
    return 0
}

verify_configs() {
    local missing=0

    check_file "credentials.toml" || missing=1
    check_file "vpn.toml" || missing=1
    check_file "hosts.toml" || missing=1

    return $missing
}

run_setup_wizard_noninteractive() {
    if [ -z "${TT_HOSTNAME:-}" ] || [ -z "${TT_CREDENTIALS:-}" ]; then
        echo "Error: TT_HOSTNAME and TT_CREDENTIALS are required for non-interactive setup"
        return 1
    fi

    local args=(
        "-m" "non-interactive"
        "-a" "${TT_LISTEN_ADDRESS:-0.0.0.0:8443}"
        "-c" "$TT_CREDENTIALS"
        "-n" "$TT_HOSTNAME"
        "--lib-settings" "vpn.toml"
        "--hosts-settings" "hosts.toml"
    )

    case "${TT_CERT_TYPE:-self-signed}" in
        self-signed)
            args+=("--cert-type" "self-signed")
            ;;
        letsencrypt)
            if [ -z "${TT_ACME_EMAIL:-}" ]; then
                echo "Error: TT_ACME_EMAIL is required when TT_CERT_TYPE=letsencrypt"
                return 1
            fi
            args+=("--cert-type" "letsencrypt" "--acme-email" "$TT_ACME_EMAIL")
            if [ "${TT_ACME_STAGING:-false}" = "true" ]; then
                args+=("--acme-staging")
            fi
            ;;
        provided)
            if [ -z "${TT_CERT_PROVIDED_CHAIN_PATH:-}" ] || [ -z "${TT_CERT_PROVIDED_KEY_PATH:-}" ]; then
                echo "Error: TT_CERT_PROVIDED_CHAIN_PATH and TT_CERT_PROVIDED_KEY_PATH are required when TT_CERT_TYPE=provided"
                return 1
            fi
            args+=(
                "--cert-type" "provided"
                "--cert-chain-path" "$TT_CERT_PROVIDED_CHAIN_PATH"
                "--cert-key-path" "$TT_CERT_PROVIDED_KEY_PATH"
            )
            ;;
        *)
            echo "Error: Unsupported TT_CERT_TYPE='$TT_CERT_TYPE'. Supported: self-signed, letsencrypt, provided"
            return 1
            ;;
    esac

    echo "Missing configuration file(s). Running trusttunnel_endpoint_setup_wizard in non-interactive mode"
    trusttunnel_endpoint_setup_wizard "${args[@]}"
}

main() {
    if ! verify_configs; then
        if [ -t 0 ]; then
            echo "Missing configuration file(s). Launching setup wizard in interactive mode"
            trusttunnel_endpoint_setup_wizard
        else
            run_setup_wizard_noninteractive
        fi
    fi

    echo "Starting trusttunnel_endpoint"
    exec trusttunnel_endpoint vpn.toml hosts.toml
}

main