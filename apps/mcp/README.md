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
| `list_issues` `get_issue` `board` | read the board (filters: project, status, cycle, mine) |
| `create_issue` `update_issue` `move_issue` `comment` | drive it (epic/cycle by key/name) |
| `activity` | an issue's change history |
| `list_projects` `list_epics` `list_cycles` `list_repos` | metadata |
| `create_project` `add_repo` `reconcile` | admin |
| `import_markdown` | outline → epics + issues |

Mutating tools go through your MCP client's own confirmation flow.
