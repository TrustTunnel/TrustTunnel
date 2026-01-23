# Development documentation

## Table of Contents

- [Getting Started](#getting-started)
    - [Prerequisites](#prerequisites)
    - [Building](#building)
    - [Cross-compiling for Linux](#cross-compiling-for-linux)
- [Usage](#usage)
    - [Setup](#setup)
    - [Customized Configuration](#customized-configuration)
- [See Also](#see-also)

## Getting Started

### Prerequisites

This project is compatible with Linux and macOS systems.

Run `make init` to prepare the development environment.

- Rust version 1.85 or higher
    - macOS/Linux: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.85 -y`
- CMake version 3.31.6 or higher
    - macOS: `brew install cmake`
    - Linux (Debian/Ubuntu): `apt install cmake`
- [libclang](https://rust-lang.github.io/rust-bindgen/requirements.html#installing-clang) library 9.0 or higher.
- C++ compiler
    - macOS: `xcode-select --install` but you likely already have it if you are using `brew`
    - Linux (Debian/Ubuntu): `apt install build-essential`

For running linters and tests, you additionally need:

- Node.js version 22.12 or higher
    - macOS: `brew install node`
    - Linux (Debian/Ubuntu): `apt install nodejs`
- Markdownlint
    - `npm install -g markdownlint-cli`

### Building

Build the binaries using Cargo:

```shell
 cargo build --bins --release
```

Or to build binaries for debug:

```shell
 cargo build --bins
```

These commands will generate the executables in the `target/release` or `target/debug` directory accordingly.

### Cross-compiling for Linux

To build for Linux (x86_64-unknown-linux-musl) from macOS or other platforms, use the Docker-based build:

```shell
docker run --rm --platform linux/amd64 -v "$(pwd)":/work -w /work adguard/core-libs:2.8 sh -c '\
    CC=x86_64-linux-musl-gcc \
    CXX=x86_64-linux-musl-g++ \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc \
    BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/opt/cross/x86_64-linux-musl" \
    cargo build --release --target x86_64-unknown-linux-musl'
```

This will produce the binaries in `target/x86_64-unknown-linux-musl/release/`.

## Usage

### Setup

To quickly configure and launch the VPN endpoint, run the following commands:

```shell
make ENDPOINT_HOSTNAME="example.org" endpoint/setup  # You can skip it if you have already configured the endpoint earlier
make endpoint/run
```

Check `Makefile` for available configuration variables.

These commands perform the following actions:

1. Build the wizard and endpoint binaries.

2. Configure the endpoint to listen to all network interfaces for TCP/UDP packets on
   port number 443.

3. Generate self-signed certificate/private key pair in the current directory under `certs/`.

4. Store all the required settings in `vpn.toml` and `hosts.toml` files.

5. Start the endpoint.

Alternatively, you can run the endpoint in a Docker container:

```shell
docker build -t trusttunnel-endpoint:latest . # Build an image

docker run -it trusttunnel-endpoint:latest --name trusttunnel-endpoint # Create a Docker container and start it in interactive mode

docker start -i trusttunnel-endpoint # If you need to start your VPN endpoint again
```

### Customized Configuration

For a more customized configuration experience, run the following commands:

```shell
make endpoint/build-wizard  # If you skipped the previous section
cargo run --bin setup_wizard  # Launch a dialogue session allowing you to tweak the settings
cargo run --bin trusttunnel_endpoint -- <lib-settings> <hosts-settings>  # File names depend on the previous step
```

For additional details about the binary, refer to the [endpoint/README.md](./endpoint/README.md)
file.

---

## See Also

- [README.md](README.md) - Quick start guide
- [PROTOCOL.md](PROTOCOL.md) - Protocol specification
- [CONFIGURATION.md](CONFIGURATION.md) - Configuration documentation
- [CHANGELOG.md](CHANGELOG.md) - Changelog
