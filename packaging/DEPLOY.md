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
BREADBOARD_BACKUP_DIR=/var/backups/breadboard
EOF
chmod 600 .env
mkdir -p /var/backups/breadboard

docker compose -f packaging/compose.yaml --project-directory . up -d --build
curl -s localhost:7280/healthz
```

The build compiles Rust in a `rust:1-bookworm` stage; no host toolchain needed.
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
must not buffer responses: the `/events` SSE stream needs to flow. Caddy handles
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

The database is a single SQLite file on the `breadboard-data` volume. The
`litestream` service in `packaging/compose.yaml` gives it continuous
replication and point-in-time restore.

### How it is wired

- `breadboard-server` opens the database in WAL mode (`journal_mode=WAL`, set
  once at connection time in `crates/breadboard-server/src/db.rs`) and never
  switches journal modes or runs manual `wal_checkpoint` calls, which is
  exactly what Litestream needs: it owns checkpointing and reads the `-wal` /
  `-shm` files itself.
- The `litestream` container mounts the same `breadboard-data` volume
  read/write (it keeps a shadow WAL next to the database) and streams changes
  to the replica in `packaging/litestream.yml`.
- The default replica is a file replica at
  `/var/backups/breadboard/litestream`, bind-mounted from
  `BREADBOARD_BACKUP_DIR` on the host. Set that in `.env` to an absolute path
  on storage that is not the same disk as the live database, for example:

  ```sh
  echo 'BREADBOARD_BACKUP_DIR=/var/backups/breadboard' >> .env
  mkdir -p /var/backups/breadboard
  ```

- For an off-box copy, uncomment the `s3` block in `packaging/litestream.yml`
  and add `LITESTREAM_ACCESS_KEY_ID` / `LITESTREAM_SECRET_ACCESS_KEY` to
  `.env`.

Check that replication is healthy:

```sh
docker compose -f packaging/compose.yaml --project-directory . logs litestream
docker compose -f packaging/compose.yaml --project-directory . run --rm \
  litestream snapshots /var/lib/breadboard/breadboard.db
```

### Restore procedure

1. Stop the server so nothing writes during the restore:

   ```sh
   docker compose -f packaging/compose.yaml --project-directory . stop breadboard
   ```

2. Restore the latest replica into a scratch file (not over the live db yet):

   ```sh
   docker compose -f packaging/compose.yaml --project-directory . run --rm \
     litestream restore -o /var/lib/breadboard/breadboard.restored.db \
     /var/lib/breadboard/breadboard.db
   ```

   The final argument is the *source database path* Litestream knows from
   `litestream.yml`; it resolves the configured replica automatically. To
   restore to a point in time add `-timestamp 2026-09-09T12:00:00Z`.

3. Verify the restored copy against the live one before swapping. Row counts
   on the busy tables are a quick check. The Litestream image has no `sqlite3`
   binary, so run one in a throwaway container against the same volume:

   ```sh
   docker run --rm -v breadboard_breadboard-data:/d keinos/sqlite3 sh -c '
     for f in breadboard.db breadboard.restored.db; do
       echo "== $f =="
       for t in issues comments sync_log notifications; do
         printf "%s: " "$t"
         sqlite3 "/d/$f" "SELECT count(*) FROM $t;"
       done
     done'
   ```

   (The volume name is `<project>_breadboard-data`; `docker volume ls` shows
   the exact name.) The counts should match, or the restored copy should be
   behind by at most the last unreplicated transaction (sub-second under
   normal load).

4. Swap it in and restart:

   ```sh
   docker compose -f packaging/compose.yaml --project-directory . run --rm \
     --entrypoint sh litestream -c \
     'mv /var/lib/breadboard/breadboard.restored.db /var/lib/breadboard/breadboard.db'
   docker compose -f packaging/compose.yaml --project-directory . up -d
   ```

To rehearse this without touching production, restore into a scratch
directory on a dev box (`litestream restore -o ./scratch.db <replica-url>`)
and diff row counts against a copy of the live file.
