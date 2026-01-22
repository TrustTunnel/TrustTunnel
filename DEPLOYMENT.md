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

> **Warning:** Do NOT change `TT_LISTEN_ADDRESS` from `0.0.0.0:443`. This is the internal container port and must match the Docker port mapping. To use a different external port, only modify `TT_PORT_TCP` and `TT_PORT_UDP`.

## Android Client Setup (Self-Signed Certificate)

When using `TT_CERT_TYPE=self-signed`, Android devices will not trust the server certificate by default. You must manually install the CA certificate:

1.  **Copy the certificate from your server:**
    ```bash
    # The certificate is generated in the data volume
    cat data/certs/cert.pem
    ```
    Copy this file to your Android device (via USB, email, or cloud storage).

2.  **Install on Android (Pixel / Android 14+):**
    - Go to **Settings** > **Security & Privacy** > **More security settings** > **Encryption & credentials**
    - Tap **Install a certificate** > **CA certificate**
    - Confirm the warning ("Install anyway")
    - Select the `cert.pem` file

3.  **Configure the TrustTunnel App:**
    - **Server Address:** `YOUR_SERVER_IP:10443` (or your custom `TT_PORT_TCP`)
    - **Certificate Domain:** `vpn.yourdomain.com` (must match `TT_HOSTNAME`)
    - **Username/Password:** As configured in `TT_CREDENTIALS`

> **Note:** If you regenerate certificates (by deleting `data/*`), you must reinstall the new certificate on all devices.

## Updates
 To update the server to the latest version, simply run:
 ```bash
 ./deploy.sh
 ```
 This will pull the latest code, rebuild the image, and restart the service.
