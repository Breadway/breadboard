# breadboard — API & contract

Source of truth for the REST surface, the SSE event types, the `bread.board.*`
event names, and the Forgejo sync rules. `cargo run -p xtask -- check-docs`
verifies the event names and workflow statuses below still match the code; CI
enforces it.

Status: **milestone 2** — health, SSE, REST CRUD (projects / epics / cycles /
issues), and Forgejo OAuth2 login are implemented. Routes marked _(planned)_
describe the intended contract for later milestones (sync, notifications).

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

**IssuePatch** — every field optional; absent = unchanged. `title`, `body`,
`status`, `priority` take a value. `estimate`, `assignee_id`, `epic_id`,
`cycle_id` accept `null` to clear. `labels` and `links` replace the whole set.

### Planned

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/api/notifications` | current user's notifications (milestone 7) |
| `POST` | `/webhooks/forgejo` | HMAC-verified inbound sync (milestone 4) |
| `POST` | `/auth/device` · `/auth/device/token` | CLI device flow (milestone 5) |
| `GET`/`POST`/`DELETE` | `/api/repos` | which `owner/repo` are synced (milestone 4) |
| `POST` | `/api/sync/reconcile` | force a full reconcile (milestone 4) |

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
