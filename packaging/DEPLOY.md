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

Choose the webhook scope that matches the repository owner:

- An organization owner can use an organization webhook in its Settings.
- A personal account owner can use a user webhook in User Settings. Forgejo
  15.0.7 supports this through `POST /api/v1/user/hooks`, with the same payload
  and events as repository and organization webhooks. This covers the account's
  current and future repositories without converting the account into an
  organization.
- Use repository hooks when the operator cannot administer the owning account.

The deployed `Breadway` namespace is a personal account. An organization hook
cannot be created for it through `/api/v1/orgs/Breadway/hooks`. Use a Breadway
user hook for account-wide coverage; do not convert the namespace or transfer
repositories merely to obtain webhook coverage.

Configure the selected hook with:

- **URL**: `<public-url>/webhooks/forgejo`
- **Content type**: `application/json`
- **Secret**: the `BREADBOARD_FORGEJO_WEBHOOK_SECRET` value
- **Events**: Issues, Issue Comment, Pull Request

Then map each repo to a project: `breadboard repos add <owner>/<name> <KEY>`
(or `POST /api/repos`). The reconcile loop is the safety net if a webhook is
missed.

Unknown or disabled repositories are acknowledged with HTTP 200 and ignored:
`apply_issue`, `apply_comment`, and `apply_pr` each check `repo_enabled` before
syncing. The signature is still required, including for ignored repositories.

### Activating an account-wide hook

1. Sign in as the repository-owning account and open its webhook settings, or
   use the authenticated `/api/v1/user/hooks` endpoint. Select the Forgejo hook
   type, the URL and events above, and the existing deployment webhook secret.
   Keep that secret out of command history, logs, and source control.
2. Keep the existing repository hooks active while validating the new hook.
   The sync engine deduplicates repeated issue and comment deliveries.
3. Create an issue or comment in a mapped repository and inspect the user
   hook's delivery record. Confirm HTTP 200 and one corresponding breadboard
   issue/comment. Confirm an event from an unmapped repository is acknowledged
   without importing it. A newly created account-owned repository should appear
   in the user hook's delivery records without adding a repository hook.
4. Once delivery has been verified, disable only the repository hooks with the
   same breadboard destination. Retain their IDs and settings for rollback.
   To roll back, reactivate those hooks and disable the user hook.

The account-wide hook changes live Forgejo settings. Reviewing this procedure
does not itself activate a hook or remove the existing repository hooks.

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
