// Reactive app state. One module-level singleton; components read its fields
// directly (Svelte 5 deep reactivity) and call its methods to mutate.

import {
  ApiError,
  BreadboardClient,
  STATUSES,
  type AgentClaim,
  type AgentRosterEntry,
  type AgentState,
  type BoardEvent,
  type ChatRoom,
  type ClaimUpdate,
  type ClaimUpsert,
  type Cycle,
  type EpicView,
  type Notification,
  type Issue,
  type IssuePatch,
  type IssueType,
  type NewIssue,
  type Priority,
  type Project,
  type RepoView,
  type Status,
  type User,
} from "@breadboard/api-client";

export const api = new BreadboardClient();

type AuthState = "loading" | "in" | "out" | "error";
export type View = "board" | "backlog" | "roadmap" | "agents" | "chat" | "inbox" | "settings";

/** Multi-select filter facets. Facets combine with AND; values inside one
 *  facet combine with OR. An empty facet means "no constraint". */
export interface Filters {
  projects: string[];
  cycles: number[];
  epics: number[];
  statuses: Status[];
  priorities: Priority[];
  labels: string[];
  assignees: number[];
  /** Separate toggle: only issues with no assignee. */
  unassigned: boolean;
  agents: string[];
  agentStates: AgentState[];
  /** Agent presence: any claim, no claim, or don't care. */
  claimed: "" | "claimed" | "unclaimed";
  types: IssueType[];
  text: string;
}

export function emptyFilters(): Filters {
  return {
    projects: [],
    cycles: [],
    epics: [],
    statuses: [],
    priorities: [],
    labels: [],
    assignees: [],
    unassigned: false,
    agents: [],
    agentStates: [],
    claimed: "",
    types: [],
    text: "",
  };
}

/** A claim with no heartbeat for this long reads as stale (never auto-released).
 *  Must match CLAIM_STALE_SECS in crates/breadboard-server/src/api.rs. */
export const CLAIM_STALE_SECS = 300;

/** Page size for issue lists; matches the server default. */
const ISSUE_PAGE = 500;

export function claimStale(lastHeartbeat: string, now = Date.now()): boolean {
  const t = Date.parse(lastHeartbeat);
  if (Number.isNaN(t)) return false;
  return (now - t) / 1000 >= CLAIM_STALE_SECS;
}

