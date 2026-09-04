# Contributing

`breadboard` — issue tracker for the bread ecosystem.

Part of the bread ecosystem; this repo follows the same branch/release workflow
as every other ecosystem product.

## Branches

One long-lived branch: **`main`**. New work — features and fixes alike — goes on
a short-lived branch:

```
feature/<short-name>
fix/<issue-number-or-short-name>
```

Branch off `main`, push back into `main` when ready. Short-lived branches are
deleted on merge. Push to `origin` only; `github` auto-mirrors.

The server is deployed as a container from `main` (`packaging/`). Bakery-track
distribution of the CLI (dev / `vX.Y.Z-rc.N` / `vX.Y.Z`) will follow the sibling
repos' workflow once the release secrets are in place.

## Local development

```sh
cargo build --workspace
cargo test --workspace
cargo run -p xtask -- check-docs
```

Frontend:

```sh
cd apps/web && npm ci && npm run dev
```

### Keeping the contract honest

`Documentation.md` is the source of truth for the REST routes, the SSE
`BoardEvent` types, the `bread.board.*` event names, and the sync rules. It has
drifted from code in other ecosystem repos before, so `xtask check-docs` verifies
that every event name and workflow status is documented, and CI enforces it.

Whenever you add/rename/remove a `BoardEvent` variant, a REST route, or a CLI
subcommand:

1. Update `Documentation.md` to match.
2. Run `cargo run -p xtask -- check-docs` before committing.

## CI

`check.yml` — on `feature/**`, `fix/**`, `main`: clippy `-D warnings`,
`cargo test`, `check-docs`, and the frontend build. Self-hosted runner; nothing
else runs on plain commits/PRs.

## Questions

File an issue on this repo's tracker.
