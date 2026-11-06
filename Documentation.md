# breadboard: API & contract

Source of truth for the REST surface, the SSE event types, the `bread.board.*`
event names, and the Forgejo sync rules. `cargo run -p xtask -- check-docs`
verifies the event names and workflow statuses below still match the code; CI
enforces it.

Status: **deployed** at `breadboard.breadway.dev`. Server (REST CRUD, OAuth2,
SSE), the Svelte web client (board with keyboard nav and per-agent claim tints ·
backlog · roadmap/epics · Agents live page · issue drawer with claim controls and
activity timeline · notification bell · settings · `cmd-k`),
the `breadboard` CLI + `ratatui` TUI (device-flow auth), the Forgejo sync engine
(bidirectional issues / comments / PR mirroring / milestone), and notifications
(assigned / @mention / cycle, with a `notified` SSE event, browser pop-ups, and
best-effort `bread-emit` for a co-located breadd; see `modules/breadboard.lua`).

Web board keys: `h`/`l`/`←`/`→` column, `j`/`k`/`↑`/`↓` card, `[`/`]` move card
between statuses, `enter`/`e` open, `c` command palette.

Filtering is multi-select and composable: project, cycle, epic, status,
priority, label, assignee (including unassigned), agent name, agent claim state,
claimed/unclaimed, issue type, and free text. Facets combine with AND, values
inside one facet with OR, so any number can be active at once. Active filters
show as removable chips in the top bar.

The **Agents** view is a grid of agent cards: one card per `(user, agent
name)`, with the agent's optional one-line message under its name, and a
rounded tile per issue coloured by board status and a dashed
tile for queued intents. Clicking a card opens a popup with that agent's full
claim list, completion ring, states, heartbeats and release actions.

The server serves the built SPA at `/` when `web_dir` (or `BREADBOARD_WEB_DIR`)
is set; unknown paths fall back to `index.html` for client-side routing.

---

## Workflow

One global workflow, no per-project variants:

`backlog` → `todo` → `doing` → `review` → `done`

`done` is the only status that maps to a **closed** Forgejo issue; `backlog`
through `review` are **open**.

Agents claim issues on top of this workflow. There are two claim modes:

- **exclusive** (default): real work. At most one per issue, first claim wins
  (`409` otherwise). Claiming from `backlog`/`todo` moves the card to `doing`
  and assigns it to the claimer, tinting the card on the board.
- **intent**: a soft reservation for work you plan to take later. Any number
  may coexist per issue (one per agent), it never blocks an exclusive claim,
  and it never moves or assigns the card. It shows as "queued" on the Agents
  page and issue drawer.

Releasing keeps the status and only removes the tint. A claim with no
heartbeat for 5+ minutes reads as **stale** for exclusive work on the Agents
page but never auto-expires: only an explicit release (or a new claim after
release) removes it. Claim, state, PR and release transitions are appended to
the issue's activity timeline as `agent_claim`, `agent_state`, `agent_pr` and
`agent_release`; routine heartbeats that change nothing are not logged.

### Agent ownership and orchestration

Ownership is per issue, not per agent: one agent may hold many exclusive
claims, and the Agents tab groups them under that agent with each issue's
state and an overall done/total ring.

A claim with no heartbeat for 5+ minutes is shown with a clock and marked
**stale**, never as `blocked`: stale means "may be abandoned", blocked is a
lifecycle state. Re-claiming an issue you already own is idempotent: it
heartbeats and resumes the existing claim instead of returning `409`. After a
restart, an agent can call the MCP `resume_claims` tool to heartbeat everything
it owns and re-arm keepalives.

Agent names are free-form and meant to be fun and memorable (Toasty Pretzel,
Crumb Baron). Reuse the same name across issues; identity is `(user, agent
name)`, so a stable name is what makes the Agents tab group that agent's work.

- An **exclusive claim is accountability** for one issue. Keep it with the
  agent that will actually finish that issue.
