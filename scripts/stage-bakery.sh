#!/usr/bin/env bash
# Prepare artifacts in a caller-owned staging directory, with no installation.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${VERSION:?VERSION is required}"
: "${BAKERY_STAGE_ROOT:?BAKERY_STAGE_ROOT is required}"
: "${CLI_BINARY:?CLI_BINARY is required}"
: "${DESKTOP_BINARY:?DESKTOP_BINARY is required}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$ ]] || {
    echo 'VERSION must be a semantic version'; exit 1;
}
test -x "$CLI_BINARY"
test -x "$DESKTOP_BINARY"
for product in breadboard-cli breadboard-desktop; do
    test ! -e "$BAKERY_STAGE_ROOT/$product/$VERSION" || {
        echo "Refusing to replace existing staged version: $product/$VERSION"; exit 1;
    }
done
for kind in cli desktop; do
    product="breadboard-$kind"
    pkg_dir="$BAKERY_STAGE_ROOT/$product/$VERSION"
    mkdir -p "$pkg_dir"
    if [[ "$kind" == cli ]]; then
        binary=breadboard; source_binary="$CLI_BINARY"
    else
        binary=breadboard-desktop; source_binary="$DESKTOP_BINARY"
        cp "$ROOT/packaging/breadboard-desktop.desktop" "$pkg_dir/"
    fi
    cp "$source_binary" "$pkg_dir/$binary-x86_64"
    strip "$pkg_dir/$binary-x86_64"
    sha256sum "$pkg_dir/$binary-x86_64" | awk '{print $1}' > "$pkg_dir/$binary-x86_64.sha256"
    cp "$ROOT/packaging/bakery/$kind.toml" "$pkg_dir/bakery.toml"
    cp "$ROOT/LICENSE" "$pkg_dir/"
done
