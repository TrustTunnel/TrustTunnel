# CHANGELOG

## 1.0.11

- [Security] Fixed traffic leaking to local network via UDP, ICMP, and SOCKS5 forwarders
  when `allow_private_network_connections` is set to `false`.
    - Added `is_global_ip` check to UDP forwarder
    - Added `is_global_ip` check to ICMP forwarder
    - Added `is_global_ip` check to SOCKS5 forwarder (TCP and UDP)
    - Handle IPv4-mapped IPv6 addresses (`::ffff:x.x.x.x`) in `is_global_ip`
  (Based on [GitHub PR #79](https://github.com/TrustTunnel/TrustTunnel/pull/79) by @andrew-morris)

## 1.0.7

- [Feature] Added per-client connection limits
    - Optional limits for simultaneous HTTP/2 and HTTP/3 connections per client credentials
    - Global default limits via `default_max_http2_conns_per_client` and `default_max_http3_conns_per_client` in main config
    - Per-client overrides via `max_http2_conns` and `max_http3_conns` in credentials file
    - Applies to both SNI-authenticated and proxy-basic authenticated connections
    - For proxy-basic: limit enforced on first authenticated request (not idle connections)

  API changes in the library:
    - Added `max_http2_conns` and `max_http3_conns` fields to `authentication::registry_based::Client`
    - Added `default_max_http2_conns_per_client` and `default_max_http3_conns_per_client` fields to `settings::Settings`
    - Added new `connection_limiter` module with `ConnectionLimiter` and `ConnectionGuard` types
    - Added `connection_limiter` field to `core::Context`

## 1.0.6

Added support for X25519MLKEM768 post-quantum group.

## 1.0.5

- [Feature] The `-a` flag now accepts `domain` and `domain:port` in addition to `ip` and `ip:port`.
  The exported client configuration will contain the domain name, which the client resolves via DNS at connect time.
- [Feature] Deep-link format (`tt://`) now supports domain names in the `addresses` field.
- [Feature] When listening on `[::]`, the endpoint now explicitly sets `IPV6_V6ONLY=false` to accept
  both IPv4 and IPv6 connections on a single socket (dual-stack).

## 1.0.1

- [Feature] Added new `trusttunnel-deeplink` library crate for encoding/decoding `tt://` URIs
- [Feature] Added `client_random_prefix` field to client configuration export
    - New CLI option `--client-random-prefix`
    - Validates hex format and checks against `rules.toml`
    - Added to deep-link format as tag 0x0B

## 0.9.127

- [Feature] Added GPG signing of the endpoint binaries.

## 0.9.122

- Endpoint now requires credentials when listening on a public address.
- Added support of shortened QUIC settings names in configuration files.

## 0.9.115

- Fixed an issue where `client_random_prefix` rules didn’t match when Anti-DPI or post-quantum cryptography was enabled.
  (https://github.com/TrustTunnel/TrustTunnel/security/advisories/GHSA-fqh7-r5gf-3r87)

## 0.9.114

- Fixed an issue where `allow_private_network_connections` set to false could be bypassed
when a numeric address was used.
  (https://github.com/TrustTunnel/TrustTunnel/security/advisories/GHSA-hgr9-frvw-5r76)

## 0.9.87

- Added automatic Let's Encrypt certificate generation to `setup_wizard`
- Added [CONFIGURATION.md](CONFIGURATION.md)
- Improved the CLI interface of `setup_wizard` and provided better post-setup
  guidance there.

## 0.9.77

- Added install script for the endpoint
- Fixed project warnings
- Changed structure of the `scripts` folder
- Added linter scripts and reformatted the code accordingly

## 0.9.61

- Removed old docker image
- Added new [docker image](Dockerfile) with improved build and run logic

## 0.9.56

- Added a [docker image](docker/Dockerfile) with a configured and running endpoint.
- Added a [Makefile](Makefile) to simplify building and running the endpoint.
- Setup Wizard now doesn't ask for parameters specified through command line arguments.
  E.g., with `setup_wizard --lib-settings vpn.toml` it won't ask a user for the library
  settings file path.

## 0.9.47

- Removed RADIUS-based authenticator

## 0.9.45

- The executable now expects that the configuration files are TOML-formatted

## 0.9.38

- Fixed enormous timeout of TCP connections establishment procedure.
  API changes in the library:
    - added `connection_establishment_timeout` field into `settings::Settings`

  The executable related changes:
    - the settings file is changed accordingly to the changes described above

## 0.9.36

- The endpoint is now capable of handling service requests on the main tls domain.
  API changes in the library:
    - `tunnel_hosts` field of `settings::TlsHostsSettings` structure is renamed to `main_hosts`
    - `path_mask` field added into `settings::ReverseProxySettings`

  The executable related changes:
    - the settings file is changed accordingly to the changes described above

## 0.9.30

- Added support for dynamic reloading of TLS hosts settings.
  API changes in the library:
    - `tunnel_tls_hosts`, `ping_tls_hosts` and `speed_tls_hosts` from `settings::Settings`,
      and `tls_hosts` from `settings::ReverseProxySettings` were extracted into a dedicated
      structure `settings::TlsHostsSettings`
    - Added a new method for the reloading settings: `core::Core::reload_tls_hosts_settings()`

  The executable related changes:
    - The TLS hosts settings must be passed as a separate argument ([see here](./README.md#running) for details)
    - The new settings file structures are described ([see here](./README.md#library-configuration))
    - The executable is now handling the SIGHUP signal to trigger the reloading
      ([see here](./README.md#dynamic-reloading-of-tls-hosts-settings) for details)

## 0.9.29

- Removed blocking `core::Core::listen()` method. The library user must now set up a tokio runtime itself.
  The library API changes:
    - Removed `core::Core::listen()`
    - `core::Core::listen_async()` renamed to `core::Core::listen()`
    - Removed `threads_number` field from `settings::Settings`

  The executable related changes:
    - `threads_number` field in a settings file is now ignored
    - The number of worker threads may be specified via commandline argument (see the executable help for details)

## 0.9.28

- Added support for configuring the library with multiple TLS certificates.
  API changes:
    - `settings::Settings::tunnel_tls_host_info` is renamed to `settings::Settings::tunnel_tls_hosts` and is now a vector of hosts
    - `settings::Settings::ping_tls_host_info` is renamed to `settings::Settings::ping_tls_hosts` and is now a vector of hosts
    - `settings::Settings::speed_tls_host_info` is renamed to `settings::Settings::speed_tls_hosts` and is now a vector of hosts
    - `settings::ReverseProxySettings::tls_host_info` is renamed to `settings::ReverseProxySettings::tls_hosts` and is now a vector of hosts

## 0.9.24

- Added speedtest support

## 0.9.13

- Test changelog entry please ignore
