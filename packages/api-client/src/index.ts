// TypeScript client for breadboard-server.
//
// Milestone 1: health + the SSE event stream. Issue/board/cycle calls are added
// alongside the server's REST routes (and can later be generated from the
// OpenAPI spec — see the plan).

export type Status = "backlog" | "todo" | "doing" | "review" | "done";
export type Priority = "none" | "low" | "medium" | "high" | "urgent";
export type IssueType = "issue" | "pr";
export type LinkKind = "blocks" | "blocked-by" | "relates-to";

export interface Health {
  status: string;
  version: string;
  db: boolean;
}

export interface ForgejoRef {
  repo: string;
  number: number;
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
  links: { kind: LinkKind; target_id: number }[];
  forgejo_ref: ForgejoRef | null;
  created_at: string;
  updated_at: string;
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
  | { type: "sync.drift"; issue_id: number; key: string; field: string };

export class BreadboardClient {
  readonly baseUrl: string;

  constructor(baseUrl = "") {
    this.baseUrl = baseUrl.replace(/\/$/, "");
  }

  private async json<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      credentials: "include",
      ...init,
      headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
    });
    if (!res.ok) {
      throw new Error(`${res.status} ${res.statusText}: ${await res.text()}`);
    }
    return res.json() as Promise<T>;
  }

  health(): Promise<Health> {
    return this.json<Health>("/healthz");
  }

  /** Subscribe to the live event stream. Returns an unsubscribe function. */
  subscribe(onEvent: (event: BoardEvent) => void): () => void {
    const es = new EventSource(`${this.baseUrl}/events`, { withCredentials: true });
    es.onmessage = (msg) => {
      try {
        onEvent(JSON.parse(msg.data) as BoardEvent);
      } catch {
        /* ignore malformed frame */
      }
    };
    return () => es.close();
  }
}