- An **orchestrator owns its own umbrella issue**, not its subagents' issues.
  A subagent that does real work gets its own issue (or slice issue linked to
  the parent) and claims that one exclusively. Sharing a single exclusive
  claim across a tree of agents hides who is responsible, makes one agent's
  completion look better than it is, and blocks subagents from claiming.
- Subagents that are only waiting, observing, or queued use **intent** claims
  on the parent: intents never move the card or block the owner, and show as
  "queued" in the agent group.
- Roll up across the tree with epics or links (parent `blocks`/`relates-to`
  child), not by merging claims. That keeps the server roll-up, the board
  tint, and per-agent completion honest.

### Where work belongs

A project is the routing decision, not a label. File an issue where the work
actually lives:

- **Repo-backed projects** (`forgejo_repo` set, e.g. `BREAD` → `Breadway/bread`)
  are for that repo's code. Their cards sync to the repo's tracker, so local
  machine config, dotfiles, environment breakage, or a misbehaving bread module
  do **not** belong there: they would pollute the repo's issue list.
- **Native (repo-less) projects** are for work that has no repo of its own:
  the machine, tooling, homelab operations, or several repos at once.
- **A new project that is not on Forgejo yet** gets its own native project in
  breadboard now and is tracked there; attach the repo later
  (`PATCH /api/projects/{id}` with `forgejo_repo`, or `POST /api/repos`).
- **No catch-all.** `ROAD` is no longer seeded and must not be used as a
  dumping ground for unrelated work. On an existing database where it holds
  cards, move each one to the project that owns it (`PUT
  /api/issues/{key}/project`) rather than adding more.

If no existing project clearly owns the work, create the right one instead of
guessing; an issue filed in the wrong project is worse than a new project key.

Priority: `none` · `low` · `medium` · `high` · `urgent`.

Issue type: `issue` · `pr` (PRs are mirrored read-mostly).

Link kinds: `blocks` · `blocked-by` · `relates-to`.

---

## HTTP

Base URL: the server's `public_url`. JSON in/out. `/healthz` is public;
`/events` and everything under `/api` require auth: a session cookie
(`bb_session`, web) or `Authorization: Bearer <token>` (CLI/TUI). `401` when
neither is valid.

