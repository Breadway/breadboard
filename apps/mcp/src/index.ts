#!/usr/bin/env node
// MCP server for breadboard. Exposes the board over stdio so Claude (and any
// other MCP client) can read and drive it.
//
// Config, in order of precedence:
//   BREADBOARD_SERVER / BREADBOARD_TOKEN env vars
//   ~/.config/breadboard/cli.toml   (server, token; written by `breadboard auth login`)

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import {
  ApiError,
  BreadboardClient,
  STATUSES,
  PRIORITIES,
  type IssueFilter,
  type IssuePatch,
} from "@breadboard/api-client";

// --- config ---------------------------------------------------------------

function loadConfig(): { server: string; token?: string } {
  let server = process.env.BREADBOARD_SERVER ?? "";
  let token = process.env.BREADBOARD_TOKEN;
  try {
    const raw = readFileSync(
      join(process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config"), "breadboard", "cli.toml"),
      "utf8",
    );
    if (!server) server = raw.match(/^\s*server\s*=\s*"([^"]+)"/m)?.[1] ?? "";
    if (!token) token = raw.match(/^\s*token\s*=\s*"([^"]+)"/m)?.[1];
  } catch {
    /* no config file */
  }
  if (!server) server = "https://breadboard.breadway.dev";
  return { server, token };
}

const cfg = loadConfig();
const client = new BreadboardClient(cfg.server, cfg.token);

// --- helpers ------------------------------------------------------------

const ok = (data: unknown) => ({
  content: [{ type: "text" as const, text: typeof data === "string" ? data : JSON.stringify(data, null, 2) }],
});
const fail = (msg: string) => ({ content: [{ type: "text" as const, text: msg }], isError: true });

async function run<T>(fn: () => Promise<T>) {
  try {
    return ok(await fn());
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      return fail("not authenticated: run `breadboard auth login` (writes ~/.config/breadboard/cli.toml)");
    }
    return fail(e instanceof Error ? e.message : String(e));
  }
}

async function resolveEpicId(keyOrId?: string): Promise<number | undefined> {
  if (!keyOrId) return undefined;
  const n = Number(keyOrId);
  if (!Number.isNaN(n)) return n;
  const epics = await client.listEpics();
  const e = epics.find((e) => e.key.toLowerCase() === keyOrId.toLowerCase());
  if (!e) throw new Error(`no epic with key ${keyOrId}`);
  return e.id;
}

async function resolveCycleId(nameOrId?: string): Promise<number | undefined> {
  if (!nameOrId) return undefined;
  const n = Number(nameOrId);
  if (!Number.isNaN(n)) return n;
  const cycles = await client.listCycles();
  const c = cycles.find((c) => c.name.toLowerCase() === nameOrId.toLowerCase());
  if (!c) throw new Error(`no cycle named ${nameOrId}`);
  return c.id;
}

// --- server -----------------------------------------------------------

const server = new McpServer({ name: "breadboard", version: "0.1.0" });

server.tool("whoami", "The signed-in user and the server URL.", {}, () =>
  run(async () => ({ server: cfg.server, user: await client.me() })),
);

server.tool(
  "list_issues",
  "List issues, optionally filtered.",
  {
    project: z.string().optional().describe("project key, e.g. BREAD"),
    status: z.enum(STATUSES as [string, ...string[]]).optional(),
    cycle: z.string().optional().describe("cycle name or id"),
    mine: z.boolean().optional().describe("only issues assigned to me"),
  },
  ({ project, status, cycle, mine }) =>
    run(async () => {
      const filter: IssueFilter = { project, mine };
      if (status) filter.status = status as IssueFilter["status"];
      if (cycle) filter.cycle_id = await resolveCycleId(cycle);
      return client.listIssues(filter);
    }),
);

server.tool(
  "get_issue",
  "One issue with its comments.",
  { key: z.string().describe("issue key, e.g. BREAD-42") },
  ({ key }) => run(() => client.getIssue(key)),
);

server.tool(
  "board",
  "The board grouped by status (counts + titles).",
  { project: z.string().optional(), cycle: z.string().optional() },
  ({ project, cycle }) =>
    run(async () => {
      const filter: IssueFilter = { project };
      if (cycle) filter.cycle_id = await resolveCycleId(cycle);
      const issues = await client.listIssues(filter);
      return STATUSES.map((s) => ({
        status: s,
        count: issues.filter((i) => i.status === s).length,
        issues: issues.filter((i) => i.status === s).map((i) => `${i.key}  ${i.title}`),
      }));
    }),
);

