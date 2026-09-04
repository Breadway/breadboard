# Contributing

`breadboard` — issue tracker for the bread ecosystem.

Part of the bread ecosystem; this repo follows the same branch/release workflow
as every other ecosystem product.

## Branches

One long-lived branch: **`main`**. Every push to `main` auto-publishes a
**dev-track** build.

New work — features and fixes alike — goes on a short-lived branch:

```
feature/<short-name>
fix/<issue-number-or-short-name>
```

Branch off `main`, push back into `main` when ready. Short-lived branches are
deleted on merge.

## The release cycle

"stable" and "beta" are **tags** on `main`, not branches:

1. Work accumulates on `main`. Each push auto-publishes a dev build —
   `bakery track set dev && bakery update --all`.
2. To stabilize: `git tag vX.Y.Z-rc.1 && git push origin vX.Y.Z-rc.1` — that tag
   triggers a beta build. Cut `-rc.2`, `-rc.3` for fixes.
3. When an RC is clean: `git tag vX.Y.Z && git push origin vX.Y.Z` — signed
   stable release.

Push tags to **`origin` only**; `github` auto-mirrors.

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

- `check.yml` — `feature/**`, `fix/**`, `main`: clippy + tests + `check-docs`.
- `dev-release.yml` — push to `main`.
- `rc-release.yml` — `vX.Y.Z-rc.N` tags.
- `release.yml` — other `v*` tags.

All CI runs on a self-hosted runner; nothing runs automatically on plain commits
or PRs beyond the track builds above.

## Questions

Open an issue on this repo's Forgejo tracker. (Once breadboard is dogfooding
itself, file it in breadboard.)
