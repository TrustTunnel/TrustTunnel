# TrustTunnel Endpoint Binary

A standalone application that allows any user to easily set up their own VPN server.

---

## Configuration

- **Configuration via TOML Files:** The VPN endpoint binary utilizes TOML formatted
  files for configuration.
  The following files are used:
    - Library Settings File: This file contains the configuration of the underlying library and
      reflects the `Settings` struct.
    - TLS Hosts Settings File: This file defines the TLS hosts that the endpoint can represent to
      the client side.
      Different types of hosts are available, each serving a specific purpose.
      It reflects the `TlsHostsSettings` struct.

- **Additional Configuration Requirements:** In addition to the TOML files, two additional items are
  required for configuration:
    - Credentials File: This file contains user authentication data.
    - Certificate Files: These files correspond to the TLS hosts defined in the TLS hosts settings.

- **Setup Wizard Tool:** A setup wizard tool is provided within the repository, located in a
  separate directory. This tool simplifies the process of generating the required settings and files
  mentioned above. Refer to the [usage instructions](../README.md#usage) for quick setup
  instructions.

- **Command Line Configuration:** The VPN endpoint binary supports additional configuration options
  through command line arguments. Users can customize the behavior of the endpoint according to
  their specific requirements. To view the available options, run the following command in the
  Terminal:

   ```shell
   trusttunnel_endpoint -h
   ```

---

## Additional Features

## Dynamic Reloading of TLS Host Settings

The endpoint supports dynamic reloading of TLS host settings.
When the SIGHUP signal is sent to the endpoint process,
it will update and reload the TLS host settings on-the-fly without requiring a restart
of the binary.

The optional `[subscription]` section is reloaded together with the TLS hosts.
It cannot be added or removed by reload; toggle its `enabled` flag instead
(a restart changes its presence). The new configuration is validated and
applied atomically; on failure the previous configuration is kept.
