#!/usr/bin/env bash
# Called only by the gated release workflows, on the Hestia publisher runner.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRACK="${1:?track is required}"
case "$TRACK" in stable|beta|dev) ;; *) echo 'Unknown bakery track'; exit 1 ;; esac
: "${MINISIGN_SEC_KEY:?BAKERY_MINISIGN_SEC_KEY_PATH secret is required}"
test -r "$MINISIGN_SEC_KEY" || { echo 'Signing key is not readable'; exit 1; }
for tool in cargo npm python3 sha256sum strip minisign jq flock; do
    command -v "$tool" >/dev/null
done
cargo tauri --version >/dev/null
DL_DIR="${DL_DIR:-/srv/breadway-dl}"
PKG_ROOT="$DL_DIR"
[[ "$TRACK" == stable ]] || PKG_ROOT="$DL_DIR/$TRACK"
test -d "$PKG_ROOT" && test -w "$PKG_ROOT"
cd "$ROOT"
if [[ "$TRACK" == dev ]]; then
    git fetch origin 'refs/tags/v*:refs/tags/v*'
    BASE_VERSION="$(git tag --list 'v*' | sed 's/^v//' | (grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' || true) | sort -V | tail -1)"
    if [[ -z "$BASE_VERSION" ]]; then
        BASE_VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"
    fi
    IFS=. read -r MA MI PA <<< "$BASE_VERSION"
    VERSION="$MA.$MI.$((PA + 1))-dev.$(date -u +%Y%m%d%H%M%S)+$(git rev-parse --short HEAD)"
else
    VERSION="${GITHUB_REF_NAME#v}"
    [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] || {
        echo 'Only semantic version release tags are supported'; exit 1;
    }
    if [[ "$TRACK" == beta ]]; then
        [[ "$VERSION" == *-rc.* ]]
    else
        [[ "$VERSION" != *-rc.* ]]
    fi
    BASE_VERSION="${VERSION%%-rc.*}"
    python3 - "$BASE_VERSION" <<'PY'
import sys, tomllib
for path in ['Cargo.toml', 'apps/desktop/Cargo.toml']:
    with open(path, 'rb') as source:
        manifest = tomllib.load(source)
    version = manifest.get('workspace', {}).get('package', manifest.get('package', {}))['version']
    if version != sys.argv[1]:
        raise SystemExit(f'{path} version does not match release tag')
PY
fi
export VERSION
export CARGO_TARGET_DIR="$ROOT/target"
npm ci
npm run build
cargo build --release --locked -p breadboard-cli
# Build the documented headless-safe deb bundle; no app process is launched.
(cd apps/desktop && cargo tauri build --ci --bundles deb --config "{\"version\":\"$VERSION\"}" -- --locked)
STAGING="$(mktemp -d "$PKG_ROOT/.breadboard-stage.XXXXXX")"
ECOSYSTEM_CI_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGING" "$ECOSYSTEM_CI_DIR"' EXIT
git clone --branch main --depth 1 https://git.breadway.dev/Breadway/bread-ecosystem.git "$ECOSYSTEM_CI_DIR"
python3 - "$ECOSYSTEM_CI_DIR/registry/bread-ecosystem.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], 'rb') as source:
    products = {p['name'] for p in tomllib.load(source)['products']}
if not {'breadboard-cli', 'breadboard-desktop'} <= products:
    raise SystemExit('Merge the breadboard bakery registry dependency before publishing')
PY
BAKERY_STAGE_ROOT="$STAGING" CLI_BINARY="$ROOT/target/release/breadboard" \
    DESKTOP_BINARY="$ROOT/target/release/breadboard-desktop" \
    bash "$ROOT/scripts/stage-bakery.sh"
TRACK="$TRACK" DL_DIR="$DL_DIR" VERSION="$VERSION" \
    STAGING="$STAGING" \
    REGISTRY="$ECOSYSTEM_CI_DIR/registry/bread-ecosystem.toml" \
    GEN_INDEX_SCRIPT="$ECOSYSTEM_CI_DIR/scripts/gen-index.sh" \
    bash "$ROOT/scripts/publish-bakery-stage.sh"