server.tool(
  "create_issue",
  "Create an issue. Only title is required. If the project maps to a synced repo it is also created on Forgejo. " +
    "If you later open a PR that resolves this issue, reference its key with a closing keyword in the PR body " +
    "(e.g. \"Closes ECO-17\" or \"Closes #17\"); breadboard links the PR to the issue and collapses them into " +
    "one card instead of showing two unrelated ones.",
  {
    title: z.string(),
    project: z.string().optional().describe("project key; defaults to the first project"),
    body: z.string().optional(),
    priority: z.enum(PRIORITIES as [string, ...string[]]).optional(),
    labels: z.array(z.string()).optional(),
    epic: z.string().optional().describe("epic key or id"),
    cycle: z.string().optional().describe("cycle name or id"),
  },
  ({ title, project, body, priority, labels, epic, cycle }) =>
    run(async () =>
      client.createIssue({
        title,
        project,
        body,
        priority: priority as never,
        labels,
        epic_id: await resolveEpicId(epic),
        cycle_id: await resolveCycleId(cycle),
      }),
    ),
);

server.tool(
  "update_issue",
  "Patch an issue. Any omitted field is left unchanged. " +
    "Workflow: move to doing when you actually start work (not just at the end); backlog/todo "
    + "→ doing → review (implemented, not yet verified/deployed) → done (verified live). "
    + "Don't jump straight from backlog/todo to review or done.",
  {
    key: z.string(),
    status: z.enum(STATUSES as [string, ...string[]]).optional(),
    priority: z.enum(PRIORITIES as [string, ...string[]]).optional(),
    title: z.string().optional(),
    body: z.string().optional(),
    labels: z.array(z.string()).optional().describe("replaces the whole label set"),
    epic: z.string().nullable().optional().describe("epic key/id, or null to clear"),
    cycle: z.string().nullable().optional().describe("cycle name/id, or null to clear"),
    assign_to_me: z.boolean().optional(),
    unassign: z.boolean().optional(),
  },
  ({ key, status, priority, title, body, labels, epic, cycle, assign_to_me, unassign }) =>
    run(async () => {
      const patch: IssuePatch = {};
      if (status) patch.status = status as IssuePatch["status"];
      if (priority) patch.priority = priority as IssuePatch["priority"];
      if (title !== undefined) patch.title = title;
      if (body !== undefined) patch.body = body;
      if (labels !== undefined) patch.labels = labels;
      if (epic !== undefined) patch.epic_id = epic === null ? null : await resolveEpicId(epic);
      if (cycle !== undefined) patch.cycle_id = cycle === null ? null : await resolveCycleId(cycle);
      if (assign_to_me) patch.assignee_id = (await client.me()).id;
      if (unassign) patch.assignee_id = null;
      return client.patchIssue(key, patch);
    }),
);

server.tool(
  "move_issue",
  "Shorthand: set an issue's status. Workflow: doing when work actually starts, review once "
    + "implemented (not yet verified/deployed), done only after you've verified it. Don't skip "
    + "doing on the way to review/done.",
  { key: z.string(), status: z.enum(STATUSES as [string, ...string[]]) },
  ({ key, status }) => run(() => client.patchIssue(key, { status: status as IssuePatch["status"] })),
);

server.tool(
  "comment",
  "Add a comment to an issue (pushed to Forgejo if the issue is linked).",
  { key: z.string(), body: z.string() },
  ({ key, body }) => run(() => client.addComment(key, body)),
);

server.tool("activity", "An issue's change history.", { key: z.string() }, ({ key }) =>
  run(() => client.getActivity(key)),
);

server.tool("list_projects", "All projects.", {}, () => run(() => client.listProjects()));
server.tool("list_epics", "All epics with done/total progress.", {}, () => run(() => client.listEpics()));
server.tool("list_cycles", "All cycles.", {}, () => run(() => client.listCycles()));
server.tool("list_repos", "Forgejo repos being synced.", {}, () => run(() => client.listRepos()));

server.tool(
  "create_project",
  "Create a repo-less project (for planning items).",
  { key: z.string(), name: z.string() },
  ({ key, name }) => run(() => client.createProject({ key: key.toUpperCase(), name })),
);

server.tool(
  "add_repo",
  "Map a Forgejo repo to a project and start syncing its issues.",
  { repo: z.string().describe("owner/name"), key: z.string(), name: z.string().optional() },
  ({ repo, key, name }) => run(() => client.addRepo(repo, key.toUpperCase(), name)),
);

server.tool("reconcile", "Run a Forgejo reconcile pass now.", {}, () =>
  run(async () => {
    await client.reconcile();
    return "reconcile queued";
  }),
);

server.tool(
  "import_markdown",
  "Import a markdown outline: '##' headings become epics, '- [ ]' / '- [x]' items become issues.",
  {
    project: z.string(),
    markdown: z.string(),
    dry_run: z.boolean().optional(),
  },
  ({ project, markdown, dry_run }) =>
    run(async () => {
      const res = await fetch(`${cfg.server.replace(/\/$/, "")}/api/import`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          ...(cfg.token ? { authorization: `Bearer ${cfg.token}` } : {}),
        },
        body: JSON.stringify({ project, markdown, dry_run }),
      });
      if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
      return res.json();
    }),
);

await server.connect(new StdioServerTransport());
console.error(`breadboard-mcp ready → ${cfg.server}${cfg.token ? "" : " (no token: read-only calls will 401)"}`);
