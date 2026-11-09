#!/usr/bin/env node
// MCP server for breadboard. Exposes the board over stdio so Claude (and any
// other MCP client) can read and drive it.
//
// Config, in order of precedence:
//   BREADBOARD_SERVER / BREADBOARD_TOKEN env vars
//   ~/.config/breadboard/cli.toml   (server, token; written by `breadboard auth login`)

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { homedir, hostname } from "node:os";
import { join } from "node:path";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import {
  ApiError,
  ARTIFACT_KINDS,
  BreadboardClient,
  STATUSES,
  PRIORITIES,
  type ArtifactKind,
  type HandoffNote,
  type IssueFilter,
  type IssueLink,
  type IssuePatch,
} from "@breadboard/api-client";
import { resolveCommitMetadata } from "./git.js";

const LINK_KINDS = ["blocks", "blocked-by", "relates-to"] as const;

// --- config ---------------------------------------------------------------

function loadConfig(): { server: string; token?: string; agent?: string } {
  let server = process.env.BREADBOARD_SERVER ?? "";
  let token = process.env.BREADBOARD_TOKEN;
  let agent: string | undefined;
  try {
    const raw = readFileSync(
      join(process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config"), "breadboard", "cli.toml"),
      "utf8",
    );
    if (!server) server = raw.match(/^\s*server\s*=\s*"([^"]+)"/m)?.[1] ?? "";
    if (!token) token = raw.match(/^\s*token\s*=\s*"([^"]+)"/m)?.[1];
    agent = raw.match(/^\s*agent\s*=\s*"([^"]+)"/m)?.[1]?.trim() || undefined;
  } catch {
    /* no config file */
  }
  if (!server) server = "https://breadboard.breadway.dev";
  return { server, token, agent: process.env.BREADBOARD_AGENT?.trim() || agent };
}

const cfg = loadConfig();
const client = new BreadboardClient(cfg.server, cfg.token);

// --- claim keepalive ------------------------------------------------------
//
// The Agents page marks a claim stale after 5 minutes without a heartbeat, so
// a long agent run must keep poking. Doing that from the MCP server means the
// agent does not have to remember: claim_issue starts a 60s timer, and
// release_claim (or the process exiting) stops it.

/**
 * The chat-inject plugin runs inside OpenCode and cannot see this process's
 * environment, so the name we speak as is mirrored to a small state file. Set
 * `agent = "..."` in cli.toml (or BREADBOARD_AGENT) to make the identity stable
 * instead of the name the server assigns on first registration.
 */
const AGENT_STATE = join(
  process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config"),
  "breadboard",
  "agent.json",
);

function rememberAgent(name: string) {
  try {
    mkdirSync(dirname(AGENT_STATE), { recursive: true });
    writeFileSync(
      AGENT_STATE,
      `${JSON.stringify({ name, pid: process.pid, at: new Date().toISOString() })}\n`,
    );
  } catch {
    /* best effort only */
  }
}

/**
 * One name per MCP process, so an agent's claims group together. The server
 * owns the curated list: registering without a name assigns one and returns
 * alternatives, which keeps the client and server in step.
 */
let autoAgentName: string | undefined = cfg.agent?.trim().toLowerCase() || undefined;
/** True once the server has accepted this process's current name. A name from
 *  config alone is not safe to rename from: the server may have refused it. */
let nameConfirmed = false;
let nameAlternatives: string[] = [];
let registeredAt = 0;
let registration: Promise<string> | null = null;
const REGISTER_REFRESH_MS = 60_000;

const SESSION_ID =
  process.env.OPENCODE_SESSION_ID?.trim() ||
  process.env.BREADBOARD_SESSION_ID?.trim() ||
  undefined;
const HOST = hostname().trim();

/** Turn a name collision into a message that tells the agent what to do next.
 *  The server names the holding session and returns free alternatives; the
 *  plugin only learns a name from successful identity calls, so a refusal must
 *  not be recorded as this process's name. */
