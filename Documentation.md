# breadboard — API & contract

Source of truth for the REST surface, the SSE event types, the `bread.board.*`
event names, and the Forgejo sync rules. `cargo run -p xtask -- check-docs`
verifies the event names and workflow statuses below still match the code; CI
enforces it.

Status: **milestone 1** — only `/healthz` and `/events` are implemented. Routes
marked _(planned)_ describe the intended contract.

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

Base URL: the server's `public_url`. JSON in/out. Auth is a session cookie (web,
via Forgejo OAuth2) or a bearer token (CLI/TUI).

### Implemented

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/healthz` | `{ status, version, db }`; `503` if the DB is unreachable |
| `GET` | `/events` | SSE stream of `BoardEvent` (see below) |

### Planned

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/api/issues` | filter by `project`, `status`, `cycle`, `epic`, `assignee`, `mine` |
| `POST` | `/api/issues` | create; only `title` is required |
| `GET` | `/api/issues/:key` | one issue with labels, links, comments |
| `PATCH` | `/api/issues/:key` | partial update; synced fields trigger writeback |
| `DELETE` | `/api/issues/:key` | |
| `GET`/`POST` | `/api/projects` | |
| `GET`/`POST` | `/api/epics` | |
| `GET`/`POST` | `/api/cycles` | |
| `POST` | `/api/cycles/:id/start` · `/end` | emits `cycle.started` / `cycle.ended` |
| `GET` | `/api/notifications` | current user's notifications |
| `GET` | `/api/me` | current user |
| `POST` | `/webhooks/forgejo` | HMAC-verified inbound sync |
| `GET` | `/auth/login` · `/auth/callback` | Forgejo OAuth2 |
| `POST` | `/auth/device` · `/auth/device/token` | CLI device flow |
| `GET`/`POST`/`DELETE` | `/api/repos` | which `owner/repo` are synced |
| `POST` | `/api/sync/reconcile` | force a full reconcile (`breadboard sync --full`) |

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

**Inbound fast path:** Forgejo webhooks (`issues`, `issue_comment`,
`pull_request`) → `POST /webhooks/forgejo`, HMAC-verified against
`forgejo.webhook_secret`. Applied immediately.

**Reconcile:** every `forgejo.reconcile_interval_mins` minutes, per enabled repo,
`GET .../issues?since=<repo_sync_state.last_synced_at>`; diff and apply anything
the webhook path missed. A divergence emits `sync.drift`.

**Writeback:** breadboard mutations to synced fields call the Forgejo REST API
with `forgejo.api_token`.

**Synced fields:**

| breadboard | Forgejo |
| --- | --- |
| `title`, `body` | issue title / body |
| `status` | open (`backlog`–`review`) / closed (`done`) |
| `assignee_id` | assignee (mapped by `forgejo_id`) |
| `labels` | labels |
| `cycle` ↔ nothing; `epic` ↔ nothing | breadboard-only |
| comments | issue comments (both ways) |
| PRs | linked, read-mostly (writeback: labels/links only) |

**Conflict resolution:** per-field last-write-wins keyed on `updated_at`. Every
change from either side appends a `sync_log` row
(`field`, `old_value`, `new_value`, `source`, `actor_id`, `at`).

**Loop prevention:** each writeback is tagged with its `source`; the webhook echo
it generates carries the same `external_id` and is recognised as already-applied.

**Scope:** only repos present in `repo_sync_state` with `enabled = 1`
(`breadboard repos add owner/repo`).

---

## CLI

`breadboard [--server URL] <command>`

| Command | Status |
| --- | --- |
| `ping` | implemented |
| `auth login` · `logout` · `status` | planned (device flow) |
| `add <title> [--project] [--epic] [--cycle]` | planned |
| `mv <key> <status>` | planned |
| `board [--cycle] [--project]` | planned |
| `ls [--mine] [--status]` | planned |
| `cycle list` · `start <name>` · `end <name>` | planned |
| `repos list` · `add <repos…>` · `remove <repos…>` | planned |
| `sync [--full]` | planned |
| `tui` | planned (milestone 5) |