### Implemented

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/healthz` | `{ status, version, db, dev_login }`; `503` if the DB is unreachable |
| `GET` | `/events` | Authenticated SSE stream of `BoardEvent` (see below); browser EventSource sends the session cookie, CLI/TUI clients use a bearer token |
| `GET` | `/auth/login` | redirect to Forgejo OAuth2 authorize (sets a state cookie) |
| `GET` | `/auth/callback` | exchange code, upsert user, set session, redirect to `/` |
| `POST` | `/auth/logout` | clear the session; `204` |
| `POST` | `/auth/dev-login` | `{ login, name? }` → session; **only when `BREADBOARD_DEV_LOGIN=1`**, else `403` |
| `GET` | `/api/me` | current user |
| `GET` | `/api/projects` | list |
| `POST` | `/api/projects` | `{ key, name, forgejo_repo? }`; `key` = uppercase letters/digits; `201` |
| `GET` | `/api/projects/{id}` | one project |
| `PATCH` | `/api/projects/{id}` | `{ name?, archived?, forgejo_repo? }` (`forgejo_repo: null` detaches) |
| `GET` | `/api/epics` | list; each is `{ …epic, progress: {status→count}, total }` |
| `POST` | `/api/epics` | `{ key, title, description?, project_id? }`; `201` |
| `GET` | `/api/epics/{id}` | one epic with `progress` / `total` |
| `PATCH` | `/api/epics/{id}` | `{ title?, description?, archived? }` |
| `DELETE` | `/api/epics/{id}` | `204` (child issues keep running, `epic_id` cleared) |
| `GET` | `/api/cycles` | list |
| `POST` | `/api/cycles` | `{ name, starts_on, ends_on, project_ids? }` (ISO dates; project ids whose new issues default to this cycle); `201` |
| `PATCH` | `/api/cycles/{id}` | `{ project_ids }` replaces selected project defaults; existing issues are not moved |
| `POST` | `/api/cycles/{id}/start` | emits `cycle.started` (with issue count) |
| `POST` | `/api/cycles/{id}/end` | sets `closed`, emits `cycle.ended` |
| `GET` | `/api/issues` | filter: `project` (key or id), `status`, `cycle_id`, `epic_id`, `assignee_id`, `mine` |

When creating an issue without `cycle_id`, Breadboard assigns the sole open cycle
selecting that project's id. No selected cycle means no default. Multiple matching
open cycles return `409 conflict`; callers must provide `cycle_id`. Explicit cycle
assignment always takes precedence. Changing project selections never moves existing
issues. Markdown imports use the same issue-creation rule.
| `POST` | `/api/issues` | create; `title` required. `project` (key or id) is required once more than one project exists; with exactly one project it may be omitted. There is no catch-all default. Key allocated from the project's counter. `201` |
| `GET` | `/api/issues/{key}` | issue + `comments[]` |
| `GET` | `/api/issues/{key}/activity` | `sync_log` timeline: `{ field, old_value, new_value, source, actor, at }[]` |
| `PATCH` | `/api/issues/{key}` | partial update (see IssuePatch below); synced-field changes append `sync_log` rows |
| `DELETE` | `/api/issues/{key}` | `204`, emits `card.deleted` |
| `PUT` | `/api/issues/{key}/project` | `{ project }` (key or id): moves the issue to a different project: re-keys it from the target project's counter and clears any Forgejo link (it belonged to the old project's repo). Best-effort closes the old linked Forgejo issue with a note, then re-creates it fresh on the new project's repo if one is mapped. Emits `card.updated` |
| `POST` | `/api/issues/{key}/comments` | `{ body }`: adds a comment; pushed to Forgejo if the issue is linked. `201` |
| `POST` | `/api/issues/{key}/claim` | agent claim: `{ agent_name, message?, mode?, session_id?, state?, note?, branch?, worktree?, repo?, pr_url?, pr_number?, head_sha? }`. `mode` is `exclusive` (default) or `intent`. Exclusive is first-wins (`409` on conflict) and moves a `backlog`/`todo` card to `doing` plus assigns it; taking the work also removes that same agent's queued intent. Intent is a soft reservation that never blocks, moves or assigns. `pr_url`, when present, must be an absolute http(s) URL. Re-claiming your own intent updates it. Emits `agent.claimed`. `201` |
| `PUT` | `/api/issues/{key}/claim` | heartbeat / update your claim (any field optional; `null` clears a nullable one); refreshes `last_heartbeat`. `?agent=` / `?mode=exclusive|intent` pick one when you hold several; with neither, the exclusive claim wins over an intent and the freshest wins otherwise. Only the claiming user may update, and a mismatched `session_id` is `409`. `pr_url` must be an absolute http(s) URL. A newly set `pr_url` also comments the link on the issue once (synced to Forgejo). Emits `agent.updated` |
| `DELETE` | `/api/issues/{key}/claim` | release your claim (only the claiming user); `?agent=` / `?mode=` select one when you hold several. The card keeps its status. Emits `agent.released`. `204` |
| `DELETE` | `/api/claims/{id}` | release one claim by id (owner only): how the Agents page releases individual exclusive/intent rows. Emits `agent.released`. `204` |
| `PUT` | `/api/agents/message` | `{ agent_name, message }`: set the signed-in agent's one-line message (collapsed to one line, max 120 chars; empty clears). Shown under the agent's name |
| `GET` | `/api/agents` | live agent claims (each joined with its issue key/title/status), exclusive first then intents, most-recent heartbeat first |
| `GET` | `/api/agents/status` | server-side roll-up mirroring the opencode `bb-status` widget line: `{ total, live, stale, queued, done, active, review, verifying, blocked, remaining, line, claims[] }`. Counts cover exclusive work only; `queued` counts intents, and the line adds `N queued` when any exist |
| `GET` | `/api/repos` | synced repos: `{ repo, project_key, enabled, last_synced_at }` |
| `POST` | `/api/repos` | `{ repo: "owner/name", key, name? }`: maps the repo to a project (created if absent), enables sync, kicks an initial import. `201` |
| `DELETE` | `/api/repos/{owner}/{name}` | disables sync for the repo (keeps imported data). `204` |
| `POST` | `/api/sync/reconcile` | run a reconcile pass now (background). `202 { "ok": true }` |
| `POST` | `/api/import` | `{ project, markdown, dry_run?, repair? }`: headings → epics, checklist items → issues (`[x]` creates closed/done remotely too); returns created/skipped entries, `remote_would_create[]`, and `pending_writebacks[]` (`key`, `error`). Ordinary imports mirror only cards created in that request. Set `repair: true` to retry existing unlinked matches; combine with `dry_run: true` to list local keys that would be created remotely without writing. Duplicate item titles in one outline and multiply-matched existing cards are rejected as ambiguous. |
| `POST` | `/webhooks/forgejo` | inbound sync: **no session auth**; HMAC-SHA256 over the raw body in `X-Forgejo-Signature` (or `X-Gitea-Signature`), verified against `forgejo.webhook_secret`. Handles `issues` and `issue_comment` events for enabled repos; always `200` once authenticated (reconcile is the safety net); `401` on a bad signature |

**IssuePatch**: every field optional; absent = unchanged. `title`, `body`,
`status`, `priority` take a value. `estimate`, `assignee_id`, `epic_id`,
`cycle_id` accept `null` to clear. `labels` and `links` replace the whole set.

### Device auth (CLI/TUI)

| Method | Path | Notes |
| --- | --- | --- |
| `POST` | `/auth/device` | start: `{ device_code, user_code, verification_uri, verification_uri_complete, expires_in, interval }` |
| `GET` | `/auth/device?user_code=…` | approval page (bounces through Forgejo OAuth if not signed in) |
| `POST` | `/auth/device/approve` | **session-authed**: `{ user_code }`; mints a `cli` token and attaches it to the pending code |
| `POST` | `/auth/device/token` | poll: `{ device_code }` → `200 { access_token }` (one-time), `202 { status: "authorization_pending" }`, `410` expired, `404` unknown |

### Notifications

| Method | Path | Notes |
| --- | --- | --- |
| `GET` | `/api/notifications` | `{ unread, notifications[] }`; `?unread=true` filters |
| `POST` | `/api/notifications/{id}/read` | mark one read; `204` |
| `POST` | `/api/notifications/read-all` | mark all read; `204` |
| `GET`/`PUT` | `/api/notifications/prefs` | `{ assigned, mention, cycle, digest }` bools |

Generated on: an issue assigned to you (not by you), an `@login` mention in a
comment or description, and a cycle you have assigned issues in starting/ending.
A `notified` SSE event wakes the recipient's clients; the web app also raises a
browser notification when permission is granted.

**Daily digest** (`kind: "digest"`): when `notifications.daily_digest` is on,
a background job runs once a day at `notifications.digest_hour` (local, default
08:00) and sends every user with the `digest` pref one summary notification of
their still-unread items since their last digest. Users with nothing pending
get nothing. The last-sent time is tracked per user so each item appears in
exactly one digest.

---

## SSE events

`GET /events` emits one JSON object per change, tagged by `type`. Each maps to a
`bread.board.*` event on the bread bus (milestone 7).
If a client falls behind the broadcast buffer, the server closes that SSE
connection so `EventSource` reconnects; clients should reload the board snapshot
after reconnecting to recover missed changes.

| SSE `type` | bread event | payload |
| --- | --- | --- |
| `card.created` | `bread.board.card.created` | `id`, `key` |
| `card.updated` | `bread.board.card.updated` | `id`, `key` |
| `card.moved` | `bread.board.card.moved` | `id`, `key`, `from`, `to` |
| `card.assigned` | `bread.board.card.assigned` | `id`, `key`, `assignee_id` |
| `card.deleted` | `bread.board.card.deleted` | `id`, `key` |
| `agent.claimed` | `bread.board.agent.claimed` | `id`, `key`, `agent`, `mode`, `session_id`, `state` |
| `agent.updated` | `bread.board.agent.updated` | `id`, `key`, `agent`, `mode`, `session_id`, `state` |
| `agent.released` | `bread.board.agent.released` | `id`, `key` |
| `cycle.started` | `bread.board.cycle.started` | `id`, `name`, `issues` |
| `cycle.ended` | `bread.board.cycle.ended` | `id`, `name` |
| `sync.drift` | `bread.board.sync.drift` | `issue_id`, `key`, `field`, `breadboard_value`, `forgejo_value`, `winner` (`breadboard`/`forgejo`), `at` |
| `notified` | `bread.board.notified` | `user_id` (clients filter by their own id) |

---

## Forgejo sync

**Direction:** bidirectional.

**Inbound fast path:** Forgejo `issues` / `issue_comment` / `pull_request`
webhooks → `POST /webhooks/forgejo`, HMAC-verified against
`forgejo.webhook_secret`, applied immediately.

**Reconcile:** every `forgejo.reconcile_interval_mins` minutes, per enabled repo,
`GET .../issues?since=<repo_sync_state.last_synced_at>&state=all` (paginated,
issues **and** PRs); apply anything the webhook path missed, then advance
`last_synced_at`. `POST /api/sync/reconcile` runs a pass on demand.

**Writeback:** with `forgejo.api_token` set:

- creating an issue in a project mapped to an enabled repo also creates it on
  Forgejo and links it (`forgejo_ref`); issues in a repo-less project (e.g. the
  roadmap) stay breadboard-only.
- retried writebacks carry a stable hidden local-key marker so a timeout or
  link race can find the already-created Forgejo issue rather than POSTing a
  duplicate. Forgejo requests have a bounded timeout.
- a `PATCH /api/issues/{key}` that changes a synced field on a linked issue is
  pushed (`PATCH issue` for title/body/state/assignees, `PUT labels` otherwise).
- adding a comment on a linked issue is pushed.

**Synced fields:**

| breadboard | Forgejo | direction |
| --- | --- | --- |
| `title`, `body` | issue title / body | both |
| `status` | open (`backlog`–`review`) ↔ closed (`done`); a fine-grained open status is preserved, reopen → `todo` | both |
| `assignee_id` | first assignee, mapped by `forgejo_id` (unknown users are imported) | both |
| `labels` | labels (created on Forgejo if missing) | both |
| `epic`, `cycle`, `priority`, `estimate` | n/a | breadboard-only |
| `links` (`blocks`/`blocked-by`/`relates-to`) | n/a | breadboard-only |
| `links` (`closes`/`closed-by`) | a synced PR's body: closing keywords (Forgejo/GitHub convention) followed by either a Forgejo issue number ("Closes #4") or breadboard's own key ("Fixes ECO-4", agents/humans naturally reference the key they can see) | inbound-derived; re-parsed on every sync of that PR, not just import |
| comments | issue/PR comments | both (see **Comment echo suppression** below) |
| PRs | title / state / labels / assignee / milestone | inbound; writeback limited to **labels** (PRs are read-mostly). A closed PR (merged or closed-unmerged) appears as `done`. A PR's closing references become `closes`/`closed-by` links, and a PR with one is hidden from the board/backlog; its state surfaces as a badge on the issue's own card instead. When a merged PR flips to `done`, its closing targets are cascaded to `done` (or `review` if labelled `verify-before-done`). That label override applies when the close is first observed: once a coordinator verifies the merge and moves the card to `done`, later PR syncs do not drag it back to `review`; a closed-unmerged PR never cascades `done` to targets (targets stay put, or a previously-cascaded `done` is reverted to `review`). Forgejo only reliably auto-closes the *first* issue in a comma-separated "Closes #a, #b" list, so breadboard doesn't rely on that |
| `milestone` | milestone title (plain string on the issue) | inbound only |

**Conflict resolution:** per-field, the side with the newer issue-level
`updated_at` wins; a value that already matches is a no-op (this absorbs the
webhook echo of a writeback). Applied changes append a `sync_log` row
(`field`, `old_value`, `new_value`, `source`, `actor_id`, `at`). When Forgejo is
the newer side its value is applied; when breadboard is the newer side no change
is applied and writeback (or the next breadboard edit) resolves it.

**Drift signal:** when the *reconcile* pass (not the webhook fast path) finds a
field the two sides disagree on, a `sync.drift` event fires in addition to the
`sync_log` row: it carries `field`, both values, the winning side, and a
timestamp. This is the live signal that a webhook was missed. It fires whether
Forgejo or breadboard won the field. The webhook path stays silent: an
out-of-order webhook is not a missed one.

**Comment echo suppression:** a breadboard-authored comment is stored with a
null `external_id`, then writeback records the Forgejo comment id on it. When
Forgejo echoes that comment back over the `issue_comment` webhook, breadboard
recognises it two ways and never duplicates it:

- if the writeback id is already recorded, the `(issue_id, external_id)` unique
  key turns the insert into an in-place update;
- if the echo wins the race with writeback (id not recorded yet), the newest
  unlinked local comment with the same body on that issue is claimed and gets
  the `external_id`. Either way the original `author_id` is kept, so the
  comment is not re-attributed to the writeback account.

A comment that originates in Forgejo is inserted once (keyed by `external_id`)
and is never written back: only `POST /api/issues/{key}/comments` triggers
writeback, and the webhook path does not. Redelivery and edits (same Forgejo id,
new body) update the row in place. Known gaps: comment deletions and comments on
an issue whose creation webhook was missed are not reconciled (webhook only).

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
| `auth login` | device flow: prints a code + URL, opens the browser, polls until approved |
| `auth logout` · `auth status` | |
| `add <title> [--project K] [--epic KEY] [--cycle NAME] [--label L]… [--priority P]` | epic/cycle resolved by key/name; `--project` is required once more than one project exists |
| `project list` · `project new <KEY> <NAME>` · `project rename <KEY> <NAME>` · `project archive`/`unarchive <KEY>` | `new` makes a repo-less project, the right home for a new project that has no repo yet |
| `mv <key> <status>` | |
| `claim <key> --agent NAME [--mode exclusive\|intent] [--session ID] [--state S] [--note N] [--branch B] [--worktree W] [--repo R] [--pr-url U] [--pr-number N] [--head-sha H]` | claim an issue; `exclusive` (default) is first-wins and moves backlog/todo to doing + assigns it, `intent` queues interest without moving or blocking |
| `heartbeat <key> [--agent NAME] [--state S] [--note N] [--branch B] [--worktree W] [--repo R] [--pr-url U] [--pr-number N] [--head-sha H] [--session ID] [--every SECS]` | heartbeat / update your claim; `--every` keeps repeating until interrupted (wrap long runs so they never go stale) |
| `release <key> [--agent NAME]` | release your claim (status stays, tint goes away) |
| `agents list [--stale]` | live agent claims with mode, state and heartbeat |
| `agents status` | server roll-up in the `bb-status` widget shape (`BB x/y done, …`) |
| `agents message <agent> "<text>"` | set (or clear, with `""`) the one-line message under an agent's name |
| `board [--cycle NAME] [--project K] [--mine]` | columns |
| `ls [--mine] [--status S] [--project K]` | |
| `show <key>` | issue + comments |
| `activity <key>` | change history |
| `inbox [--read]` | your notifications; `--read` marks all read |
| `comment <key> "<text>"` | add a comment (pushed to Forgejo if linked) |
| `project list` · `project new <KEY> <NAME>` · `project rename <KEY> <NAME>` · `project archive`/`unarchive <KEY>` | `new` makes a repo-less project, the right home for a new project that has no repo yet |
| `epic list` · `epic new <KEY> <TITLE> [--project]` · `epic rename <KEY> <TITLE>` · `epic archive`/`unarchive <KEY>` | `list` shows done/total |
| `cycle list` · `cycle new <name> <start> <end>` · `cycle start <name>` · `cycle end <name>` | dates `YYYY-MM-DD` |
| `repos list` · `repos add <owner/name> <key> [--name N]` · `repos remove <owner/name>` | |
| `sync` | `POST /api/sync/reconcile` |
| `import <file.md> --project <KEY> [--dry-run]` | markdown outline → epics + issues |
| `tui` | full-screen board: `hjkl`/arrows move, `[` `]` move a card between statuses, `enter` detail, `r` refresh, `q` quit |

## Desktop shell

`apps/desktop/`: a Tauri 2 window that loads the live site (so cookies + OAuth
work unchanged). Global shortcuts: `Ctrl+Shift+B` toggle window, `Ctrl+Shift+N`
show + open the new-issue palette (the SPA acts on a `#new` URL hash). Built on
demand, not in CI. See `apps/desktop/README.md`.