export function ago(iso: string, now = Date.now()): string {
  const s = Math.max(0, Math.floor((now - Date.parse(iso)) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

/** Fields a human can see on a card tint, the Agents view or the issue drawer.
 *  A bare heartbeat only moves last_heartbeat/updated_at, which the 30s clock
 *  already repaints from. */
function claimVisibleChange(a: AgentClaim, b: AgentClaim): boolean {
  return (
    a.agent_name !== b.agent_name ||
    a.mode !== b.mode ||
    a.state !== b.state ||
    a.note !== b.note ||
    a.branch !== b.branch ||
    a.worktree !== b.worktree ||
    a.repo !== b.repo ||
    a.pr_url !== b.pr_url ||
    a.pr_number !== b.pr_number ||
    a.head_sha !== b.head_sha ||
    a.board_status !== b.board_status ||
    a.issue_status !== b.issue_status ||
    a.session_id !== b.session_id
  );
}

export interface Toast {
  id: number;
  msg: string;
  kind: "info" | "error";
  undo?: () => void;
}

/** Facet sets prepared once per filter change instead of once per issue.
 *  `claims` is null unless a claim facet is active, which keeps agent
 *  heartbeats from invalidating issue filtering. */
interface MatchCtx {
  projectKeys: Set<string>;
  projectKeyById: Map<number, string>;
  /** Cycle constraint; empty when the caller ignores cycles. */
  cycles: Set<number>;
  /** Number of cycles in the real filter, for the closed-cycle archive rule. */
  filterCycleCount: number;
  closedCycles: Set<number>;
  epics: Set<number>;
  statuses: Set<Status>;
  priorities: Set<Priority>;
  labels: Set<string>;
  assignees: Set<number>;
  agents: Set<string>;
  agentStates: Set<AgentState>;
  types: Set<IssueType>;
  text: string;
  unassigned: boolean;
  claimed: Filters["claimed"];
  claims: Map<number, AgentClaim> | null;
}

const NO_ISSUES: Issue[] = [];

class Store {
  me = $state<User | null>(null);
  auth = $state<AuthState>("loading");
  authError = $state<string | null>(null);
  /** True when the server allows POST /auth/dev-login (local demos). */
  devLoginAllowed = $state(false);
  issues = $state<Issue[]>([]);
  projects = $state<Project[]>([]);
  cycles = $state<Cycle[]>([]);
  epics = $state<EpicView[]>([]);
  connected = $state(false);
  toast = $state<Toast | null>(null);
  /** Ticking clock so heartbeat ages and stale tints update without events. */
  clock = $state(Date.now());
  /** Bumped when any chat message arrives over SSE. */
  chatTick = $state(0);
  /** Room of the most recent chat event, so the open view can ignore other rooms. */
  chatRoom = $state<number | null>(null);
  /** Total unread chat messages for the signed-in human. */
  chatUnread = $state(0);
  /** Rooms as last fetched, so the Agents card can show a DM link and its
   *  unread count without opening the Chat view. */
  chatRooms = $state<ChatRoom[]>([]);
  /** Pending in-app DM deep link: the Chat view opens or creates this DM. The
   *  route /chat?dm=<agent> sets it on a cold load too. */
  chatDm = $state<string | null>(null);

  repos = $state<RepoView[]>([]);
  notifications = $state<Notification[]>([]);
  unread = $state(0);

  agents = $state<AgentClaim[]>([]);
  /** Claim awaiting a handoff note before release. Set by releaseById when no
   *  note was supplied; the ReleaseHandoff dialog consumes it. */
  pendingRelease = $state<AgentClaim | null>(null);
  /** Exact server refusal shown inside the release dialog. */
  releaseError = $state<string | null>(null);
  /** Sessions, profiles and claims, one row per lowercased name. Roster rows
   *  with no claims still render, as idle sessions. */
  roster = $state<AgentRosterEntry[]>([]);
  agentLine = $state<string>("");

  // Bumped whenever a live event touches an issue, so the open issue panel
  // knows to refetch its comments and activity.
  issueTick = $state<{ key: string; n: number }>({ key: "", n: 0 });

  // Bumped whenever a review record changes, so an issue's review panel and a
  // reviewer's queue both refetch without a full board reload.
  reviewTick = $state(0);

  // UI state
  view = $state<View>("board");
  selectedKey = $state<string | null>(null);
  /** Set by the agent activity panel, consumed and cleared by Chat: open this
   *  room and scroll to this message (when present). */
  chatFocus = $state<{ room: number; message: number | null } | null>(null);
  paletteOpen = $state(false);
  paletteStatus = $state<Status | null>(null);
  /** Every facet is multi-select; facets combine with AND, values within a
   *  facet with OR. */
  filters = $state<Filters>(emptyFilters());

  // Filter state is prepared once per change and shared by every consumer.
  // Without this, each byStatus()/filtered read re-walked all issues and
  // rebuilt ten Sets plus the claims map per issue (BB-127).
  #filterCtx = $derived.by(() => this.#prepareFilters(false));
  #cycleFilterCtx = $derived.by(() => this.#prepareFilters(true));
  #filtered = $derived.by(() =>
    this.issues.filter((i) => this.#matches(i, this.#filterCtx)),
  );
  #byStatus = $derived.by(() => this.#groupByStatus());
  /** Non-reactive cache so untouched columns keep their array identity. */
  #statusLists: Map<Status, Issue[]> = new Map();

  #unsub: (() => void) | null = null;
  #clockTimer: ReturnType<typeof setInterval> | undefined;
  #lastChatPush = 0;
  #chatPushReady = false;
  #chatPushTimer: ReturnType<typeof setTimeout> | undefined;
  // Locally read-through chat message id per room. The server writes the read
  // receipt in the background, so a room-list fetch can still report unread
  // messages this client has already displayed (BB-205). Counts at or below
  // the watermark are hidden until the server catches up.
  #chatReadThrough: Map<number, number> = new Map();
  // Room-list fetches apply latest-request-wins in the shared cache, not just
  // in the Chat component: an older snapshot must not overwrite a newer one
  // for the sidebar badge or the agent cards (BB-184).
  #chatRoomsSeq = 0;
  #chatRoomsApplied = 0;
  #everLive = false;
  #issuesGen = 0;
  #retry: ReturnType<typeof setTimeout> | undefined;
  #retryDelay = 1000;
  #toastTimer: ReturnType<typeof setTimeout> | undefined;
  #toastSeq = 0;

  get isDev() {
    return import.meta.env.DEV;
  }

  get serverBase() {
    return api.baseUrl || location.origin;
  }

  /** Number of individual filter values selected, for the trigger badge. */
  get filterCount(): number {
    const f = this.filters;
    return (
      f.projects.length +
      f.cycles.length +
      f.epics.length +
      f.statuses.length +
      f.priorities.length +
      f.labels.length +
      f.assignees.length +
      (f.unassigned ? 1 : 0) +
      f.agents.length +
      f.agentStates.length +
      (f.claimed ? 1 : 0) +
      f.types.length +
      (f.text.trim() ? 1 : 0)
    );
  }

  get hasFilters(): boolean {
    return this.filterCount > 0;
  }

  /** Issues that pass every facet. Claims narrow the set too.
   *  Computed once per filter/data change and shared by every consumer. */
  get filtered(): Issue[] {
    return this.#filtered;
  }

  /** True when `issue` passes every active facet. `ignoreCycles` lets a cycle
   *  strip scope its own progress while still respecting the other filters. */
  matches(issue: Issue, opts: { ignoreCycles?: boolean } = {}): boolean {
    return this.#matches(issue, opts.ignoreCycles ? this.#cycleFilterCtx : this.#filterCtx);
  }

  #prepareFilters(ignoreCycles: boolean): MatchCtx {
    const f = this.filters;
    // Only track claims when a claim facet can actually exclude an issue, so
    // heartbeat updates do not invalidate issue filtering.
    const wantsClaims = f.claimed !== "" || f.agents.length > 0 || f.agentStates.length > 0;
    const claims = wantsClaims
      ? new Map(
          this.agents
            .filter((a) => a.mode === "exclusive")
            .map((a) => [a.issue_id, a] as const),
        )
      : null;
    return {
      projectKeys: new Set(f.projects),
      projectKeyById: new Map(this.projects.map((p) => [p.id, p.key] as const)),
      cycles: ignoreCycles ? new Set<number>() : new Set(f.cycles),
      filterCycleCount: f.cycles.length,
      closedCycles: new Set(this.cycles.filter((c) => c.closed).map((c) => c.id)),
      epics: new Set(f.epics),
      statuses: new Set(f.statuses),
      priorities: new Set(f.priorities),
      labels: new Set(f.labels),
      assignees: new Set(f.assignees),
      agents: new Set(f.agents),
      agentStates: new Set(f.agentStates),
      types: new Set(f.types),
      text: f.text.trim().toLowerCase(),
      unassigned: f.unassigned,
      claimed: f.claimed,
      claims,
    };
  }

  #matches(i: Issue, c: MatchCtx): boolean {
    // A PR that closes a real issue collapses into that issue's card
    // instead of showing as a second, seemingly-unrelated card; its
    // status surfaces there as a small badge instead (see Card.svelte).
    // Asking for PRs explicitly overrides the collapse so the filter is
    // not silently empty.
    if (!c.types.has("pr") && i.type === "pr" && i.links.some((l) => l.kind === "closes")) return false;
    // Once a cycle (release) ships and is closed, its done work archives
    // off the default board/backlog view instead of piling up forever;
    // still there if you explicitly pick that cycle from the filter.
    if (c.filterCycleCount === 0 && i.status === "done" && i.cycle_id != null && c.closedCycles.has(i.cycle_id)) return false;
    if (c.projectKeys.size && !c.projectKeys.has(c.projectKeyById.get(i.project_id) ?? "?")) return false;
    if (c.cycles.size && (i.cycle_id == null || !c.cycles.has(i.cycle_id))) return false;
    if (c.epics.size && (i.epic_id == null || !c.epics.has(i.epic_id))) return false;
    if (c.statuses.size && !c.statuses.has(i.status)) return false;
    if (c.priorities.size && !c.priorities.has(i.priority)) return false;
    if (c.labels.size && !i.labels.some((l) => c.labels.has(l))) return false;
    if (c.assignees.size && (i.assignee_id == null || !c.assignees.has(i.assignee_id))) return false;
    if (c.unassigned && i.assignee_id != null) return false;
    if (c.types.size && !c.types.has(i.type)) return false;
    if (c.text) {
      const hay = `${i.key} ${i.title} ${i.body} ${i.labels.join(" ")}`.toLowerCase();
      if (!hay.includes(c.text)) return false;
    }
    if (c.claims) {
      const claim = c.claims.get(i.id);
      if (c.claimed === "claimed" && !claim) return false;
      if (c.claimed === "unclaimed" && claim) return false;
      if (c.agents.size && (!claim || !c.agents.has(claim.agent_name))) return false;
      if (c.agentStates.size && (!claim || !c.agentStates.has(claim.state))) return false;
    }
    return true;
  }

  #groupByStatus(): Map<Status, Issue[]> {
    const grouped = new Map<Status, Issue[]>();
    for (const status of STATUSES) grouped.set(status, []);
    for (const i of this.#filtered) grouped.get(i.status)?.push(i);
    const previous = this.#statusLists;
    for (const [status, list] of grouped) {
      list.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      const old = previous.get(status);
      if (old && old.length === list.length && old.every((v, idx) => v === list[idx])) {
        grouped.set(status, old);
      }
    }
    this.#statusLists = grouped;
    return grouped;
  }

  toggleFilter<K extends "projects" | "cycles" | "epics" | "statuses" | "priorities" | "labels" | "assignees" | "agents" | "agentStates" | "types">(
    facet: K,
    value: Filters[K][number],
  ) {
    const list = this.filters[facet] as unknown as (typeof value)[];
    const i = list.indexOf(value);
    if (i === -1) list.push(value);
    else list.splice(i, 1);
  }

  isFiltered<K extends "projects" | "cycles" | "epics" | "statuses" | "priorities" | "labels" | "assignees" | "agents" | "agentStates" | "types">(
    facet: K,
    value: Filters[K][number],
  ): boolean {
    return (this.filters[facet] as unknown as (typeof value)[]).includes(value);
  }

  setOnlyFilter(facet: "epics" | "cycles", value: number) {
    this.filters[facet] = [value];
  }

  clearFilters() {
    this.filters = emptyFilters();
  }

  /** Toggle "assigned to me" as an assignee facet value. */
  toggleMine() {
    if (!this.me) return;
    this.toggleFilter("assignees", this.me.id);
  }

  /** Column contents for one status, sorted most-recently-updated first.
   *  Grouped once per change and shared across the five columns. */
  byStatus(status: Status): Issue[] {
    return this.#byStatus.get(status) ?? NO_ISSUES;
  }

  projectKey(id: number): string {
    return this.projects.find((p) => p.id === id)?.key ?? "?";
  }

  project(id: number): Project | undefined {
    return this.projects.find((p) => p.id === id);
  }

  findIssue(id: number): Issue | undefined {
    return this.issues.find((i) => i.id === id);
  }

  issueByKey(key: string | null): Issue | undefined {
    return key ? this.issues.find((i) => i.key === key) : undefined;
  }

  /** The open cycle running today, else the next open one to start. */
  get activeCycle(): Cycle | undefined {
    const today = new Date().toISOString().slice(0, 10);
    const open = this.cycles.filter((c) => !c.closed).sort((a, b) => a.starts_on.localeCompare(b.starts_on));
    return open.find((c) => c.starts_on <= today && today <= c.ends_on) ?? open.find((c) => c.starts_on > today) ?? open[0];
  }

  async init() {
    clearTimeout(this.#retry);
    try {
      const h = await api.health();
      this.devLoginAllowed = h.dev_login ?? false;
    } catch {
      /* backend unreachable: init below reports it */
    }
    try {
      this.me = await api.me();
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        this.auth = "out";
        return;
      }
      // Network error, 502 mid-deploy, etc: say so and keep retrying with
      // backoff instead of sitting on "connecting…" forever.
      this.auth = "error";
      this.authError = errMsg(e);
      this.#retry = setTimeout(() => void this.init(), this.#retryDelay);
      this.#retryDelay = Math.min(this.#retryDelay * 2, 30_000);
      return;
    }
    this.#retryDelay = 1000;
    this.authError = null;
    try {
      await this.loadAll();
    } catch (e) {
      this.auth = "error";
      this.authError = errMsg(e);
      this.#retry = setTimeout(() => void this.init(), this.#retryDelay);
      return;
    }
    this.auth = "in";
    await this.loadInbox();
    void this.initChatPushes();
    void this.loadAgents();
    if (!this.#clockTimer) {
      this.#clockTimer = setInterval(() => (this.clock = Date.now()), 30_000);
    }
    this.#connect();
    if ("Notification" in window && Notification.permission === "default") {
      Notification.requestPermission().catch(() => {});
    }
  }

  retryNow() {
    this.#retryDelay = 1000;
    this.auth = "loading";
    void this.init();
  }

  /** Drain pushes that predate this session so we do not notify for old chat. */
  async initChatPushes() {
    try {
      const pushes = await api.listChatPushes(undefined, 0, 100);
      const last = pushes[pushes.length - 1];
      if (last) {
        this.#lastChatPush = last.id;
        await api.ackChatPushes(last.id);
      }
      this.#chatPushReady = true;
    } catch {
      /* notifications are best-effort */
    }
  }

  /** Browser notification for chat DMs/pings (and subscribed rooms). */
  async notifyChatPushes() {
    if (!this.#chatPushReady) return;
    if (!("Notification" in window) || Notification.permission !== "granted") return;
    try {
      const pushes = await api.listChatPushes(undefined, this.#lastChatPush, 50);
      if (!pushes.length) return;
      this.#lastChatPush = pushes[pushes.length - 1].id;
      for (const push of pushes) {
        const where = push.room_name ? `#${push.room_name}` : "DM";
        const text =
          push.message.body ||
          (push.message.attachments.length ? "sent an attachment" : "new message");
        if ("Notification" in window && Notification.permission === "granted") {
          try {
            new Notification(`breadboard · ${where}`, {
              body: `${push.message.author_display}: ${text}`,
            });
          } catch {
            /* some platforms refuse notifications without a service worker */
          }
        }
      }
      await api.ackChatPushes(this.#lastChatPush);
    } catch {
      /* best-effort */
    }
  }

  async loadInbox() {
    try {
      const box = await api.inbox();
      this.notifications = box.notifications;
      this.unread = box.unread;
    } catch {
      /* ignore */
    }
  }

  async markRead(id: number) {
    this.notifications = this.notifications.map((n) =>
      n.id === id ? { ...n, read: true } : n,
    );
    this.unread = Math.max(0, this.unread - 1);
    await api.markRead(id).catch(() => this.loadInbox());
  }

  async markAllRead() {
    this.notifications = this.notifications.map((n) => ({ ...n, read: true }));
    this.unread = 0;
    await api.markAllRead().catch(() => this.loadInbox());
  }

  /** Exclusive claims only: intents are reservations, not active work. */
  get claimsByIssue(): Map<number, AgentClaim> {
    return new Map(
      this.agents.filter((a) => a.mode === "exclusive").map((a) => [a.issue_id, a]),
    );
  }

  get activeAgents(): number {
    return this.agents.filter((a) => a.mode === "exclusive").length;
  }

  get queuedAgents(): number {
    return this.agents.filter((a) => a.mode === "intent").length;
  }

  claimForIssue(issueId: number): AgentClaim | undefined {
    return this.agents.find((a) => a.issue_id === issueId && a.mode === "exclusive");
  }

  /** Every claim on an issue, exclusive first, then queued intents. */
  claimsForIssue(issueId: number): AgentClaim[] {
    return this.agents
      .filter((a) => a.issue_id === issueId)
      .sort((a, b) => (a.mode === b.mode ? a.id - b.id : a.mode === "exclusive" ? -1 : 1));
  }

  async loadAgents() {
    try {
      const [claims, status, roster] = await Promise.all([
        api.listAgents(),
        api.agentStatus(),
        api.listAgentRoster(),
      ]);
      this.agents = claims;
      this.agentLine = status.line;
      this.roster = roster;
    } catch {
      /* ignore */
    }
  }

  /** Fetch the room list once and keep the shared unread total honest for
   *  views outside Chat. */
  async loadChatRooms(): Promise<ChatRoom[]> {
    const seq = ++this.#chatRoomsSeq;
    const fetched = await api.listChatRooms();
    // A response that started before a newer one already applied (for example
    // the snapshot taken before a DM existed) is stale even if it resolves
    // last. Return the cache instead of overwriting it.
    if (seq < this.#chatRoomsApplied) return this.chatRooms;
    this.#chatRoomsApplied = seq;
    const next = this.#applyChatReads(fetched);
    this.chatRooms = next;
    this.chatUnread = next.reduce((n, r) => n + r.unread, 0);
    return next;
  }

  /** Hide unread counts this client has already read on screen. */
  #applyChatReads(list: ChatRoom[]): ChatRoom[] {
    if (!this.#chatReadThrough.size) return list;
    let changed = false;
    const next = list.map((room) => {
      if (!room.unread) return room;
      const mark = this.#chatReadThrough.get(room.id);
      if (mark == null) return room;
      // Only clear when the newest message is at or below the read point.
      // A newer message keeps its real count.
      if (!room.last_message || room.last_message.id > mark) return room;
      changed = true;
      return { ...room, unread: 0 };
    });
    return changed ? next : list;
  }

  /** Record that this client has displayed every message up to `messageId` in
   *  a room, and clear its badge now instead of waiting for the server's
   *  background read receipt and the next poll. */
  markChatRoomRead(roomId: number, messageId: number) {
    if (!messageId) return;
    const previous = this.#chatReadThrough.get(roomId) ?? 0;
    if (messageId <= previous) return;
    this.#chatReadThrough.set(roomId, messageId);
    let changed = false;
    const next = this.chatRooms.map((room) => {
      if (room.id !== roomId || !room.unread) return room;
      changed = true;
      return { ...room, unread: 0 };
    });
    if (changed) {
      this.chatRooms = next;
      this.chatUnread = next.reduce((n, r) => n + r.unread, 0);
    }
  }

  /** The DM shared with this agent, when it already exists. */
  dmRoomFor(agentName: string): ChatRoom | undefined {
    const wanted = agentName.trim().toLowerCase();
    return this.chatRooms.find(
      (r) =>
        r.kind === "dm" &&
        r.members.some((m) => m.toLowerCase() === `agent:${wanted}`),
    );
  }

  /** The agent on the other side of a DM room, for URL syncing. */
  chatDmAgent(room: ChatRoom | undefined): string | null {
    if (!room || room.kind !== "dm") return null;
    const member = room.members.find((m) => m.toLowerCase().startsWith("agent:"));
    return member ? member.slice("agent:".length) : null;
  }

  /** Stable, copyable deep link to the DM with an agent. */
  chatDmHref(agentName: string): string {
    return `/chat?dm=${encodeURIComponent(agentName)}`;
  }

  /** Land on the chat page and open (or create) the DM with this agent. */
  openAgentDm(agentName: string) {
    this.chatFocus = null;
    this.chatDm = agentName;
    this.view = "chat";
    this.updateChatUrl(agentName);
  }

  /** Keep the address bar on the room currently open. One agent name yields
   *  the copyable DM deep link; a shared room keeps /chat; leaving Chat falls
   *  back to the board route. */
  updateChatUrl(agentName: string | null, inChat = false) {
    const href = agentName ? this.chatDmHref(agentName) : inChat ? "/chat" : "/";
    if (location.pathname + location.search !== href) {
      history.replaceState(null, "", href);
    }
  }

  /** Kill an agent: removes it from the roster, every view and its name. */
  async killAgent(name: string): Promise<boolean> {
    try {
      const removal = await api.killAgent(name);
      this.roster = this.roster.filter((r) => r.agent_name !== removal.agent_name);
      this.agents = this.agents.filter((a) => a.agent_name.toLowerCase() !== removal.agent_name);
      const claims = removal.claims_released
        ? `; released ${removal.claims_released} claim${removal.claims_released === 1 ? "" : "s"}`
        : "";
      this.info(`Removed ${removal.agent_name}${claims}`);
      return true;
    } catch (e) {
      this.flash(`Couldn't kill ${name}: ${errMsg(e)}`);
      return false;
    }
  }

  /** Pause: keeps claims and the roster row, blocks new claims and chat wakeups. */
  async pauseAgent(name: string): Promise<boolean> {
    try {
      const entry = await api.pauseAgent(name);
      this.roster = [...this.roster.filter((r) => r.agent_name !== entry.agent_name), entry];
      this.info(`Paused ${entry.agent_name}; claims kept, new claims blocked`);
      return true;
    } catch (e) {
      this.flash(`Couldn't pause ${name}: ${errMsg(e)}`);
      return false;
    }
  }

  async resumeAgent(name: string): Promise<boolean> {
    try {
      const entry = await api.resumeAgent(name);
      this.roster = [...this.roster.filter((r) => r.agent_name !== entry.agent_name), entry];
      this.info(`Resumed ${entry.agent_name}`);
      return true;
    } catch (e) {
      this.flash(`Couldn't resume ${name}: ${errMsg(e)}`);
      return false;
    }
  }

  /** Hide an agent from the default Agents view; claims and chat stay. */
  async hideAgent(name: string): Promise<boolean> {
    try {
      const entry = await api.hideAgent(name);
      this.roster = [...this.roster.filter((r) => r.agent_name !== entry.agent_name), entry];
      const claims =
        entry.claim_count > 0
          ? `; it still holds ${entry.claim_count} claim${entry.claim_count === 1 ? "" : "s"}`
          : "";
      this.info(`Hid ${entry.agent_name}${claims}`, () => void this.unhideAgent(entry.agent_name));
      return true;
    } catch (e) {
      this.flash(`Couldn't hide ${name}: ${errMsg(e)}`);
      return false;
    }
  }

  async unhideAgent(name: string): Promise<boolean> {
    try {
      const entry = await api.unhideAgent(name);
      this.roster = [...this.roster.filter((r) => r.agent_name !== entry.agent_name), entry];
      this.info(`Revealed ${entry.agent_name}`);
      return true;
    } catch (e) {
      this.flash(`Couldn't reveal ${name}: ${errMsg(e)}`);
      return false;
    }
  }

  /** Set or clear an agent's orchestrator. The local parent link patches
   *  optimistically so a drop re-renders before the round trip; the server row
   *  replaces it on success, and the toast offers one-click undo. A previous
   *  raw parent session id that never registered cannot be restored by name,
   *  so no undo is offered in that one case. */
  async setAgentOrchestrator(name: string, orchestrator: string | null): Promise<boolean> {
    const child = this.roster.find((r) => r.agent_name === name);
    if (!child) {
      this.flash(`Couldn't move ${name}: it is not on the roster`);
      return false;
    }
    const target = orchestrator
      ? this.roster.find((r) => r.agent_name === orchestrator)
      : undefined;
    if (orchestrator && !target) {
      this.flash(`Couldn't move ${name} under ${orchestrator}: that agent is not on the roster`);
      return false;
    }
    const previousParent = child.parent_session_id ?? null;
    const previousOrchestrator = child.orchestrator ?? null;
    const previousOrchestratorSession = child.orchestrator_session_id ?? null;

    child.parent_session_id = target?.session_id ?? null;
    child.orchestrator = target?.agent_name ?? null;
    child.orchestrator_session_id = target?.session_id ?? null;
    this.roster = [...this.roster];

    try {
      const entry = await api.setAgentOrchestrator(name, orchestrator);
      this.roster = [...this.roster.filter((r) => r.agent_name !== entry.agent_name), entry];
      const where = orchestrator ? `under ${orchestrator}` : "at the top level";
      const canUndo = previousOrchestrator != null || previousParent == null;
      this.info(
        `Moved ${name} ${where}`,
        canUndo ? () => void this.setAgentOrchestrator(name, previousOrchestrator) : undefined,
      );
      return true;
    } catch (e) {
      const current = this.roster.find((r) => r.agent_name === name);
      if (current) {
        current.parent_session_id = previousParent;
        current.orchestrator = previousOrchestrator;
        current.orchestrator_session_id = previousOrchestratorSession;
        this.roster = [...this.roster];
      }
      this.flash(`Couldn't move ${name}: ${errMsg(e)}`);
      return false;
    }
  }

  async claim(key: string, body: ClaimUpsert): Promise<boolean> {
    try {
      const c = await api.claimIssue(key, body);
      this.agents = [...this.agents.filter((a) => a.id !== c.id), c];
      const what = c.mode === "intent" ? "Queued for" : "Claimed";
      this.info(`${what} ${c.key} as ${c.agent_name}`);
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        this.flash(`Already claimed: ${errMsg(e)}`);
      } else {
        this.flash(`Couldn't claim ${key}: ${errMsg(e)}`);
      }
      return false;
    }
  }

  async heartbeat(key: string, body: ClaimUpdate, agent?: string): Promise<boolean> {
    try {
      const c = await api.heartbeatClaim(key, body, agent);
      this.agents = [...this.agents.filter((a) => a.id !== c.id), c];
      return true;
    } catch (e) {
      this.flash(`Couldn't update claim on ${key}: ${errMsg(e)}`);
      return false;
    }
  }

  /** Release one claim by id; the card keeps its status. Without a handoff
   *  note this opens the release dialog instead of calling the server, because
   *  the server refuses a bare release for work that held anything. */
  async releaseById(
    id: number,
    undoable = true,
    note?: string,
    noNote = false,
  ): Promise<boolean> {
    const snapshot = this.agents;
    const claim = snapshot.find((a) => a.id === id);
    if (!claim) return false;
    if (note == null && !noNote) {
      this.releaseError = null;
      this.pendingRelease = claim;
      return false;
    }
    this.agents = this.agents.filter((a) => a.id !== id);
    try {
      await api.releaseClaimById(id, note, noNote);
      if (this.pendingRelease?.id === id) this.pendingRelease = null;
      this.releaseError = null;
      this.info(
        `Released ${claim.key}`,
        undoable ? () => void this.reclaim(claim) : undefined,
      );
      return true;
    } catch (e) {
      this.agents = snapshot;
      this.releaseError = errMsg(e);
      this.flash(`Couldn't release the claim: ${errMsg(e)}`);
      return false;
    }
  }

  private async reclaim(claim: AgentClaim): Promise<void> {
    await this.claim(claim.key, {
      agent_name: claim.agent_name,
      mode: claim.mode,
      session_id: claim.session_id ?? undefined,
      state: claim.state as AgentState,
      note: claim.note ?? undefined,
      branch: claim.branch ?? undefined,
      worktree: claim.worktree ?? undefined,
      repo: claim.repo ?? undefined,
      pr_url: claim.pr_url ?? undefined,
      pr_number: claim.pr_number ?? undefined,
      head_sha: claim.head_sha ?? undefined,
    });
  }

  async loadAll() {
    // First page paints the board; the rest of the history fills in behind it
    // so a 2.5MB snapshot never blocks startup (BB-124).
    const gen = ++this.#issuesGen;
    const [issues, projects, cycles, epics] = await Promise.all([
      api.listIssues({ limit: ISSUE_PAGE }),
      api.listProjects(),
      api.listCycles(),
      api.listEpics(),
    ]);
    if (gen !== this.#issuesGen) return;
    this.issues = issues;
    this.projects = projects;
    this.cycles = cycles;
    this.epics = epics;
    if (issues.length === ISSUE_PAGE) void this.#fillIssues(gen, issues.length);
  }

  async #fillIssues(gen: number, start: number) {
    try {
      for (let offset = start; ; offset += ISSUE_PAGE) {
        const page = await api.listIssues({ limit: ISSUE_PAGE, offset });
        if (gen !== this.#issuesGen) return;
        if (!page.length) return;
        // Merge by id so a live event that landed while this page was in
        // flight is not overwritten by the older snapshot.
        const known = new Set(this.issues.map((i) => i.id));
        const fresh = page.filter((i) => !known.has(i.id));
        if (fresh.length) this.issues = [...this.issues, ...fresh];
        if (page.length < ISSUE_PAGE) return;
      }
    } catch {
      // The next SSE reconnect reloads everything.
    }
  }

  async loadSettings() {
    const [projects, repos, cycles] = await Promise.all([
      api.listProjects(),
      api.listRepos(),
      api.listCycles(),
    ]);
    this.projects = projects;
    this.repos = repos;
    this.cycles = cycles;
  }

  #connect() {
    this.#unsub?.();
    this.#unsub = api.subscribe(
      (ev) => void this.#onEvent(ev),
      (live) => {
        // The server's bus has no replay, so anything broadcast while the
        // stream was down is gone: reload everything when it comes back.
        if (live && !this.connected && this.#everLive) {
          void this.loadAll().catch(() => {});
          void this.loadInbox();
          void this.loadAgents();
        }
        if (live) this.#everLive = true;
        this.connected = live;
      },
    );
  }

  async #onEvent(ev: BoardEvent) {
    switch (ev.type) {
      case "card.created":
      case "card.updated":
      case "card.moved":
      case "card.assigned": {
        try {
          const fresh = await api.getIssue(ev.key);
          this.#upsert(fresh);
          this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
        } catch {
          /* deleted between event and fetch */
        }
        break;
      }
      case "agent.claimed":
        void this.loadAgents();
        this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
        break;
      case "agent.released": {
        void this.loadAgents();
        // A release can leave a handoff note and refresh the session links, so
        // refetch the issue rather than only bumping the drawer tick.
        try {
          const fresh = await api.getIssue(ev.key);
          this.#upsert(fresh);
        } catch {
          /* deleted between event and fetch */
        }
        this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
        break;
      }
      case "agent.removed":
      case "agent.paused":
      case "agent.resumed":
      case "agent.hidden":
      case "agent.unhidden":
      case "agent.orchestrator":
        void this.loadAgents();
        break;
      case "agent.updated": {
        // The event carries the claim, so a heartbeat patches one entry instead
        // of refetching both agent lists for every heartbeat (BB-128).
        const claim = ev.claim;
        if (!claim) {
          void this.loadAgents();
          this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
          break;
        }
        const idx = this.agents.findIndex((a) => a.id === claim.id);
        if (idx === -1) {
          this.agents = [...this.agents, claim];
          this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
          break;
        }
        // Only refetch the open drawer's comments/activity when something it
        // shows changed; a bare heartbeat just moves last_heartbeat.
        if (claimVisibleChange(this.agents[idx], claim)) {
          this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
        }
        this.agents[idx] = claim;
        break;
      }
      case "review.updated":
        this.reviewTick++;
        break;
      case "chat.message":
        this.chatRoom = ev.room_id;
        this.chatTick++;
        this.#scheduleChatPushes();
        break;
      case "chat.room.deleted":
        this.chatRoom = null;
        this.chatTick++;
        break;
      case "card.deleted":
        this.issues = this.issues.filter((i) => i.key !== ev.key);
        if (this.selectedKey === ev.key) this.selectedKey = null;
        break;
      case "cycle.started":
      case "cycle.ended":
        this.cycles = await api.listCycles();
        break;
      case "sync.drift":
        this.flash(`Sync drift on ${ev.key}: ${ev.field} (${ev.winner} won)`, "info");
        break;
      case "notified":
        if (ev.user_id === this.me?.id) {
          const before = this.notifications.map((n) => n.id);
          await this.loadInbox();
          const fresh = this.notifications.find((n) => !before.includes(n.id) && !n.read);
          if (fresh && "Notification" in window && Notification.permission === "granted") {
            try {
              new Notification("breadboard", { body: fresh.body });
            } catch {
              /* ignore */
            }
          }
        }
        break;
    }
  }

  #upsert(issue: Issue) {
    const idx = this.issues.findIndex((i) => i.id === issue.id);
    if (idx === -1) this.issues = [...this.issues, issue];
    else this.issues[idx] = issue;
  }

  /** Coalesce a burst of chat events into one push check. */
  #scheduleChatPushes() {
    if (!this.#chatPushReady) return;
    clearTimeout(this.#chatPushTimer);
    this.#chatPushTimer = setTimeout(() => void this.notifyChatPushes(), 500);
  }

  flash(msg: string, kind: Toast["kind"] = "error", undo?: () => void) {
    const t: Toast = { id: ++this.#toastSeq, msg, kind, undo };
    this.toast = t;
    clearTimeout(this.#toastTimer);
    this.#toastTimer = setTimeout(() => {
      if (this.toast?.id === t.id) this.toast = null;
    }, undo ? 5000 : 3200);
  }

  info(msg: string, undo?: () => void) {
    this.flash(msg, "info", undo);
  }

  openPalette(status: Status | null = null) {
    this.paletteStatus = status;
    this.paletteOpen = true;
  }

  // --- mutations (optimistic) ------------------------------------------

  async move(key: string, status: Status, opts: { undoable?: boolean } = {}) {
    const issue = this.issues.find((i) => i.key === key);
    if (!issue || issue.status === status) return;
    const prev = issue.status;
    issue.status = status; // optimistic
    try {
      const updated = await api.patchIssue(key, { status });
      this.#upsert(updated);
      if (opts.undoable !== false) {
        this.info(`Moved ${key} to ${statusName(status)}`, () => void this.move(key, prev, { undoable: false }));
      }
    } catch (e) {
      issue.status = prev; // revert
      this.flash(`Couldn't move ${key}: ${errMsg(e)}`);
    }
  }

  /** Patch an issue. Resolves to the updated issue, or null on failure (the
   *  toast has already explained it); never rejects. */
  async patch(key: string, patch: IssuePatch): Promise<Issue | null> {
    try {
      const updated = await api.patchIssue(key, patch);
      this.#upsert(updated);
      return updated;
    } catch (e) {
      this.flash(`Couldn't update ${key}: ${errMsg(e)}`);
      return null;
    }
  }

  async moveProject(key: string, project: string) {
    try {
      const updated = await api.moveIssueProject(key, project);
      this.issues = this.issues.filter((i) => i.key !== key);
      this.#upsert(updated);
      if (this.selectedKey === key) this.selectedKey = updated.key;
      this.info(`${key} is now ${updated.key}`);
      return updated;
    } catch (e) {
      this.flash(`Couldn't move ${key}: ${errMsg(e)}`);
      return null;
    }
  }

  async create(input: NewIssue): Promise<Issue | null> {
    try {
      const issue = await api.createIssue(input);
      this.#upsert(issue);
      this.info(`Created ${issue.key}`);
      return issue;
    } catch (e) {
      this.flash(`Couldn't create the issue: ${errMsg(e)}`);
      return null;
    }
  }

  async remove(key: string) {
    const snapshot = this.issues;
    this.issues = this.issues.filter((i) => i.key !== key);
    if (this.selectedKey === key) this.selectedKey = null;
    try {
      await api.deleteIssue(key);
      this.info(`Deleted ${key}`);
    } catch (e) {
      this.issues = snapshot;
      this.flash(`Couldn't delete ${key}: ${errMsg(e)}`);
    }
  }

  async devLogin(login: string) {
    await fetch("/auth/dev-login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      credentials: "include",
      body: JSON.stringify({ login }),
    });
    this.auth = "loading";
    await this.init();
  }

  async logout() {
    await fetch("/auth/logout", { method: "POST", credentials: "include" }).catch(() => {});
    location.reload();
  }
}

export function statusName(s: string): string {
  return s[0].toUpperCase() + s.slice(1);
}

export function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** True when the key event came from something you can type into. */
export function isTyping(e: KeyboardEvent): boolean {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
}

/** True for a bare key press: no Ctrl, Alt or Meta held. Shift is allowed
 *  since it's part of characters like `?` and `[`. */
export function bare(e: KeyboardEvent): boolean {
  return !e.ctrlKey && !e.metaKey && !e.altKey;
}

export const store = new Store();
