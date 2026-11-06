#!/usr/bin/env bash
# Publish two staged products and the signed track index as one recoverable
# transaction. The shared lock is the one used by every gen-index invocation.
set -euo pipefail

: "${TRACK:?TRACK is required}"
: "${DL_DIR:?DL_DIR is required}"
: "${VERSION:?VERSION is required}"
: "${STAGING:?STAGING is required}"
: "${REGISTRY:?REGISTRY is required}"
: "${GEN_INDEX_SCRIPT:?GEN_INDEX_SCRIPT is required}"
case "$TRACK" in stable) PKG_ROOT="$DL_DIR" ;; dev|beta) PKG_ROOT="$DL_DIR/$TRACK" ;; *) exit 1 ;; esac

products=(breadboard-cli breadboard-desktop)
# This path is shared with the generator in every ecosystem repository. Keep
# it locked from the snapshot read through index/signature publication.
exec {PUBLISH_LOCK}>"$PKG_ROOT/.gen-index.lock"
flock -x "$PUBLISH_LOCK"

for product in "${products[@]}"; do
    test -d "$STAGING/$product/$VERSION" || {
        echo "Missing staged release: $product/$VERSION" >&2
        exit 1
    }
    test ! -e "$PKG_ROOT/$product/$VERSION" || {
        echo "Refusing to overwrite a published version: $product/$VERSION" >&2
        exit 1
    }
    if [[ -e "$PKG_ROOT/$product/latest" && ! -L "$PKG_ROOT/$product/latest" ]]; then
        echo "Refusing to replace a non-symlink latest path: $product/latest" >&2
        exit 1
    fi
done

INDEX_ROOT="$STAGING/index-root"
INDEX_PKG_ROOT="$INDEX_ROOT"
[[ "$TRACK" == stable ]] || INDEX_PKG_ROOT="$INDEX_ROOT/$TRACK"
mkdir -p "$INDEX_PKG_ROOT"

# Point untouched products at the live tree. The two changed product views
# get private directories whose latest links point at the staged versions.
mapfile -t registry_products < <(python3 - "$REGISTRY" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as source:
    for product in tomllib.load(source)["products"]:
        print(product["name"])
PY
)
for product in "${registry_products[@]}"; do
    if [[ " ${products[*]} " == *" $product "* ]]; then
        continue
    fi
    if [[ -d "$PKG_ROOT/$product" ]]; then
        ln -s "$PKG_ROOT/$product" "$INDEX_PKG_ROOT/$product"
    fi
done
for product in "${products[@]}"; do
    live_product="$PKG_ROOT/$product"
    snapshot_product="$INDEX_PKG_ROOT/$product"
    mkdir -p "$snapshot_product"
    if [[ -d "$live_product" ]]; then
        for entry in "$live_product"/*; do
            [[ -e "$entry" || -L "$entry" ]] || continue
            [[ "${entry##*/}" == latest ]] && continue
            ln -s "$entry" "$snapshot_product/${entry##*/}"
        done
    fi
    ln -s "$STAGING/$product/$VERSION" "$snapshot_product/latest"
done

# The generator uses its own flock. Point that inner lock at the private
# snapshot, while this process holds the real shared lock across publication.
TRACK="$TRACK" DL_DIR="$INDEX_ROOT" INDEX_LOCK="$STAGING/.index-generation.lock" \
    bash "$GEN_INDEX_SCRIPT"

INDEX_NEW="$INDEX_PKG_ROOT/index.json"
SIGNATURE_NEW="$INDEX_NEW.minisig"
test -s "$INDEX_NEW" && test -s "$SIGNATURE_NEW" || {
    echo "Index generation did not produce a signed index" >&2
    exit 1
}

INDEX_LIVE="$PKG_ROOT/index.json"
SIGNATURE_LIVE="$INDEX_LIVE.minisig"
INDEX_BACKUP="$STAGING/index.json.previous"
SIGNATURE_BACKUP="$STAGING/index.json.minisig.previous"
HAD_INDEX=0
HAD_SIGNATURE=0
if [[ -e "$INDEX_LIVE" ]]; then cp -p "$INDEX_LIVE" "$INDEX_BACKUP"; HAD_INDEX=1; fi
if [[ -e "$SIGNATURE_LIVE" ]]; then cp -p "$SIGNATURE_LIVE" "$SIGNATURE_BACKUP"; HAD_SIGNATURE=1; fi

old_latest=()
had_latest=()
product_dir_existed=()
for product in "${products[@]}"; do
    latest="$PKG_ROOT/$product/latest"
    if [[ -L "$latest" ]]; then
        old_latest+=("$(readlink -- "$latest")")
        had_latest+=(1)
    else
        old_latest+=("")
        had_latest+=(0)
    fi
    if [[ -d "$PKG_ROOT/$product" ]]; then product_dir_existed+=(1); else product_dir_existed+=(0); fi
done

moved_versions=(0 0)
committed=0
rollback() {
    rc=$?
    trap - ERR INT TERM HUP
    [[ "$rc" -ne 0 ]] || rc=1
    if [[ "$committed" -eq 1 ]]; then exit "$rc"; fi
    set +e
    for index in 1 0; do
        product="${products[$index]}"
        latest="$PKG_ROOT/$product/latest"
        tmp_link="$PKG_ROOT/$product/.latest.rollback.$$"
        rm -f -- "$tmp_link"
        if [[ "${had_latest[$index]}" -eq 1 ]]; then
            ln -s -- "${old_latest[$index]}" "$tmp_link" && mv -Tf -- "$tmp_link" "$latest"
        else
            rm -f -- "$latest"
        fi
        if [[ "${moved_versions[$index]}" -eq 1 ]]; then
            rm -rf -- "$PKG_ROOT/$product/$VERSION"
        fi
        if [[ "${product_dir_existed[$index]}" -eq 0 ]]; then
            rmdir -- "$PKG_ROOT/$product" 2>/dev/null || true
        fi
    done
    if [[ "$HAD_INDEX" -eq 1 ]]; then
        cp -p -- "$INDEX_BACKUP" "$INDEX_LIVE.rollback.$$" &&
            mv -f -- "$INDEX_LIVE.rollback.$$" "$INDEX_LIVE" ||
            echo "ERROR: could not restore previous index.json" >&2
    else
        rm -f -- "$INDEX_LIVE"
    fi
    if [[ "$HAD_SIGNATURE" -eq 1 ]]; then
        cp -p -- "$SIGNATURE_BACKUP" "$SIGNATURE_LIVE.rollback.$$" &&
            mv -f -- "$SIGNATURE_LIVE.rollback.$$" "$SIGNATURE_LIVE" ||
            echo "ERROR: could not restore previous index signature" >&2
    else
        rm -f -- "$SIGNATURE_LIVE"
    fi
    exit "$rc"
}
trap rollback ERR INT TERM HUP

for index in 0 1; do
    product="${products[$index]}"
    mkdir -p "$PKG_ROOT/$product"
    mv -- "$STAGING/$product/$VERSION" "$PKG_ROOT/$product/$VERSION"
    moved_versions[$index]=1
done
for product in "${products[@]}"; do
    latest="$PKG_ROOT/$product/latest"
    tmp_link="$PKG_ROOT/$product/.latest.publish.$$"
    ln -s -- "$VERSION" "$tmp_link"
    mv -Tf -- "$tmp_link" "$latest"
done
mv -f -- "$INDEX_NEW" "$INDEX_LIVE"
mv -f -- "$SIGNATURE_NEW" "$SIGNATURE_LIVE"
committed=1
trap - ERR INT TERM HUP
echo "Published ${products[*]} $VERSION and signed $TRACK index"