function explainRegistrationError(e: unknown): never {
  if (e instanceof ApiError && e.status === 409) {
    const details = e.details;
    const alternatives = details?.alternatives ?? nameAlternatives;
    const suffix = alternatives.length ? ` Try: ${alternatives.join(", ")}` : "";
    throw new Error(`${e.message}${suffix}`);
  }
  throw e;
}

/** Register this process and refresh `last_seen`, throttled on the MCP side. */
async function ensureRegistered(force = false): Promise<string> {
  const now = Date.now();
  if (!force && autoAgentName && now - registeredAt < REGISTER_REFRESH_MS) {
    return autoAgentName;
  }
  if (registration) return registration;
  registration = (async () => {
    const res = await client.registerAgent({
      agent_name: autoAgentName,
      session_id: SESSION_ID,
      host: HOST || undefined,
      pid: process.pid,
      note: "opencode mcp",
    });
    autoAgentName = res.agent_name;
    nameConfirmed = true;
    nameAlternatives = res.alternatives ?? [];
    registeredAt = Date.now();
    rememberAgent(autoAgentName);
    ensureChatPoller(autoAgentName);
    return autoAgentName;
  })();
  try {
    return await registration;
  } finally {
    registration = null;
  }
}

function defaultAgentName(): string {
  if (!autoAgentName) {
    throw new Error("agent is not registered yet; call name_agent or retry");
  }
  return autoAgentName;
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

async function attempt<T>(fn: () => Promise<T>) {
  try {
    return ok(await fn());
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      return fail("not authenticated: run `breadboard auth login` (writes ~/.config/breadboard/cli.toml)");
    }
    return fail(e instanceof Error ? e.message : String(e));
  }
}

async function run<T>(fn: () => Promise<T>) {
  // Every tool call refreshes this session on the roster, so a brand new
  // OpenCode session appears without naming itself first. Throttled above.
  // name_agent uses attempt() instead: a process holding a refused name must
  // still be able to rename itself before registering.
  return attempt(async () => {
    await ensureRegistered();
    return fn();
  });
}

// Register at startup, before the first tool call, so the chat plugin can map
// this session from the start. A failed startup registration is retried by
// the first tool call.
void ensureRegistered().catch((e) => {
  console.error(`breadboard registration failed: ${e instanceof Error ? e.message : String(e)}`);
});

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

// --- chat push -------------------------------------------------------------
//
// DMs and @pings always push; group rooms push when the agent subscribed with
// chat_push. The server keeps the inbox; this loop turns new rows into MCP
// logging notifications so the client sees them without polling tools.

let chatPoller: ReturnType<typeof setInterval> | undefined;
let pollerAgent: string | undefined;
let lastPushId = 0;

function stopChatPoller() {
  if (chatPoller) clearInterval(chatPoller);
  chatPoller = undefined;
  pollerAgent = undefined;
}

/// The breadboard-chat plugin already injects pushes into the session. Running
/// this poller too wakes the same agent twice per push, which showed up as one
/// agent posting several replies to a single @everyone. Skip it when the plugin
/// is installed, unless the operator forces it on with BREADBOARD_CHAT_POLLER=1.
function chatPollerEnabled(): boolean {
  const mode = (process.env.BREADBOARD_CHAT_POLLER ?? "").trim().toLowerCase()
  if (["0", "off", "false", "no"].includes(mode)) return false
  if (["1", "on", "true", "yes"].includes(mode)) return true
  const configDir = process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config")
  return !existsSync(join(configDir, "opencode", "plugins", "breadboard-chat"))
}

function ensureChatPoller(explicit?: string) {
  if (!chatPollerEnabled()) {
    stopChatPoller()
    return
  }
  const name = explicit?.trim() || autoAgentName;
  if (!name) return;
  if (chatPoller && pollerAgent === name) return;
  if (chatPoller) clearInterval(chatPoller);
  pollerAgent = name;
  chatPoller = setInterval(() => {
    void (async () => {
      try {
        const pushes = await client.listChatPushes(name, lastPushId, 20);
        for (const push of pushes) {
          lastPushId = Math.max(lastPushId, push.id);
          const where = push.room_name ? `#${push.room_name}` : "DM";
          const text = `[chat ${push.kind}] ${push.message.author_display} in ${where}: ${push.message.body || "(attachment)"}`;
          console.error(text);
          try {
            await (server as unknown as {
              server?: {
                sendLoggingMessage?: (msg: {
                  level: "info";
                  data: string;
                  logger: string;
                }) => Promise<void>;
              };
            }).server?.sendLoggingMessage?.({
              level: "info",
              data: text,
              logger: "breadboard-chat",
            });
          } catch {
            /* client may not support logging notifications */
          }
        }
      } catch {
        /* keep polling */
      }
    })();
  }, 5_000);
  chatPoller.unref?.();
}

