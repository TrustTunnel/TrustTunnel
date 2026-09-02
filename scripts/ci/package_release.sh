#!/bin/bash

set -e -x

# The released version: prefer the TRUSTTUNNEL_VERSION override (set by CI from
# CHANGELOG.md), then fall back to the nearest git tag (a plain local build).
# endpoint/Cargo.toml is intentionally a 0.0.0 placeholder and is NOT the source
# of truth.
if [ -n "${TRUSTTUNNEL_VERSION:-}" ]; then
  VERSION="${TRUSTTUNNEL_VERSION}"
else
  VERSION=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null | sed 's/^v//')
fi

if [ -z "$VERSION" ]; then
  echo "::error::Could not determine the release version (set TRUSTTUNNEL_VERSION or run from a tagged commit)" >&2
  exit 1
fi

GPG_KEY=devteam@adguard.com

echo "$GPG_SECRET_KEY" | gpg --import --batch --yes

mkdir -p artifacts

for ARCH in x86_64 aarch64; do
  RELEASE_DIR="target/${ARCH}-unknown-linux-musl/release"

  pushd "$RELEASE_DIR"
    llvm-objcopy --only-keep-debug trusttunnel_endpoint trusttunnel_endpoint.debug
    llvm-strip trusttunnel_endpoint
    llvm-objcopy --add-gnu-debuglink=trusttunnel_endpoint.debug trusttunnel_endpoint

    llvm-objcopy --only-keep-debug setup_wizard setup_wizard.debug
    llvm-strip setup_wizard
    llvm-objcopy --add-gnu-debuglink=setup_wizard.debug setup_wizard

    cp "${OLDPWD}/LICENSE" .
    cp "${OLDPWD}/scripts/trusttunnel.service.template" .

    gpg --default-key "${GPG_KEY}" \
        --detach-sig \
        --passphrase "${GPG_PASSWORD}" \
        --pinentry-mode loopback \
        trusttunnel_endpoint
    gpg --default-key "${GPG_KEY}" \
        --detach-sig \
        --passphrase "${GPG_PASSWORD}" \
        --pinentry-mode loopback \
        setup_wizard

    NAME="trusttunnel-v${VERSION}-linux-${ARCH}"
    tar zcf "${NAME}.tar.gz" --transform "s,^,${NAME}/," \
      trusttunnel_endpoint trusttunnel_endpoint.sig \
      setup_wizard setup_wizard.sig \
      LICENSE trusttunnel.service.template
    mv "${NAME}.tar.gz" "${OLDPWD}/artifacts/"

    NAME_DBG="${NAME}-dbgsym"
    tar zcf "${NAME_DBG}.tar.gz" --transform "s,^,${NAME_DBG}/," \
      trusttunnel_endpoint.debug setup_wizard.debug
    mv "${NAME_DBG}.tar.gz" "${OLDPWD}/artifacts/"
  popd
done
