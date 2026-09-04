# AGENTS.md — Repo hygiene

This repo follows the branch/release workflow in `CONTRIBUTING.md`, which is the
same one every bread-ecosystem product uses. Read that before any git, branch, or
release work. Do not invent a different workflow.

Day-to-day: one long-lived branch (`main`). New work goes on `feature/<name>` or
`fix/<name>`, then back into `main`. There is no `dev` or `beta` branch — those
names are **bakery tracks**, published from `main` (dev) and from tags
(`vX.Y.Z-rc.N` = beta, `vX.Y.Z` = stable).

The API surface, REST routes, event names (`bread.board.*`), and the sync
contract live in `Documentation.md`, kept honest by
`cargo run -p xtask -- check-docs` (enforced in CI). Treat that file, not this
one, as the source of truth for the contract.

## Remotes

- `origin` — Forgejo (`git.breadway.dev`) — authoritative.
- `github` — mirror. It auto-mirrors from `origin`; push `origin` only.

## CI

Runs on a self-hosted runner. Nothing runs on plain commits/PRs beyond what is
listed here.

- `check.yml` — `feature/**`, `fix/**`, `main`: clippy + tests + `check-docs`.
- `dev-release.yml` — push to `main`: builds `breadboard-server` + `breadboard`,
  publishes the dev track to `dl.breadway.dev`.
- `rc-release.yml` — `vX.Y.Z-rc.N` tag: beta track.
- `release.yml` — other `v*` tags: signed stable release.

Distribution is `bakery`, not a PKGBUILD.

## Architecture (local)

- `breadboard-server` — axum: adapters (Forgejo webhooks + reconcile) → sync
  engine → SQLite (WAL) → REST + SSE. In-process `broadcast` bus fans
  `BoardEvent`s to SSE clients and (milestone 7) to `bread-emit` + breadbar.
- `breadboard-core` — shared types. No I/O, no DB, no HTTP.
- `breadboard-client` / `breadboard-cli` — the CLI and (later) the `ratatui` TUI.
- `apps/web` — Svelte 5 SPA, talks REST + SSE. `apps/desktop` wraps that build in
  Tauri 2 (mirrors the `bos-settings` stack).

## Sync invariants (do not break)

- Every field change from either side appends a `sync_log` row.
- Conflicts resolve per-field by `updated_at` (last-write-wins).
- Writeback is tagged so the webhook echo it produces is not re-applied
  (loop prevention via `external_id` + source).
- A repo is only synced if it is in `repo_sync_state` with `enabled = 1`.

## Don't

- Don't commit straight to `main`.
- Don't embed credentials in remote URLs — SSH or a credential helper only.
- Don't add AI-attribution trailers to commits or PRs.
- Don't put the Forgejo API token, webhook secret, or OAuth secret in the repo —
  they come from `breadboard.toml` / `BREADBOARD_*` env only.
