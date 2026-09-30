# Bakery clients

The root `bakery.toml` describes the server and CLI together. The two client
packages published by CI use separate manifests under `packaging/bakery/`:

| Product | Installed executable | Metadata |
| --- | --- | --- |
| `breadboard-cli` | `breadboard` | CLI and terminal UI, no service or config overwrite |
| `breadboard-desktop` | `breadboard-desktop` | Desktop entry and runtime dependencies |

Both include LICENSE. The desktop uses a generic system development icon, so
its desktop entry remains usable without adding an unsigned icon download.
The server container remains the deployment described in DEPLOY.md.

`dev-release.yml` builds from main, `rc-release.yml` builds beta from
`vX.Y.Z-rc.N`, and `release.yml` builds stable from `vX.Y.Z`. They check out the
triggering commit, compile the CLI with the lockfile, build the web workspace,
and build the desktop deb bundle through Tauri with the lockfile. Building
does not run the desktop app. Bakery distributes the resulting native binary
with its declared dependencies; no FUSE or AppImage runtime is needed.

Each native binary is stripped and checksummed in an isolated staging
directory. Only after both are prepared are their version directories and
latest links installed into the download root and the track index regenerated
and signed. Published version directories are immutable. A per-track client
publish lock prevents two breadboard jobs from racing with each other; the
ecosystem generator also serializes index generation across repositories.

## Activation

Workflows are disabled until the repository Actions variable
`BREADBOARD_BAKERY_ENABLED` is `true`. Before enabling it:

1. Merge the bread-ecosystem registry dependency adding `breadboard-cli` and
   `breadboard-desktop`. The publisher refuses to update either latest link if
   either product is missing from the registry.
2. Use a runner labelled `self-hosted` and `hestia` with write access to
   `/srv/breadway-dl` and its `dev` and `beta` directories. It needs the normal
   Rust and npm build tools, Tauri CLI, GTK 3/WebKitGTK 4.1/libsoup3 development
   libraries, plus `python3`, `strip`, `jq`, `flock`, and `minisign`.
3. Provision `BAKERY_MINISIGN_SEC_KEY_PATH` with the existing publisher's
   signing-key path. If it requires a password, also provision
   `BAKERY_MINISIGN_SEC_KEY_PASSWORD`. These workflows use the existing Hestia
   filesystem publishing pattern, so REGISTRY_TOKEN and an upload token are
   unnecessary. They do not use GitHub release credentials.
4. Enable the variable, then let the next main push produce a dev build.
   Inspect its job result and signed dev index entries before tagging an RC.
   Stable/RC tags must match the base versions in both Cargo manifests. This
   preparation does not create or push a release tag.

Missing signing credentials fail before build or publication. Roll back by
disabling the variable and restoring each previous latest link, then
regenerating that track's signed index through the approved publisher.

## Local staging verification

`stage-bakery.sh` only writes the specified staging root; it never installs
binaries or alters personal config. After building both clients, use:

```sh
VERSION=0.1.0 BAKERY_STAGE_ROOT=/tmp/breadboard-artifacts \
  CLI_BINARY=target/release/breadboard \
  DESKTOP_BINARY=apps/desktop/target/release/breadboard-desktop \
  bash scripts/stage-bakery.sh
```

Inspect the package directories, binary checksums, manifests and desktop entry.
Do not point this staging-only command at the live download root.
