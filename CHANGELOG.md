# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

- `allow_private_network_connections = false` no longer blocks the `[reverse_proxy]` `server_address`: the origin server address comes from the endpoint configuration, so a loopback or private address stays reachable while tunneled client traffic to private networks remains forbidden.
- `is_global_ipv6` incorrectly classified global unicast IPv6 addresses (e.g. `2001:4860:4860::8888`) as non-global.
- UDP timeout cleanup no longer leaks outbound sockets: the expired-connection path now passes the reverse-oriented meta to `on_connection_closed`, matching the direct forwarder's orientation contract so the kernel UDP socket and its `outbound_udp_sockets` metric are actually released after `udp_connections_timeout_secs` of inactivity.

### Security

## [1.0.41] - 2026-04-30

### Added

- `non_connect_auth_failure_status_code` config key to control the HTTP status code returned on authentication failure for non-CONNECT requests. Extend allowed values for `auth_failure_status_code` and `non_connect_auth_failure_status_code` to 407, 405, 404, and 403.

## [1.0.28] - 2026-03-25

### Added

- `trusttunnel_endpoint -c` can now generate `client_random_prefix` values automatically, append matching allow rules to `rules.toml`, and embed the generated value into exported client configs.

## [1.0.17] - 2026-03-10

### Added

- `ping_enable`, `ping_path`, `speedtest_enable` and `speedtest_path` config keys to configure ping and speedtest handlers.
- `auth_failure_status_code` config key to control the HTTP status code returned on authentication failure (407 or 405). Defaults to 407.

### Fixed

- Reverse proxy routing for H2/H3.

## [1.0.16] - 2026-03-09

### Fixed

- HTTP/1.1 codec busy loop when receiving partial request headers.

## [1.0.13] - 2026-03-04

### Fixed

- Change deep-link format from `tt://` to `tt://?`. For backward compatibility, `tt://` is still supported.

## [1.0.11] - 2026-03-02

### Security

