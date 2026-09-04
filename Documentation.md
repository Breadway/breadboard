# breadboard — API & contract

Source of truth for the REST surface, the SSE event types, the `bread.board.*`
event names, and the Forgejo sync rules. `cargo run -p xtask -- check-docs`
verifies the event names and workflow statuses below still match the code; CI
enforces it.

Status: **milestone 5** — server (REST CRUD, OAuth2, SSE, Forgejo sync engine),
the Svelte web client, and the **`breadboard` CLI + `ratatui` TUI** with
device-flow auth. Routes marked _(planned)_ are for later milestones
(notifications; PR mirroring; comment writeback).

The server serves the built SPA at `/` when `web_dir` (or `BREADBOARD_WEB_DIR`)
is set; unknown paths fall back to `index.html` for client-side routing.

---

## Workflow

One global workflow, no per-project variants:

`backlog` → `todo` → `doing` → `review` → `done`

`done` is the only status that maps to a **closed** Forgejo issue; `backlog`
through `review` are **open**.

Priority: `none` · `low` · `medium` · `high` · `urgent`.

Issue type: `issue` · `pr` (PRs are mirrored read-mostly).

Link kinds: `blocks` · `blocked-by` · `relates-to`.

---

## HTTP

Base URL: the server's `public_url`. JSON in/out. `/healthz` and `/events` are
public; everything under `/api` requires auth — a session cookie (`bb_session`,
web) or `Authorization: Bearer <token>` (CLI/TUI). `401` when neither is valid.

### Implemented

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/healthz` | `{ status, version, db }`; `503` if the DB is unreachable |
| `GET` | `/events` | SSE stream of `BoardEvent` (see below) |
| `GET` | `/auth/login` | redirect to Forgejo OAuth2 authorize (sets a state cookie) |
| `GET` | `/auth/callback` | exchange code, upsert user, set session, redirect to `/` |
| `POST` | `/auth/logout` | clear the session; `204` |
| `POST` | `/auth/dev-login` | `{ login, name? }` → session; **only when `BREADBOARD_DEV_LOGIN=1`**, else `403` |
| `GET` | `/api/me` | current user |
| `GET` | `/api/projects` | list |
| `POST` | `/api/projects` | `{ key, name, forgejo_repo? }`; `key` = uppercase letters/digits; `201` |
| `GET` | `/api/projects/{id}` | one project |
| `GET` | `/api/epics` | list |
| `POST` | `/api/epics` | `{ key, title, description?, project_id? }`; `201` |
| `GET` | `/api/epics/{id}` | one epic |
| `GET` | `/api/cycles` | list |
| `POST` | `/api/cycles` | `{ name, starts_on, ends_on }` (ISO dates); `201` |
| `POST` | `/api/cycles/{id}/start` | emits `cycle.started` (with issue count) |
| `POST` | `/api/cycles/{id}/end` | sets `closed`, emits `cycle.ended` |
| `GET` | `/api/issues` | filter: `project` (key or id), `status`, `cycle_id`, `epic_id`, `assignee_id`, `mine` |
| `POST` | `/api/issues` | create; only `title` required. `project` defaults to the first non-archived project; key allocated from that project's counter. `201` |
| `GET` | `/api/issues/{key}` | issue + `comments[]` |
| `PATCH` | `/api/issues/{key}` | partial update (see IssuePatch below); synced-field changes append `sync_log` rows |
| `DELETE` | `/api/issues/{key}` | `204`, emits `card.deleted` |
| `GET` | `/api/repos` | synced repos: `{ repo, project_key, enabled, last_synced_at }` |
| `POST` | `/api/repos` | `{ repo: "owner/name", key, name? }` — maps the repo to a project (created if absent), enables sync, kicks an initial import. `201` |
| `DELETE` | `/api/repos/{owner}/{name}` | disables sync for the repo (keeps imported data). `204` |
| `POST` | `/api/sync/reconcile` | run a reconcile pass now (background). `202` |
| `POST` | `/webhooks/forgejo` | inbound sync — **no session auth**; HMAC-SHA256 over the raw body in `X-Forgejo-Signature` (or `X-Gitea-Signature`), verified against `forgejo.webhook_secret`. Handles `issues` and `issue_comment` events for enabled repos; always `200` once authenticated (reconcile is the safety net); `401` on a bad signature |

**IssuePatch** — every field optional; absent = unchanged. `title`, `body`,
`status`, `priority` take a value. `estimate`, `assignee_id`, `epic_id`,
`cycle_id` accept `null` to clear. `labels` and `links` replace the whole set.

### Device auth (CLI/TUI)

| Method | Path | Notes |
| --- | --- | --- |
| `POST` | `/auth/device` | start: `{ device_code, user_code, verification_uri, verification_uri_complete, expires_in, interval }` |
| `GET` | `/auth/device?user_code=…` | approval page (bounces through Forgejo OAuth if not signed in) |
| `POST` | `/auth/device/approve` | **session-authed** — `{ user_code }`; mints a `cli` token and attaches it to the pending code |
| `POST` | `/auth/device/token` | poll — `{ device_code }` → `200 { access_token }` (one-time), `202 { status: "authorization_pending" }`, `410` expired, `404` unknown |

### Planned

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/api/notifications` | current user's notifications (milestone 7) |

