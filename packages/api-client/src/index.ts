// TypeScript client for breadboard-server (REST + SSE).
//
// Mirrors the routes in Documentation.md. All `/api/*` calls send credentials
// (session cookie) and throw `ApiError` on non-2xx.

export type Status = "backlog" | "todo" | "doing" | "review" | "done";
export type Priority = "none" | "low" | "medium" | "high" | "urgent";
export type IssueType = "issue" | "pr";
export type LinkKind = "blocks" | "blocked-by" | "relates-to" | "closes" | "closed-by";

export const STATUSES: Status[] = ["backlog", "todo", "doing", "review", "done"];
export const PRIORITIES: Priority[] = ["none", "low", "medium", "high", "urgent"];

export interface Health {
  status: string;
  version: string;
  db: boolean;
  dev_login?: boolean;
}

export interface User {
  id: number;
  forgejo_id: number;
  login: string;
  display_name: string;
  avatar_url: string | null;
}

export interface Project {
  id: number;
  key: string;
  name: string;
  forgejo_repo: string | null;
  next_number: number;
  archived: boolean;
}

export interface Epic {
  id: number;
  key: string;
  title: string;
  description: string | null;
  project_id: number | null;
  archived: boolean;
  created_at: string;
  updated_at: string;
}

export interface Cycle {
  id: number;
  name: string;
  starts_on: string;
  ends_on: string;
  closed: boolean;
  project_ids: number[];
}

export interface NewCycle {
  name: string; starts_on: string; ends_on: string; project_ids?: number[];
}

export interface ForgejoRef {
  repo: string;
  number: number;
}

export interface IssueLink {
  kind: LinkKind;
  target_id: number;
}

export interface Issue {
  id: number;
  key: string;
  project_id: number;
  title: string;
  body: string;
  status: Status;
  priority: Priority;
  type: IssueType;
  estimate: number | null;
  assignee_id: number | null;
  reporter_id: number | null;
  epic_id: number | null;
  cycle_id: number | null;
  labels: string[];
  links: IssueLink[];
  forgejo_ref: ForgejoRef | null;
  milestone: string | null;
  created_at: string;
  updated_at: string;
}

export interface Comment {
  id: number;
  issue_id: number;
  author_id: number | null;
  body: string;
  external_id: number | null;
  created_at: string;
  updated_at: string;
}

export type IssueDetail = Issue & { comments: Comment[] };

export type EpicView = Epic & {
  progress: Record<string, number>;
  total: number;
};

export interface ActivityEntry {
  field: string;
  old_value: string | null;
  new_value: string | null;
  source: string;
  actor: string | null;
  at: string;
}

export type ReviewState = "requested" | "accepted" | "changes_requested" | "approved";
export type ReviewerKind = "human" | "agent";

/** One reviewer on one issue/PR pair. Humans and agents share the record. */
export interface ReviewRequest {
  id: number;
  issue_id: number;
  issue_key: string;
  issue_title: string;
  issue_status: Status;
  pr_url: string | null;
  pr_number: number | null;
  pr_repo: string | null;
  requested_by: number | null;
  requested_by_login: string | null;
  reviewer_id: number | null;
  reviewer: string;
  reviewer_kind: ReviewerKind;
  state: ReviewState;
  note: string | null;
  requested_at: string;
  updated_at: string;
  responded_at: string | null;
  escalated_at: string | null;
}

export interface NewReviewRequest {
  reviewer: string;
  reviewer_kind?: ReviewerKind;
  pr_url?: string;
  pr_number?: number;
  pr_repo?: string;
  note?: string;
}

export interface ReviewRequestPatch {
  state?: ReviewState;
  note?: string;
  agent_name?: string;
}

export type NotificationKind =
  | "assigned"
  | "mention"
  | "cycle_started"
  | "cycle_ended"
  | "review"
  | "digest";

export interface Notification {
  id: number;
  user_id: number;
  kind: NotificationKind;
  issue_id: number | null;
  body: string;
  read: boolean;
  created_at: string;
}

export interface Inbox {
  unread: number;
  notifications: Notification[];
}

export interface NotificationPrefs {
  assigned: boolean;
  mention: boolean;
  cycle: boolean;
  digest: boolean;
}

export interface RepoView {
  repo: string;
  project_key: string | null;
  enabled: boolean;
  last_synced_at: string | null;
}

