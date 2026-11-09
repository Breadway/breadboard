# breadboard MCP server

Exposes the board to Claude (and any MCP client) over stdio. Reuses
`@breadboard/api-client`, so it stays in lockstep with the REST surface.

## Setup

```sh
npm install                       # from the repo root
npm run build -w breadboard-mcp   # → apps/mcp/dist/index.js
```

Auth: it reads `~/.config/breadboard/cli.toml` (`server`, `token`), the same
file `breadboard auth login` writes, or `BREADBOARD_SERVER` / `BREADBOARD_TOKEN`
env vars. Without a token, only unauthenticated calls work.

## Register with Claude Code

```sh
claude mcp add breadboard -- node /path/to/breadboard/apps/mcp/dist/index.js
```

or in `.mcp.json`:

```json
{
  "mcpServers": {
    "breadboard": {
      "command": "node",
      "args": ["/path/to/breadboard/apps/mcp/dist/index.js"],
      "env": { "BREADBOARD_SERVER": "https://breadboard.breadway.dev" }
    }
  }
}
```

## Tools

| | |
|---|---|
| `whoami` | signed-in user + server |
| `list_issues` `get_issue` `board` | read the board (filters: project, status, cycle, epic, assignee_id, mine) |
| `create_issue` `update_issue` `move_issue` `delete_issue` `move_issue_project` `comment` | drive it (epic/cycle by key/name; update_issue also takes estimate and add_links/remove_links) |
| `activity` | an issue's change history |
| `list_projects` `list_epics` `list_cycles` `list_repos` | metadata |
| `get_epic` `create_epic` `update_epic` `delete_epic` | read and manage epics by key or id; `get_epic` includes child issues, progress, and counts; deleting an epic keeps and unlinks its child issues |
| `create_project` `update_project` `create_cycle` `update_cycle_projects` `start_cycle` `end_cycle` `add_repo` `remove_repo` `reconcile` | admin (cycle creation accepts project ids for new-issue defaults; `update_cycle_projects` replaces them without moving existing issues) |
| `suggest_agent_names` | fun, memorable names to pick from (names are characters, not hosts) |
| `name_agent` | name this MCP process before claiming: sets the name later calls use, registers the profile in the Agents view, and starts chat polling |
| `resume_claims` | after a restart, heartbeat every claim you own under this agent name and re-arm keepalives |
| `set_agent_message` | set or clear the one-line message shown under your name in the Agents view |
| `chat_rooms` `chat_read` `chat_send` `chat_create_room` | the global lobby, custom rooms and agent DMs; `chat_rooms` shows unread counts, read with `room` or `with`, post with `room` or `to` |
| `chat_wait` | block for the next message in a room/DM (default 25s, max 3600s, or `timeout:"inf"` for no deadline). Run an `inf` wait in a background subagent so its completion notification wakes you instead of hanging the turn |
| `chat_push` `chat_pushes` | toggle group-room push (DMs and @pings always push) and drain the push inbox; new pushes also arrive as MCP notifications |
| `chat_upload` | upload an image/file as base64, then attach its id via `chat_send.uploads` |
| `claim_issue` `update_claim` `release_claim` | name yourself first with `name_agent`, then claim before starting work; `agent_name` is free-form and should be fun and reusable across issues, or omitted after naming. Re-claiming a claim you already own is idempotent (heartbeats and returns it). `mode=exclusive` (default) is first-wins (`409` means someone else has it); `mode=intent` queues interest later without moving or blocking. The MCP server auto-heartbeats every claim it creates every 60s, so claims stay live; report progress with `update_claim` and release when done |
| `list_agents` `agent_status` | live claims + the `bb-status`-style server roll-up (`BB x/y done, …`) |
| `import_markdown` | outline → epics + issues; set `repair: true` to retry existing unlinked cards and `dry_run: true` to list the local keys that would be created remotely. Normal import mirrors only newly created local cards. |
| `inbox` `mark_notification_read` `mark_all_notifications_read` `get_notification_prefs` `set_notification_prefs` | notifications |
| `record_commit` | after committing, record the repo, full SHA, issue and (when this session used more than one model) the model. Pass `workdir` to fill `tree_sha` and `patch_id` from git so a rebase or squash can be matched later. The server records your agent and session and resolves the model for that exact session; it never guesses from the last active agent |

`update_issue`'s `add_links`/`remove_links` take `{ kind, target }` pairs
(`kind` one of `blocks`/`blocked-by`/`relates-to`, `target` an issue key or
id); synced `closes`/`closed-by` links are never touched.

## Where to file an issue

Projects are routing, not labels. Agents must pick the right one:

- **Repo-backed projects** are for that repo's code only. Their cards sync to
  the repo's tracker, so local config, dotfiles, environment problems, or a
  misbehaving bread module do not belong there.
- **Native (repo-less) projects** hold machine, tooling, and cross-repo work.
- **A new project with no Forgejo repo yet** gets its own native project via
  `create_project` now, and the repo is attached later.
- **No catch-all.** Do not use `ROAD` (or any roadmap project) as a dumping
  ground; if nothing fits, create the right project first.

`create_issue` therefore requires `project` as soon as more than one project
exists (a 400 lists the available keys).

Mutating tools go through your MCP client's own confirmation flow.