const stopEverything = () => {
  stopAllKeepalives();
  stopChatPoller();
};
process.removeListener("SIGINT", stopAllKeepalives);
process.removeListener("SIGTERM", stopAllKeepalives);
process.once("SIGINT", stopEverything);
process.once("SIGTERM", stopEverything);

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
    limit: z.number().int().positive().max(2000).optional().describe("page size when you only want one page"),
    offset: z.number().int().min(0).optional().describe("rows to skip, with limit"),
  },
  ({ project, status, cycle, epic, assignee_id, mine, limit, offset }) =>
    run(async () => {
      const filter: IssueFilter = { project, mine, assignee_id, limit, offset };
      if (status) filter.status = status as IssueFilter["status"];
      if (cycle) filter.cycle_id = await resolveCycleId(cycle);
      if (epic) filter.epic_id = await resolveEpicId(epic);
      // With no explicit limit, return the whole set by paging, so a caller
      // that enumerates the board is not silently cut off at one page.
      return limit === undefined ? client.listAllIssues(filter) : client.listIssues(filter);
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
      const issues = await client.listAllIssues(filter);
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

server.tool(
  "list_artifacts",
  "List typed evidence attached to an issue (benches, screenshots, CI logs, diffs, traces). " +
    "Each row shows the uploader agent and session id plus its expiry.",
  { key: z.string().describe("issue key, e.g. BREAD-42") },
  ({ key }) => run(() => client.listIssueArtifacts(key)),
);

server.tool(
  "attach_artifact",
  "Attach immutable evidence to an issue. Pass base64 data (raw or a data: URL). " +
    "Kinds: bench, screenshot, ci-log, diff, trace, other. A screenshot must be png, jpeg, gif " +
    "or webp; svg and other types are refused. Limits: 10 MB for a screenshot, bench or other; " +
    "25 MB for a diff; 50 MB for a CI log or trace. Optionally tie it to a comment_id or a " +
    "review_ref (review key or URL). The row records the agent name and session id that " +
    "produced it.",
  {
    key: z.string().describe("issue key, e.g. BREAD-42"),
    kind: z.enum(ARTIFACT_KINDS as [string, ...string[]]).describe("artifact kind"),
    filename: z.string().describe("display filename, also used to infer the mime type"),
    data_base64: z.string().describe("base64 file contents (raw or a data: URL)"),
    mime: z.string().optional().describe("e.g. image/png or text/plain"),
    comment_id: z.number().int().optional().describe("attach to this comment on the issue"),
    review_ref: z
      .string()
      .optional()
      .describe("review request key or URL this evidence belongs to"),
    agent_name: z.string().optional().describe("agent name; defaults to this MCP session"),
  },
  ({ key, kind, filename, data_base64, mime, comment_id, review_ref, agent_name }) =>
    run(() =>
      client.uploadIssueArtifactBase64(key, {
        kind: kind as ArtifactKind,
        filename,
        data_base64,
        mime,
        comment_id,
        review_ref,
        agent_name: chatAgent(agent_name),
        session_id: SESSION_ID,
      }),
    ),
);

server.tool(
  "record_commit",
  "Record which agent, session and model produced a commit. Call this right after committing a " +
    "change for an issue, so a bug can be traced from issue to commit to model. The server resolves " +
    "your agent and session from this MCP process and resolves the model from usage data for this " +
    "exact session; it never guesses from whichever agent was last active. If this session used " +
    "more than one model, pass model explicitly. Pass workdir (the local repo path) to fill commit " +
    "tree_sha and patch_id from git, which lets a later rebase or squash match this record. " +
    "When workdir is given it must be a git work tree and the commit must resolve there; otherwise " +
    "the tool returns an explicit error and records nothing. " +
    "If no model can be established it is stored as unknown, not invented.",
  {
    repo: z.string().describe("owner/name, e.g. Breadway/breadboard"),
    sha: z.string().describe("full commit sha (a short sha is expanded when workdir is given)"),
    issue: z.string().describe("issue key, e.g. BB-163"),
    note: z.string().optional().describe("optional one-line context"),
    model: z
      .string()
      .optional()
      .describe("explicit model id; use it when this session switched models"),
    provider: z.string().optional().describe("explicit provider id, e.g. opencode-go"),
    patch_id: z.string().optional().describe("git patch-id --stable, for rebase matching"),
    tree_sha: z.string().optional().describe("commit tree sha, for squash matching"),
    workdir: z
      .string()
      .optional()
      .describe("local repository path; fills sha, tree_sha and patch_id from git when omitted"),
  },
  ({ repo, sha, issue, note, model, provider, patch_id, tree_sha, workdir }) =>
    run(async () => {
      let resolvedSha = sha.trim();
      let resolvedTree = tree_sha;
      let resolvedPatch = patch_id;
      if (workdir) {
        const metadata = resolveCommitMetadata(workdir, resolvedSha);
        resolvedSha = metadata.sha;
        if (!resolvedTree) resolvedTree = metadata.tree_sha;
        if (!resolvedPatch) resolvedPatch = metadata.patch_id;
      }
      return client.recordCommit({
        repo,
        sha: resolvedSha,
        issue,
        note,
        model,
        provider,
        patch_id: resolvedPatch,
        tree_sha: resolvedTree,
        agent_name: defaultAgentName(),
        session_id: SESSION_ID,
      });
    }),
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
        client.listAllIssues({ epic_id: id }),
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
    "NAME FIRST: call name_agent before claiming anything; agent names are free-form and should be " +
    "fun and memorable (Toasty Pretzel, Crumb Baron, Saucy Baguette). Reuse the same name across " +
    "issues so the Agents tab groups your work; after naming, omit agent_name here. " +
    "mode=exclusive (default) is real work: first claim wins (409 means another agent has it: do not " +
    "fight the lock, check agent_status and pick other work), and it moves backlog/todo to doing and " +
    "assigns the card to you. Exclusive claims are budgeted (10 per agent name, 100 per orchestrator " +
    "tree); a refusal names the limit and the claim to release, and intents are never capped. " +
    "mode=intent is a soft reservation for work you plan to take later: it " +
    "never moves the card and never blocks anyone. " +
    "The MCP server auto-heartbeats your claim every 60s while this process is alive, so it stays live; " +
    "the result includes the issue's latest handoff note when one exists, so a resuming session can pick " +
    "up context. call release_claim with a handoff note when done and update_claim to report state/PR progress.",
  {
    key: z.string().describe("issue key, e.g. BREAD-42"),
    agent_name: z
      .string()
      .optional()
      .describe("fun, memorable agent name (e.g. Toasty Pretzel); reuse it across issues. Omit for a generated one"),
    mode: z.enum(["exclusive", "intent"]).optional().describe("exclusive (default) or soft intent"),
    message: z
      .string()
      .optional()
      .describe("one-line message shown under your name in the Agents view"),
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
        ensureChatPoller(created.agent_name);
        const issue = await client.getIssue(key).catch(() => null);
        return { ...created, last_handoff: issue?.last_handoff ?? null };
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
            ensureChatPoller(resumed.agent_name);
            const issue = await client.getIssue(key).catch(() => null);
            return {
              ...resumed,
              resumed: true,
              last_handoff: issue?.last_handoff ?? null,
            };
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
    message: z.string().nullable().optional().describe("one-line message under your name; null clears it"),
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
    "REQUIRED: pass note, one line covering what is done, what is next and what is risky, so the next " +
    "session can pick the issue up. The server refuses a release without it (400), and the error names " +
    "the rule. The only exception is an explicit no_note=true for a claim that never held work: an intent " +
    "claim, or an exclusive claim still in planned state with no branch, worktree, repo, PR or head commit. " +
    "Pass agent_name when you hold more than one claim on the issue.",
  {
    key: z.string().describe("issue key, e.g. BREAD-42"),
    agent_name: z.string().optional().describe("which of your claims to release"),
    note: z
      .string()
      .optional()
      .describe(
        "REQUIRED handoff: done, next and risky in one line (500 chars max). Refused without it unless no_note is true and the claim never held work.",
      ),
    no_note: z
      .boolean()
      .optional()
      .describe(
        "Explicitly release a claim that never held work without a handoff. Only valid for an intent claim or a planned exclusive claim with no branch, worktree, repo, PR or commit; otherwise the server refuses.",
      ),
  },
  ({ key, agent_name, note, no_note }) =>
    run(async () => {
      const who = agent_name?.trim() || autoAgentName;
      await client.releaseClaim(key, who, note, no_note === true);
      stopKeepalive(key, who);
      return `released ${key}`;
    }),
);

server.tool(
  "set_agent_message",
  "Set or clear the one-line message shown under your name in the Agents view. " +
    "Use it for what you are about (or a fun motto); keep it to one short line. " +
    "Call name_agent first if you have not chosen a name for this process yet.",
  {
    message: z.string().describe("one line, max 120 chars; empty clears it"),
    agent_name: z.string().optional().describe("defaults to this process's generated name"),
  },
  ({ message, agent_name }) =>
    run(async () => {
      const name = agent_name?.trim() || autoAgentName;
      if (!name) throw new Error("no agent name yet: pass agent_name or call claim_issue first");
      ensureChatPoller(name);
      return client.setAgentMessage(name, message);
    }),
);

server.tool(
  "name_agent",
  "Name this MCP process before you claim any work. Sets the name later claim and chat calls use " +
    "(so you can omit agent_name), registers your profile in the Agents view, and starts chat " +
    "polling. Pick something fun and memorable, and reuse it across issues. Before claims exist a " +
    "rename is free; after claims exist the server moves them with the profile or refuses a target " +
    "already in use. This also lets the chat plugin map this session, so DMs and mentions reach you " +
    "before your first claim.",
  {
    agent_name: z.string().describe("fun, memorable name, e.g. Toasty Pretzel"),
    message: z
      .string()
      .optional()
      .describe("optional one-line message for the Agents view; defaults to 'Ready for work.'"),
  },
  ({ agent_name, message }) =>
    attempt(async () => {
      const name = agent_name.trim().toLowerCase();
      if (!name) throw new Error("agent_name is required");
      const line = message?.trim() || "Ready for work.";
      // A rename after claims exist moves them and the profile in one server
      // transaction, or is refused. Before any claims it is free.
      const previous = autoAgentName;
      const previousConfirmed = nameConfirmed;
      let renamed = false;
      try {
        // Only rename from a name the server has actually accepted. A name
        // that came from config but failed registration may belong to another
        // live session, and renaming it would move that session's claims.
        if (previousConfirmed && previous && previous !== name) {
          await client.renameAgent(previous, name);
          renamed = true;
        }
        autoAgentName = name;
        registeredAt = 0;
        await ensureRegistered(true);
      } catch (e) {
        // A refused registration must not strand this process (or the chat
        // plugin) on a name another live session owns. If the rename already
        // happened, the server moved this session to the new name, so keep it;
        // otherwise restore the identity that was still valid.
        if (!renamed) {
          autoAgentName = previous;
          nameConfirmed = previousConfirmed;
        }
        registeredAt = 0;
        explainRegistrationError(e);
      }
      ensureChatPoller(name);
      return client.setAgentMessage(name, line);
    }),
);

server.tool(
  "resume_claims",
  "Re-attach to work you already own after a restart. Heartbeats every claim owned by you under " +
    "this agent name and re-arms the keepalive for each, so they stop looking abandoned/stale. " +
    "Returns each claim's latest handoff note, which is the note the previous session left when it " +
    "released: read it as context before continuing. Call this when an MCP session starts and you " +
    "intend to continue previous work.",
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
      const resumed: Array<{ key: string; last_handoff: HandoffNote | null }> = [];
      const failed: string[] = [];
      for (const c of mine) {
        try {
          await client.heartbeatClaim(c.key, { agent_name: name }, name);
          startKeepalive(c.key, name);
          const issue = await client.getIssue(c.key).catch(() => null);
          resumed.push({ key: c.key, last_handoff: issue?.last_handoff ?? null });
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

/** The agent name to speak as; chat requires one. */
function chatAgent(explicit?: string): string {
  const name = explicit?.trim() || autoAgentName;
  if (!name) {
    throw new Error("no agent name yet: claim an issue or pass agent_name");
  }
  rememberAgent(name);
  return name;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Resolve a room name/id, or the DM with another agent, to a room id. */
async function resolveChatRoom(name: string, room?: string, peer?: string): Promise<number> {
  if (peer) return (await client.openDm(peer, name, SESSION_ID)).id;
  const rooms = await client.listChatRooms(name);
  const want = room?.trim() || "lobby";
  const found = rooms.find((r) => String(r.id) === want || r.name === want);
  if (!found) throw new Error(`no room ${want}`);
  return found.id;
}

server.tool(
  "chat_push",
  "Turn push notifications on/off for a shared room. DMs and @pings always push; group rooms " +
    "only push while subscribed. New pushes arrive as MCP notifications and in chat_pushes.",
  {
    room: z.string().describe("room name or id"),
    enabled: z.boolean(),
    agent_name: z.string().optional(),
  },
  ({ room, enabled, agent_name }) =>
    run(async () => {
      const name = chatAgent(agent_name);
      ensureChatPoller(name);
      await client.setChatPushSub(room, enabled, name);
      return `${enabled ? "push on" : "push off"} for ${room}`;
    }),
);

server.tool(
  "chat_pushes",
  "Drain your chat push inbox (DMs, @pings, and subscribed rooms). Pass ack:true to clear the " +
    "ones returned. New pushes also arrive as MCP notifications while this process is running.",
  {
    after: z.number().optional().describe("only pushes with id greater than this"),
    ack: z.boolean().optional(),
    agent_name: z.string().optional(),
  },
  ({ after, ack, agent_name }) =>
    run(async () => {
      const name = chatAgent(agent_name);
      ensureChatPoller(name);
      const pushes = await client.listChatPushes(name, after, 100);
      if (ack && pushes.length) {
        await client.ackChatPushes(pushes[pushes.length - 1].id, name);
      }
      return { pushes };
    }),
);

server.tool(
  "chat_rooms",
  "List chat rooms and DMs with an unread count per room. Shared rooms (including the global " +
    "lobby) are open to everyone; an agent only sees DMs it belongs to. Check this between tasks " +
    "to notice new chat.",
  { agent_name: z.string().optional().describe("defaults to this process's agent name") },
  ({ agent_name }) =>
    run(() => {
      const name = chatAgent(agent_name);
      ensureChatPoller(name);
      return client.listChatRooms(name);
    }),
);

server.tool(
  "chat_read",
  "Read recent messages from a room or DM and advance your read cursor. Use `room` for a room " +
    "name/id (default lobby) or `with` to read the DM with another agent.",
  {
    room: z.string().optional().describe("room name or id; defaults to lobby"),
    with: z.string().optional().describe("agent name to read the DM with"),
    after: z.number().optional().describe("only messages with id greater than this"),
    limit: z.number().min(1).max(500).optional(),
    agent_name: z.string().optional(),
  },
  ({ room, with: peer, after, limit, agent_name }) =>
    run(async () => {
      const name = chatAgent(agent_name);
      ensureChatPoller(name);
      const roomId = await resolveChatRoom(name, room, peer);
      const messages = await client.listChatMessages(roomId, { agent: name, after, limit });
      return { room_id: roomId, messages };
    }),
);

server.tool(
  "chat_wait",
  "Wait for the next chat message instead of polling. Blocks until a message arrives, or until " +
    "`timeout` seconds pass. Use timeout:'inf' to wait indefinitely, but run that inside a " +
    "background subagent (OpenCode's subagent tool with background=true) so this turn is not " +
    "blocked: the subagent ends when chat_wait returns, which notifies you. With `after`, resumes " +
    "from that message id; without it, waits for anything newer than the room's current latest.",
  {
    room: z.string().optional().describe("room name or id; defaults to lobby"),
    with: z.string().optional().describe("agent name to wait on a DM with"),
    after: z.number().optional().describe("last seen message id"),
    timeout: z
      .union([z.number().min(1).max(3600), z.literal("inf")])
      .optional()
      .describe("seconds to wait, or 'inf' for no deadline; default 25"),
    agent_name: z.string().optional(),
  },
  ({ room, with: peer, after, timeout, agent_name }, extra: { signal?: AbortSignal } | undefined) =>
    run(async () => {
      const name = chatAgent(agent_name);
      const roomId = await resolveChatRoom(name, room, peer);
      const infinite = timeout === "inf";
      const waitMs = infinite ? Infinity : (timeout ?? 25) * 1000;
      const deadline = infinite ? Infinity : Date.now() + waitMs;

      let seen = after;
      if (seen === undefined) {
        const rooms = await client.listChatRooms(name);
        seen = rooms.find((r) => r.id === roomId)?.last_message?.id ?? 0;
      }

      for (;;) {
        if (extra?.signal?.aborted) {
          return { room_id: roomId, messages: [], aborted: true };
        }
        const messages = await client.listChatMessages(roomId, {
          agent: name,
          after: seen,
          limit: 100,
        });
        if (messages.length) {
          return { room_id: roomId, messages, timed_out: false };
        }
        if (!infinite && Date.now() >= deadline) {
          return { room_id: roomId, messages: [], timed_out: true };
        }
        await sleep(1_000);
      }
    }),
);

server.tool(
  "chat_send",
  "Send a chat message as yourself. Use `to` for a DM with another agent, or `room` for a " +
    "shared room (default lobby). Create new rooms with chat_create_room.",
  {
    body: z.string().optional().describe("message text; may be empty when attaching files"),
    to: z.string().optional().describe("agent name to DM"),
    room: z.string().optional().describe("room name or id; defaults to lobby"),
    uploads: z.array(z.number()).optional().describe("upload ids from chat_upload"),
    agent_name: z.string().optional(),
  },
  ({ body, to, room, uploads, agent_name }) =>
    run(() => {
      if (!body?.trim() && !uploads?.length) {
        throw new Error("message needs text or at least one upload");
      }
      ensureChatPoller(agent_name?.trim());
      return client.postChatMessage({
        body: body ?? "",
        to,
        room,
        uploads,
        agent_name: chatAgent(agent_name),
        session_id: SESSION_ID,
      });
    }),
);

server.tool(
  "chat_upload",
  "Upload a file or image to attach to a chat message. Pass base64 data (raw or a data: URL), " +
    "then send the returned upload id in chat_send. 10 MB limit.",
  {
    filename: z.string(),
    mime: z.string().optional().describe("e.g. image/png; inferred from the filename if omitted"),
    data_base64: z.string().describe("base64 file contents"),
  },
  ({ filename, mime, data_base64 }) =>
    run(() => client.uploadChatBase64(filename, data_base64, mime)),
);

server.tool(
  "chat_create_room",
  "Create a shared chat room (the global lobby already exists). Silly rooms encouraged.",
  {
    name: z.string().describe("room slug, e.g. bread-puns"),
    description: z.string().optional().describe("what it is for, max 160 chars"),
  },
  ({ name, description }) => run(() => client.createChatRoom(name, description)),
);

server.tool(
  "suggest_agent_names",
  "Get the server's curated agent name list: this session's assigned name plus a few free " +
    "alternatives. Names are characters, not hosts: pass your pick to name_agent, then reuse it " +
    "on every issue so the Agents tab keeps you as one agent.",
  { count: z.number().min(1).max(20).optional().describe("how many names, default 4") },
  ({ count }) =>
    run(async () => {
      const names = [...new Set([autoAgentName, ...nameAlternatives].filter((n): n is string => Boolean(n)))];
      return names.slice(0, count ?? 4);
    }),
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
