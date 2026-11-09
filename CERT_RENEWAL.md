
# Let's encrypt certificate renewal

## Table of contents

- [Prerequisites](#prerequisites)
- [Install Certbot](#install-certbot)
- [Issue the certificate](#issue-the-certificate)
    - [Option A: standalone (recommended if nothing is listening on port 80)](#option-a-standalone-recommended-if-nothing-is-listening-on-port-80)
    - [Option B: webroot (recommended if you already have an HTTP server)](#option-b-webroot-recommended-if-you-already-have-an-http-server)
- [Configure TrustTunnel to use the Certbot certificate](#configure-trusttunnel-to-use-the-certbot-certificate)
- [Enable automatic renewal](#enable-automatic-renewal)
- [Ensure TrustTunnel reloads the renewed certificate](#ensure-trusttunnel-reloads-the-renewed-certificate)
- [Test renewal](#test-renewal)
- [Troubleshooting](#troubleshooting)

TrustTunnel endpoint needs a valid TLS certificate to work. TrustTunnel's `trusttunnel_endpoint_setup_wizard` can help you generate a certificate automatically, but for a long-lived setup you should use Let's Encrypt with [Certbot][certbot] and enable automated renewal.

If you previously generated a Let's Encrypt certificate without Certbot, re-issue it with Certbot so it can be renewed automatically.

This manual describes a practical setup that:

- **Issues** a certificate via Certbot.
- **Configures TrustTunnel** to use Certbot-managed certificate files.
- **Ensures renewal is automatic**.
- **Verifies** renewal via a dry run.

[certbot]: https://eff-certbot.readthedocs.io/en/stable/

## Prerequisites

- A public DNS name (A/AAAA record) pointing to the endpoint.
- Port **80/tcp** reachable from the Internet during issuance/renewal (HTTP-01 validation).
- Root access on the endpoint.

Note: This guide uses HTTP-01. If you need a wildcard certificate, use DNS-01 instead.

## Install Certbot

Use the installation method recommended for your distribution. Examples:

```bash
# Debian/Ubuntu
sudo apt update
sudo apt install -y certbot
```

## Issue the certificate

Choose one of the modes below.

### Option A: standalone (recommended if nothing is listening on port 80)

Certbot will temporarily start its own web server on port 80.

```bash
sudo certbot certonly --standalone -d example.com
```

### Option B: webroot (recommended if you already have an HTTP server)

Your HTTP server must serve `/.well-known/acme-challenge/` from the specified webroot.

```bash
sudo certbot certonly --webroot -w /var/www/html -d example.com
```

After successful issuance, Certbot will print the paths you need:

```console
Certificate is saved at: /etc/letsencrypt/live/example.com/fullchain.pem
Key is saved at:         /etc/letsencrypt/live/example.com/privkey.pem
```

These files are symlinks managed by Certbot and are updated automatically on renewal.

## Configure TrustTunnel to use the Certbot certificate

If you haven't generated TrustTunnel configuration yet, choose "Provide path to existing certificate" during the `trusttunnel_endpoint_setup_wizard` and provide these paths in any order, separated by space.

```console
? How would you like to create a certificate? ›
  Issue a Let's Encrypt certificate (requires a public domain)
  Generate a self-signed certificate
❯ Provide path to existing certificate
? Path to certificate file(s):
  - Single file containing both cert and key: /path/to/combined.pem
  - Separate files: /path/to/cert.pem /path/to/key.pem
 › /etc/letsencrypt/live/example.com/fullchain.pem /etc/letsencrypt/live/example.com/privkey.pem
```

If you previously generated TrustTunnel configuration, change paths in your TrustTunnel `hosts.toml` file to point to these files.

Was:

```toml
[[main_hosts]]
hostname = "example.com"
cert_chain_path = "certs/cert.pem"
private_key_path = "certs/key.pem"
```

Now:

```toml
[[main_hosts]]
hostname = "example.com"
cert_chain_path = "/etc/letsencrypt/live/example.com/fullchain.pem"
private_key_path = "/etc/letsencrypt/live/example.com/privkey.pem"
```

## Enable automatic renewal

On most modern Linux distributions, Certbot installs a systemd timer automatically.

To confirm the timer exists:

```bash
systemctl list-timers | grep -E 'certbot|letsencrypt'
```

If your system does not use systemd timers, you can use cron (example):

```bash
sudo crontab -e
```

Add:

```bash
0 3 * * * certbot renew --quiet
```

## Ensure TrustTunnel reloads the renewed certificate

Certbot updates files on renewal, but your service may need a restart/reload to pick up the new certificate.

If TrustTunnel runs under systemd, the simplest approach is to use a deploy hook to restart it after a successful renewal.

To save a deploy hook that will run after each successful renewal:

```bash
sudo certbot reconfigure --deploy-hook "systemctl restart trusttunnel"
```

For older versions of certbot (<2.3.0), add the following line to the `[renewalparams]` section of
`/etc/letsencrypt/renewal/<yourdomain>.conf`:

```conf
renew_hook = systemctl restart trusttunnel
```

Adjust `trusttunnel` to your actual systemd unit name.

## Test renewal

Always run a dry-run once to ensure renewal works end-to-end:

```bash
sudo certbot renew --dry-run
```

If you use `--standalone`, port 80 must be available during the dry run.

## Troubleshooting

- **Port 80 is busy**: stop the process using port 80 temporarily, or switch to `--webroot`.
- **DNS issues**: verify the hostname resolves to the endpoint's public IP.
- **Firewall issues**: allow inbound 80/tcp from the Internet.
- **Permissions**: TrustTunnel must be able to read `/etc/letsencrypt/live/.../fullchain.pem` and `privkey.pem`.
