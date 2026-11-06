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
| `create_project` `update_project` `create_cycle` `start_cycle` `end_cycle` `add_repo` `remove_repo` `reconcile` | admin (cycles can be created with ISO dates and started/ended by name or id; update_project can rename, archive/unarchive, attach/detach forgejo_repo) |
| `claim_issue` `update_claim` `release_claim` | exclusive agent presence: claim before starting work (first wins, `409` means someone else is on it), heartbeat at least every 5 min while working, release when done |
| `list_agents` `agent_status` | live claims + the `bb-status`-style server roll-up (`BB x/y done, …`) |
| `import_markdown` | outline → epics + issues |
| `inbox` `mark_notification_read` `mark_all_notifications_read` `get_notification_prefs` `set_notification_prefs` | notifications |

`update_issue`'s `add_links`/`remove_links` take `{ kind, target }` pairs
(`kind` one of `blocks`/`blocked-by`/`relates-to`, `target` an issue key or
id); synced `closes`/`closed-by` links are never touched.

Mutating tools go through your MCP client's own confirmation flow.