- Fixed traffic leaking to local network via UDP, ICMP, and SOCKS5 forwarders
  when `allow_private_network_connections` is set to `false`.
    - Added `is_global_ip` check to UDP forwarder
    - Added `is_global_ip` check to ICMP forwarder
    - Added `is_global_ip` check to SOCKS5 forwarder (TCP and UDP)
    - Handle IPv4-mapped IPv6 addresses (`::ffff:x.x.x.x`) in `is_global_ip`
  (Based on [GitHub PR #79](https://github.com/TrustTunnel/TrustTunnel/pull/79) by @andrew-morris)

## [1.0.7] - 2026-02-26

### Added

- Per-client connection limits
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

## [1.0.6] - 2026-02-26

### Added

- Support for X25519MLKEM768 post-quantum group.

## [1.0.5] - 2026-02-26

### Added

- The `-a` flag now accepts `domain` and `domain:port` in addition to `ip` and `ip:port`.
  The exported client configuration will contain the domain name, which the client resolves via DNS at connect time.
- Deep-link format (`tt://`) now supports domain names in the `addresses` field.
- When listening on `[::]`, the endpoint now explicitly sets `IPV6_V6ONLY=false` to accept
  both IPv4 and IPv6 connections on a single socket (dual-stack).

## [1.0.1] - 2026-02-25

### Added

- New `trusttunnel-deeplink` library crate for encoding/decoding `tt://` URIs
- `client_random_prefix` field to client configuration export
    - New CLI option `--client-random-prefix`
    - Validates hex format and checks against `rules.toml`
    - Added to deep-link format as tag 0x0B

## [0.9.127] - 2026-02-09

### Added

- GPG signing of the endpoint binaries.

## [0.9.122] - 2026-01-30

### Changed

- Endpoint now requires credentials when listening on a public address.
- Added support of shortened QUIC settings names in configuration files.

## [0.9.115] - 2026-01-23

### Security

- Fixed an issue where `client_random_prefix` rules didn't match when Anti-DPI or post-quantum cryptography was enabled.
  (<https://github.com/TrustTunnel/TrustTunnel/security/advisories/GHSA-fqh7-r5gf-3r87>)

## [0.9.114] - 2026-01-23

### Security

- Fixed an issue where `allow_private_network_connections` set to false could be bypassed
  when a numeric address was used.
  (<https://github.com/TrustTunnel/TrustTunnel/security/advisories/GHSA-hgr9-frvw-5r76>)

## [0.9.87] - 2025-12-22

### Added

- Automatic Let's Encrypt certificate generation to `setup_wizard`
- [CONFIGURATION.md](CONFIGURATION.md)
- Improved the CLI interface of `setup_wizard` and provided better post-setup
  guidance there.

## [0.9.77] - 2025-12-21

### Added

- Install script for the endpoint
- Linter scripts and reformatted the code accordingly

### Changed

- Structure of the `scripts` folder

### Fixed

- Project warnings

## [0.9.61] - 2024-12-10

### Changed

- Removed old docker image
- Added new [docker image](Dockerfile) with improved build and run logic

## [0.9.56] - 2023-07-20

### Added

- A [docker image](docker/Dockerfile) with a configured and running endpoint.
- A [Makefile](Makefile) to simplify building and running the endpoint.
- Setup Wizard now doesn't ask for parameters specified through command line arguments.
  E.g., with `setup_wizard --lib-settings vpn.toml` it won't ask a user for the library
  settings file path.

## [0.9.47] - 2023-06-26

### Removed

- RADIUS-based authenticator

## [0.9.45] - 2023-06-06

### Changed

- The executable now expects that the configuration files are TOML-formatted

## [0.9.38] - 2023-04-03

### Fixed

- Enormous timeout of TCP connections establishment procedure.
  API changes in the library:
    - added `connection_establishment_timeout` field into `settings::Settings`

  The executable related changes:
    - the settings file is changed accordingly to the changes described above

## [0.9.36] - 2023-04-03

### Changed

- The endpoint is now capable of handling service requests on the main tls domain.
  API changes in the library:
    - `tunnel_hosts` field of `settings::TlsHostsSettings` structure is renamed to `main_hosts`
    - `path_mask` field added into `settings::ReverseProxySettings`

  The executable related changes:
    - the settings file is changed accordingly to the changes described above

## [0.9.30] - 2023-03-06

### Added

- Support for dynamic reloading of TLS hosts settings.
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

## [0.9.29] - 2023-03-06

### Changed

- Removed blocking `core::Core::listen()` method. The library user must now set up a tokio runtime itself.
  The library API changes:
    - Removed `core::Core::listen()`
    - `core::Core::listen_async()` renamed to `core::Core::listen()`
    - Removed `threads_number` field from `settings::Settings`

  The executable related changes:
    - `threads_number` field in a settings file is now ignored
    - The number of worker threads may be specified via commandline argument (see the executable help for details)

## [0.9.28] - 2023-03-03

### Changed

- Added support for configuring the library with multiple TLS certificates.
  API changes:
    - `settings::Settings::tunnel_tls_host_info` is renamed to `settings::Settings::tunnel_tls_hosts` and is now a vector of hosts
    - `settings::Settings::ping_tls_host_info` is renamed to `settings::Settings::ping_tls_hosts` and is now a vector of hosts
    - `settings::Settings::speed_tls_host_info` is renamed to `settings::Settings::speed_tls_hosts` and is now a vector of hosts
    - `settings::ReverseProxySettings::tls_host_info` is renamed to `settings::ReverseProxySettings::tls_hosts` and is now a vector of hosts

## [0.9.24] - 2022-11-21

### Added

- Speedtest support

## [0.9.13]

### Changed

- Test changelog entry please ignore

[Unreleased]: https://github.com/TrustTunnel/TrustTunnel/compare/1f3ffda5...HEAD
[1.0.41]: https://github.com/TrustTunnel/TrustTunnel/compare/32bc4a47...1f3ffda5
[1.0.28]: https://github.com/TrustTunnel/TrustTunnel/compare/v1.0.17...32bc4a47
[1.0.17]: https://github.com/TrustTunnel/TrustTunnel/compare/v1.0.16...v1.0.17
[1.0.16]: https://github.com/TrustTunnel/TrustTunnel/compare/v1.0.13...v1.0.16
[1.0.13]: https://github.com/TrustTunnel/TrustTunnel/compare/fcb591a5...v1.0.13
[1.0.11]: https://github.com/TrustTunnel/TrustTunnel/compare/v1.0.7...fcb591a5
[1.0.7]: https://github.com/TrustTunnel/TrustTunnel/compare/4675d862...v1.0.7
[1.0.6]: https://github.com/TrustTunnel/TrustTunnel/compare/f116809c...4675d862
[1.0.5]: https://github.com/TrustTunnel/TrustTunnel/compare/c4534c94...f116809c
[1.0.1]: https://github.com/TrustTunnel/TrustTunnel/compare/1f2def68...c4534c94
[0.9.127]: https://github.com/TrustTunnel/TrustTunnel/compare/a3fb4efe...1f2def68
[0.9.122]: https://github.com/TrustTunnel/TrustTunnel/compare/v0.9.115...a3fb4efe
[0.9.115]: https://github.com/TrustTunnel/TrustTunnel/compare/881d1fd8...v0.9.115
[0.9.114]: https://github.com/TrustTunnel/TrustTunnel/compare/v0.9.87...881d1fd8
[0.9.87]: https://github.com/TrustTunnel/TrustTunnel/compare/e1b72906...v0.9.87
[0.9.77]: https://github.com/TrustTunnel/TrustTunnel/compare/8e018cd5...e1b72906
[0.9.61]: https://github.com/TrustTunnel/TrustTunnel/compare/a95f1ac9...8e018cd5
[0.9.56]: https://github.com/TrustTunnel/TrustTunnel/compare/v0.9.47...a95f1ac9
[0.9.47]: https://github.com/TrustTunnel/TrustTunnel/compare/07bf6696...v0.9.47
[0.9.45]: https://github.com/TrustTunnel/TrustTunnel/compare/v0.9.38...07bf6696
[0.9.38]: https://github.com/TrustTunnel/TrustTunnel/compare/ca3ca686...v0.9.38
[0.9.36]: https://github.com/TrustTunnel/TrustTunnel/compare/cfd1d175...ca3ca686
[0.9.30]: https://github.com/TrustTunnel/TrustTunnel/compare/02bff06b...cfd1d175
[0.9.29]: https://github.com/TrustTunnel/TrustTunnel/compare/fab0fa25...02bff06b
[0.9.28]: https://github.com/TrustTunnel/TrustTunnel/compare/v0.9.24...fab0fa25
[0.9.24]: https://github.com/TrustTunnel/TrustTunnel/compare/c0005a6...v0.9.24
[0.9.13]: https://github.com/TrustTunnel/TrustTunnel/commit/c0005a6
