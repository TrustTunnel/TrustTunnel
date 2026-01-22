# TrustTunnel Deployment Guide

## Prerequisites
- A server (Hetzner, Contabo, AWS, etc.) with a fresh Debian/Ubuntu OS.
- Root access (SSH).

## Quick Start (Out of the Box)

1.  **Clone the Repository**
    ```bash
    git clone https://github.com/TrustTunnel/TrustTunnel.git
    cd TrustTunnel
    ```

2.  **Run Setup**
    This script will install Docker and prepare the environment.
    ```bash
    chmod +x setup.sh
    ./setup.sh
    ```

3.  **Configure**
    Edit the `.env` file with your details:
    ```bash
    nano .env
    ```
    *Important: Set `TT_HOSTNAME` and `TT_CREDENTIALS`.*
    *Optional: Change `TT_PORT_TCP` if you have a conflict (e.g. to 8443).*

4.  **Deploy**
    ```bash
    chmod +x deploy.sh
    ./deploy.sh
    ```

5.  **Firewall (UFW)**
    Ensure these ports are open:
    ```bash
    ufw allow 443/udp           # VPN Data
    ufw allow ${TT_PORT_TCP}/tcp # VPN Handshake/Fallback (default 443)
    ```

## Configuration Reference (`.env`)

| Variable | Description | Default |
| :--- | :--- | :--- |
| `TT_HOSTNAME` | Domain name for the VPN endpoint (e.g. `vpn.example.com`) | Required |
| `TT_CREDENTIALS` | Admin credentials (`user:pass`) | Required |
| `TT_CERT_TYPE` | `self-signed` or `letsencrypt` | `self-signed` |
| `TT_ACME_EMAIL` | Email for Let's Encrypt | Required for LE |
| `TT_PORT_TCP` | Host TCP port | `443` |
| `TT_PORT_UDP` | Host UDP port | `443` |
| `TT_LISTEN_ADDRESS` | Bind address internal | `0.0.0.0:443` |

## Updates
 To update the server to the latest version, simply run:
 ```bash
 ./deploy.sh
 ```
 This will pull the latest code, rebuild the image, and restart the service.
