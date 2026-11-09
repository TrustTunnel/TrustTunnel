BUILD_TYPE ?= release
ifeq ($(BUILD_TYPE), release)
	CARGO_BUILD_TYPE = --release
endif
LOG_LEVEL ?= trace
CONFIG_FILE ?= vpn.toml
HOSTS_CONFIG_FILE ?= hosts.toml
DOCKER_IMAGE_NAME ?= trusttunnel-endpoint
ENDPOINT_URL ?= git@github.com:TrustTunnel/TrustTunnel.git
ENDPOINT_VERSION ?= master
ENDPOINT_HOSTNAME ?= vpn.endpoint
DOCKER_DIR = docker
DOCKER_ENDPOINT_DIR = TrustTunnel
DOCKER_ENDPOINT_CONFIG_DIR = config
LISTEN_ADDRESS ?= 0.0.0.0
LISTEN_PORT ?= 443

.PHONY: init
## Initialize the development environment (git hooks, etc.)
init:
	git config core.hooksPath ./scripts/hooks

.PHONY: endpoint/build-wizard
## Build the setup wizard
endpoint/build-wizard:
	cargo build $(CARGO_BUILD_TYPE) --bin trusttunnel_endpoint_setup_wizard

.PHONY: endpoint/setup
## Run the setup wizard to create all the required configuration files
endpoint/setup: endpoint/build-wizard
	cargo run $(CARGO_BUILD_TYPE) --bin trusttunnel_endpoint_setup_wizard -- \
		--hostname "$(ENDPOINT_HOSTNAME)" \
		--address "$(LISTEN_ADDRESS):$(LISTEN_PORT)" \
		--lib-settings "$(CONFIG_FILE)" \
		--hosts-settings "$(HOSTS_CONFIG_FILE)"

.PHONY: endpoint/build
## Build the endpoint
endpoint/build:
	cargo build $(CARGO_BUILD_TYPE) --bin trusttunnel_endpoint

.PHONY: endpoint/run
## Run the endpoint with the existing configuration files
endpoint/run: endpoint/build
	cargo run $(CARGO_BUILD_TYPE) --bin trusttunnel_endpoint -- \
		-l "$(LOG_LEVEL)" "$(CONFIG_FILE)" "$(HOSTS_CONFIG_FILE)"

.PHONY: endpoint/gen_client_config
## Generate the config for specified client to be used with vpn client and exit
endpoint/gen_client_config:
	$(if $(CLIENT_NAME),,$(error CLIENT_NAME is not set. Specify the client name to generate the config for))
	$(if $(ENDPOINT_ADDRESS),,$(error ENDPOINT_ADDRESS is not set. Set it to `ip:port` that client is going to use to connect to the endpoint))
	cargo run $(CARGO_BUILD_TYPE) --bin trusttunnel_endpoint -- \
		-c "$(CLIENT_NAME)" --address "$(ENDPOINT_ADDRESS)" "$(CONFIG_FILE)" "$(HOSTS_CONFIG_FILE)"

.PHONY: endpoint/clean
## Clean cargo artifacts
endpoint/clean:
	cargo clean

.PHONY: lint
lint: lint-md lint-rust

## Lint markdown files.
## `markdownlint-cli` should be installed:
##    macOS: `brew install markdownlint-cli`
##    Linux: `npm install -g markdownlint-cli`
.PHONY: lint-md
lint-md:
	markdownlint .

## Check Rust code formatting with rustfmt.
## `rustfmt` should be installed:
##    rustup component add rustfmt
.PHONY: lint-rust
lint-rust:
	cargo fmt --all -- --check
	cargo clippy -- -D warnings

## Fix linter issues that are auto-fixable.
.PHONY: lint-fix
lint-fix: lint-fix-rust lint-fix-md

## Auto-fix Rust code formatting issues with rustfmt.
.PHONY: lint-fix-rust
lint-fix-rust:
	cargo clippy --fix --allow-dirty
	cargo fmt --all

## Auto-fix markdown files.
.PHONY: lint-fix-md
lint-fix-md:
	markdownlint --fix .

.PHONY: test
test: test-rust

test-rust:
	cargo test --workspace
