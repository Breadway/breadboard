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

// --- claim keepalive ------------------------------------------------------
//
// The Agents page marks a claim stale after 5 minutes without a heartbeat, so
// a long agent run must keep poking. Doing that from the MCP server means the
// agent does not have to remember: claim_issue starts a 60s timer, and
// release_claim (or the process exiting) stops it.

const FUN_ADJECTIVES = [
  "Toasty", "Crumbly", "Buttery", "Golden", "Flaky", "Zesty", "Plucky",
  "Snazzy", "Bouncy", "Peppy", "Crusty", "Doughy", "Spry", "Merry",
  "Brazen", "Nimble", "Frosted", "Saucy", "Hearty", "Sprightly",
];

const FUN_NOUNS = [
  "Pretzel", "Baguette", "Crumpet", "Brioche", "Scone", "Bagel",
  "Focaccia", "Pumpernickel", "Croissant", "Muffin", "Churro", "Bun",
  "Loaf", "Waffle", "Pita", "Ciabatta", "Biscuit", "Rye", "Pumper",
  "Roll",
];

/** One playful name per MCP process, so an agent's claims group together. */
let autoAgentName: string | undefined;

function pickFunName(): string {
  const a = FUN_ADJECTIVES[Math.floor(Math.random() * FUN_ADJECTIVES.length)];
  const n = FUN_NOUNS[Math.floor(Math.random() * FUN_NOUNS.length)];
  return `${a} ${n}`;
}

function defaultAgentName(): string {
  autoAgentName ??= pickFunName();
  return autoAgentName;
}

function suggestNames(count: number): string[] {
  const out = new Set<string>();
  while (out.size < count) out.add(pickFunName());
  return [...out];
}

/** Claims owned by the signed-in user under this agent name. */
async function ownClaims(agent: string) {
  const [me, claims] = await Promise.all([client.me(), client.listAgents()]);
  const name = agent.trim().toLowerCase();
  return claims.filter(
    (c) => c.claimed_by === me.id && c.agent_name.toLowerCase() === name,
  );
}

const KEEPALIVE_MS = 60_000;
const keepalives = new Map<string, ReturnType<typeof setInterval>>();

const keepaliveId = (key: string, agent: string) => `${key}:${agent}`;

function stopKeepalive(key: string, agent?: string) {
  if (agent) {
    const id = keepaliveId(key, agent);
    const timer = keepalives.get(id);
    if (timer) clearInterval(timer);
    keepalives.delete(id);
    return;
  }
  for (const [id, timer] of keepalives) {
    if (id.startsWith(`${key}:`)) {
      clearInterval(timer);
      keepalives.delete(id);
    }
  }
}

function startKeepalive(key: string, agent: string) {
  const id = keepaliveId(key, agent);
  if (keepalives.has(id)) return;
  const timer = setInterval(() => {
    void client.heartbeatClaim(key, { agent_name: agent }, agent).catch((e) => {
      // Only a missing claim stops the timer. A network blip or a transient
      // 5xx must not silently let the claim go stale.
      const gone = e instanceof ApiError && (e.status === 404 || e.status === 410);
      if (gone) {
        const t = keepalives.get(id);
        if (t) clearInterval(t);
        keepalives.delete(id);
      } else {
        console.error(`breadboard keepalive failed for ${key}/${agent}: ${e instanceof Error ? e.message : String(e)}`);
      }
    });
  }, KEEPALIVE_MS);
  timer.unref?.();
  keepalives.set(id, timer);
}

