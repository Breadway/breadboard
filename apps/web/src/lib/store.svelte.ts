// Reactive app state. One module-level singleton; components read its fields
// directly (Svelte 5 deep reactivity) and call its methods to mutate.

import {
  ApiError,
  BreadboardClient,
  type BoardEvent,
  type Cycle,
  type Epic,
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

type AuthState = "loading" | "in" | "out";

class Store {
  me = $state<User | null>(null);
  auth = $state<AuthState>("loading");
  issues = $state<Issue[]>([]);
  projects = $state<Project[]>([]);
  cycles = $state<Cycle[]>([]);
  epics = $state<EpicView[]>([]);
  connected = $state(false);
  toast = $state<string | null>(null);

  repos = $state<RepoView[]>([]);
  notifications = $state<Notification[]>([]);
  unread = $state(0);
  bellOpen = $state(false);

  // UI state
  view = $state<"board" | "backlog" | "roadmap" | "settings">("board");
  selectedKey = $state<string | null>(null);
  paletteOpen = $state(false);
  filterProject = $state<string>("");
  filterCycle = $state<number | 0>(0);
  filterMine = $state(false);

  #unsub: (() => void) | null = null;

  get isDev() {
    return import.meta.env.DEV;
  }

  get serverBase() {
    return api.baseUrl || location.origin;
  }

  get filtered(): Issue[] {
    return this.issues.filter((i) => {
      if (this.filterProject) {
        const p = this.projects.find((p) => p.id === i.project_id);
        if (!p || p.key !== this.filterProject) return false;
      }
      if (this.filterCycle && i.cycle_id !== this.filterCycle) return false;
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

  findIssue(id: number): Issue | undefined {
    return this.issues.find((i) => i.id === id);
  }

  async init() {
    try {
      this.me = await api.me();
      this.auth = "in";
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        this.auth = "out";
        return;
      }
      throw e;
    }
    await this.loadAll();
    await this.loadInbox();
    this.#connect();
    if ("Notification" in window && Notification.permission === "default") {
      Notification.requestPermission().catch(() => {});
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
    const [projects, repos] = await Promise.all([api.listProjects(), api.listRepos()]);
    this.projects = projects;
    this.repos = repos;
  }

  #connect() {
    this.#unsub?.();
    this.#unsub = api.subscribe(
      (ev) => void this.#onEvent(ev),
      (live) => (this.connected = live),
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
        } catch {
          /* deleted between event and fetch */
        }
        break;
      }
      case "card.deleted":
        this.issues = this.issues.filter((i) => i.key !== ev.key);
        if (this.selectedKey === ev.key) this.selectedKey = null;
        break;
      case "cycle.started":
      case "cycle.ended":
        this.cycles = await api.listCycles();
        break;
      case "sync.drift":
        this.flash(`sync drift on ${ev.key} (${ev.field})`);
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

  flash(msg: string) {
    this.toast = msg;
    setTimeout(() => {
      if (this.toast === msg) this.toast = null;
    }, 3000);
  }

  // --- mutations (optimistic) ------------------------------------------

  async move(key: string, status: Status) {
    const issue = this.issues.find((i) => i.key === key);
    if (!issue || issue.status === status) return;
    const prev = issue.status;
    issue.status = status; // optimistic
    try {
      const updated = await api.patchIssue(key, { status });
      this.#upsert(updated);
    } catch (e) {
      issue.status = prev; // revert
      this.flash(`couldn't move ${key}: ${errMsg(e)}`);
    }
  }

  async patch(key: string, patch: IssuePatch) {
    try {
      const updated = await api.patchIssue(key, patch);
      this.#upsert(updated);
      return updated;
    } catch (e) {
      this.flash(`update failed: ${errMsg(e)}`);
      throw e;
    }
  }

  async moveProject(key: string, project: string) {
    try {
      const updated = await api.moveIssueProject(key, project);
      this.#upsert(updated);
      if (this.selectedKey === key) this.selectedKey = updated.key;
      return updated;
    } catch (e) {
      this.flash(`move failed: ${errMsg(e)}`);
      throw e;
    }
  }

  async create(input: NewIssue): Promise<Issue | null> {
    try {
      const issue = await api.createIssue(input);
      this.#upsert(issue);
      return issue;
    } catch (e) {
      this.flash(`create failed: ${errMsg(e)}`);
      return null;
    }
  }

  async remove(key: string) {
    const snapshot = this.issues;
    this.issues = this.issues.filter((i) => i.key !== key);
    if (this.selectedKey === key) this.selectedKey = null;
    try {
      await api.deleteIssue(key);
    } catch (e) {
      this.issues = snapshot;
      this.flash(`delete failed: ${errMsg(e)}`);
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
    await fetch("/auth/logout", { method: "POST", credentials: "include" });
    location.reload();
  }
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export const store = new Store();