---

## SSE events

`GET /events` emits one JSON object per change, tagged by `type`. Each maps to a
`bread.board.*` event on the bread bus (milestone 7).

| SSE `type` | bread event | payload |
| --- | --- | --- |
| `card.created` | `bread.board.card.created` | `id`, `key` |
| `card.updated` | `bread.board.card.updated` | `id`, `key` |
| `card.moved` | `bread.board.card.moved` | `id`, `key`, `from`, `to` |
| `card.assigned` | `bread.board.card.assigned` | `id`, `key`, `assignee_id` |
| `card.deleted` | `bread.board.card.deleted` | `id`, `key` |
| `cycle.started` | `bread.board.cycle.started` | `id`, `name`, `issues` |
| `cycle.ended` | `bread.board.cycle.ended` | `id`, `name` |
| `sync.drift` | `bread.board.sync.drift` | `issue_id`, `key`, `field` |

---

## Forgejo sync

**Direction:** bidirectional.

**Inbound fast path:** Forgejo `issues` / `issue_comment` webhooks →
`POST /webhooks/forgejo`, HMAC-verified against `forgejo.webhook_secret`, applied
immediately. `pull_request` events are accepted but not yet synced.

**Reconcile:** every `forgejo.reconcile_interval_mins` minutes, per enabled repo,
`GET .../issues?since=<repo_sync_state.last_synced_at>&state=all` (paginated);
apply anything the webhook path missed, then advance `last_synced_at`. `POST
/api/sync/reconcile` runs a pass on demand.

**Writeback:** after a `PATCH /api/issues/{key}` changes a synced field on a
linked issue in an enabled repo, the change is pushed to Forgejo with
`forgejo.api_token` (`PATCH issue` for title/body/state/assignees, `PUT labels`
otherwise).

**Synced fields:**

| breadboard | Forgejo | direction |
| --- | --- | --- |
| `title`, `body` | issue title / body | both |
| `status` | open (`backlog`–`review`) ↔ closed (`done`); a fine-grained open status is preserved, reopen → `todo` | both |
| `assignee_id` | first assignee, mapped by `forgejo_id` (unknown users are imported) | both |
| `labels` | labels (created on Forgejo if missing) | both |
| `epic`, `cycle`, `priority`, `estimate`, `links` | — | breadboard-only |
| comments | issue comments | inbound only (writeback: milestone 4b) |
| PRs | — | not yet |
| milestone | — | not yet |

**Conflict resolution:** per-field, the side with the newer issue-level
`updated_at` wins; a value that already matches is a no-op (this absorbs the
webhook echo of a writeback). Applied changes append a `sync_log` row
(`field`, `old_value`, `new_value`, `source`, `actor_id`, `at`). When breadboard
is the newer side and the two still differ, a `sync.drift` event fires and no
change is applied (writeback, or the next breadboard edit, resolves it).

**Scope:** only repos in `repo_sync_state` with `enabled = 1`, each mapped to a
project via `projects.forgejo_repo` (`POST /api/repos`). Issues in an enabled
repo with no mapped project are skipped with a warning.

---

## CLI

`breadboard [--server URL] <command>`. Config at
`$XDG_CONFIG_HOME/breadboard/cli.toml` (`server`, `token`).

| Command | Notes |
| --- | --- |
| `ping` | connectivity check |
| `auth login` | device flow — prints a code + URL, opens the browser, polls until approved |
| `auth logout` · `auth status` | |
| `add <title> [--project K] [--epic KEY] [--cycle NAME] [--label L]… [--priority P]` | epic/cycle resolved by key/name |
| `mv <key> <status>` | |
| `board [--cycle NAME] [--project K] [--mine]` | columns |
| `ls [--mine] [--status S] [--project K]` | |
| `show <key>` | issue + comments |
| `cycle list` · `cycle new <name> <start> <end>` · `cycle start <name>` · `cycle end <name>` | dates `YYYY-MM-DD` |
| `repos list` · `repos add <owner/name> <key> [--name N]` · `repos remove <owner/name>` | |
| `sync` | `POST /api/sync/reconcile` |
| `tui` | full-screen board — `hjkl`/arrows move, `[` `]` move a card between statuses, `enter` detail, `r` refresh, `q` quit |
