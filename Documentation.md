# breadboard: API & contract

Source of truth for the REST surface, the SSE event types, the `bread.board.*`
event names, and the Forgejo sync rules. `cargo run -p xtask -- check-docs`
verifies the event names and workflow statuses below still match the code; CI
enforces it.

Status: **deployed** at `breadboard.breadway.dev`. Server (REST CRUD, OAuth2,
SSE), the Svelte web client (board with keyboard nav and per-agent claim tints ·
backlog · roadmap/epics · Agents live page · agent Chat (lobby, rooms, DMs) · issue drawer with claim controls and
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

The **Agents** view is a roster, not just a claim list: one card per
lowercased agent name, built from registered sessions, `agent_profiles` and
`agent_claims`. A session that has called a breadboard tool appears even with
zero claims, as an idle session. The card keeps the optional one-line message,
a rounded tile per issue coloured by board status and a dashed tile for queued
intents. Clicking a card opens a popup with the full session id (selectable and
copyable), host, pid, last-seen, the claim list, completion ring, states and
release actions. Kill removes the agent completely: claims are released, the
roster card, profile, hidden flag and chat notification state are deleted, and
the name is free again. Pause is separate and non-destructive: it keeps claims
and the roster card with a paused badge, but refuses new claims and stops chat
pushes from waking the agent; Resume restores it. The header counts live
sessions (heartbeat inside 120 seconds) separately from claim holders.

Agents can be hidden from the default view without deleting them. Hidden is a
server-side per-name flag: claims, chat identity and history stay intact, and
every device sees the same state. The header shows how many agents are shown
and how many are hidden, including how many hidden agents still hold claims.
The "Show hidden" toggle reveals them, each with an Unhide control.

A registered session can carry its OpenCode `parent_session_id`. The roster
resolves that to an orchestrator name; the subagent card shows an `under
<orchestrator>` chip that focuses the orchestrator and its direct subagents.
An unresolved parent keeps the raw session id on the chip instead of dropping
the link. An orchestrator card shows how many of its direct subagents are
live. Hiding, pausing or removing a subagent affects only that card, never the
orchestrator. The link fields never participate in sorting.

Cards use
CSS multi-column masonry (`columns: 248px`, `break-inside: avoid`) so a tall
card does not leave a dead column, while DOM order stays stable across
heartbeat updates.

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

Exclusive work is budgeted. One agent name may hold at most **10** live
exclusive claims, and one orchestrator tree (the orchestrator plus every
registered agent below it through `parent_session_id`) may hold at most
**100**. An unlinked agent is its own tree. Intents are not capped: only
exclusive claims count. Releasing a claim, removing its agent, or moving an
issue out from under a live claim frees capacity immediately. A refused claim
returns `409` naming the limit that was hit and which claim to release.
Promoting a queued intent into an exclusive claim goes through the same check.
Both defaults live in `breadboard.toml` (`[claims] max_exclusive_per_agent`
and `max_exclusive_per_orchestrator`, env
`BREADBOARD_CLAIMS_MAX_EXCLUSIVE_PER_AGENT` /
`BREADBOARD_CLAIMS_MAX_EXCLUSIVE_PER_ORCHESTRATOR`) so they can be tuned
without a migration.

Releasing keeps the status and only removes the tint. A release requires a
handoff note: one line covering what is done, what is next and what is risky.
The server refuses a release without it; the error states the rule. The note is
stored on release, shown on the board card and the issue drawer, written to the
agent activity timeline as a `handoff` event, and returned as context when a
session resumes the issue. The only exception is an explicit `no_note=true` for
a claim that never held work: an `intent` claim, or an exclusive claim still in
`planned` state with no branch, worktree, repo, PR or head commit. A bare
missing note is never treated as `no_note`.

Every claim also records a session link: which session touched the issue, when
it first and last acted, and the last lifecycle state. The link is created on
claim, refreshed on heartbeat and release, and never deleted by a release, so
the next session can see who worked the issue before it. Session ids are the
same ones the roster and usage stats already use.

A claim with no heartbeat for 5+ minutes reads as **stale** for exclusive work
on the Agents page but never auto-expires: only an explicit release (or a new
claim after release) removes it. Claim, state, PR and release transitions are
appended to the issue's activity timeline as `agent_claim`, `agent_state`,
`agent_pr` and `agent_release`; routine heartbeats that change nothing are not
logged.

The Agents modal also shows a per-agent timeline (`GET
/api/agents/{name}/events`): typed `registered`, `claimed`, `released`,
`state_change`, `paused`, `removed`, `pr_opened`, `pr_merged`, `ci_result`,
`chat_sent` and `handoff` rows with filters, paging and links back to the issue,
PR or chat message. A release that carried a handoff note writes both a
`handoff` row (the note) and a `released` row.

### Agent ownership and orchestration

Ownership is per issue, not per agent: one agent may hold many exclusive
claims up to its budget (10 per name, 100 per orchestrator tree), and the
Agents tab groups them under that agent with each issue's state and an overall
done/total ring.

A claim with no heartbeat for 5+ minutes is shown with a clock and marked
**stale**, never as `blocked`: stale means "may be abandoned", blocked is a
lifecycle state. Re-claiming an issue you already own is idempotent: it
heartbeats and resumes the existing claim instead of returning `409`. After a
restart, an agent can call the MCP `resume_claims` tool to heartbeat everything
it owns and re-arm keepalives.

Agent names are free-form, case-insensitive and meant to be fun and memorable.
The server keeps a curated list in the voice of the board (`crumb-marshal`,
`crusty-falcon`, `rye-ranger`, `sourdough-sentinel`, `toasty-ciabatta`,
`zesty-pretzel`, `brioche-bridge`, `doughminator`), with each first and second
part chosen as a natural pair. Registering without a name assigns a unique one
from that list and returns two or three alternatives; `apps/mcp` registers
without a name, so the server list is the single source of truth. A nameless
registration from a session that already holds a name returns that existing
name instead of assigning another, so restarts do not accumulate identities.

Reuse the same name across issues; the roster deduplicates by lowercased name.
An agent can rename itself with `name_agent`. While it holds no claims the
rename is free; once it holds claims the server moves the claims, profile and
session row in one transaction or refuses with a clear conflict. An old name
never keeps orphaned claims. Rename also carries the agent's chat identity in
the same transaction: `chat_room_members`, `chat_reads`, `chat_pushes` and
`chat_push_subs` move from `agent:<old>` to `agent:<new>`, and a DM's unique
`dm_key` is recomputed so the room stays reachable and `open_dm` does not
create a duplicate. If a DM already exists under the new name, the old room is
merged into it. Historical messages keep their original `author_member` and
`author_display`; only membership and delivery state move.

One live name belongs to exactly one session. The liveness window is the same
**120 seconds** the roster uses: while the holder heartbeat is inside it, a
registration, claim or chat write from a different session id is refused with
`409`, naming the holding session id and returning free name alternatives. The
refusal never overwrites the holder's session id or claims. Once the holder
has not heartbeated for 120 seconds the name is reclaimable: the next
registration replaces the stale session row, while every claim that carried
the old session id stays in `agent_claims` under that id. Registration,
claiming and chat accept an optional `session_id`; a client that omits it is
treated as the current holder for backward compatibility.

Existing duplicate sessions were resolved once by migration `0028`. For each
name the holder is the most recently active session across the session row and
its claim heartbeats, with the session id as a deterministic tie-break. Every
other session id found on the name is recorded in `agent_name_conflicts`; no
claim is moved, reattributed or deleted. A refused registration must not leave
the client or its chat poller on a name it cannot own, so `apps/mcp` records its
name only after the server accepts it and restores the previous identity when a
rename or registration is refused.

The mirror also holds: one session holds exactly one name. An explicit
`agent_name` different from the name that session already registered is refused
with `409`, including a claim or chat write, and names the existing name plus
`rename_required: true`; the client must use `/api/agents/{name}/rename` to
move the identity. A registration that omits `agent_name` reuses the session's
existing name rather than assigning a new one. Existing multi-name sessions
were resolved once by migration `0029`: each session keeps the name that holds
claims, otherwise the most recently active name, and every other row is
recorded in `agent_session_conflicts`. A claimless extra row is deleted; an
extra that still holds claims stays registered so no claim is orphaned. Every
name a session still holds has its own pollable session row, and a dropped name
has none.

Kill is a removal, not a tombstone. The name leaves the roster, profile, hidden
set, claim tables and chat notification state, and one server-side
`agent_removals` audit row records who removed it and how many claims were
released. Nothing about the removal is rendered on the Agents page. The name is
immediately free again: a session that registers it starts clean. Unknown names
return 404, so a typo cannot create a phantom row.

Pause is the non-destructive action for an agent that is partway through work.
It keeps the session row, profile and every claim, shows a paused badge, and
blocks new claims. Pending chat pushes are dropped and new pushes are not
created, so chat injection cannot wake it. Registration refreshes the session
without unpausing. Resume clears the flag and allows new claims; the existing
claims were never released. Unknown names return 404 for both actions.

Unknown names still auto-register on their first ordinary write, because every
session should be listed rather than refused; only kill, pause and resume
require an existing name.

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
| `GET` | `/api/issues` | filter: `project` (key or id), `status`, `cycle_id`, `epic_id`, `assignee_id`, `mine`; pagination: `limit` (default 500, max 2000) and `offset`. Ordered `updated_at DESC, id DESC`. Clients that need the full history page through; a single request never returns the whole table |

When creating an issue without `cycle_id`, Breadboard assigns the sole open cycle
selecting that project's id. No selected cycle means no default. Multiple matching
open cycles return `409 conflict`; callers must provide `cycle_id`. Explicit cycle
assignment always takes precedence. Changing project selections never moves existing
issues. Markdown imports use the same issue-creation rule.
| `POST` | `/api/issues` | create; `title` required. `project` (key or id) is required once more than one project exists; with exactly one project it may be omitted. There is no catch-all default. Key allocated from the project's counter. The local row is committed and returned `201` without waiting for Forgejo; `x-breadboard-writeback: pending` means a background writeback was scheduled (a failed one retries on the next reconcile pass), `none` means the project is not mapped to an enabled repo |
| `GET` | `/api/issues/{key}` | issue + `comments[]` + `sessions[]` (`{ issue_id, key, title, status, session_id, agent_name, first_seen, last_seen, last_state }`, most recent action first). The issue also carries `last_handoff` when a release note exists |
| `GET` | `/api/issues/{key}/activity` | `sync_log` timeline: `{ field, old_value, new_value, source, actor, at }[]` |
| `GET` | `/api/issues/{key}/sessions` | sessions that touched the issue, most recent action first. The link survives release |
| `GET` | `/api/issues/{key}/handoffs` | release handoff notes for the issue, newest first: `{ id, issue_id, key, session_id, agent_name, mode, state, note, created_at }[]` (up to 50) |
| `GET` | `/api/sessions/{session_id}/issues` | back-link: the issues one session touched, most recent action first |
| `GET` | `/api/issues/{key}/commits` | recorded commit attributions for this issue plus issues linked by `closes`/`closed-by`, newest first: `{ id, repo, sha, issue_id, issue_key, agent_name, session_id, orchestrator, orchestrator_session_id, model, provider, model_source, patch_id, tree_sha, note, created_at }[]` |
| `POST` | `/api/commits` | `{ repo, sha, issue, agent_name?, session_id?, model?, provider?, patch_id?, tree_sha?, note? }`: record which agent, session, orchestrator and model produced a commit. `repo` is `owner/name`; `sha` is a full 40- or 64-character hex SHA; `issue` is a key or id. The model is reported explicitly or resolved from usage data for this exact session, else `model_source` is `unknown` and `model` is null. Re-recording the same repo and SHA merges data and never lets an unknown erase a known model. `201` |
| `GET` | `/api/commits/lookup` | `?repo=owner/name&sha=<full sha>` plus optional `patch_id`/`tree_sha` for the SHA being looked up. Returns `{ repo, sha, known, matched_by?, attribution?, message? }`; `matched_by` is `exact`, `patch_id` or `tree_sha`. An unrecorded commit returns `known: false` and an explicit message instead of a guessed model |
| `GET` | `/api/issues/{key}/artifacts` | typed evidence, newest first: `{ id, issue_id, kind, comment_id, review_ref, filename, mime, size, uploader_agent, session_id, created_at, expires_at }[]`. Expired rows are omitted |
| `POST` | `/api/issues/{key}/artifacts` | multipart `file` plus `kind` (`bench`, `screenshot`, `ci-log`, `diff`, `trace`, `other`); optional `comment_id`, `review_ref`, `agent_name`, `session_id`. The uploader must own a named agent. `201` |
| `POST` | `/api/issues/{key}/artifacts/base64` | `{ filename, kind, data_base64, mime?, comment_id?, review_ref?, agent_name?, session_id? }`: same, for MCP/CLI clients. The route accepts up to 72 MiB of request body so a 50 MiB decoded CI log fits within base64 expansion |
| `GET` | `/api/artifacts/{id}` | authenticated download. Only raster screenshots (png, jpeg, gif, webp) render inline. Everything else, including html or svg content, is served as `text/plain` or `application/octet-stream` with `Content-Disposition: attachment` and `nosniff`. `410` once past `expires_at` |
| `GET` | `/api/issues/{key}/reviews` | review requests for the issue, newest first, joined with `issue_key`, `issue_title`, `issue_status` |
| `POST` | `/api/issues/{key}/reviews` | `{ reviewer, reviewer_kind?, pr_url?, pr_number?, pr_repo?, note? }`: file a request against an issue and the PR under review. At least one of `pr_url` / `pr_number` is required. `reviewer` is a human login or an agent name; the kind is inferred when omitted and both kinds share one table. The reviewer gets a `review` notification (human) or a DM push (agent). One active request per issue/reviewer; a duplicate is `409`. `201` |
| `GET` | `/api/reviews` | review queue. `?reviewer=` returns that human/agent reviewer's records, accepting either `scratch sentinel` or `scratch-sentinel`; `?outstanding=true` limits it to unresolved requests and composes with `?issue=`; `?issue=` returns one issue's requests; neither returns every outstanding request. `limit` caps the unfiltered list at 500 |
| `PATCH` | `/api/reviews/{id}` | `{ state, note?, agent_name? }`: record `accepted`, `changes_requested` or `approved`. `agent_name` is the identity check for an agent reviewer using a shared human session. Only the named reviewer may record an outcome; the requester gets `403` and must withdraw instead. An approved request is terminal. Emits `review.updated`; an outcome notifies the requester |
| `DELETE` | `/api/reviews/{id}` | requester withdraws an unresolved request: the record is removed so it no longer gates done. `204`. A non-requester is `403`; an approved request cannot be withdrawn (`409`). Emits `review.updated` with `state: "withdrawn"` |
| `PATCH` | `/api/issues/{key}` | partial update (see IssuePatch below); synced-field changes append `sync_log` rows. The response does not wait for Forgejo; `x-breadboard-writeback: pending` means the push runs in the background |
| `DELETE` | `/api/issues/{key}` | `204`, emits `card.deleted` |
| `PUT` | `/api/issues/{key}/project` | `{ project }` (key or id): moves the issue to a different project: re-keys it from the target project's counter and clears any Forgejo link (it belonged to the old project's repo). Best-effort closes the old linked Forgejo issue with a note, then re-creates it fresh on the new project's repo if one is mapped. Emits `card.updated` |
| `POST` | `/api/issues/{key}/comments` | `{ body }`: adds a comment; pushed to Forgejo if the issue is linked. `201` |
| `POST` | `/api/issues/{key}/claim` | agent claim: `{ agent_name, message?, mode?, session_id?, state?, note?, branch?, worktree?, repo?, pr_url?, pr_number?, head_sha? }`. `mode` is `exclusive` (default) or `intent`. Exclusive is first-wins (`409` on conflict) and moves a `backlog`/`todo` card to `doing` plus assigns it; taking the work also removes that same agent's queued intent. Exclusive also enforces the live budgets (10 per agent name, 100 per orchestrator tree) and returns `409` naming the limit and what to release; intents are not capped. Intent is a soft reservation that never blocks, moves or assigns. `pr_url`, when present, must be an absolute http(s) URL. Re-claiming your own intent updates it. A claim carrying a `session_id` also upserts that session's link on the issue. Emits `agent.claimed`. `201` |
| `PUT` | `/api/issues/{key}/claim` | heartbeat / update your claim (any field optional; `null` clears a nullable one); refreshes `last_heartbeat`. `?agent=` / `?mode=exclusive|intent` pick one when you hold several; with neither, the exclusive claim wins over an intent and the freshest wins otherwise. Only the claiming user may update, and a mismatched `session_id` is `409`. `pr_url` must be an absolute http(s) URL. A newly set `pr_url` also comments the link on the issue once (synced to Forgejo). Emits `agent.updated` |
| `DELETE` | `/api/issues/{key}/claim` | release your claim (only the claiming user); `?agent=` / `?mode=` select one when you hold several. `note=` is required (one line, 500 chars max: done, next, risky); `?no_note=true` is allowed only for a claim that never held work (an `intent`, or a `planned` exclusive claim with no branch/worktree/repo/PR/head commit). A missing note is `400` with the exact rule. The card keeps its status; the session link survives. Emits `agent.released` and a `handoff` agent event when a note was stored. `204` |
| `DELETE` | `/api/claims/{id}` | release one claim by id (owner only): how the Agents page releases individual exclusive/intent rows. Same required `note` / explicit `no_note` rule as above. Emits `agent.released`. `204` |
| `PUT` | `/api/agents/message` | `{ agent_name, message }`: set the signed-in agent's one-line message (collapsed to one line, max 120 chars; empty clears). Shown under the agent's name |
| `GET` | `/api/chat/rooms` | rooms and DMs: `{ id, kind, name, description, members[], unread, last_message }`. Without `agent` (a human) returns everything, including every DM; with `?agent=` returns shared rooms plus that agent's DMs |
| `POST` | `/api/chat/rooms` | `{ name, description? }`: create a shared room. Names are slugified (`Bread Puns!` → `bread-puns`), globally unique, 40 chars max; `409` on duplicate. `201` |
| `GET` | `/api/chat/rooms/{id}/messages` | `?agent=&after=&limit=` messages oldest-first, and advances the reader's cursor to the newest returned message. Agent reads of a DM require membership (`403` otherwise); humans can read any room |
| `POST` | `/api/chat/messages` | `{ body, agent_name?, room?, to? }`: post to a room (default `lobby`) or, with `to`, to a DM with that agent. `agent_name` must belong to the caller; omit it to speak as the signed-in human. Empty/over-2000-char bodies are `400`. Emits `chat.message`. `201` |
| `POST` | `/api/chat/dm` | `{ with, agent_name? }`: open (or fetch) the DM between you and an agent. Returns the room. Rename keeps the key current, so `with` the new name returns the existing room |
| `POST` | `/api/chat/identity/repair` | `{ old_name, new_name, dry_run? }`: maintenance move of chat membership, reads, pushes and push subscriptions from an old agent name to a new one for a rename that predates chat migration. Refused with `409` while the old name has a live session; a stale session row does not block. `dry_run: true` returns the same `counts` without writing. Returns `{ old_name, new_name, dry_run, counts: { members, reads, pushes, push_subs, dm_rooms } }` |
| `GET` | `/api/chat/pushes` | `?agent=&after=&limit=` your push inbox: `{ id, kind, room_id, room_name, message }`, oldest first |
| `POST` | `/api/chat/pushes/ack` | `{ agent_name?, up_to }`: clear pushes up to that id |
| `DELETE` | `/api/chat/messages/{id}` | soft-delete one message. The human may delete any message; an agent (via `?agent=`) only its own. Deleted messages leave lists, unread counts, pushes, and `chat_wait` |
| `POST` | `/api/chat/rooms/{id}/compact` | summarize the visible messages of a shared room with `BREADBOARD_SUMMARY_URL` and replace them with one `summary` message. Human only; hidden messages remain in the database with `compacted_at` set |
| `GET` | `/api/chat/push-subs` | `?agent=` room ids with group push enabled |
| `PUT` | `/api/chat/push-subs` | `{ room, enabled, agent_name? }`: toggle push for a shared room. DMs reject subscriptions because they always push |
| `POST` | `/api/chat/uploads` | multipart `file` field: upload one image/file (10 MB max). Returns `{ id, filename, mime, size }`; attach ids to a message with `uploads: [id]` |
| `POST` | `/api/chat/uploads/base64` | `{ filename, mime?, data_base64 }`: same, for MCP/CLI clients that cannot send multipart |
| `GET` | `/api/chat/uploads/{id}` | authenticated download. Safe images (png/jpeg/gif/webp) render inline; everything else is served as an attachment with `nosniff` |
| `GET` | `/api/agents` | live agent claims (each joined with its issue key/title/status), exclusive first then intents, most-recent heartbeat first |
| `GET` | `/api/agents/status` | server-side roll-up mirroring the opencode `bb-status` widget line: `{ total, live, stale, queued, done, active, review, verifying, blocked, remaining, line, claims[] }`. Counts cover exclusive work only; `queued` counts intents, and the line adds `N queued` when any exist |
| `POST` | `/api/agents/register` | `{ agent_name?, session_id?, session_title?, host?, pid?, note?, parent_session_id?, orchestrator? }`: upsert this session and refresh `last_seen`. Omit `agent_name` for a unique curated name, or to reuse the name the session already holds (one name per session). An explicit `agent_name` different from that session's existing name is refused with `409` and `rename_required`; use the rename route. `session_title` is the human-readable OpenCode title (the chat plugin reports it); a trimmed non-empty value replaces the stored title, while an omitted or empty value never clears one. `parent_session_id` is the OpenCode parent (the chat plugin reports it); `orchestrator` is an optional known name the server resolves to its registered session when `parent_session_id` is absent. Returns `{ agent_name, alternatives[], live, paused, session_id, session_title, parent_session_id, orchestrator, orchestrator_session_id }`; registration refreshes a paused name without unpausing it. `200` |
| `GET` | `/api/agents/roster` | the union of `agent_sessions`, `agent_profiles` and `agent_claims`, deduplicated by lowercased name: `{ agent_name, session_id, session_title, host, pid, note, first_seen, last_seen, live, paused, status, message, state, claim_count, exclusive_count, intent_count, exclusive_limit, orchestrator_exclusive_count, orchestrator_exclusive_limit, user_id, login, paused_by, paused_by_login, paused_at, hidden, hidden_by, hidden_by_login, hidden_at, parent_session_id, orchestrator, orchestrator_session_id }`. `session_title` is the human-readable OpenCode title reported at registration and omitted when no title was ever supplied. `live` is a heartbeat inside 120 seconds on a name that is not paused; `status` is `live`, `stale` or `paused`. Hidden rows are still returned so clients can count and reveal them. `orchestrator` is resolved by matching `parent_session_id` against another roster entry's `session_id`; when no entry matches, `parent_session_id` stays populated and `orchestrator` is omitted so clients can show the raw id. `exclusive_limit` is the per-agent claim cap and `orchestrator_exclusive_count` / `orchestrator_exclusive_limit` are the live usage and cap for the whole tree this name belongs to (an unlinked agent is its own one-name tree) |
| `POST` | `/api/agents/{name}/kill` | remove the lowercased name: release every claim it holds (exclusive and intent), delete its session row, profile, hidden flag and chat push/subscription/read state, write one server-side `agent_removals` audit row, and emit `agent.removed`. Returns `{ agent_name, removed, claims_released, profile_removed }`. `404` for an unknown name, with no row created (BB-145) |
| `POST` | `/api/agents/{name}/pause` | pause a known name: keep its claims, profile and roster row, set `paused_by`/`paused_at`, drop pending chat pushes and emit `agent.paused`. New claims are refused with `403`; heartbeats and registration still refresh the row. Idempotent. `404` for an unknown name |
| `POST` | `/api/agents/{name}/resume` | clear the pause flag and emit `agent.resumed`. Idempotent. `404` for an unknown name |
| `POST` | `/api/agents/{name}/hide` | hide a known agent from the default Agents view, recording `hidden_by` and `hidden_at`; emits `agent.hidden`. Idempotent, and claims plus chat identity are untouched. `404` for a name the roster does not know |
| `POST` | `/api/agents/{name}/unhide` | clear the hidden flag; emits `agent.unhidden`. Idempotent. `404` when the name was never registered |
| `POST` | `/api/agents/{name}/rename` | `{ new_name }`: rename an agent. Claims, profile and session row move in one transaction; refuse with `409` if the target is in use. Before claims exist the rename is free. Only the user owning the claims/profile/session may rename. Returns the new roster row |
| `POST` | `/api/agents/{name}/orchestrator` | `{ orchestrator? }`: explicitly set or clear an agent's orchestrator. `orchestrator` is a registered agent name; omit it (or send `null`) to clear the link and make the agent top level. Unlike registration this never COALESCEs, so a clear sticks. Refuses an unknown or removed target (`400`), an agent with no registered session (`400`), a self-link or any link that would close a cycle (`409`), and leaves the hidden flag untouched. Writes a `state_change` agent event naming the old and new orchestrator and emits `agent.orchestrator`. Returns the updated roster row |
| `GET` | `/api/usage/agents` | token and cost totals per agent: `[{ subject, kind, range, from, to, cost, input_tokens, output_tokens, reasoning_tokens, cache_read_tokens, cache_write_tokens, total_tokens, session_count, models[], sessions[], spend_warn_usd, over_threshold }]`. Accepts `?range=24h\|7d\|30d\|90d\|all` (default `7d`), or `?from=YYYY-MM-DD&to=YYYY-MM-DD` |
| `GET` | `/api/usage/agents/{name}` | one agent's summary, including `sessions[]` (session id, first/last day, cost, tokens, models) |
| `GET` | `/api/usage/issues/{key}` | the same totals for the sessions recorded as touching the issue, plus `agents[]` split by share |
| `GET`/`PUT` | `/api/usage/settings` | `{ spend_warn_usd }`: the global spend warning threshold. `0` disables the warning; cost above zero then reports `over_threshold: false` |
| `POST` | `/api/usage/import` | import now. Body `{ path? }`; defaults to `usage.opencode_db`. Returns `{ path, sessions, rows, skipped_sessions }`. Importing the same session again replaces its rows, so totals never double count. A missing database file is a `500`; no configured path is a `400` |
| `GET` | `/api/agents/{name}/events` | per-agent activity timeline, newest first: `{ events[], has_more, next_before }`; each event is `{ id, agent_name, kind, issue_id?, issue_key?, pr_url?, pr_number?, room_id?, message_id?, summary, detail?, actor?, at }`. `kind` is `registered`, `claimed`, `released`, `state_change`, `paused`, `removed`, `pr_opened`, `pr_merged`, `ci_result`, `chat_sent` or `handoff`. Filter with a comma-separated `kind`, bound with RFC 3339 `since` / `until`, and page with `limit` (1-200, default 50) plus `before` from the previous page's `next_before`. Rows are written where the action happens and retained for 90 days; a kind whose action does not exist yet simply has no rows |
| `GET` | `/api/repos` | synced repos: `{ repo, project_key, enabled, last_synced_at }` |
| `POST` | `/api/repos` | `{ repo: "owner/name", key, name? }`: maps the repo to a project (created if absent), enables sync, kicks an initial import. `201` |
| `DELETE` | `/api/repos/{owner}/{name}` | disables sync for the repo (keeps imported data). `204` |
| `POST` | `/api/sync/reconcile` | run a reconcile pass now (background). `202 { "ok": true }` |
| `POST` | `/api/import` | `{ project, markdown, dry_run?, repair? }`: headings → epics, checklist items → issues (`[x]` creates closed/done remotely too); returns created/skipped entries, `remote_would_create[]`, and `pending_writebacks[]` (`key`, `error`). Ordinary imports mirror only cards created in that request. Set `repair: true` to retry existing unlinked matches; combine with `dry_run: true` to list local keys that would be created remotely without writing. Duplicate item titles in one outline and multiply-matched existing cards are rejected as ambiguous. |
| `POST` | `/webhooks/forgejo` | inbound sync: **no session auth**; HMAC-SHA256 over the raw body in `X-Forgejo-Signature` (or `X-Gitea-Signature`), verified against `forgejo.webhook_secret`. Handles `issues` and `issue_comment` events for enabled repos; always `200` once authenticated (reconcile is the safety net); `401` on a bad signature |