export interface ImportRequest {
  /** Project key to file everything under. */
  project: string;
  markdown: string;
  dry_run?: boolean;
  repair?: boolean;
}

export interface ImportResult {
  epics_created: string[];
  issues_created: string[];
  skipped: string[];
  remote_would_create: string[];
  pending_writebacks: { key: string; error: string }[];
}

export interface NewIssue {
  title: string;
  body?: string;
  project?: string;
  status?: Status;
  priority?: Priority;
  estimate?: number | null;
  assignee_id?: number | null;
  epic_id?: number | null;
  cycle_id?: number | null;
  labels?: string[];
}

export interface IssuePatch {
  title?: string;
  body?: string;
  status?: Status;
  priority?: Priority;
  estimate?: number | null;
  assignee_id?: number | null;
  epic_id?: number | null;
  cycle_id?: number | null;
  labels?: string[];
  links?: IssueLink[];
}

export interface IssueFilter {
  project?: string;
  status?: Status;
  cycle_id?: number;
  epic_id?: number;
  assignee_id?: number;
  mine?: boolean;
  /** Page size. The server defaults to 500 and caps at 2000. */
  limit?: number;
  offset?: number;
}

export type AgentState =
  | "planned"
  | "implementing"
  | "awaiting-review"
  | "reworking"
  | "awaiting-ci"
  | "awaiting-merge"
  | "verifying"
  | "blocked"
  | "needs-user"
  | "done";

export type ClaimMode = "exclusive" | "intent";

export const AGENT_STATES: AgentState[] = [
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
];

/** Only absolute http(s) URLs may become links (blocks javascript: etc). */
export function safeHttpUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  const u = url.trim();
  return /^https?:\/\/[^\s]+$/i.test(u) ? u : null;
}

export interface AgentClaim {
  id: number;
  issue_id: number;
  key: string;
  issue_title: string;
  issue_status: Status;
  claimed_by: number;
  claimed_by_login: string;
  agent_name: string;
  message: string | null;
  mode: ClaimMode;
  session_id: string | null;
  state: AgentState;
  note: string | null;
  branch: string | null;
  worktree: string | null;
  repo: string | null;
  pr_url: string | null;
  pr_number: number | null;
  head_sha: string | null;
  board_status: Status;
  claimed_at: string;
  updated_at: string;
  last_heartbeat: string;
}

export interface ClaimUpsert {
  agent_name: string;
  /** One-line message shown under the agent name (max 120 chars). */
  message?: string;
  /** exclusive (default): real work, first-wins. intent: soft reservation. */
  mode?: ClaimMode;
  session_id?: string;
  state?: AgentState;
  note?: string;
  branch?: string;
  worktree?: string;
  repo?: string;
  pr_url?: string;
  pr_number?: number;
  head_sha?: string;
}

export interface ClaimUpdate {
  agent_name?: string;
  state?: AgentState;
  message?: string | null;
  session_id?: string | null;
  note?: string | null;
  branch?: string | null;
  worktree?: string | null;
  repo?: string | null;
  pr_url?: string | null;
  pr_number?: number | null;
  head_sha?: string | null;
}

export type ChatRoomKind = "room" | "dm";
export type ChatAuthorKind = "agent" | "human";

export interface ChatUpload {
  id: number;
  filename: string;
  mime: string;
  size: number;
}

export type ArtifactKind = "bench" | "screenshot" | "ci-log" | "diff" | "trace" | "other";

export const ARTIFACT_KINDS: ArtifactKind[] = [
  "bench",
  "screenshot",
  "ci-log",
  "diff",
  "trace",
  "other",
];

/** Immutable evidence attached to an issue. */
export interface IssueArtifact {
  id: number;
  issue_id: number;
  kind: ArtifactKind;
  /** Optional comment this evidence belongs to. */
  comment_id: number | null;
  /** Optional review request key or URL. */
  review_ref: string | null;
  filename: string;
  mime: string;
  size: number;
  /** Agent name, or `human:<login>` for a web upload. */
  uploader_agent: string;
  session_id: string | null;
  created_at: string;
  expires_at: string;
}

export interface NewIssueArtifact {
  filename: string;
  kind: ArtifactKind;
  data_base64: string;
  mime?: string;
  comment_id?: number | null;
  review_ref?: string | null;
  agent_name?: string;
  session_id?: string;
}

