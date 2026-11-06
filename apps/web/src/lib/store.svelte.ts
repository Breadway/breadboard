// Reactive app state. One module-level singleton; components read its fields
// directly (Svelte 5 deep reactivity) and call its methods to mutate.

import {
  ApiError,
  BreadboardClient,
  type AgentClaim,
  type AgentStatusSummary,
  type AgentState,
  type BoardEvent,
  type ClaimUpdate,
  type ClaimUpsert,
  type Cycle,
  type EpicView,
  type Notification,
  type Issue,
  type IssuePatch,
  type NewIssue,
  type Project,
  type RepoView,
  type Status,
  type User,
} from "@breadboard/api-client";

export const api = new BreadboardClient();

type AuthState = "loading" | "in" | "out" | "error";
export type View = "board" | "backlog" | "roadmap" | "agents" | "inbox" | "settings";

/** A claim with no heartbeat for this long reads as stale (never auto-released). */
export const CLAIM_STALE_SECS = 300;

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

export interface Toast {
  id: number;
  msg: string;
  kind: "info" | "error";
  undo?: () => void;
}

class Store {
  me = $state<User | null>(null);
  auth = $state<AuthState>("loading");
  authError = $state<string | null>(null);
  issues = $state<Issue[]>([]);
  projects = $state<Project[]>([]);
  cycles = $state<Cycle[]>([]);
  epics = $state<EpicView[]>([]);
  connected = $state(false);
  toast = $state<Toast | null>(null);

  repos = $state<RepoView[]>([]);
  notifications = $state<Notification[]>([]);
  unread = $state(0);

  agents = $state<AgentClaim[]>([]);
  agentLine = $state<string>("");

  // Bumped whenever a live event touches an issue, so the open issue panel
  // knows to refetch its comments and activity.
  issueTick = $state<{ key: string; n: number }>({ key: "", n: 0 });

  // UI state
  view = $state<View>("board");
  selectedKey = $state<string | null>(null);
  paletteOpen = $state(false);
  paletteStatus = $state<Status | null>(null);
  filterProject = $state<string>("");
  filterCycle = $state<number | 0>(0);
  filterEpic = $state<number | 0>(0);
  filterMine = $state(false);

  #unsub: (() => void) | null = null;
  #everLive = false;
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

  get filtered(): Issue[] {
    return this.issues.filter((i) => {
      // A PR that closes a real issue collapses into that issue's card
      // instead of showing as a second, seemingly-unrelated card; its
      // status surfaces there as a small badge instead (see Card.svelte).
      if (i.type === "pr" && i.links.some((l) => l.kind === "closes")) return false;
      // Once a cycle (release) ships and is closed, its done work archives
      // off the default board/backlog view instead of piling up forever;
      // still there if you explicitly pick that cycle from the filter.
      if (!this.filterCycle && i.status === "done" && i.cycle_id) {
        const cycle = this.cycles.find((c) => c.id === i.cycle_id);
        if (cycle?.closed) return false;
      }
      if (this.filterProject) {
        const p = this.projects.find((p) => p.id === i.project_id);
        if (!p || p.key !== this.filterProject) return false;
      }
      if (this.filterCycle && i.cycle_id !== this.filterCycle) return false;
      if (this.filterEpic && i.epic_id !== this.filterEpic) return false;
      if (this.filterMine && i.assignee_id !== this.me?.id) return false;
      return true;
    });
  }

  byStatus(status: Status): Issue[] {
    return this.filtered
      .filter((i) => i.status === status)
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
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
    void this.loadAgents();
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

  get agentsByIssue(): Map<number, AgentClaim> {
    return new Map(this.agents.map((a) => [a.issue_id, a]));
  }

  get activeAgents(): number {
    return this.agents.length;
  }

  claimForIssue(issueId: number): AgentClaim | undefined {
    return this.agents.find((a) => a.issue_id === issueId);
  }

  async loadAgents() {
    try {
      const [claims, status] = await Promise.all([api.listAgents(), api.agentStatus()]);
      this.agents = claims;
      this.agentLine = status.line;
    } catch {
      /* ignore */
    }
  }

  async claim(key: string, body: ClaimUpsert): Promise<boolean> {
    try {
      const c = await api.claimIssue(key, body);
      this.agents = [...this.agents.filter((a) => a.issue_id !== c.issue_id), c];
      this.info(`Claimed ${c.key} as ${c.agent_name}`);
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

  async heartbeat(key: string, body: ClaimUpdate): Promise<boolean> {
    try {
      const c = await api.heartbeatClaim(key, body);
      this.agents = [...this.agents.filter((a) => a.issue_id !== c.issue_id), c];
      return true;
    } catch (e) {
      this.flash(`Couldn't update claim on ${key}: ${errMsg(e)}`);
      return false;
    }
  }

  async release(key: string, undoable = true): Promise<boolean> {
    const snapshot = this.agents;
    const claim = snapshot.find((a) => a.key === key);
    this.agents = this.agents.filter((a) => a.key !== key);
    try {
      await api.releaseClaim(key);
      this.info(
        `Released ${key}`,
        undoable && claim
          ? () => void this.reclaim(claim)
          : undefined,
      );
      return true;
    } catch (e) {
      this.agents = snapshot;
      this.flash(`Couldn't release ${key}: ${errMsg(e)}`);
      return false;
    }
  }

  private async reclaim(claim: AgentClaim): Promise<void> {
    await this.claim(claim.key, {
      agent_name: claim.agent_name,
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
    const [issues, projects, cycles, epics] = await Promise.all([
      api.listIssues(),
      api.listProjects(),
      api.listCycles(),
      api.listEpics(),
    ]);
    this.issues = issues;
    this.projects = projects;
    this.cycles = cycles;
    this.epics = epics;
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
      case "agent.updated":
      case "agent.released":
        void this.loadAgents();
        this.issueTick = { key: ev.key, n: this.issueTick.n + 1 };
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