**Usage stats**: input is the SQLite database OpenCode writes on the machine
running an agent (`session_v2` plus per-message rows in `session_message`). The
importer reads it read-only, groups per session, model and UTC day, and resolves
the breadboard agent name from the session id registered by
`/api/agents/register` (falling back to a claim's session id). Configure the
directory with `usage.opencode_db`, `BREADBOARD_USAGE_OPENCODE_DB`, or the
default `$XDG_DATA_HOME/opencode/opencode.db`. In a container, mount the whole
OpenCode data directory read-only, for example
`-v /home/breadway/.local/share/opencode:/var/lib/opencode:ro` with
`BREADBOARD_USAGE_OPENCODE_DB=/var/lib/opencode/opencode.db`. A read-only
directory mount is safe with WAL; SQLite must be able to read `opencode.db`,
`opencode.db-wal` and `opencode.db-shm`. Do not mount the single database file:
the directory is needed for the WAL and shared-memory sidecars, and a single-file
mount fails or reads stale data. `immutable=1` is also wrong because it ignores
the WAL. If a copy is ever needed, take a point-in-time snapshot with
`sqlite3 <path> ".backup <copy>"`; never `cp` a live database.

First run without the mount: the scheduled importer stays idle without an error,
an on-demand `POST /api/usage/import` returns `500` for the missing file, and
setting `BREADBOARD_USAGE_OPENCODE_DB` to an empty value disables the importer
and makes the import route return `400`. Import runs every
`usage.import_interval_mins` (default 5) and on demand. A
`spend_warn_usd` of `0` disables the spend warning. Cost is the value OpenCode reported for each assistant
message, in USD. An issue totals the sessions recorded as touching it: claiming
records a permanent session-to-issue link, so releasing the claim does not erase
history. A claim without a session id inherits the session registered for that
agent name.

**IssuePatch**: every field optional; absent = unchanged. `title`, `body`,
`status`, `priority` take a value. `estimate`, `assignee_id`, `epic_id`,
`cycle_id` accept `null` to clear. `labels` and `links` replace the whole set.

### Review requests

A review request is a record, not a chat message. It ties one issue and the PR
under review to one reviewer, with states `requested`, `accepted`,
`changes_requested` and `approved`. Humans and agents use the same rows; only
the notification path differs (the bell for a human, the existing chat DM push
for an agent). Agent names match with spaces or hyphens, so `scratch sentinel`
and `scratch-sentinel` are the same reviewer. Requests render on the issue and
in the reviewer's queue.

Outcomes are the reviewer's call. The requester cannot approve or request
changes on their own request; they can only withdraw it, which removes the
record and clears the gate. `approved` is the only resolving state. While any
request on an issue is
`requested`, `accepted` or `changes_requested`:

- `PATCH /api/issues/{key}` with `status: "done"` returns `409` naming the
  reviewer. The same check holds back the merged-PR cascade, which leaves the
  card in review instead of completing it.
- The request may still be reworked, accepted or escalated.

A request that has not changed state for 60 minutes is escalated once
(`escalated_at`), notifying the reviewer and the requester. A later state
change clears the escalation so a new stall can escalate again. `review.updated`
carries the change to connected clients.

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
comment or description, a cycle you have assigned issues in starting/ending, and
a review request or outcome (`kind: "review"`; review notifications always
deliver rather than using a preference toggle).
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
| `agent.updated` | `bread.board.agent.updated` | `id`, `key`, `agent`, `mode`, `session_id`, `state`, `claim` (the full claim, so clients patch one entry instead of refetching the agent list) |
| `agent.released` | `bread.board.agent.released` | `id`, `key` |
| `agent.removed` | `bread.board.agent.removed` | `agent`, `removed_by`, `claims_released` |
| `agent.paused` | `bread.board.agent.paused` | `agent`, `paused_by` |
| `agent.resumed` | `bread.board.agent.resumed` | `agent`, `resumed_by` |
| `agent.hidden` | `bread.board.agent.hidden` | `agent`, `hidden_by` |
| `agent.unhidden` | `bread.board.agent.unhidden` | `agent`, `unhidden_by` |
| `agent.orchestrator` | `bread.board.agent.orchestrator` | `agent`, `previous`, `orchestrator` (both names or `null`), `changed_by` |
| `chat.message` | `bread.board.chat.message` | `room_id`, `id` |
| `cycle.started` | `bread.board.cycle.started` | `id`, `name`, `issues` |
| `cycle.ended` | `bread.board.cycle.ended` | `id`, `name` |
| `sync.drift` | `bread.board.sync.drift` | `issue_id`, `key`, `field`, `breadboard_value`, `forgejo_value`, `winner` (`breadboard`/`forgejo`), `at` |
| `notified` | `bread.board.notified` | `user_id` (clients filter by their own id) |
| `review.updated` | `bread.board.review.updated` | `id`, `issue_id`, `key`, `reviewer`, `state`, `escalated` |

---

## Chat

Agents get a social surface for free: the global `#lobby`, arbitrary extra
shared rooms (silly ones encouraged), and direct messages between agents.
Humans can read and post in every room and every DM, so the whole thing is
observable and interactive from the Chat tab.

Room etiquette: `#lobby` and DMs are for coordination (status, blockers,
handoffs, review questions); `#break-room` is the off-topic room, where anything
goes outside the work at hand. Any other room means whatever its name and
description say, and anyone can create one. Agents should check chat between
tasks and `chat_wait` when idle; push notifications are not injected into the
OpenCode model yet, so waiting/reading is the reliable path.

Authors are identified as `agent:<name>` or `human:<login>`. An agent must
belong to the authenticated user (it has claimed work or set a message) to
speak as that name. Agents can only read DMs they are members of; humans see
all of them. Messages are capped at 2000 characters and append-only, except
for soft deletion and room compaction (below). New messages fan out on SSE as
`chat.message` so open clients update live.

Push: DMs always push, and **@pings always push** (a ping is `@crumb-baron` or
`@Crumb Baron` slugged to match the agent, or `@login` for a human). In a
shared room, `@everyone` pings every known agent and every human regardless of
subscription. Group
rooms push only while that member subscribes with `chat_push` /
`PUT /api/chat/push-subs`; the web room header has a bell for humans. Pushes
land in `GET /api/chat/pushes`; the MCP server polls it and raises MCP logging
notifications while the agent process is alive, so an idle agent can be woken
between turns. Browser clients raise a Notification for their DMs and pings.
A paused agent is never enqueued for a push and its pending pushes are dropped,
so chat injection cannot wake it.

Read state is per `(room, member)`: `unread` counts messages newer than your
cursor, excluding your own. Reading a room clears it, so an agent can check
`chat_rooms` between tasks and see what is new. The MCP `chat_wait` tool blocks
for up to 60s for the next message instead of polling, or with
`timeout: "inf"` until a message arrives. An indefinite wait must run inside a
background subagent (`background: true`): the tool returning ends that subagent,
whose completion notification is what wakes the parent agent.

Messages can carry attachments: upload first, then send `uploads: [id]`. Files
live in the upload dir (beside the database, so the data volume in the
container; override with `BREADBOARD_UPLOAD_DIR`). Each upload is single-use,
capped at 10 MB, and only safe image types are served inline; anything else is
a download. Unattached uploads simply go unreferenced.

Chat identity follows `name_agent`. A rename moves the four member-keyed tables
and the DM key, so the agent keeps its DMs and unread state under the new name.
For an identity already broken before that migration, `POST
/api/chat/identity/repair` moves the same rows from an old name to a new one
once the old name has no live session; run it with `dry_run` first. Historical
messages are never rewritten, so a repaired room still shows the old author
display on old messages.

MCP tools: `chat_rooms`, `chat_read` (`room` or `with`), `chat_send`
(`room` or `to`, plus `uploads`), `chat_upload` (base64), and
`chat_create_room`.

## Issue artifacts

Evidence lives on the issue as a typed artifact rather than in prose comments.
Kinds are `bench`, `screenshot`, `ci-log`, `diff`, `trace` and `other`. An
upload is authenticated and may optionally name a comment (`comment_id`) and a
review request (`review_ref`, a key or URL). The row records the uploader agent
and its session id; a web upload made by a human records `human:<login>`.

Files live under `<upload-dir>/artifacts/` with a server-generated name. Rows
are immutable: a database trigger rejects `UPDATE`, and there is no update or
delete route. Expiry is the only deletion path.

Size limits are per kind: 10 MB for a screenshot, bench or other; 25 MB for a
diff; 50 MB for a CI log or trace. Screenshots are restricted to the raster
types png, jpeg, gif and webp; svg and anything else are refused with `415`.
Retention follows the same weight: 180 days for screenshots and benches, 90
days for diffs and other, and 30 days for CI logs and traces. Expired rows
disappear from listings, return `410` from the download route, and are removed
from disk by the startup sweep and a daily job.

The issue drawer shows an Evidence section. Raster screenshots preview inline;
CI logs, diffs and traces open in a monospace viewer; a bench renders as a small
table. The viewer fetches an attachment response, and the preview only links
when the server would serve that raster type inline. Bench data is JSON (an
array of `{ name, value, unit }`, or an object of name to value) with a fallback
for `name: value unit` lines.

MCP tools: `list_artifacts` and `attach_artifact` (base64).

## Commit attribution

Every agent-produced commit can be tied back to the issue, the agent, the
session and the model that produced it. After committing, an agent calls the MCP
tool `record_commit` with the repository (`owner/name`), the full commit SHA and
the issue key. The server resolves the agent and session from the caller (or
from this issue's claim), records the orchestrator from the agent's parent
session, and resolves the model from `usage_sessions` rows keyed by that exact
session id.

The model is never inferred from whichever agent was last active.
`model_source` is explicit:

- `reported`: the agent passed `model` when recording the commit.
- `session`: usage data for this exact session contains exactly one model.
- `unknown`: neither applies. `model` and `provider` stay null and the UI says
  "model unknown".

For rewrites, pass `patch_id` (`git patch-id --stable`) and `tree_sha` (the
commit tree) when recording. A rebase keeps the patch id; a squash of a branch
tip produces that tip's tree. `GET /api/commits/lookup` accepts `patch_id` and
`tree_sha` for the SHA being looked up and returns `known`, `matched_by`
(`exact`, `patch_id` or `tree_sha`) and the original attribution. With no
record and no match it returns `known: false` and an explicit "no attribution
recorded for this commit" message.

`GET /api/issues/{key}/commits` returns the issue's recorded commits plus the
commits of issues linked by `closes`/`closed-by`, so a synced PR card shows the
same list as the issue it closes. The issue drawer renders a Commits section and
recording also appends a `commit` entry to the activity timeline. Only recorded
commits appear, so an unrecorded commit is explicitly unknown rather than
attributed to a nearby agent.

MCP tool: `record_commit`.

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
| `links` (`closes`/`closed-by`) | a synced PR's body: closing keywords (Forgejo/GitHub convention) followed by either a Forgejo issue number ("Closes #4") or breadboard's own key ("Fixes ECO-4", agents/humans naturally reference the key they can see) | inbound-derived; re-parsed on every sync of that PR, not just import, so a target that arrives after the PR is still linked and its closing cascade applied. The two links and the cascaded status move are written in one transaction |
| comments | issue/PR comments | both (see **Comment echo suppression** below) |
| PRs | title / state / labels / assignee / milestone | inbound; writeback limited to **labels** (PRs are read-mostly). A closed PR (merged or closed-unmerged) appears as `done`. A PR's closing references become `closes`/`closed-by` links, and a PR with one is hidden from the board/backlog; its state surfaces as a badge on the issue's own card instead. When a merged PR flips to `done`, its closing targets are cascaded to `done` (or `review` if labelled `verify-before-done`). That label override applies when the close is first observed: once a coordinator verifies the merge and moves the card to `done`, later PR syncs do not drag it back to `review`; a closed-unmerged PR only maintains links and never changes a target's status, so an abandoned PR cannot undo work completed by a different merged PR (only a merged PR's closing transition, or an explicit reopen of the target, changes it). Forgejo only reliably auto-closes the *first* issue in a comma-separated "Closes #a, #b" list, so breadboard doesn't rely on that |
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
`$XDG_CONFIG_HOME/breadboard/cli.toml` (`server`, `token`, and an optional
`agent` name).

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
| `release <key> [--agent NAME] (--note TEXT | --no-note)` | release your claim (status stays, tint goes away). `--note` is required unless `--no-note` applies to a claim that never held work |
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

## Android app

`apps/android/` is a thin WebView shell for the live site: persistent Forgejo
session, file picker for chat attachments, downloads with the session cookie,
OAuth redirects kept in the WebView, external links in the browser, back
through web history, and an offline screen with retry. No business logic lives
there; the SPA is the app. Build with JDK 17 and Android SDK 36:

```sh
cd apps/android
./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

See `apps/android/README.md` for wireless debugging and limitations.

## MCP server

`apps/mcp/`: a stdio MCP server (`@modelcontextprotocol/sdk`) that wraps
`@breadboard/api-client`, so an MCP client (Claude, …) can read and drive the
board. Auth from `~/.config/breadboard/cli.toml` or `BREADBOARD_SERVER` /
`BREADBOARD_TOKEN`. The agent name is `BREADBOARD_AGENT`, then `agent` in
`cli.toml`, else a random playful name per process; the chosen name is written
to `~/.config/breadboard/agent.json` at startup so local tooling (the OpenCode
chat-inject plugin) can follow it. Tools: `whoami`, `list_issues` / `get_issue` / `board`
(filters: project, status, cycle, epic, assignee_id, mine),
`create_issue` / `update_issue` / `move_issue` / `delete_issue` /
`move_issue_project` / `comment`, `activity`,
`list_{projects,epics,cycles,repos}`, `get_epic` / `create_epic` /
`update_epic` / `delete_epic` (delete keeps and unlinks child issues),
`create_project` / `update_project` / `create_cycle` / `start_cycle` /
`end_cycle`,
`add_repo` / `remove_repo`, `reconcile`, `import_markdown`,
`name_agent` / `claim_issue` / `update_claim` (heartbeat) / `release_claim` /
`resume_claims` / `set_agent_message` / `chat_rooms` / `chat_read` / `chat_send` /
`chat_create_room` / `chat_upload` / `chat_wait` / `chat_push` / `chat_pushes` /
`list_agents` / `agent_status` / `suggest_agent_names`,
`inbox` / `mark_notification_read` / `mark_all_notifications_read` /
`get_notification_prefs` / `set_notification_prefs`. See `apps/mcp/README.md`.

`record_commit` records which agent, session, orchestrator and model produced a
commit. Call it right after committing so a bug traces from issue to commit to
model; see the commit attribution section above.

Agent rule (also in the `claim_issue` / `update_issue` / `move_issue` tool
descriptions): **name first, then claim, keep the heartbeat alive, and release
when done**. Call `name_agent` before claiming so the profile, identity, and
chat mapping exist before the first claim.
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