export type ChatMessageKind = "message" | "summary";

export interface ChatMention {
  name: string;
  slug: string;
  kind: "agent" | "human" | "room";
}

export interface ChatMessage {
  id: number;
  room_id: number;
  author_member: string;
  author_display: string;
  author_kind: ChatAuthorKind;
  /** Missing on older servers, where every message is a normal one. */
  message_kind?: ChatMessageKind;
  body: string;
  attachments: ChatUpload[];
  created_at: string;
}

export type ChatPushKind = "dm" | "ping" | "room";

export interface ChatPush {
  id: number;
  kind: ChatPushKind;
  room_id: number;
  room_name: string | null;
  message: ChatMessage;
}

export interface ChatRoom {
  id: number;
  kind: ChatRoomKind;
  name: string | null;
  description: string | null;
  members: string[];
  /** Messages newer than your read cursor, excluding your own. */
  unread: number;
  last_message: ChatMessage | null;
}

export interface AgentStatusSummary {
  total: number;
  live: number;
  stale: number;
  queued: number;
  done: number;
  active: number;
  review: number;
  verifying: number;
  blocked: number;
  remaining: number;
  line: string;
  claims: AgentClaim[];
}

/** Token and cost roll-up imported from OpenCode's session store. */
export interface UsageTotals {
  cost: number;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  total_tokens: number;
  session_count: number;
}

export interface UsageModelShare {
  model: string;
  provider?: string;
  cost: number;
  total_tokens: number;
  /** Fraction of cost, or of tokens when no cost is recorded. */
  share: number;
}

export interface UsageAgentShare {
  agent_name: string;
  cost: number;
  total_tokens: number;
  share: number;
}

export interface UsageSession {
  session_id: string;
  agent_name?: string;
  cost: number;
  total_tokens: number;
  first_day: string;
  last_day: string;
  models: string[];
}

export interface UsageSummary extends UsageTotals {
  subject: string;
  kind: "agent" | "issue";
  range: string;
  from?: string;
  to: string;
  models: UsageModelShare[];
  sessions: UsageSession[];
  agents?: UsageAgentShare[];
  spend_warn_usd: number;
  over_threshold: boolean;
}

export interface UsageSettings {
  /** Global spend warning threshold in USD. 0 disables warnings. */
  spend_warn_usd: number;
}

export interface UsageImportReport {
  path: string;
  sessions: number;
  rows: number;
  skipped_sessions: number;
}

export type UsageRange = "24h" | "7d" | "30d" | "90d" | "all";

/** How the model on a commit was established. */
export type CommitModelSource = "reported" | "session" | "unknown";

/** How a commit lookup found a record for a rewritten SHA. */
export type CommitMatch = "exact" | "patch_id" | "tree_sha";

export interface CommitAttribution {
  id: number;
  repo: string;
  sha: string;
  issue_id: number | null;
  issue_key: string | null;
  agent_name: string;
  session_id: string | null;
  orchestrator: string | null;
  orchestrator_session_id: string | null;
  /** Raw model id, without a provider prefix. Null means unknown. */
  model: string | null;
  provider: string | null;
  model_source: CommitModelSource;
  patch_id: string | null;
  tree_sha: string | null;
  note: string | null;
  created_at: string;
}

export interface CommitLookup {
  repo: string;
  sha: string;
  known: boolean;
  matched_by?: CommitMatch;
  attribution?: CommitAttribution;
  message?: string;
}

/** `POST /api/commits`. `issue` is a key or numeric id. */
export interface NewCommitAttribution {
  repo: string;
  sha: string;
  issue: string;
  agent_name?: string;
  session_id?: string;
  /** Explicit model id; omit to resolve from this exact session or stay unknown. */
  model?: string;
  provider?: string;
  /** `git patch-id --stable`, for matching a rebased commit. */
  patch_id?: string;
  /** Tree SHA, for matching a squash of this commit's branch tip. */
  tree_sha?: string;
  note?: string;
}

/** `POST /api/agents/register`. Omit `agent_name` to ask the server for a
 *  curated name. */
export interface AgentRegister {
  agent_name?: string;
  session_id?: string;
  host?: string;
  pid?: number;
  note?: string;
  /** OpenCode parent session id; the chat plugin reports this. */
  parent_session_id?: string;
  /** Optional orchestrator name; resolved to its session when known. */
  orchestrator?: string;
}

