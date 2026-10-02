#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PKG_ROOT="$TMP/dl"
mkdir -p "$PKG_ROOT/breadboard-cli/1.0" "$PKG_ROOT/breadboard-desktop/1.0" "$PKG_ROOT/sibling/1.0"
printf old > "$PKG_ROOT/index.json"
printf old-signature > "$PKG_ROOT/index.json.minisig"
ln -s 1.0 "$PKG_ROOT/breadboard-cli/latest"
ln -s 1.0 "$PKG_ROOT/breadboard-desktop/latest"
ln -s 1.0 "$PKG_ROOT/sibling/latest"
printf '{}' > "$PKG_ROOT/breadboard-cli/1.0/bakery.toml"
printf cli > "$PKG_ROOT/breadboard-cli/1.0/breadboard-x86_64"
printf '{}' > "$PKG_ROOT/breadboard-desktop/1.0/bakery.toml"
printf desktop > "$PKG_ROOT/breadboard-desktop/1.0/desktop-x86_64"
printf '{}' > "$PKG_ROOT/sibling/1.0/bakery.toml"

REGISTRY="$TMP/registry.toml"
cat > "$REGISTRY" <<'EOF'
[[products]]
name = "breadboard-cli"
[[products]]
name = "breadboard-desktop"
[[products]]
name = "sibling"
EOF

GENERATOR="$TMP/gen-index.sh"
cat > "$GENERATOR" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exec {GENERATOR_LOCK}>"$INDEX_LOCK"
flock -x "$GENERATOR_LOCK"
if [[ -n "${FAKE_GEN_ENTERED:-}" ]]; then : > "$FAKE_GEN_ENTERED"; fi
if [[ "${FAKE_GEN_FAIL:-0}" == 1 ]]; then exit 42; fi
if [[ -n "${FAKE_GEN_RELEASE:-}" ]]; then
    while [[ ! -e "$FAKE_GEN_RELEASE" ]]; do sleep 0.01; done
fi
if [[ "$TRACK" == stable ]]; then out="$DL_DIR"; else out="$DL_DIR/$TRACK"; fi
cli="$(basename "$(readlink -f "$out/breadboard-cli/latest")")"
desktop="$(basename "$(readlink -f "$out/breadboard-desktop/latest")")"
printf '{"cli":"%s","desktop":"%s"}\n' "$cli" "$desktop" > "$out/index.json"
printf 'signed:%s:%s\n' "$cli" "$desktop" > "$out/index.json.minisig"
EOF
chmod +x "$GENERATOR"

make_stage() {
    local stage="$1"
    mkdir -p "$stage/breadboard-cli/2.0" "$stage/breadboard-desktop/2.0"
    printf '{}' > "$stage/breadboard-cli/2.0/bakery.toml"
    printf cli-v2 > "$stage/breadboard-cli/2.0/breadboard-x86_64"
    printf '{}' > "$stage/breadboard-desktop/2.0/bakery.toml"
    printf desktop-v2 > "$stage/breadboard-desktop/2.0/desktop-x86_64"
}

run_publisher() {
    local stage="$1"
    local -a optional_env=()
    for name in FAKE_GEN_FAIL FAKE_GEN_ENTERED FAKE_GEN_RELEASE; do
        if [[ -v "$name" ]]; then optional_env+=("$name=${!name}"); fi
    done
    env TRACK=stable DL_DIR="$PKG_ROOT" VERSION=2.0 STAGING="$stage" \
        REGISTRY="$REGISTRY" GEN_INDEX_SCRIPT="$GENERATOR" \
        "${optional_env[@]}" bash "$ROOT/scripts/publish-bakery-stage.sh"
}

assert_old_publication() {
    [[ "$(readlink "$PKG_ROOT/breadboard-cli/latest")" == 1.0 ]]
    [[ "$(readlink "$PKG_ROOT/breadboard-desktop/latest")" == 1.0 ]]
    [[ "$(cat "$PKG_ROOT/index.json")" == old ]]
    [[ "$(cat "$PKG_ROOT/index.json.minisig")" == old-signature ]]
    [[ ! -e "$PKG_ROOT/breadboard-cli/2.0" ]]
    [[ ! -e "$PKG_ROOT/breadboard-desktop/2.0" ]]
}

# A signing/generation failure occurs before any live path changes.
STAGE="$TMP/sign-failure"
mkdir -p "$STAGE"
make_stage "$STAGE"
if FAKE_GEN_FAIL=1 run_publisher "$STAGE" >"$TMP/generation.log" 2>&1; then
    echo "publisher unexpectedly accepted a failed index signer" >&2
    exit 1
fi
assert_old_publication

# A failure publishing the signature after links and index move restores the
# old links, index, signature, and removes the unreferenced new versions.
STAGE="$TMP/rename-failure"
mkdir -p "$STAGE"
make_stage "$STAGE"
mkdir -p "$TMP/fail-bin"
cat > "$TMP/fail-bin/mv" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
destination="${@: -1}"
marker="$(dirname "$0")/.failed-once"
if [[ "${destination##*/}" == index.json.minisig && ! -e "$marker" ]]; then
    : > "$marker"
    exit 1
fi
exec /usr/bin/mv "$@"
EOF
chmod +x "$TMP/fail-bin/mv"
if PATH="$TMP/fail-bin:$PATH" run_publisher "$STAGE" >"$TMP/rename.log" 2>&1; then
    cat "$TMP/rename.log" >&2
    echo "publisher unexpectedly accepted a failed signature install" >&2
    exit 1
fi
assert_old_publication

# Hold the publisher inside generation, then make another repo contend on the
# shared generator lock. It must wait and observe both newly published links.
STAGE="$TMP/concurrent"
mkdir -p "$STAGE"
make_stage "$STAGE"
FAKE_GEN_ENTERED="$TMP/generator-entered" FAKE_GEN_RELEASE="$TMP/generator-release" \
    run_publisher "$STAGE" > "$TMP/publisher.log" 2>&1 &
publisher_pid=$!
for _ in $(seq 1 300); do
    [[ -e "$TMP/generator-entered" ]] && break
    kill -0 "$publisher_pid" 2>/dev/null || { cat "$TMP/publisher.log" >&2; exit 1; }
    sleep 0.01
done
[[ -e "$TMP/generator-entered" ]] || { echo "publisher did not enter generation" >&2; exit 1; }
(
    exec {OTHER_LOCK}>"$PKG_ROOT/.gen-index.lock"
    flock -x "$OTHER_LOCK"
    printf '%s %s\n' \
        "$(readlink "$PKG_ROOT/breadboard-cli/latest")" \
        "$(readlink "$PKG_ROOT/breadboard-desktop/latest")" > "$TMP/other-generator-observed"
) &
other_pid=$!
sleep 0.1
if [[ -e "$TMP/other-generator-observed" ]]; then
    echo "competing generator entered while publication lock was held" >&2
    exit 1
fi
: > "$TMP/generator-release"
wait "$publisher_pid"
wait "$other_pid"
[[ "$(cat "$TMP/other-generator-observed")" == "2.0 2.0" ]]
[[ "$(cat "$PKG_ROOT/index.json")" == '{"cli":"2.0","desktop":"2.0"}' ]]
[[ "$(cat "$PKG_ROOT/index.json.minisig")" == 'signed:2.0:2.0' ]]
echo "publisher rollback and shared-lock concurrency checks passed"