## MCP server

`apps/mcp/`: a stdio MCP server (`@modelcontextprotocol/sdk`) that wraps
`@breadboard/api-client`, so an MCP client (Claude, …) can read and drive the
board. Auth from `~/.config/breadboard/cli.toml` or `BREADBOARD_SERVER` /
`BREADBOARD_TOKEN`. Tools: `whoami`, `list_issues` / `get_issue` / `board`
(filters: project, status, cycle, epic, assignee_id, mine),
`create_issue` / `update_issue` / `move_issue` / `delete_issue` /
`move_issue_project` / `comment`, `activity`,
`list_{projects,epics,cycles,repos}`, `get_epic` / `create_epic` /
`update_epic` / `delete_epic` (delete keeps and unlinks child issues),
`create_project` / `update_project` / `create_cycle` / `start_cycle` /
`end_cycle`,
`add_repo` / `remove_repo`, `reconcile`, `import_markdown`,
`claim_issue` / `update_claim` (heartbeat) / `release_claim` / `resume_claims` /
`set_agent_message` /
`list_agents` / `agent_status` / `suggest_agent_names`,
`inbox` / `mark_notification_read` / `mark_all_notifications_read` /
`get_notification_prefs` / `set_notification_prefs`. See `apps/mcp/README.md`.

Agent rule (also in the `claim_issue` / `update_issue` / `move_issue` tool
descriptions): **claim first, keep the heartbeat alive, release when done**.
No implementation work without an exclusive claim; `409` means another agent
holds it, so pick other work instead of fighting the lock. `mode: "intent"`
queues interest in work you are not starting yet. The MCP server auto-
heartbeats every claim it creates every 60 seconds while it is running, so an
agent using MCP cannot go stale mid-run; CLI-driven agents pass
`heartbeat --every 60` (or loop it) for the same guarantee.

## Operations

Deployed as one container (`packaging/`) behind a TLS proxy. The database is a
single SQLite file in WAL mode on the `breadboard-data` volume.

**Backups:** the `litestream` service in `packaging/compose.yaml` continuously
replicates the database (file replica by default, S3-compatible optional). The
server sets `journal_mode=WAL` once and never checkpoints or switches journal
modes by hand, so Litestream owns checkpointing. Restore procedure and the
row-count verification step are in `packaging/DEPLOY.md`.