export interface AgentRegistration {
  agent_name: string;
  /** Two or three other free curated names. */
  alternatives: string[];
  live: boolean;
  paused: boolean;
  session_id?: string | null;
  parent_session_id?: string | null;
  orchestrator?: string | null;
  orchestrator_session_id?: string | null;
}

/** One roster row: the union of sessions, profiles and claims by lowercased
 *  name. A roster entry may have zero claims. */
export interface AgentRosterEntry {
  agent_name: string;
  session_id?: string | null;
  host?: string | null;
  pid?: number | null;
  note?: string | null;
  first_seen?: string | null;
  last_seen?: string | null;
  live: boolean;
  paused: boolean;
  status: "live" | "stale" | "paused";
  message?: string | null;
  state?: AgentState | null;
  claim_count: number;
  exclusive_count: number;
  intent_count: number;
  user_id?: number | null;
  login?: string | null;
  paused_by?: number | null;
  paused_by_login?: string | null;
  paused_at?: string | null;
  /** Hidden from the default Agents view. Claims and chat identity remain. */
  hidden: boolean;
  hidden_by?: number | null;
  hidden_by_login?: string | null;
  hidden_at?: string | null;
  /** The session that spawned this agent, as reported by the chat plugin. */
  parent_session_id?: string | null;
  /** Resolved orchestrator name, when the parent session is registered. */
  orchestrator?: string | null;
  orchestrator_session_id?: string | null;
}

/** One typed action in an agent's activity timeline. */
export type AgentEventKind =
  | "registered"
  | "claimed"
  | "released"
  | "state_change"
  | "paused"
  | "removed"
  | "pr_opened"
  | "pr_merged"
  | "ci_result"
  | "chat_sent"
  | "handoff";

export const AGENT_EVENT_KINDS: AgentEventKind[] = [
  "registered",
  "claimed",
  "released",
  "state_change",
  "paused",
  "removed",
  "pr_opened",
  "pr_merged",
  "ci_result",
  "chat_sent",
  "handoff",
];

/** `GET /api/agents/{name}/events` row. Links are optional: a state change
 *  points at an issue, a chat event at a room/message, a PR event at its URL. */
export interface AgentEvent {
  id: number;
  agent_name: string;
  kind: AgentEventKind;
  issue_id?: number | null;
  issue_key?: string | null;
  pr_url?: string | null;
  pr_number?: number | null;
  room_id?: number | null;
  message_id?: number | null;
  summary: string;
  detail?: string | null;
  actor?: string | null;
  at: string;
}

/** Newest-first cursor page; pass `next_before` as `before` for the next. */
export interface AgentEventPage {
  events: AgentEvent[];
  has_more: boolean;
  next_before?: number | null;
}

/** Kill response: the name is removed, so there is no roster entry. */
export interface AgentRemoval {
  agent_name: string;
  removed: boolean;
  claims_released: number;
  profile_removed: boolean;
}

