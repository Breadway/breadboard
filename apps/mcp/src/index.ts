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
  type IssueLink,
  type IssuePatch,
} from "@breadboard/api-client";

const LINK_KINDS = ["blocks", "blocked-by", "relates-to"] as const;

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

async function resolveProjectId(keyOrId: string): Promise<number> {
  const n = Number(keyOrId);
  if (!Number.isNaN(n)) return n;
  const projects = await client.listProjects();
  const p = projects.find((p) => p.key.toLowerCase() === keyOrId.toLowerCase());
  if (!p) throw new Error(`no project with key ${keyOrId}`);
  return p.id;
}

async function resolveIssueId(keyOrId: string): Promise<number> {
  const n = Number(keyOrId);
  if (!Number.isNaN(n)) return n;
  const issue = await client.getIssue(keyOrId);
  return issue.id;
}

/**
 * IssuePatch.links replaces an issue's *entire* link set (it also holds
 * sync-managed closes/closed-by links), so adding or removing one manual
 * link means fetching the current set and writing the whole thing back,
 * not just the delta.
 */
async function mergeLinks(
  key: string,
  add?: { kind: (typeof LINK_KINDS)[number]; target: string }[],
  remove?: { kind: (typeof LINK_KINDS)[number]; target: string }[],
): Promise<IssueLink[]> {
  const current = await client.getIssue(key);
  let links = [...current.links];
  if (remove?.length) {
    const removeIds = await Promise.all(
      remove.map(async (r) => ({ kind: r.kind, target_id: await resolveIssueId(r.target) })),
    );
    links = links.filter(
      (l) => !removeIds.some((r) => r.kind === l.kind && r.target_id === l.target_id),
    );
  }
  if (add?.length) {
    const addLinks = await Promise.all(
      add.map(async (a) => ({ kind: a.kind as IssueLink["kind"], target_id: await resolveIssueId(a.target) })),
    );
    for (const l of addLinks) {
      if (!links.some((x) => x.kind === l.kind && x.target_id === l.target_id)) links.push(l);
    }
  }
  return links;
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
    epic: z.string().optional().describe("epic key or id"),
    assignee_id: z.number().optional().describe("filter by an arbitrary assignee's user id"),
    mine: z.boolean().optional().describe("only issues assigned to me"),
  },
  ({ project, status, cycle, epic, assignee_id, mine }) =>
    run(async () => {
      const filter: IssueFilter = { project, mine, assignee_id };
      if (status) filter.status = status as IssueFilter["status"];
      if (cycle) filter.cycle_id = await resolveCycleId(cycle);
      if (epic) filter.epic_id = await resolveEpicId(epic);
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
  {
    project: z.string().optional(),
    cycle: z.string().optional(),
    epic: z.string().optional().describe("epic key or id"),
    assignee_id: z.number().optional().describe("filter by an arbitrary assignee's user id"),
  },
  ({ project, cycle, epic, assignee_id }) =>
    run(async () => {
      const filter: IssueFilter = { project, assignee_id };
      if (cycle) filter.cycle_id = await resolveCycleId(cycle);
      if (epic) filter.epic_id = await resolveEpicId(epic);
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
    estimate: z.number().optional(),
    labels: z.array(z.string()).optional(),
    epic: z.string().optional().describe("epic key or id"),
    cycle: z.string().optional().describe("cycle name or id"),
  },
  ({ title, project, body, priority, estimate, labels, epic, cycle }) =>
    run(async () =>
      client.createIssue({
        title,
        project,
        body,
        priority: priority as never,
        estimate,
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
    estimate: z.number().nullable().optional().describe("story points, or null to clear"),
    labels: z.array(z.string()).optional().describe("replaces the whole label set"),
    epic: z.string().nullable().optional().describe("epic key/id, or null to clear"),
    cycle: z.string().nullable().optional().describe("cycle name/id, or null to clear"),
    assign_to_me: z.boolean().optional(),
    unassign: z.boolean().optional(),
    add_links: z
      .array(z.object({ kind: z.enum(LINK_KINDS), target: z.string().describe("target issue key or id") }))
      .optional()
      .describe("manual links to add (blocks/blocked-by/relates-to); synced closes/closed-by links are untouched"),
    remove_links: z
      .array(z.object({ kind: z.enum(LINK_KINDS), target: z.string().describe("target issue key or id") }))
      .optional()
      .describe("manual links to remove"),
  },
  ({ key, status, priority, title, body, estimate, labels, epic, cycle, assign_to_me, unassign, add_links, remove_links }) =>
    run(async () => {
      const patch: IssuePatch = {};
      if (status) patch.status = status as IssuePatch["status"];
      if (priority) patch.priority = priority as IssuePatch["priority"];
      if (title !== undefined) patch.title = title;
      if (body !== undefined) patch.body = body;
      if (estimate !== undefined) patch.estimate = estimate;
      if (labels !== undefined) patch.labels = labels;
      if (epic !== undefined) patch.epic_id = epic === null ? null : await resolveEpicId(epic);
      if (cycle !== undefined) patch.cycle_id = cycle === null ? null : await resolveCycleId(cycle);
      if (assign_to_me) patch.assignee_id = (await client.me()).id;
      if (unassign) patch.assignee_id = null;
      if (add_links || remove_links) {
        patch.links = await mergeLinks(key, add_links, remove_links);
      }
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
  "delete_issue",
  "Permanently delete an issue. There is no undo.",
  { key: z.string() },
  ({ key }) =>
    run(async () => {
      await client.deleteIssue(key);
      return `deleted ${key}`;
    }),
);

server.tool(
  "move_issue_project",
  "Move an issue to a different project; it is reassigned a new key/number in that project.",
  { key: z.string(), project: z.string().describe("target project key") },
  ({ key, project }) => run(() => client.moveIssueProject(key, project.toUpperCase())),
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
  "Create a project. Repo-less by default (for planning items); pass forgejo_repo to also map a "
    + "synced Forgejo repo at creation time (equivalent to add_repo right after).",
  {
    key: z.string(),
    name: z.string(),
    forgejo_repo: z.string().optional().describe("owner/name"),
  },
  ({ key, name, forgejo_repo }) =>
    run(() => client.createProject({ key: key.toUpperCase(), name, forgejo_repo })),
);

server.tool(
  "update_project",
  "Rename a project, archive/unarchive it, or attach/detach its Forgejo repo mapping. Omitted "
    + "fields are left unchanged; pass forgejo_repo: null to detach it.",
  {
    project: z.string().describe("project key or id"),
    name: z.string().optional(),
    archived: z.boolean().optional(),
    forgejo_repo: z.string().nullable().optional().describe("owner/name to attach, or null to detach"),
  },
  ({ project, name, archived, forgejo_repo }) =>
    run(async () => {
      const id = await resolveProjectId(project);
      const patch: { name?: string; archived?: boolean; forgejo_repo?: string | null } = {};
      if (name !== undefined) patch.name = name;
      if (archived !== undefined) patch.archived = archived;
      if (forgejo_repo !== undefined) patch.forgejo_repo = forgejo_repo;
      return client.updateProject(id, patch);
    }),
);

server.tool(
  "add_repo",
  "Map a Forgejo repo to a project and start syncing its issues.",
  { repo: z.string().describe("owner/name"), key: z.string(), name: z.string().optional() },
  ({ repo, key, name }) => run(() => client.addRepo(repo, key.toUpperCase(), name)),
);

server.tool(
  "remove_repo",
  "Stop syncing a Forgejo repo (disables it; the project and its already-synced issues stay).",
  { repo: z.string().describe("owner/name") },
  ({ repo }) =>
    run(async () => {
      await client.removeRepo(repo);
      return `disabled ${repo}`;
    }),
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
  ({ project, markdown, dry_run }) => run(() => client.importMarkdown({ project, markdown, dry_run })),
);

server.tool(
  "inbox",
  "List your notification inbox.",
  { unread_only: z.boolean().optional() },
  ({ unread_only }) => run(() => client.inbox(unread_only)),
);

server.tool(
  "mark_notification_read",
  "Mark one notification as read.",
  { id: z.number() },
  ({ id }) =>
    run(async () => {
      await client.markRead(id);
      return "marked read";
    }),
);

server.tool("mark_all_notifications_read", "Mark every notification as read.", {}, () =>
  run(async () => {
    await client.markAllRead();
    return "all marked read";
  }),
);

server.tool("get_notification_prefs", "Your notification preferences.", {}, () =>
  run(() => client.getNotificationPrefs()),
);

server.tool(
  "set_notification_prefs",
  "Set your notification preferences (full replace, all four fields required).",
  {
    assigned: z.boolean().describe("notify when an issue is assigned to me"),
    mention: z.boolean().describe("notify when I'm @mentioned in a comment or description"),
    cycle: z.boolean().describe("notify when a cycle I have issues in starts or ends"),
    digest: z.boolean().describe("send a daily digest"),
  },
  (prefs) => run(() => client.setNotificationPrefs(prefs)),
);

await server.connect(new StdioServerTransport());
console.error(`breadboard-mcp ready → ${cfg.server}${cfg.token ? "" : " (no token: read-only calls will 401)"}`);
