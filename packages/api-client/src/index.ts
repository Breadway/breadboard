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

export type NotificationKind =
  | "assigned"
  | "mention"
  | "cycle_started"
  | "cycle_ended"
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
}

export interface ImportResult {
  epics_created: string[];
  issues_created: string[];
  skipped: string[];
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

export interface AgentClaim {
  issue_id: number;
  key: string;
  issue_title: string;
  issue_status: Status;
  claimed_by: number;
  claimed_by_login: string;
  agent_name: string;
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
  session_id?: string | null;
  note?: string | null;
  branch?: string | null;
  worktree?: string | null;
  repo?: string | null;
  pr_url?: string | null;
  pr_number?: number | null;
  head_sha?: string | null;
}

export interface AgentStatusSummary {
  total: number;
  live: number;
  stale: number;
  done: number;
  active: number;
  review: number;
  verifying: number;
  blocked: number;
  remaining: number;
  line: string;
  claims: AgentClaim[];
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
  | { type: "agent.claimed"; id: number; key: string; agent: string; session_id: string | null; state: string }
  | { type: "agent.updated"; id: number; key: string; agent: string; session_id: string | null; state: string }
  | { type: "agent.released"; id: number; key: string };

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
    return res.json() as Promise<T>;
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
  createCycle = (c: { name: string; starts_on: string; ends_on: string }) =>
    this.req<Cycle>("POST", "/api/cycles", c);
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

  listAgents = () => this.req<AgentClaim[]>("GET", "/api/agents");
  agentStatus = () => this.req<AgentStatusSummary>("GET", "/api/agents/status");
  claimIssue = (key: string, claim: ClaimUpsert) =>
    this.req<AgentClaim>("POST", `/api/issues/${key}/claim`, claim);
  heartbeatClaim = (key: string, update: ClaimUpdate) =>
    this.req<AgentClaim>("PUT", `/api/issues/${key}/claim`, update);
  releaseClaim = (key: string) =>
    this.req<void>("DELETE", `/api/issues/${key}/claim`);

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