// Mirrors breadboard_core::event::BoardEvent (serde tag = "type").
export type BoardEvent =
  | { type: "card.created"; id: number; key: string }
  | { type: "card.updated"; id: number; key: string }
  | { type: "card.moved"; id: number; key: string; from: Status; to: Status }
  | { type: "card.assigned"; id: number; key: string; assignee_id: number | null }
  | { type: "card.deleted"; id: number; key: string }
  | { type: "cycle.started"; id: number; name: string; issues: number }
  | { type: "cycle.ended"; id: number; name: string }
  | {
      type: "sync.drift";
      issue_id: number;
      key: string;
      field: string;
      breadboard_value: string;
      forgejo_value: string;
      winner: "breadboard" | "forgejo";
      at: string;
    }
  | { type: "notified"; user_id: number }
  | { type: "agent.claimed"; id: number; key: string; agent: string; mode: string; session_id: string | null; state: string }
  | { type: "agent.updated"; id: number; key: string; agent: string; mode: string; session_id: string | null; state: string; claim: AgentClaim }
  | { type: "agent.released"; id: number; key: string }
  | { type: "agent.removed"; agent: string; removed_by: number | null; claims_released: number }
  | { type: "agent.paused"; agent: string; paused_by: number | null }
  | { type: "agent.resumed"; agent: string; resumed_by: number | null }
  | { type: "agent.hidden"; agent: string; hidden_by: number | null }
  | { type: "agent.unhidden"; agent: string; unhidden_by: number | null }
  | { type: "chat.message"; room_id: number; id: number }
  | { type: "chat.room.deleted"; id: number }
  | {
      type: "review.updated";
      id: number;
      issue_id: number;
      key: string;
      reviewer: string;
      state: string;
      escalated: boolean;
    };

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export class BreadboardClient {
  readonly baseUrl: string;
  #token?: string;

  /** In a browser, `token` is omitted and the session cookie is used. Outside
   *  a browser (MCP server, scripts), pass a bearer token. */
  constructor(baseUrl = "", token?: string) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.#token = token;
  }

  private async req<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = {};
    if (body !== undefined) headers["content-type"] = "application/json";
    if (this.#token) headers.authorization = `Bearer ${this.#token}`;
    const res = await fetch(`${this.baseUrl}${path}`, {
      method,
      credentials: "include",
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (!res.ok) {
      let msg = res.statusText;
      try {
        const j = (await res.json()) as { error?: string };
        if (j?.error) msg = j.error;
      } catch {
        /* keep statusText */
      }
      throw new ApiError(res.status, msg);
    }
    if (res.status === 204) return undefined as T;
    const text = await res.text();
    if (!text || text.trim() === "") {
      return undefined as T;
    }
    try {
      return JSON.parse(text) as T;
    } catch {
      // tolerate empty or non-JSON body for void endpoints (e.g. older reconcile)
      return undefined as T;
    }
  }

  health = () => this.req<Health>("GET", "/healthz");
  me = () => this.req<User>("GET", "/api/me");

  listProjects = () => this.req<Project[]>("GET", "/api/projects");
  getProject = (id: number) => this.req<Project>("GET", `/api/projects/${id}`);
  createProject = (p: { key: string; name: string; forgejo_repo?: string }) =>
    this.req<Project>("POST", "/api/projects", p);
  updateProject = (
    id: number,
    patch: { name?: string; archived?: boolean; forgejo_repo?: string | null },
  ) => this.req<Project>("PATCH", `/api/projects/${id}`, patch);

  listRepos = () => this.req<RepoView[]>("GET", "/api/repos");
  addRepo = (repo: string, key: string, name?: string) =>
    this.req<RepoView>("POST", "/api/repos", { repo, key, name });
  removeRepo = (repo: string) => this.req<void>("DELETE", `/api/repos/${repo}`);
  reconcile = () => this.req<void>("POST", "/api/sync/reconcile");
  importMarkdown = (req: ImportRequest) => this.req<ImportResult>("POST", "/api/import", req);

  inbox = (unread = false) =>
    this.req<Inbox>("GET", `/api/notifications${unread ? "?unread=true" : ""}`);
  markRead = (id: number) => this.req<void>("POST", `/api/notifications/${id}/read`);
  markAllRead = () => this.req<void>("POST", "/api/notifications/read-all");
  getNotificationPrefs = () => this.req<NotificationPrefs>("GET", "/api/notifications/prefs");
  setNotificationPrefs = (p: NotificationPrefs) =>
    this.req<NotificationPrefs>("PUT", "/api/notifications/prefs", p);

  listEpics = () => this.req<EpicView[]>("GET", "/api/epics");
  getEpic = (id: number) => this.req<EpicView>("GET", `/api/epics/${id}`);
  createEpic = (e: { key: string; title: string; description?: string; project_id?: number }) =>
    this.req<Epic>("POST", "/api/epics", e);
  updateEpic = (
    id: number,
    patch: { title?: string; description?: string | null; archived?: boolean },
  ) => this.req<Epic>("PATCH", `/api/epics/${id}`, patch);
  deleteEpic = (id: number) => this.req<void>("DELETE", `/api/epics/${id}`);

  listCycles = () => this.req<Cycle[]>("GET", "/api/cycles");
  createCycle = (c: NewCycle) =>
    this.req<Cycle>("POST", "/api/cycles", c);
  updateCycle = (id: number, patch: { project_ids: number[] }) =>
    this.req<Cycle>("PATCH", `/api/cycles/${id}`, patch);
  startCycle = (id: number) => this.req<Cycle>("POST", `/api/cycles/${id}/start`);
  endCycle = (id: number) => this.req<Cycle>("POST", `/api/cycles/${id}/end`);

  listIssues = (filter: IssueFilter = {}) => {
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(filter)) {
      if (v !== undefined && v !== false) q.set(k, String(v));
    }
    const qs = q.toString();
    return this.req<Issue[]>("GET", `/api/issues${qs ? `?${qs}` : ""}`);
  };
  /** Every page for a filter. `listIssues` returns one bounded page; callers
   *  that need the full set (MCP tools, scripts) use this. */
  listAllIssues = async (filter: IssueFilter = {}): Promise<Issue[]> => {
    const pageSize = filter.limit ?? 500;
    const out: Issue[] = [];
    for (let pageNo = 0; pageNo < 40; pageNo++) {
      const offset = (filter.offset ?? 0) + pageNo * pageSize;
      const page = await this.listIssues({ ...filter, limit: pageSize, offset });
      out.push(...page);
      if (page.length < pageSize) break;
    }
    return out;
  };
  getIssue = (key: string) => this.req<IssueDetail>("GET", `/api/issues/${key}`);
  createIssue = (i: NewIssue) => this.req<Issue>("POST", "/api/issues", i);
  patchIssue = (key: string, patch: IssuePatch) =>
    this.req<Issue>("PATCH", `/api/issues/${key}`, patch);
  deleteIssue = (key: string) => this.req<void>("DELETE", `/api/issues/${key}`);
  moveIssueProject = (key: string, project: string) =>
    this.req<Issue>("PUT", `/api/issues/${key}/project`, { project });
  addComment = (key: string, body: string) =>
    this.req<Comment>("POST", `/api/issues/${key}/comments`, { body });
  getActivity = (key: string) =>
    this.req<ActivityEntry[]>("GET", `/api/issues/${key}/activity`);
  listIssueCommits = (key: string) =>
    this.req<CommitAttribution[]>("GET", `/api/issues/${key}/commits`);
  recordCommit = (commit: NewCommitAttribution) =>
    this.req<CommitAttribution>("POST", "/api/commits", commit);
  getCommitAttribution = (
    repo: string,
    sha: string,
    opts: { patch_id?: string; tree_sha?: string } = {},
  ) => {
    const q = new URLSearchParams({ repo, sha });
    if (opts.patch_id) q.set("patch_id", opts.patch_id);
    if (opts.tree_sha) q.set("tree_sha", opts.tree_sha);
    return this.req<CommitLookup>("GET", `/api/commits/lookup?${q.toString()}`);
  };

  listIssueReviews = (key: string) =>
    this.req<ReviewRequest[]>("GET", `/api/issues/${key}/reviews`);
  listReviews = (opts: { reviewer?: string; outstanding?: boolean; issue?: string; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.reviewer) q.set("reviewer", opts.reviewer);
    if (opts.outstanding) q.set("outstanding", "true");
    if (opts.issue) q.set("issue", opts.issue);
    if (opts.limit) q.set("limit", String(opts.limit));
    const qs = q.toString();
    return this.req<ReviewRequest[]>("GET", `/api/reviews${qs ? `?${qs}` : ""}`);
  };
  requestReview = (key: string, request: NewReviewRequest) =>
    this.req<ReviewRequest>("POST", `/api/issues/${key}/reviews`, request);
  updateReview = (id: number, patch: ReviewRequestPatch) =>
    this.req<ReviewRequest>("PATCH", `/api/reviews/${id}`, patch);
  withdrawReview = (id: number) =>
    this.req<void>("DELETE", `/api/reviews/${id}`);
  listIssueArtifacts = (key: string) =>
    this.req<IssueArtifact[]>("GET", `/api/issues/${key}/artifacts`);
  /** Multipart upload from the browser (session cookie). */
  uploadIssueArtifact = async (
    key: string,
    file: File,
    opts: {
      kind: ArtifactKind;
      comment_id?: number | null;
      review_ref?: string | null;
      agent_name?: string;
      session_id?: string;
    },
  ): Promise<IssueArtifact> => {
    const fd = new FormData();
    fd.append("file", file);
    fd.append("kind", opts.kind);
    if (opts.comment_id != null) fd.append("comment_id", String(opts.comment_id));
    if (opts.review_ref) fd.append("review_ref", opts.review_ref);
    if (opts.agent_name) fd.append("agent_name", opts.agent_name);
    if (opts.session_id) fd.append("session_id", opts.session_id);
    const headers: Record<string, string> = {};
    if (this.#token) headers.authorization = `Bearer ${this.#token}`;
    const res = await fetch(`${this.baseUrl}/api/issues/${key}/artifacts`, {
      method: "POST",
      credentials: "include",
      headers,
      body: fd,
    });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
    return (await res.json()) as IssueArtifact;
  };
  /** Base64 upload for MCP/CLI clients. */
  uploadIssueArtifactBase64 = (key: string, artifact: NewIssueArtifact) =>
    this.req<IssueArtifact>("POST", `/api/issues/${key}/artifacts/base64`, artifact);
  artifactUrl = (id: number) => `${this.baseUrl}/api/artifacts/${id}`;

  listAgents = () => this.req<AgentClaim[]>("GET", "/api/agents");
  agentStatus = () => this.req<AgentStatusSummary>("GET", "/api/agents/status");
  listAgentRoster = () => this.req<AgentRosterEntry[]>("GET", "/api/agents/roster");
  registerAgent = (body: AgentRegister = {}) =>
    this.req<AgentRegistration>("POST", "/api/agents/register", body);
  killAgent = (name: string) =>
    this.req<AgentRemoval>("POST", `/api/agents/${encodeURIComponent(name)}/kill`);
  pauseAgent = (name: string) =>
    this.req<AgentRosterEntry>("POST", `/api/agents/${encodeURIComponent(name)}/pause`);
  resumeAgent = (name: string) =>
    this.req<AgentRosterEntry>("POST", `/api/agents/${encodeURIComponent(name)}/resume`);
  hideAgent = (name: string) =>
    this.req<AgentRosterEntry>("POST", `/api/agents/${encodeURIComponent(name)}/hide`);
  unhideAgent = (name: string) =>
    this.req<AgentRosterEntry>("POST", `/api/agents/${encodeURIComponent(name)}/unhide`);
  renameAgent = (name: string, new_name: string) =>
    this.req<AgentRosterEntry>("POST", `/api/agents/${encodeURIComponent(name)}/rename`, {
      new_name,
    });
  listAgentUsage = (range: UsageRange = "7d") =>
    this.req<UsageSummary[]>("GET", `/api/usage/agents?range=${range}`);
  getAgentUsage = (name: string, range: UsageRange = "7d") =>
    this.req<UsageSummary>(
      "GET",
      `/api/usage/agents/${encodeURIComponent(name)}?range=${range}`,
    );
  getIssueUsage = (key: string, range: UsageRange = "7d") =>
    this.req<UsageSummary>("GET", `/api/usage/issues/${encodeURIComponent(key)}?range=${range}`);
  getUsageSettings = () => this.req<UsageSettings>("GET", "/api/usage/settings");
  setUsageSettings = (settings: UsageSettings) =>
    this.req<UsageSettings>("PUT", "/api/usage/settings", settings);
  importUsage = (path?: string) =>
    this.req<UsageImportReport>("POST", "/api/usage/import", path ? { path } : {});
  /** One agent's activity timeline, newest first. `before` is the cursor from
   *  the previous page (`next_before`). */
  agentEvents = (
    name: string,
    opts: { kinds?: AgentEventKind[]; since?: string; until?: string; limit?: number; before?: number } = {},
  ) => {
    const q = new URLSearchParams();
    if (opts.kinds?.length) q.set("kind", opts.kinds.join(","));
    if (opts.since) q.set("since", opts.since);
    if (opts.until) q.set("until", opts.until);
    if (opts.limit) q.set("limit", String(opts.limit));
    if (opts.before) q.set("before", String(opts.before));
    const qs = q.toString();
    return this.req<AgentEventPage>(
      "GET",
      `/api/agents/${encodeURIComponent(name)}/events${qs ? `?${qs}` : ""}`,
    );
  };
  claimIssue = (key: string, claim: ClaimUpsert) =>
    this.req<AgentClaim>("POST", `/api/issues/${key}/claim`, claim);
  heartbeatClaim = (key: string, update: ClaimUpdate, agent?: string) =>
    this.req<AgentClaim>("PUT", `/api/issues/${key}/claim${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`, update);
  releaseClaim = (key: string, agent?: string) =>
    this.req<void>("DELETE", `/api/issues/${key}/claim${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`);
  releaseClaimById = (id: number) =>
    this.req<void>("DELETE", `/api/claims/${id}`);
  listChatRooms = (agent?: string) =>
    this.req<ChatRoom[]>("GET", `/api/chat/rooms${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`);
  createChatRoom = (name: string, description?: string) =>
    this.req<ChatRoom>("POST", "/api/chat/rooms", { name, description });
  listChatMessages = (
    room: number,
    opts: { agent?: string; after?: number; limit?: number } = {},
  ) => {
    const q = new URLSearchParams();
    if (opts.agent) q.set("agent", opts.agent);
    if (opts.after) q.set("after", String(opts.after));
    if (opts.limit) q.set("limit", String(opts.limit));
    const qs = q.toString();
    return this.req<ChatMessage[]>("GET", `/api/chat/rooms/${room}/messages${qs ? `?${qs}` : ""}`);
  };
  chatMentions = () => this.req<ChatMention[]>("GET", "/api/chat/mentions");
  deleteChatRoom = (id: number) => this.req<void>("DELETE", `/api/chat/rooms/${id}`);
  deleteChatMessage = (id: number, agent?: string) =>
    this.req<void>(
      "DELETE",
      `/api/chat/messages/${id}${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`,
    );
  compactChatRoom = (id: number) =>
    this.req<ChatMessage>("POST", `/api/chat/rooms/${id}/compact`);
  postChatMessage = (body: {
    body: string;
    room?: string;
    to?: string;
    agent_name?: string;
    uploads?: number[];
  }) => this.req<ChatMessage>("POST", "/api/chat/messages", body);

  /** Multipart upload from the browser (session cookie). */
  uploadChatFile = async (file: File): Promise<ChatUpload> => {
    const fd = new FormData();
    fd.append("file", file);
    const headers: Record<string, string> = {};
    if (this.#token) headers.authorization = `Bearer ${this.#token}`;
    const res = await fetch(`${this.baseUrl}/api/chat/uploads`, {
      method: "POST",
      credentials: "include",
      headers,
      body: fd,
    });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
    return (await res.json()) as ChatUpload;
  };

  /** Base64 upload for MCP/CLI clients. */
  uploadChatBase64 = (filename: string, data_base64: string, mime?: string) =>
    this.req<ChatUpload>("POST", "/api/chat/uploads/base64", { filename, mime, data_base64 });
  listChatPushes = (agent?: string, after?: number, limit?: number) => {
    const q = new URLSearchParams();
    if (agent) q.set("agent", agent);
    if (after) q.set("after", String(after));
    if (limit) q.set("limit", String(limit));
    const qs = q.toString();
    return this.req<ChatPush[]>("GET", `/api/chat/pushes${qs ? `?${qs}` : ""}`);
  };
  ackChatPushes = (up_to: number, agent?: string) =>
    this.req<void>("POST", "/api/chat/pushes/ack", { up_to, agent_name: agent });
  getChatPushSubs = (agent?: string) =>
    this.req<number[]>(
      "GET",
      `/api/chat/push-subs${agent ? `?agent=${encodeURIComponent(agent)}` : ""}`,
    );
  setChatPushSub = (room: string, enabled: boolean, agent?: string) =>
    this.req<void>("PUT", "/api/chat/push-subs", { room, enabled, agent_name: agent });

  openDm = (withAgent: string, agent_name?: string) =>
    this.req<ChatRoom>("POST", "/api/chat/dm", { with: withAgent, agent_name });

  setAgentMessage = (agent_name: string, message: string) =>
    this.req<{ agent_name: string; message: string | null }>("PUT", "/api/agents/message", {
      agent_name,
      message,
    });

  /**
   * Subscribe to the live event stream. `onStatus(true|false)` reports the
   * live/disconnected state (EventSource reconnects on its own). Returns an
   * unsubscribe function.
   */
  subscribe(
    onEvent: (event: BoardEvent) => void,
    onStatus?: (live: boolean) => void,
  ): () => void {
    const es = new EventSource(`${this.baseUrl}/events`, { withCredentials: true });
    es.onopen = () => onStatus?.(true);
    es.onmessage = (msg) => {
      onStatus?.(true);
      try {
        onEvent(JSON.parse(msg.data) as BoardEvent);
      } catch {
        /* ignore malformed frame */
      }
    };
    es.onerror = () => onStatus?.(es.readyState === EventSource.OPEN);
    return () => es.close();
  }
}
