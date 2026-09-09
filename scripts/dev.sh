#!/usr/bin/env bash
# Local dev: run breadboard-server (API on :7280) and the Vite dev server
# (UI on :5280, proxying /api + /events + /auth to :7280).
#
#   BREADBOARD_DEV_LOGIN=1 scripts/dev.sh
#
# Open http://127.0.0.1:5280: the sign-in screen has a "dev login" box when
# BREADBOARD_DEV_LOGIN=1, so you don't need to complete the Forgejo OAuth flow.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p breadboard-server
./target/debug/breadboard-server &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT

npm --prefix apps/web run dev
