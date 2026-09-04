# Deploying breadboard

breadboard is one container: a Rust server that also serves its built SPA. Put a
TLS-terminating reverse proxy in front of it.

## 1. Forgejo OAuth application

On your Forgejo instance, create an OAuth2 application:

- **Redirect URI**: `<public-url>/auth/callback`
- **Confidential client**: yes

Note the client id and secret.

## 2. A service token

A Forgejo personal access token with `write:issue` + `write:repository` scope,
used for sync writeback.

## 3. Run it

```sh
git clone <this-repo> breadboard && cd breadboard

cat > .env <<'EOF'
BREADBOARD_PUBLIC_URL=https://issues.example.com
BREADBOARD_FORGEJO_BASE_URL=https://git.example.com
BREADBOARD_FORGEJO_OAUTH_CLIENT_ID=...
BREADBOARD_FORGEJO_OAUTH_CLIENT_SECRET=...
BREADBOARD_FORGEJO_WEBHOOK_SECRET=$(openssl rand -hex 32)
BREADBOARD_FORGEJO_API_TOKEN=...
EOF
chmod 600 .env

docker compose -f packaging/compose.yaml --project-directory . up -d --build
curl -s localhost:7280/healthz
```

The build compiles Rust in a `rust:1-bookworm` stage — no host toolchain needed.
By default the container binds `127.0.0.1:7280`; set `BREADBOARD_BIND` in `.env`
to change that (e.g. `0.0.0.0:7280` for another host to reach it).

**If the target host is small / busy**, build the image elsewhere and load it:

```sh
podman build -t breadboard:local -f packaging/Containerfile .   # on a dev box
podman save breadboard:local | ssh host 'docker load'
ssh host 'cd ~/breadboard && docker compose -f packaging/compose.yaml --project-directory . up -d'
```

## 4. Reverse proxy

Point a proxy at `127.0.0.1:7280` (see `packaging/caddy/breadboard.caddy`). It
must not buffer responses — the `/events` SSE stream needs to flow. Caddy handles
this automatically; nginx needs `proxy_buffering off;` for `/events`.

## 5. Forgejo webhook

On each repo to sync (or org-wide), add a webhook:

- **URL**: `<public-url>/webhooks/forgejo`
- **Content type**: `application/json`
- **Secret**: the `BREADBOARD_FORGEJO_WEBHOOK_SECRET` value
- **Events**: Issues, Issue Comment, Pull Request

Then map each repo to a project: `breadboard repos add <owner>/<name> <KEY>`
(or `POST /api/repos`). The reconcile loop is the safety net if a webhook is
missed.

## Updating

```sh
git pull
docker compose -f packaging/compose.yaml --project-directory . up -d --build
```

## Backups

The database is a single SQLite file on the `breadboard-data` volume. For
continuous replication, fill in a replica in `packaging/litestream.yml` and
enable the `litestream` service in `packaging/compose.yaml`.