const stopAllKeepalives = () => {
  for (const timer of keepalives.values()) clearInterval(timer);
  keepalives.clear();
};
process.once("SIGINT", stopAllKeepalives);
process.once("SIGTERM", stopAllKeepalives);

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
  "Create an issue. Title is required, and so is the right project once there is more than one. " +
    "ROUTING RULES (important): file the issue where the work actually lives. " +
    "(1) A repo-backed project is for that repo's code and issues only; its cards sync to the repo's tracker, " +
    "so never put local machine config, dotfiles, environment breakage, or a misbehaving bread module there. " +
    "(2) Work about the machine, tooling, or several repos at once belongs in a native (repo-less) project; " +
    "create one with create_project if none fits. " +
    "(3) A brand-new project that is not on Forgejo yet should get its own native breadboard project now and be " +
    "tracked there; attach the repo later with update_project or add_repo. " +
    "(4) Do not use a catch-all/roadmap project (for example ROAD) as a dumping ground. " +
    "If no existing project clearly owns the work, stop and create the right one rather than guessing. " +
    "If you later open a PR that resolves this issue, reference its key with a closing keyword in the PR body " +
    "(e.g. \"Closes ECO-17\" or \"Closes #17\"); breadboard links the PR to the issue and collapses them into " +
    "one card instead of showing two unrelated ones.",
  {
    title: z.string(),
    project: z.string().optional().describe("project key, e.g. BREAD; required when more than one project exists"),
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
  "Patch an issue. Any omitted field is left unchanged. CLAIM FIRST: agents must " +
    "call claim_issue before starting work (first claim wins; 409 means someone else is on it: " +
    "check list_agents / agent_status and work something else). Heartbeat with update_claim at " +
    "least every 5 minutes while work is in progress or the Agents page marks the claim stale. " +
    "Release with release_claim when done. Workflow: move to doing when you actually start work (not just at the end); backlog/todo "
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
  "Shorthand: set an issue's status. CLAIM FIRST with claim_issue (and heartbeat via " +
    "update_claim); 409 means another agent holds it. Workflow: doing when work actually starts, review once "
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

server.tool(
  "get_epic",
  "Get an epic by key or id, including its progress and child issues.",
  { epic: z.string().describe("epic key or id") },
  ({ epic }) =>
    run(async () => {
      const id = await resolveEpicId(epic);
      if (id === undefined) throw new Error("epic key or id is required");
      const [details, issues] = await Promise.all([
        client.getEpic(id),
        client.listIssues({ epic_id: id }),
      ]);
      return { ...details, issues };
    }),
);

server.tool(
  "create_epic",
  "Create an epic, optionally scoped to a project.",
  {
    key: z.string().describe("epic key"),
    title: z.string(),
    description: z.string().optional(),
    project: z.string().optional().describe("project key or id"),
  },
  ({ key, title, description, project }) =>
    run(async () =>
      client.createEpic({
        key: key.toUpperCase(),
        title,
        description,
        project_id: project === undefined ? undefined : await resolveProjectId(project),
      }),
    ),
);

server.tool(
  "update_epic",
  "Update an epic by key or id. Omitted fields are left unchanged; description may be null to clear it.",
  {
    epic: z.string().describe("epic key or id"),
    title: z.string().optional(),
    description: z.string().nullable().optional(),
    archived: z.boolean().optional(),
  },
  ({ epic, title, description, archived }) =>
    run(async () => {
      const id = await resolveEpicId(epic);
      if (id === undefined) throw new Error("epic key or id is required");
      return client.updateEpic(id, { title, description, archived });
    }),
);

server.tool(
  "delete_epic",
  "Delete an epic by key or id. Child issues are kept and unlinked from the epic.",
  { epic: z.string().describe("epic key or id") },
  ({ epic }) =>
    run(async () => {
      const id = await resolveEpicId(epic);
      if (id === undefined) throw new Error("epic key or id is required");
      await client.deleteEpic(id);
      return `deleted epic ${epic}; child issues were kept and unlinked`;
    }),
);

server.tool(
  "create_cycle",
  "Create a cycle with inclusive ISO dates.",
  {
    name: z.string(),
    starts_on: z.string().describe("cycle start date in YYYY-MM-DD form"),
    ends_on: z.string().describe("cycle end date in YYYY-MM-DD form"),
    project_ids: z.array(z.number().int()).optional().describe("projects whose new issues default to this cycle"),
  },
  ({ name, starts_on, ends_on, project_ids }) => run(() => client.createCycle({ name, starts_on, ends_on, project_ids })),
);

server.tool("update_cycle_projects", "Replace the projects selected for a cycle's new-issue defaults. Existing issues are not moved.", {
  cycle: z.string().describe("cycle name or id"),
  project_ids: z.array(z.number().int()),
}, ({ cycle, project_ids }) => run(async () => {
  const id = await resolveCycleId(cycle);
  if (id === undefined) throw new Error("cycle name or id is required");
  return client.updateCycle(id, { project_ids });
}));

server.tool(
  "start_cycle",
  "Start a cycle by name or id.",
  { cycle: z.string().describe("cycle name or id") },
  ({ cycle }) =>
    run(async () => {
      const id = await resolveCycleId(cycle);
      if (id === undefined) throw new Error("cycle name or id is required");
      return client.startCycle(id);
    }),
);

server.tool(
  "end_cycle",
  "End a cycle by name or id.",
  { cycle: z.string().describe("cycle name or id") },
  ({ cycle }) =>
    run(async () => {
      const id = await resolveCycleId(cycle);
      if (id === undefined) throw new Error("cycle name or id is required");
      return client.endCycle(id);
    }),
);

server.tool("list_repos", "Forgejo repos being synced.", {}, () => run(() => client.listRepos()));

server.tool(
  "create_project",
  "Create a project. Repo-less by default, which is the right choice for a new project that does not have a " +
    "Forgejo repo yet, and for work about the machine/tooling rather than one repo's code. " +
    "Pass forgejo_repo only when this project tracks an actual repo (equivalent to add_repo right after). " +
    "Prefer creating a focused project over filing unrelated work into another project's tracker.",
  {
    key: z.string(),
    name: z.string(),
    forgejo_repo: z.string().optional().describe("owner/name, only for a project that tracks that repo"),
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

const AGENT_STATES = [
  "planned",
  "implementing",
  "awaiting-review",
  "reworking",
  "awaiting-ci",
  "awaiting-merge",
  "verifying",
  "blocked",
  "needs-user",
  "done",
] as const;

server.tool(
  "claim_issue",
  "Claim an issue as an agent. REQUIRED FIRST STEP before any implementation work: " +
    "no code changes without holding the claim. " +
    "NAME YOURSELF: agent names are free-form and should be fun and memorable " +
    "(Toasty Pretzel, Crumb Baron, Saucy Baguette). Reuse the same name across issues so the " +
    "Agents tab groups your work; omit agent_name to get one playful name for this MCP process. " +
    "mode=exclusive (default) is real work: first claim wins (409 means another agent has it: do not " +
    "fight the lock, check agent_status and pick other work), and it moves backlog/todo to doing and " +
    "assigns the card to you. mode=intent is a soft reservation for work you plan to take later: it " +
    "never moves the card and never blocks anyone. " +
    "The MCP server auto-heartbeats your claim every 60s while this process is alive, so it stays live; " +
    "call release_claim when done and update_claim to report state/PR progress.",
  {
    key: z.string().describe("issue key, e.g. BREAD-42"),
    agent_name: z
      .string()
      .optional()
      .describe("fun, memorable agent name (e.g. Toasty Pretzel); reuse it across issues. Omit for a generated one"),
    mode: z.enum(["exclusive", "intent"]).optional().describe("exclusive (default) or soft intent"),
    session_id: z.string().optional().describe("agent session id for ownership checks"),
    state: z.enum(AGENT_STATES).optional().describe("lifecycle state, default implementing"),
    note: z.string().optional().describe("what you are doing right now"),
    branch: z.string().optional(),
    worktree: z.string().optional(),
    repo: z.string().optional(),
    pr_url: z.string().optional(),
    pr_number: z.number().optional(),
    head_sha: z.string().optional(),
  },
  ({ key, agent_name, ...claim }) =>
    run(async () => {
      const name = agent_name?.trim() || defaultAgentName();
      const mode = (claim.mode ?? "exclusive") as "exclusive" | "intent";
      try {
        const created = await client.claimIssue(key, { ...claim, agent_name: name } as never);
        startKeepalive(key, created.agent_name);
        return created;
      } catch (e) {
        // Re-claiming your own claim after a restart is not a conflict: find
        // it, heartbeat it, and re-arm the keepalive. Only resume a claim of
        // the same mode; an intent blocked by someone else's exclusive claim
        // must still report the conflict.
        if (e instanceof ApiError && e.status === 409) {
          const mine = (await ownClaims(name)).find(
            (c) => c.key.toLowerCase() === key.toLowerCase() && c.mode === mode,
          );
          if (mine) {
            const resumed = await client.heartbeatClaim(key, { agent_name: name }, name);
            startKeepalive(key, resumed.agent_name);
            return { ...resumed, resumed: true };
          }
        }
        throw e;
      }
    }),
);

server.tool(
  "update_claim",
  "Heartbeat / update your claim (refreshes the live signal on the Agents page). Only the claiming user may update.",
  {
    key: z.string().describe("issue key, e.g. BREAD-42"),
    agent_name: z.string().optional(),
    state: z.enum(AGENT_STATES).optional(),
    session_id: z.string().nullable().optional(),
    note: z.string().nullable().optional(),
    branch: z.string().nullable().optional(),
    worktree: z.string().nullable().optional(),
    repo: z.string().nullable().optional(),
    pr_url: z.string().nullable().optional(),
    pr_number: z.number().nullable().optional(),
    head_sha: z.string().nullable().optional(),
  },
  ({ key, ...update }) => run(() => client.heartbeatClaim(key, update as never)),
);

server.tool(
  "release_claim",
  "Release your claim (exclusive or intent). The card keeps its status; only the agent tint goes away. " +
    "Pass agent_name when you hold more than one claim on the issue.",
  {
    key: z.string().describe("issue key, e.g. BREAD-42"),
    agent_name: z.string().optional().describe("which of your claims to release"),
  },
  ({ key, agent_name }) =>
    run(async () => {
      const who = agent_name?.trim() || autoAgentName;
      await client.releaseClaim(key, who);
      stopKeepalive(key, who);
      return `released ${key}`;
    }),
);

server.tool(
  "resume_claims",
  "Re-attach to work you already own after a restart. Heartbeats every claim owned by you under " +
    "this agent name and re-arms the keepalive for each, so they stop looking abandoned/stale. " +
    "Call this when an MCP session starts and you intend to continue previous work.",
  {
    agent_name: z
      .string()
      .optional()
      .describe("agent name whose claims to resume; defaults to this process's generated name"),
  },
  ({ agent_name }) =>
    run(async () => {
      const name = agent_name?.trim() || autoAgentName;
      if (!name) {
        throw new Error("no agent name yet: pass agent_name or call claim_issue first");
      }
      const mine = await ownClaims(name);
      if (mine.length === 0) {
        return `no claims found for ${name}`;
      }
      const resumed: string[] = [];
      const failed: string[] = [];
      for (const c of mine) {
        try {
          await client.heartbeatClaim(c.key, { agent_name: name }, name);
          startKeepalive(c.key, name);
          resumed.push(c.key);
        } catch {
          failed.push(c.key);
        }
      }
      return {
        agent_name: name,
        resumed,
        ...(failed.length ? { failed } : {}),
      };
    }),
);

server.tool(
  "suggest_agent_names",
  "Get a handful of fun, memorable agent names to choose from. Names are characters, not hosts: " +
    "pick one and reuse it on every issue so the Agents tab keeps you as one agent.",
  { count: z.number().min(1).max(20).optional().describe("how many names, default 8") },
  ({ count }) => ok(suggestNames(count ?? 8)),
);

server.tool("list_agents", "Live agent claims (exclusive presence per issue).", {}, () =>
  run(() => client.listAgents()),
);

server.tool(
  "agent_status",
  "Server-side roll-up mirroring the opencode bb-status widget line, computed from live claims.",
  {},
  () => run(() => client.agentStatus()),
);

server.tool(
  "import_markdown",
  "Import a markdown outline: '##' headings become epics, '- [ ]' / '- [x]' items become issues.",
  {
    project: z.string(),
    markdown: z.string(),
    dry_run: z.boolean().optional(),
    repair: z.boolean().optional().describe("Retry existing unlinked local cards; use dry_run first to list remote keys."),
  },
  ({ project, markdown, dry_run, repair }) => run(() => client.importMarkdown({ project, markdown, dry_run, repair })),
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
