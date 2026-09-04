# breadboard

Issue tracker for the bread ecosystem — a Linear-lite planning layer that sits on
top of Forgejo (`git.breadway.dev`).

Forgejo stays the place issues are filed and discussed. breadboard adds what a
per-repo issue list can't give you:

- **cross-repo epics** — one epic spans `bread`, `breadbar`, `bos`, …
- **a board that spans every repo** — every in-flight task in one place
- **cycles** (the sprint concept), priorities, estimates
- **a roadmap** view for native (non-repo) planning items

…and keeps breadboard and Forgejo in **bidirectional sync** (webhooks in, API
writeback, periodic reconcile, per-field last-write-wins with an append-only
audit log).

## Philosophy

Deliberately *not* Jira. The things it refuses to become:

| Avoids | Instead |
| --- | --- |
| Slow, heavy UI | sub-100ms interactions, optimistic updates, keyboard-first, `cmd-k` |
| Configuration sprawl | one global workflow, one settings page, no custom fields |
| Notification noise | nothing pings by default; opt in per channel; optional daily digest |
| Process theater | create a card with just a title; cycles and estimates are optional |

The one global workflow: `Backlog → Todo → Doing → Review → Done`.

## Layout

```
crates/
  breadboard-core     shared domain types (serde + chrono only)
  breadboard-server   axum service: REST + SSE, Forgejo sync, SQLite (WAL) + Litestream
  breadboard-client   Rust API client (used by the CLI/TUI)
  breadboard-cli      `breadboard` — CLI + TUI
apps/
  web                 Svelte 5 SPA (Vite) — the primary UI
  desktop             Tauri 2 shell wrapping the web build
packages/
  api-client          generated TypeScript client
migrations/           sqlx migrations
xtask/                `cargo run -p xtask -- check-docs`
```

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo run -p breadboard-cli -- ping
```

Full stack (API + hot-reloading UI) in one command:

```sh
npm install
BREADBOARD_DEV_LOGIN=1 scripts/dev.sh      # UI on :5280, API on :7280
```

Open <http://127.0.0.1:5280>. With `BREADBOARD_DEV_LOGIN=1` the sign-in screen
has a "dev login" box, so you can skip the Forgejo OAuth round-trip locally.

Config: `~/.config/breadboard/breadboard.toml` (server) and
`~/.config/breadboard/cli.toml` (CLI). Every key is optional; secrets can also
come from `BREADBOARD_*` env vars.

## Deployment

The server serves the built SPA itself when `BREADBOARD_WEB_DIR` points at
`apps/web/dist` — one origin, no separate web server. On Hestia it runs as a
container (`packaging/Containerfile` + `packaging/compose.yaml`, with a
Litestream sidecar), public via the Aegis Caddy edge
(`packaging/caddy/aegis-breadboard.caddy` → `breadboard.breadway.dev`).

CLI + desktop are distributed through bakery. See `Documentation.md` for the API
and event contract, and the plan for the rollout.

Part of the [bread ecosystem](https://git.breadway.dev/Breadway). MIT licensed.
