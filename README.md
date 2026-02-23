<!-- markdownlint-disable MD041 -->
<p align="center">
<picture>
<source media="(prefers-color-scheme: dark)" srcset="https://cdn.adguardcdn.com/website/github.com/TrustTunnel/logo_dark.svg" width="300px" alt="TrustTunnel" />
<img src="https://cdn.adguardcdn.com/website/github.com/TrustTunnel/logo_light.svg" width="300px" alt="TrustTunnel" />
</picture>
</p>

<p align="center"><a href="https://github.com/TrustTunnel/TrustTunnelClient">Console client</a>
  · <a href="https://github.com/TrustTunnel/TrustTunnelFlutterClient">Flutter-based app</a>
  · <a href="https://agrd.io/ios_trusttunnel">App store</a>
  · <a href="https://agrd.io/android_trusttunnel">Play store</a>
</p>

---

## Table of Contents

- [Introduction](#introduction)
- [Server Features](#server-features)
- [Client Features](#client-features)
- [Quick start](#quick-start)
    - [Endpoint setup](#endpoint-setup)
        - [Install the endpoint](#install-the-endpoint)
        - [Updating the endpoint](#updating-the-endpoint)
        - [TrustTunnel Flutter Client 1.0 Warning](#trusttunnel-flutter-client-10-warning)
        - [Endpoint configuration wizard](#endpoint-configuration-wizard)
        - [Let's Encrypt certificate lifecycle](#lets-encrypt-certificate-lifecycle)
        - [Running endpoint](#running-endpoint)
        - [Export client configuration](#export-client-configuration)
    - [Client setup](#client-setup)
        - [Install the client](#install-the-client)
        - [Updating the client](#updating-the-client)
        - [Client configuration wizard](#client-configuration-wizard)
        - [Running client](#running-client)
- [See also](#see-also)
- [Roadmap](#roadmap)
- [License](#license)

---

## Introduction

TrustTunnel is a modern, open-source VPN protocol originally developed by
[AdGuard VPN][adguard-vpn] and now available for anyone to use and audit.

It delivers fast, secure, and reliable VPN connections without the usual trade-offs.
By design, TrustTunnel traffic is indistinguishable from regular HTTPS traffic,
allowing it to bypass throttling and deep-packet inspection while maintaining
strong privacy protections.

The TrustTunnel project includes the VPN endpoint (this repository), the
[library and CLI for the client][trusttunnel-client],
and the [GUI application][trusttunnel-flutter-client].

For a detailed Russian-language walkthrough of endpoint startup arguments,
environment variables, and JWT/mixed auth run modes, see
[ENDPOINT_RUN_GUIDE_RU.md](./ENDPOINT_RUN_GUIDE_RU.md).

[adguard-vpn]: https://adguard-vpn.com
[trusttunnel-client]: https://github.com/TrustTunnel/TrustTunnelClient
[trusttunnel-flutter-client]: https://github.com/TrustTunnel/TrustTunnelFlutterClient
[app-store]: https://agrd.io/ios_trusttunnel
[play-store]: https://agrd.io/android_trusttunnel

## Server Features

- **VPN Protocol**: The library implements the VPN protocol compatible
  with HTTP/1.1, HTTP/2, and QUIC. By mimicking regular network traffic, it
  becomes impossible to detect and block.

- **Flexible Traffic Tunneling**: TrustTunnel can tunnel TCP, UDP, and ICMP
  traffic to and from the client.

- **Platform Compatibility**: The server is compatible with Linux and macOS.
  The client is available for Android, Apple, Windows, and Linux.

---

## Client Features

- **Traffic Tunneling**: The library is capable of tunneling TCP, UDP, and ICMP
  traffic from the client to the endpoint and back.

- **Cross-Platform Support**: It supports Linux, macOS, and Windows platforms,
  providing a consistent experience across different operating systems.

- **System-Wide Tunnel and SOCKS5 Proxy**: It can be set up as a system-wide
  tunnel, utilizing a virtual network interface, as well as a SOCKS5 proxy.

- **Split Tunneling**: The library supports split tunneling, allowing users to
  exclude connections to certain domains or hosts from routing through the VPN
  endpoint, or vice versa, only routing connections to specific domains or hosts
  through the endpoint based on an exclusion list.

- **Custom DNS Upstream**: Users can specify a custom DNS upstream, which is
  used for DNS queries routed through the VPN endpoint.

---

## Quick start

### Endpoint setup

#### Install the endpoint

An installation script is available that can be run with the following command:

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnel/refs/heads/master/scripts/install.sh | sh -s -
```

The installation script will download the prebuilt package from the latest
GitHub release for the appropriate system architecture and unpack it to
`/opt/trusttunnel`. The output directory could be overridden by specifying
`-o DIR` flag at the end of the command above.

If you want to install a specific version (instead of the latest), use `-V <version>`:

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnel/refs/heads/master/scripts/install.sh | sh -s - -V <version>
```

> [!NOTE]
> Currently only `linux-x86_64` and `linux-aarch64` architectures are provided
> for the prebuilt packages.

#### Updating the endpoint

The installation script always installs the latest available version.
So, to update your installation, run the install command again:

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnel/refs/heads/master/scripts/install.sh | sh -s -
```

This re-runs the installer and replaces the binaries in the installation
directory (`/opt/trusttunnel` by default, or the directory you specified with `-o DIR`).

> [!NOTE]
> Don't forget to stop the endpoint before updating:
>
> ```bash
> sudo systemctl stop trusttunnel
> ```
>
> To start the endpoint again after updating:
>
> ```bash
> sudo systemctl start trusttunnel
> ```

#### TrustTunnel Flutter Client 1.0 Warning

> [!WARNING]
> TrustTunnel Flutter Client **doesn't support** self-signed certificates **yet**.
> If you want to use the TrustTunnel Flutter Client, you should have a valid
> certificate issued by a publicly trusted Certificate Authority (CA) associated
> with a registered domain for the IP address of the endpoint. Otherwise,
> the TrustTunnel Flutter Client will be unable to connect to the endpoint.

#### Endpoint configuration wizard

Please refer to the [CONFIGURATION.md](CONFIGURATION.md) for the more detailed
documentation on how to configure the endpoint.

The installation directory contains `setup_wizard` binary that helps generate
the config files required for the endpoint to run:

```bash
cd /opt/trusttunnel/
./setup_wizard -h
```

The setup wizard supports interactive mode, so you could run it and it will ask
for data required for endpoint configuration.

```bash
cd /opt/trusttunnel/
sudo ./setup_wizard
```

> [!NOTE]
> `sudo` is required to manage TLS certificates properly.

The wizard will ask for the following fields, some of them have the default
values you could safely use:

- **The address to listen on** - specify the address for the endpoint to listen
  on. Use the default `0.0.0.0:443` if you want the endpoint to listen on port
  443 (HTTPS) on all interfaces.
- **Path to credentials file** - path where the user credentials for
  authorization will be stored.
- **Username** - the username the user will use for authorization.
- **Password** - the user's password.
- **Add one more user?** - select `yes` if you want to add more users, or `no`
  to continue the configuration process.
- **Path to the rules file** - path to store the filtering rules.
- **Connection filtering rules** - you can add rules that the endpoint will use
  to allow or disallow user's connections based on:
    - Client IP address
    - TLS random prefix
    - TLS random with mask

  Press `n` to allow all connections.
- **Path to a file to store the library settings** - path to store the main
  endpoint configuration file.
- **Certificate selection** - choose how to obtain a TLS certificate:
    - **Issue a Let's Encrypt certificate** (requires a public domain) - the
      setup wizard has built-in ACME support and can automatically obtain a free,
      publicly trusted certificate from Let's Encrypt. You'll need:
        - A registered domain pointing to your server's IP address
        - Port 80 accessible from the internet (for HTTP-01 challenge), or
        - Ability to add DNS TXT records (for DNS-01 challenge)
    - **Generate a self-signed certificate** - suitable for testing or when using
      the CLI client only. Note: The Flutter client does not support self-signed
      certificates **yet**.
    - **Provide path to existing certificate** - use your own certificate files
      obtained from another CA or tool like [certbot][certbot].
- **Path to a file to store the TLS hosts settings** - path to store the TLS host settings file.

At this point all required configuration files are created and saved on disk.

[certbot]: https://eff-certbot.readthedocs.io/en/stable/

#### Let's Encrypt certificate lifecycle

The setup wizard can obtain a Let's Encrypt certificate during initial setup, but you are responsible for ensuring it stays valid over time (renewal and service reload/restart).

If you're using Certbot to manage certificates and renew them automatically, follow the guide in [CERT_RENEWAL.md](CERT_RENEWAL.md).

#### Running endpoint

The installed package contains the systemd service template, named
`trusttunnel.service.template`.

This template can be used to set up the endpoint as a systemd service:

> [!NOTE]
> The template file assumes that the TrustTunnel Endpoint binary and all its
> configuration files are located in `/opt/trusttunnel` and have the default
> file names. Modify the template if you have used the different paths.

```bash
cd /opt/trusttunnel/
cp trusttunnel.service.template /etc/systemd/system/trusttunnel.service
sudo systemctl daemon-reload
sudo systemctl enable --now trusttunnel
```

#### Export client configuration

The endpoint binary can generate client configurations in two formats:

##### Deep-Link Format (Default)

Generate a compact `tt://` URI suitable for QR codes and mobile apps:

```shell
# <client_name> - name of the client whose credentials will be included
# <public_ip> - `ip` or `ip:port` that the user will use to connect to the endpoint
#             If only `ip` is specified, the port from the `listen_address` field will be used
cd /opt/trusttunnel/
./trusttunnel_endpoint vpn.toml hosts.toml -c <client_name> -a <public_ip>

# Or explicitly specify the format:
./trusttunnel_endpoint vpn.toml hosts.toml -c <client_name> -a <public_ip> --format deeplink
```

This outputs a `tt://` deep-link URI that can be:

- Shared directly with mobile clients
- Used with the [CLI client][trusttunnel-client] or [TrustTunnel Flutter Client][trusttunnel-flutter-client]

**Note**: If your certificate is signed by a trusted CA (e.g., Let's Encrypt), it will be
automatically omitted from the deep-link to keep it compact. Self-signed
certificates are included automatically.

##### TOML Format (For CLI Client)

Generate a traditional TOML configuration file:

```shell
cd /opt/trusttunnel/
./trusttunnel_endpoint vpn.toml hosts.toml -c <client_name> -a <public_ip> --format toml
```

This outputs a TOML configuration file suitable for the CLI client.

Both formats contain all necessary information to connect to the endpoint. See the
[TrustTunnel Flutter Client documentation][trusttunnel-flutter-configuration] for setup instructions.

Congratulations! You've done setting up the endpoint!

[trusttunnel-flutter-configuration]: https://github.com/TrustTunnel/TrustTunnelFlutterClient/blob/master/README.md#server-configuration

### Client setup

You have a choice to use a [CLI client][trusttunnel-client] or a
[GUI client][trusttunnel-flutter-client] (available on [App Store][app-store]
and [Play Store][play-store]).

#### Install the client

##### Linux / macOS

An installation script is available:

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnelClient/refs/heads/master/scripts/install.sh | sh -s -
```

The installation script will download the prebuilt package from the latest GitHub release for the appropriate system architecture and unpack it to `/opt/trusttunnel_client`. The output directory could be overridden by specifying `-o DIR` flag at the end of the command above.

> [!NOTE]
> Install script supports x86_64, aarch64, armv7, mips and mipsel architectures
> for linux and arm64 and x86_64 for macos.

##### Windows

Download the latest release archive from the
[TrustTunnel Client releases page][trusttunnel-client-releases].

Extract the archive to a directory of your choice, for example `C:\TrustTunnel\`.

[trusttunnel-client-releases]: https://github.com/TrustTunnel/TrustTunnelClient/releases/latest

#### Updating the client

##### Linux / macOS

The installation script always installs the latest available version.
So, to update your installation, run the install command again:

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnelClient/refs/heads/master/scripts/install.sh | sh -s -
```

This re-runs the installer and replaces the binaries in the installation directory (`/opt/trusttunnel_client` by default, or the directory you specified with `-o DIR`).

> [!NOTE]
> Don't forget to stop the client before updating (for example, by stopping the running process).

##### Windows

Download the latest release from the
[releases page][trusttunnel-client-releases] and replace the files
in your installation directory.

#### Client configuration wizard

The installation directory contains `setup_wizard` binary that helps generate
the config files required for the client to run.

##### Linux / macOS

```bash
cd /opt/trusttunnel_client/
./setup_wizard -h
```

To configure the client to use the config that was generated by endpoint, run
the following command:

```bash
./setup_wizard --mode non-interactive \
     --endpoint_config <endpoint_config> \
     --settings trusttunnel_client.toml
```

##### Windows

```cmd
setup_wizard.exe --mode non-interactive ^
    --endpoint_config <endpoint_config> ^
    --settings trusttunnel_client.toml
```

In both cases, `<endpoint_config>` is the path to the configuration file
generated by the endpoint.

`trusttunnel_client.toml` will contain all required configuration for the
client.

> [!TIP]
> The generated configuration contains basic settings to connect to the endpoint.
> For advanced features, edit `trusttunnel_client.toml` directly. You can configure:
>
> - **VPN mode**: Route all traffic (`general`) or only specific destinations (`selective`)
> - **Kill switch**: Block traffic when VPN disconnects
> - **DNS upstreams**: Custom DNS resolvers (DoH, DoT, DoQ supported)
> - **Exclusions**: Domains/IPs to bypass or route through VPN
> - **Listener type**: TUN device or SOCKS5 proxy
>
> See the [TrustTunnel CLI Client README](https://github.com/TrustTunnel/TrustTunnelClient/blob/master/trusttunnel/README.md#configuration-reference) for all available options.

<!-- markdownlint-disable MD028 -->
> [!NOTE]
> After editing the config, restart the client for the changes to take effect.

#### Running client

##### Linux / macOS

```bash
cd /opt/trusttunnel_client/
sudo ./trusttunnel_client -c trusttunnel_client.toml
```

`sudo` is required to set up the routes and tun interface.

##### Windows

Open a terminal **as Administrator** and run:

```cmd
trusttunnel_client.exe -c trusttunnel_client.toml
```

Administrator privileges are required to set up routes and the TUN interface.

## Local build and Docker image

### Build endpoint binary locally

```bash
cargo build --manifest-path endpoint/Cargo.toml --release
```

The release binary will be available at:

```bash
./target/release/trusttunnel_endpoint
```

### Build Docker image locally

```bash
docker build -t trusttunnel-endpoint:local .
```

Run the container by mounting your configuration files:

```bash
docker run --rm -p 8443:8443 \
  -v /path/to/vpn.toml:/etc/trusttunnel/vpn.toml:ro \
  -v /path/to/hosts.toml:/etc/trusttunnel/hosts.toml:ro \
  trusttunnel-endpoint:local
```

### Use image from GHCR

Published image tags:

- `ghcr.io/<owner>/trusttunnel-endpoint:latest`
- `ghcr.io/<owner>/trusttunnel-endpoint:v<version>`

Pull and run:

```bash
docker pull ghcr.io/<owner>/trusttunnel-endpoint:latest
docker run --rm -p 8443:8443 \
  -v /path/to/vpn.toml:/etc/trusttunnel/vpn.toml:ro \
  -v /path/to/hosts.toml:/etc/trusttunnel/hosts.toml:ro \
  ghcr.io/<owner>/trusttunnel-endpoint:latest
```

## See Also

- [CONFIGURATION.md](CONFIGURATION.md) - Configuration documentation
- [DEVELOPMENT.md](DEVELOPMENT.md) - Development documentation
- [PROTOCOL.md](PROTOCOL.md) - Protocol specification
- [CHANGELOG.md](CHANGELOG.md) - Changelog
- [VERIFY_RELEASES.md](VERIFY_RELEASES.md) - How to verify releases

## Roadmap

While our VPN currently supports tunneling TCP/UDP/ICMP traffic, we plan to add support for
peer-to-peer communication between clients.

Stay tuned for this feature in upcoming releases.

## License

This project is licensed under the Apache 2.0 License. See [LICENSE](LICENSE) for details.

## Monitoring

TrustTunnel exposes Prometheus metrics on `/metrics` when `[metrics]` is enabled in `settings.toml`.

Example:

```toml
[metrics]
enabled = true
jwt_error_enabled = true
address = "127.0.0.1:1987"
request_timeout_secs = 3
```

Key metrics include:

- `vpn_handshake_duration_seconds`
- `vpn_jwt_validation_errors_total`
- `vpn_active_connections`
- `vpn_request_latency_seconds`
- `vpn_traffic_bytes_total`

See [METRICS.md](METRICS.md) and `monitoring/` for Prometheus + Grafana examples.
