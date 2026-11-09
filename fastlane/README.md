# fastlane for `vpn-libs-endpoint`

## Purpose

This directory contains the fastlane automation used for macOS release signing
and notarization of:

- `trusttunnel_endpoint`

The flow was adapted from `trusttunnel-client` to fit the Cargo-based endpoint
repository.

## Prerequisites

Make sure the macOS build environment provides:

- Xcode command line tools
- Ruby with Bundler support
- access to the signing certificate repository used by `match`
- App Store Connect API credentials via Bamboo environment variables

If Xcode command line tools are missing, install them with:

```sh
xcode-select --install
```

Install Ruby dependencies with:

```sh
bundle config --local path '.bundle/vendor'
bundle install
```

## Environment

Example defaults are provided in `.env.default`.

Important environment variables include:

- `BUILD_DIR`
- `MATCH_GIT_URL`
- `MATCH_PASSWORD`
- `MATCH_KEYCHAIN_PASSWORD`
- `MATCH_KEYCHAIN_NAME`
- `MATCH_APP_IDENTIFIER`
- `bamboo_appStoreConnectApiKeyId`
- `bamboo_appStoreConnectApiKeyIssuerId`
- `bamboo_appStoreConnectApiKeyBase64Password`

## Available lanes

### `certs`

```sh
[bundle exec] fastlane certs
```

Syncs the Developer ID signing identity required for macOS release signing.

### `remove_certs`

```sh
[bundle exec] fastlane remove_certs
```

Removes the temporary local keychain created for signing.

### `notari`

```sh
[bundle exec] fastlane notari id:"<bundle_id>" bundle:"<path_to_binary>"
```

Notarizes the specified binary using the default App Store Connect credentials.

Required options:

- `id`: bundle identifier used for notarization
- `bundle`: path to the signed binary

## Notes

- Executables are compressed into a temporary archive before notarization.
- Stapling is intentionally skipped for executables.
- This directory is maintained manually for the endpoint repository and is not
  auto-generated.
