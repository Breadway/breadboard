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
  | { type: "chat.message"; room_id: number; id: number }
  | { type: "chat.room.deleted"; id: number };

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

  listAgents = () => this.req<AgentClaim[]>("GET", "/api/agents");
  agentStatus = () => this.req<AgentStatusSummary>("GET", "/api/agents/status");
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
