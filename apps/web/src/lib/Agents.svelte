<script lang="ts">
  import { onMount } from "svelte";
  import { fade, fly } from "svelte/transition";
  import {
    STATUSES,
    safeHttpUrl,
    type AgentClaim,
    type AgentState,
  } from "@breadboard/api-client";
  import { ago, claimStale, store, statusName } from "./store.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";
  import Ring from "./ui/Ring.svelte";
  import UsageStats from "./UsageStats.svelte";
  import ActivityPanel from "./ActivityPanel.svelte";
  import { agentColor } from "./ui/colors";

  // Tick every 15s so relative heartbeats and stale badges age live; SSE
  // pushes claim/heartbeat/release events straight into store.agents.
  let now = $state(Date.now());
  let staleOnly = $state(false);
  let showHidden = $state(false);
  let focus = $state<string | null>(null);
  let openAgentId = $state<string | null>(null);
  let copied = $state<string | null>(null);
  let panelEl: HTMLElement | undefined = $state();

  onMount(() => {
    void store.loadAgents();
    void store.loadChatRooms().catch(() => {});
    const t = setInterval(() => {
      now = Date.now();
      void store.loadAgents();
      void store.loadChatRooms().catch(() => {});
    }, 15_000);
    return () => clearInterval(t);
  });

  const claims = $derived(store.agents);
  const work = $derived(claims.filter((c) => c.mode === "exclusive"));
  const queued = $derived(claims.filter((c) => c.mode === "intent"));
  const staleCount = $derived(work.filter((c) => claimStale(c.last_heartbeat, now)).length);
  const liveCount = $derived(work.length - staleCount);
  const liveSessions = $derived(store.roster.filter((r) => r.live).length);

  const tallies = $derived.by(() => {
    const t = { active: 0, review: 0, verifying: 0, blocked: 0, done: 0 };
    for (const c of work) {
      if (c.board_status === "done") t.done++;
      else if (c.state === "implementing" || c.state === "reworking" || c.state === "planned") t.active++;
      else if (c.state === "awaiting-review" || c.state === "awaiting-ci" || c.state === "awaiting-merge") t.review++;
      else if (c.state === "verifying") t.verifying++;
      else t.blocked++;
    }
    return t;
  });

  interface Group {
    id: string;
    name: string;
    login: string | null;
    message: string | null;
    claims: AgentClaim[];
    work: AgentClaim[];
    intents: AgentClaim[];
    done: number;
    live: number;
    stale: number;
    lastHeartbeat: string | null;
    sessionId: string | null;
    host: string | null;
    pid: number | null;
    presence: "live" | "stale" | "paused";
    paused: boolean;
    pausedBy: string | null;
    pausedAt: string | null;
    note: string | null;
    state: AgentState | null;
    lastSeen: string | null;
    hidden: boolean;
    hiddenBy: string | null;
    hiddenAt: string | null;
    parentSessionId: string | null;
    orchestrator: string | null;
    orchestratorSessionId: string | null;
    /** Live exclusive claim budget for this name (per agent). */
    exclusiveLimit: number;
    /** Live exclusive claims across the orchestrator tree this name is in. */
    treeExclusive: number;
    /** Live exclusive claim budget for one orchestrator tree. */
    treeLimit: number;
  }

  // The roster is the union of sessions, profiles and claims, so a name with
  // no claim still gets a card: that is the idle-session case the human asked
  // for. The claim grid itself is unchanged.
  const allGroups = $derived.by(() => {
    const map = new Map<string, Group>();
    const ensure = (name: string): Group => {
      const id = name.trim().toLowerCase();
      let g = map.get(id);
      if (!g) {
        g = {
          id,
          name: id,
          login: null,
          message: null,
          claims: [],
          work: [],
          intents: [],
          done: 0,
          live: 0,
          stale: 0,
          lastHeartbeat: null,
          sessionId: null,
          host: null,
          pid: null,
          presence: "stale",
          paused: false,
          pausedBy: null,
          pausedAt: null,
          note: null,
          state: null,
          lastSeen: null,
          hidden: false,
          hiddenBy: null,
          hiddenAt: null,
          parentSessionId: null,
          orchestrator: null,
          orchestratorSessionId: null,
          exclusiveLimit: 10,
          treeExclusive: 0,
          treeLimit: 100,
        };
        map.set(id, g);
      }
      return g;
    };

    for (const c of claims) {
      const g = ensure(c.agent_name);
      g.claims.push(c);
      g.login ??= c.claimed_by_login;
      g.message ??= c.message;
      if (c.mode === "intent") g.intents.push(c);
      else {
        g.work.push(c);
        if (c.board_status === "done") g.done++;
        if (claimStale(c.last_heartbeat, now)) g.stale++;
        else g.live++;
      }
      if (!g.lastHeartbeat || c.last_heartbeat > g.lastHeartbeat) {
        g.lastHeartbeat = c.last_heartbeat;
        g.state = c.state;
        g.note = c.note;
      }
    }

    for (const r of store.roster) {
      const g = ensure(r.agent_name);
      g.login ??= r.login ?? null;
      g.message = r.message ?? g.message;
      g.sessionId = r.session_id ?? g.sessionId;
      g.host = r.host ?? g.host;
      g.pid = r.pid ?? g.pid;
      g.presence = r.status;
      g.paused = r.paused;
      g.pausedBy = r.paused_by_login ?? null;
      g.pausedAt = r.paused_at ?? null;
      g.note = g.note ?? r.note ?? null;
      g.state = g.state ?? r.state ?? null;
      g.lastSeen = r.last_seen ?? g.lastSeen;
      g.hidden = r.hidden;
      g.hiddenBy = r.hidden_by_login ?? null;
      g.hiddenAt = r.hidden_at ?? null;
      g.parentSessionId = r.parent_session_id ?? null;
      g.orchestrator = r.orchestrator ?? null;
      g.orchestratorSessionId = r.orchestrator_session_id ?? null;
      g.exclusiveLimit = r.exclusive_limit;
      g.treeExclusive = r.orchestrator_exclusive_count;
      g.treeLimit = r.orchestrator_exclusive_limit;
      if (r.last_seen && (!g.lastHeartbeat || r.last_seen > g.lastHeartbeat)) {
        g.lastHeartbeat = r.last_seen;
      }
    }

    for (const g of map.values()) {
      g.claims.sort((a, b) => {
        const byStatus = STATUSES.indexOf(a.board_status) - STATUSES.indexOf(b.board_status);
        return byStatus !== 0 ? byStatus : a.key.localeCompare(b.key, undefined, { numeric: true });
      });
    }
    // Sort only on claim counts and name, never on live/stale: a heartbeat
    // flips presence every few seconds and must not reorder the masonry.
    const rank = (g: Group) => (g.paused ? 1 : 0);
    return [...map.values()].sort(
      (a, b) =>
        rank(a) - rank(b) ||
        b.work.length - a.work.length ||
        b.intents.length - a.intents.length ||
        a.name.localeCompare(b.name),
    );
  });

  const claimHolders = $derived(allGroups.filter((g) => g.work.length > 0).length);
  const idleSessions = $derived(allGroups.filter((g) => g.work.length === 0).length);
  const pausedCount = $derived(allGroups.filter((g) => g.paused).length);
  const hiddenGroups = $derived(allGroups.filter((g) => g.hidden));
  const hiddenCount = $derived(hiddenGroups.length);
  const hiddenWithClaims = $derived(hiddenGroups.filter((g) => g.work.length > 0).length);

  interface AgentNode {
    group: Group;
    depth: number;
    children: AgentNode[];
    /** Direct children below the three-level cap, summarised as "N more". */
    overflow: Group[];
  }

  interface AgentSection {
    key: string;
    kind: "orchestrator" | "independent" | "unlinked";
    root: Group | null;
    rawParent: string | null;
    nodes: AgentNode[];
    directTotal: number;
    directLive: number;
    rootRendered: boolean;
  }

  const byId = $derived.by(() => {
    const map = new Map<string, Group>();
    for (const g of allGroups) map.set(g.id, g);
    return map;
  });

  // Hierarchy comes only from parent_session_id resolved against another
  // roster row's session_id. A name-only orchestrator hint, a shared host or a
  // busy neighbour never invents an edge here.
  const bySession = $derived.by(() => {
    const map = new Map<string, Group>();
    for (const g of allGroups) {
      if (g.sessionId && !map.has(g.sessionId)) map.set(g.sessionId, g);
    }
    return map;
  });

  const parentById = $derived.by(() => {
    const map = new Map<string, string>();
    for (const g of allGroups) {
      if (!g.parentSessionId) continue;
      const parent = bySession.get(g.parentSessionId);
      if (parent && parent.id !== g.id) map.set(g.id, parent.id);
    }
    return map;
  });

  const childrenById = $derived.by(() => {
    const map = new Map<string, Group[]>();
    for (const g of allGroups) {
      const parentId = parentById.get(g.id);
      if (!parentId) continue;
      const list = map.get(parentId) ?? [];
      list.push(g);
      map.set(parentId, list);
    }
    return map;
  });

  // Orchestrator -> direct subagents. Counts include hidden cards: hiding is
  // display-only, so the relationship stays honest.
  const subagentCounts = $derived.by(() => {
    const counts = new Map<string, { total: number; live: number }>();
    for (const g of allGroups) {
      const parentId = parentById.get(g.id);
      const parent = parentId ? byId.get(parentId) : null;
      if (!parent) continue;
      const count = counts.get(parent.name) ?? { total: 0, live: 0 };
      count.total++;
      if (g.presence === "live" && !g.paused) count.live++;
      counts.set(parent.name, count);
    }
    return counts;
  });

  const orchestratorCount = $derived(
    allGroups.filter((g) => (childrenById.get(g.id)?.length ?? 0) > 0).length,
  );
  const subagentCount = $derived(allGroups.filter((g) => parentById.has(g.id)).length);
  const unlinkedGroups = $derived(
    allGroups.filter((g) => !!g.parentSessionId && !parentById.has(g.id)),
  );
  const unlinkedCount = $derived(unlinkedGroups.length);

  // Focus keeps the whole subtree, not just direct children, so a nested
  // orchestrator can be inspected without losing its grandchildren.
  const focusTreeIds = $derived.by(() => {
    const ids = new Set<string>();
    if (!focus) return ids;
    const start = allGroups.find((g) => g.name === focus);
    if (!start) return ids;
    ids.add(start.id);
    const walk = (id: string, seen: Set<string>) => {
      for (const child of childrenById.get(id) ?? []) {
        if (seen.has(child.id)) continue;
        seen.add(child.id);
        ids.add(child.id);
        walk(child.id, seen);
      }
    };
    walk(start.id, new Set([start.id]));
    return ids;
  });

  const baseVisibleIds = $derived.by(() => {
    const ids = new Set<string>();
    for (const g of allGroups) {
      if (g.hidden && !showHidden && g.name !== focus) continue;
      if (focus && !focusTreeIds.has(g.id)) continue;
      if (staleOnly && g.stale === 0 && g.name !== focus) continue;
      ids.add(g.id);
    }
    return ids;
  });

  // Keep a hidden orchestrator as a structural placeholder when a visible
  // child needs it, instead of letting that child float unlabelled.
  const keptIds = $derived.by(() => {
    const keep = new Set(baseVisibleIds);
    for (const id of baseVisibleIds) {
      let parent = parentById.get(id);
      const seen = new Set<string>([id]);
      while (parent && !seen.has(parent)) {
        keep.add(parent);
        seen.add(parent);
        parent = parentById.get(parent);
      }
    }
    return keep;
  });

  function canRenderCard(g: Group): boolean {
    if (!keptIds.has(g.id)) return false;
    if (g.hidden && !showHidden && g.name !== focus) return false;
    return true;
  }

  const shownCount = $derived(
    allGroups.filter((g) => canRenderCard(g)).length,
  );
  const focusSubagentCount = $derived(focus ? Math.max(0, focusTreeIds.size - 1) : 0);

  // Sections stack in roster order. Cards inside a section keep roster order
  // too, so heartbeats and refreshes never reshuffle the tree.
  const sections = $derived.by((): AgentSection[] => {
    type Descriptor =
      | { kind: "orchestrator"; root: Group }
      | { kind: "independent" }
      | { kind: "unlinked"; raw: string };

    const descriptors: Descriptor[] = [];
    const independent: Group[] = [];
    const unlinked = new Map<string, Group[]>();
    const unlinkedDescriptors = new Map<string, Descriptor>();
    let independentDescriptor: Descriptor | null = null;

    for (const g of allGroups) {
      if (!keptIds.has(g.id)) continue;
      const parentId = parentById.get(g.id);
      if (parentId && keptIds.has(parentId)) continue;
      const rawParent = g.parentSessionId && !parentId ? g.parentSessionId : null;
      if (rawParent) {
        let descriptor = unlinkedDescriptors.get(rawParent);
        if (!descriptor) {
          descriptor = { kind: "unlinked", raw: rawParent };
          unlinkedDescriptors.set(rawParent, descriptor);
          descriptors.push(descriptor);
        }
        const list = unlinked.get(rawParent) ?? [];
        list.push(g);
        unlinked.set(rawParent, list);
      } else if ((childrenById.get(g.id)?.length ?? 0) > 0) {
        descriptors.push({ kind: "orchestrator", root: g });
      } else {
        if (!independentDescriptor) {
          independentDescriptor = { kind: "independent" };
          descriptors.push(independentDescriptor);
        }
        independent.push(g);
      }
    }

    const makeNode = (
      group: Group,
      depth: number,
      path: Set<string>,
    ): AgentNode => {
      const node: AgentNode = { group, depth, children: [], overflow: [] };
      const kids = (childrenById.get(group.id) ?? []).filter(
        (child) => keptIds.has(child.id) && !path.has(child.id),
      );
      if (depth >= 3) {
        node.overflow = kids;
      } else {
        const next = new Set(path);
        next.add(group.id);
        node.children = kids.map((child) => makeNode(child, depth + 1, next));
      }
      return node;
    };

    return descriptors.map((descriptor): AgentSection => {
      if (descriptor.kind === "orchestrator") {
        const root = descriptor.root;
        const kids = childrenById.get(root.id) ?? [];
        return {
          key: `orch:${root.id}`,
          kind: "orchestrator",
          root,
          rawParent: null,
          nodes: [makeNode(root, 1, new Set([root.id]))],
          directTotal: kids.length,
          directLive: kids.filter((c) => c.presence === "live" && !c.paused).length,
          rootRendered: canRenderCard(root),
        };
      }
      if (descriptor.kind === "unlinked") {
        const groups = unlinked.get(descriptor.raw) ?? [];
        return {
          key: `unlinked:${descriptor.raw}`,
          kind: "unlinked",
          root: null,
          rawParent: descriptor.raw,
          nodes: groups.map((g) => makeNode(g, 1, new Set([g.id]))),
          directTotal: groups.length,
          directLive: 0,
          rootRendered: true,
        };
      }
      return {
        key: "independent",
        kind: "independent",
        root: null,
        rawParent: null,
        nodes: independent.map((g) => makeNode(g, 1, new Set([g.id]))),
        directTotal: independent.length,
        directLive: 0,
        rootRendered: true,
      };
    });
  });

  const selected = $derived(allGroups.find((g) => g.id === openAgentId));
  const selectedSubagents = $derived(
    selected ? subagentCounts.get(selected.name) : undefined,
  );
  const selectedDm = $derived(selected ? store.dmRoomFor(selected.name) : undefined);
  const selectedDmUnread = $derived(selectedDm?.unread ?? 0);
  const selectedDmHref = $derived(selected ? store.chatDmHref(selected.name) : "");
  const selectedDmUrl = $derived(
    selected ? new URL(selectedDmHref, location.origin).toString() : "",
  );

  function openAgent(g: Group) {
    openAgentId = g.id;
    // The DM may have been created or read since the page loaded.
    void store.loadChatRooms().catch(() => {});
  }

  /** Land on the DM with this agent; Chat opens or creates it. */
  function messageAgent(g: Group) {
    store.openAgentDm(g.name);
    closeAgent();
  }

  async function copyDmLink() {
    if (!selected) return;
    try {
      await navigator.clipboard.writeText(selectedDmUrl);
      copied = selectedDmUrl;
      setTimeout(() => {
        if (copied === selectedDmUrl) copied = null;
      }, 1500);
    } catch {
      /* the link is still selectable and copyable by hand */
    }
  }

  function focusOn(name: string | null) {
    if (!name) return;
    focus = name;
    staleOnly = false;
    closeAgent();
  }

  function jumpTo(g: Group) {
    focusOn(g.name);
    queueMicrotask(() => {
      requestAnimationFrame(() => {
        document
          .querySelector<HTMLElement>(`[data-section="orch:${g.id}"]`)
          ?.scrollIntoView({ block: "start", behavior: "smooth" });
      });
    });
  }

  function clearFocus() {
    focus = null;
  }

  function closeAgent() {
    openAgentId = null;
  }

  function openIssue(key: string) {
    store.selectedKey = key;
    openAgentId = null;
  }

  // The activity panel links chat entries back to the room and message.
  function openActivityChat(room: number, message: number | null) {
    store.view = "chat";
    store.chatFocus = { room, message };
    openAgentId = null;
  }

  function shortKey(key: string): string {
    return key.split("-").pop() ?? key;
  }

  async function copySession(id: string | null) {
    if (!id) return;
    try {
      await navigator.clipboard.writeText(id);
      copied = id;
      setTimeout(() => (copied = null), 1500);
    } catch {
      /* selection still works without the clipboard API */
    }
  }

  async function killGroup(g: Group) {
    const ok = window.confirm(
      `Kill ${g.name}?\n\nThis removes the agent completely: its claims are released, its roster card and profile disappear, and the name is freed. This cannot be undone.`,
    );
    if (ok) await store.killAgent(g.name);
  }

  async function pauseGroup(g: Group) {
    await store.pauseAgent(g.name);
  }

  async function resumeGroup(g: Group) {
    await store.resumeAgent(g.name);
  }

  async function hideGroup(g: Group) {
    await store.hideAgent(g.name);
  }

  async function unhideGroup(g: Group) {
    await store.unhideAgent(g.name);
  }

  $effect(() => {
    if (!selected) return;
    queueMicrotask(() => panelEl?.focus());
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        closeAgent();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });
</script>

{#snippet claimRow(c: (typeof claims)[number])}
  {@const stale = c.mode === "exclusive" && claimStale(c.last_heartbeat, now)}
  {@const pr = safeHttpUrl(c.pr_url)}
  <div class="row" class:stale class:intent={c.mode === "intent"}>
    <StatusGlyph status={c.board_status} size={12} />
    <button class="key" onclick={() => openIssue(c.key)} title="Open {c.key}">{c.key}</button>
    {#if c.mode === "intent"}<span class="queued-pill">queued</span>{/if}
    <span class="title" title={c.issue_title}>{c.issue_title}</span>
    <span class="state {c.state === 'blocked' || c.state === 'needs-user' ? 'bad' : c.state === 'done' ? 'ok' : c.state.startsWith('awaiting') || c.state === 'verifying' ? 'warn' : 'idle'}">
      {c.state.replace(/-/g, " ")}
    </span>
    {#if stale}
      <span class="stale-pill" title="Last heartbeat {ago(c.last_heartbeat, now)}; claim may be abandoned">
        <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
        stale
      </span>
    {/if}
    {#if pr}
      <a class="chip-pr" href={pr} target="_blank" rel="noreferrer" title="Pull request">
        <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="12" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4 5.1v5.8M12 10.9V6.5a2 2 0 0 0-2-2H7.5m1.5-2-2 2 2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
        PR{c.pr_number ? ` #${c.pr_number}` : ""}
      </a>
    {:else if c.branch}
      <span class="chip-branch" title="Branch">
        <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4.5" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="11.5" cy="6" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.5 5.1v5.8M11.5 7.6c0 2.2-2 2.6-4.6 3.3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
        {c.branch}
      </span>
    {/if}
    <span
      class="hb"
      class:stale
      title={stale
        ? `Last heartbeat ${ago(c.last_heartbeat, now)}; claim may be abandoned`
        : `Last heartbeat ${ago(c.last_heartbeat, now)} ago`}
    >
      {#if stale}
        <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
      {:else}
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 8h3l1.7-4 2.6 8 1.7-4h4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
      {/if}
      {ago(c.last_heartbeat, now)}
    </span>
    <span class="acts">
      <button class="btn sm" onclick={() => openIssue(c.key)}>Open</button>
      {#if c.claimed_by === store.me?.id}
        <button class="btn sm danger" onclick={() => void store.releaseById(c.id)}>Release</button>
      {/if}
    </span>
  </div>
{/snippet}

{#snippet agentCard(g: Group)}
  {@const ownerDiffers = g.login != null && g.login !== store.me?.login}
  {@const direct = childrenById.get(g.id) ?? []}
  {@const isOrchestrator = direct.length > 0}
  {@const parentId = parentById.get(g.id)}
  {@const parent = parentId ? byId.get(parentId) : null}
  {@const unlinkedParent = !parent && g.parentSessionId ? g.parentSessionId : null}
  {@const directLive = direct.filter((c) => c.presence === "live" && !c.paused).length}
  <div
    class="agent-card"
    class:stale={g.stale > 0}
    class:paused={g.paused}
    class:hidden={g.hidden}
    class:orchestrator={isOrchestrator}
    class:subagent-card={!!parent || !!unlinkedParent}
    style="--agent:{agentColor(g.name)}"
    data-agent={g.id}
    data-claims={g.work.length}
    data-hidden={g.hidden}
    data-role={isOrchestrator ? "orchestrator" : parent ? "subagent" : unlinkedParent ? "unlinked" : "solo"}
    data-parent={parent?.name ?? ""}
    data-direct-subagents={direct.length}
    role="button"
    tabindex="0"
    onclick={() => openAgent(g)}
    onkeydown={(e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openAgent(g);
      }
    }}
    aria-label="Open {g.name}, {isOrchestrator ? 'orchestrator, ' : parent ? 'subagent, ' : ''}{g.work.length} issues, {g.presence}{parent ? `, under ${parent.name}` : ''}{g.hidden ? ', hidden' : ''}"
  >
    <header class="acheck">
      <span class="avatar" aria-hidden="true">
        <svg viewBox="0 0 16 16"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
      </span>
      <div class="aid">
        {#if isOrchestrator}
          <span class="role-badge orch">orchestrator</span>
        {:else if parent}
          <span class="role-badge sub">subagent</span>
        {:else if unlinkedParent}
          <span class="role-badge unlinked">unlinked</span>
        {/if}
        <span class="aname">{g.name}</span>
        {#if g.message}<span class="amessage" title={g.message}>{g.message}</span>{/if}
        {#if ownerDiffers}<span class="alogin">{g.login}</span>{/if}
      </div>
      <span class="badges">
        {#if isOrchestrator}
          <span
            class="subagent-badge"
            title="{directLive} live of {direct.length} direct subagent{direct.length === 1 ? '' : 's'}"
          >{directLive} live</span>
        {/if}
        {#if g.hidden}
          <span class="hidden-badge" title="Hidden from the default view; claims and chat identity remain">hidden</span>
        {/if}
        {#if g.paused}
          <span class="paused-badge" title="Claims kept; new claims blocked until resumed">paused</span>
        {:else if g.stale > 0}
          <span class="stale-badge" title="No heartbeat for a while; these claims may be abandoned">
            <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
            {g.stale}
          </span>
        {:else if g.presence === "stale"}
          <span class="idle-badge" title="No recent heartbeat">stale</span>
        {/if}
        {#if g.intents.length}<span class="queued-badge" title="Queued intents">{g.intents.length} queued</span>{/if}
      </span>
    </header>

    {#if parent}
      <button
        class="rel-link"
        title="Jump to orchestrator {parent.name}"
        onclick={(e) => {
          e.stopPropagation();
          jumpTo(parent);
        }}
        onkeydown={(e) => e.stopPropagation()}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 12V6a2 2 0 0 1 2-2h6m-2-2 2 2-2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
        orchestrator
      </button>
    {:else if unlinkedParent}
      <span class="rel-raw" title="Parent session {unlinkedParent} is not a registered roster entry">
        parent <code>{unlinkedParent}</code>
      </span>
    {/if}

    {#if g.claims.length}
      <div class="tiles" aria-hidden="true">
        {#each g.claims as c (c.id)}
          {@const stale = c.mode === "exclusive" && claimStale(c.last_heartbeat, now)}
          <span
            class="tile t-{c.board_status}"
            class:intent={c.mode === "intent"}
            class:stale
            title="{c.key} · {c.issue_title} · {c.board_status}{c.mode === 'intent' ? ' (queued)' : ''}{stale ? ' · stale' : ''}"
          >{shortKey(c.key)}</span>
        {/each}
      </div>
    {:else}
      <div class="idle-line">
        <span class="idle-dot" class:live={g.presence === "live"}></span>
        idle session{g.sessionId ? ` · ${g.sessionId}` : ""}
      </div>
    {/if}

    <footer class="afoot">
      <span class="counts">{g.work.length} issue{g.work.length === 1 ? "" : "s"}</span>
      <span
        class="budget"
        class:warn={g.exclusiveLimit > 0 && g.work.length * 5 >= g.exclusiveLimit * 4}
        class:full={g.exclusiveLimit > 0 && g.work.length >= g.exclusiveLimit}
        title="Live exclusive claims: {g.work.length} of {g.exclusiveLimit} allowed per agent"
      >{g.work.length}/{g.exclusiveLimit}</span>
      {#if isOrchestrator || parent}
        <span
          class="budget tree"
          class:warn={g.treeLimit > 0 && g.treeExclusive * 5 >= g.treeLimit * 4}
          class:full={g.treeLimit > 0 && g.treeExclusive >= g.treeLimit}
          title="Live exclusive claims across {isOrchestrator ? "this orchestrator and its subagents" : "this agent's orchestrator tree"}: {g.treeExclusive} of {g.treeLimit}"
        >tree {g.treeExclusive}/{g.treeLimit}</span>
      {/if}
      <span class="grow"></span>
      {#if g.work.length}
        <span class="completion">
          <Ring done={g.done} total={g.work.length} color={agentColor(g.name)} size={14} />
          <b>{g.done}</b><span class="cslash">/</span>{g.work.length} done
        </span>
      {:else}
        <span class="completion muted">{g.intents.length ? "queued only" : "idle session"}</span>
      {/if}
      <span class="gtick" title={g.lastHeartbeat ?? "no heartbeat"}>
        {g.lastHeartbeat ? ago(g.lastHeartbeat, now) : "no heartbeat"}
      </span>
      {#if g.hidden}
        <button
          class="btn sm unhide-card"
          onclick={(e) => {
            e.stopPropagation();
            void unhideGroup(g);
          }}
          onkeydown={(e) => e.stopPropagation()}
        >Unhide</button>
      {/if}
    </footer>
  </div>
{/snippet}

{#snippet hiddenGroup(g: Group)}
  {@const kids = childrenById.get(g.id) ?? []}
  <div class="hidden-group" data-hidden-group={g.id}>
    <span class="role-badge orch">orchestrator</span>
    <b class="group-name">{g.name}</b>
    <span class="hidden-badge">hidden</span>
    <span class="group-meta">{kids.length} subagent{kids.length === 1 ? "" : "s"} still shown</span>
    <span class="grow"></span>
    <button class="btn ghost sm" onclick={() => focusOn(g.name)}>Focus</button>
    <button class="btn ghost sm" onclick={() => void unhideGroup(g)}>Unhide</button>
  </div>
{/snippet}

{#snippet subtree(node: AgentNode)}
  <div class="node" data-node={node.group.id} data-depth={node.depth}>
    {#if canRenderCard(node.group)}
      {@render agentCard(node.group)}
    {:else}
      {@render hiddenGroup(node.group)}
    {/if}
    {#if node.children.length}
      <div class="inset" data-inset-depth={node.depth}>
        <div class="cardgrid">
          {#each node.children as child (child.group.id)}
            {@render subtree(child)}
          {/each}
        </div>
      </div>
    {/if}
    {#if node.overflow.length}
      <div class="overflow-note" data-overflow-for={node.group.id}>
        <span>{node.overflow.length} more under <b>{node.group.name}</b></span>
        <button class="btn ghost sm" onclick={() => focusOn(node.group.name)}>View</button>
      </div>
    {/if}
  </div>
{/snippet}

<div class="wrap">
  <header class="summary">
    <div class="progress">
      <div class="ptop">
        <span class="plabel">{tallies.done} of {work.length} done</span>
        <span class="plive" class:off={!store.connected}>
          <i></i>{store.connected ? "live" : "reconnecting"}
        </span>
      </div>
      <div class="track" role="progressbar" aria-valuenow={tallies.done} aria-valuemin={0} aria-valuemax={work.length}>
        {#if work.length}
          <span class="fill" style="width:{(tallies.done / work.length) * 100}%"></span>
        {/if}
      </div>
      <span class="widget-line" title="Server roll-up, same shape as the opencode bb-status widget">{store.agentLine || "No active agent claims"}</span>
    </div>

    <div class="stats">
      <div class="stat"><b>{shownCount}</b><span>shown</span></div>
      <div class="stat" class:warn={hiddenCount > 0}><b>{hiddenCount}</b><span>hidden</span></div>
      <div class="stat" class:live={liveSessions > 0}><b>{liveSessions}</b><span>live sessions</span></div>
      <div class="stat"><b>{orchestratorCount}</b><span>orchestrators</span></div>
      <div class="stat"><b>{subagentCount}</b><span>subagents</span></div>
      <div class="stat" class:warn={unlinkedCount > 0}><b>{unlinkedCount}</b><span>unlinked</span></div>
      <div class="stat"><b>{claimHolders}</b><span>claim holders</span></div>
      <div class="stat"><b>{tallies.active}</b><span>active</span></div>
      <div class="stat"><b>{tallies.review}</b><span>review</span></div>
      <div class="stat" class:bad={tallies.blocked > 0}><b>{tallies.blocked}</b><span>blocked</span></div>
      <div class="stat" class:warn={pausedCount > 0}><b>{pausedCount}</b><span>paused</span></div>
    </div>
  </header>

  <div class="bar">
    <div class="seg">
      <button class:on={!staleOnly} onclick={() => (staleOnly = false)}>
        All <span>{shownCount} shown</span>
      </button>
      <button class:on={staleOnly} onclick={() => (staleOnly = true)}>
        Stale <span class:bad={staleCount > 0}>{staleCount}</span>
      </button>
    </div>
    <button
      class="hidden-toggle"
      class:on={showHidden}
      aria-pressed={showHidden}
      title={hiddenCount
        ? `${hiddenCount} hidden agent${hiddenCount === 1 ? "" : "s"}${hiddenWithClaims ? `, ${hiddenWithClaims} still hold claims` : ""}`
        : "No hidden agents"}
      onclick={() => (showHidden = !showHidden)}
    >
      {showHidden ? "Showing hidden" : "Show hidden"} <span>{hiddenCount}</span>
    </button>
    {#if focus}
      <span class="focus-banner">
        <span>
          Focused on <b>{focus}</b> · {focusSubagentCount}
          subagent{focusSubagentCount === 1 ? "" : "s"} in this tree
        </span>
        <button class="btn ghost sm" onclick={clearFocus}>Clear</button>
      </span>
    {/if}
    <span class="legend" aria-label="Issue status colours">
      {#each STATUSES as s (s)}
        <span class="lg"><i class="dot t-{s}"></i>{statusName(s)}</span>
      {/each}
      <span class="lg"><i class="dot intent-dot"></i>queued</span>
    </span>
    <span class="grow"></span>
    <span class="live-note">
      {shownCount} shown · {hiddenCount} hidden{#if hiddenWithClaims} ({hiddenWithClaims} still hold{hiddenWithClaims === 1 ? "s" : ""} claims){/if}
      · {liveSessions} live sessions · {liveCount} claims heartbeating · {idleSessions} idle
    </span>
    <button class="btn ghost sm" onclick={() => void store.loadAgents()}>Refresh</button>
  </div>

  {#if !sections.length}
    <div class="empty">
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <rect x="4" y="7" width="16" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path d="M12 3v4M8.5 12.5h.01M15.5 12.5h.01M9.5 16h5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
      </svg>
      <p>
        {focus
          ? `No agents are linked to ${focus}.`
          : staleOnly
            ? "No stale claims. Every agent heartbeated in the last 5 minutes."
            : hiddenCount > 0 && !showHidden
              ? `All ${hiddenCount} agent${hiddenCount === 1 ? " is" : "s are"} hidden. Use Show hidden to reveal them.`
              : "No agent is working an issue right now."}
      </p>
      <p class="hint">
        Agents claim with <code>breadboard claim KEY --agent NAME</code> or the <code>claim_issue</code> MCP tool.
        Claims tint the card on the board until released.
      </p>
    </div>
  {:else}
    <div class="sections">
      {#each sections as section (section.key)}
        <section
          class="group"
          class:orch-section={section.kind === "orchestrator"}
          class:unlinked-section={section.kind === "unlinked"}
          data-section={section.key}
          data-kind={section.kind}
          data-orchestrator={section.root?.name ?? ""}
        >
          <header class="group-head">
            {#if section.kind === "orchestrator" && section.root}
              {@const root = section.root}
              <span class="role-badge orch">orchestrator</span>
              <b class="group-name" title={root.name}>{root.name}</b>
              {#if !section.rootRendered}<span class="hidden-badge">hidden</span>{/if}
              {#if root.paused}<span class="paused-badge" title="Claims kept; new claims blocked until resumed">paused</span>{/if}
              <span class="group-meta">
                {section.directTotal} subagent{section.directTotal === 1 ? "" : "s"} · {section.directLive} live
              </span>
              <span class="grow"></span>
              {#if !section.rootRendered}
                <button class="btn ghost sm" onclick={() => void unhideGroup(root)}>Unhide</button>
              {/if}
              <button class="btn ghost sm" onclick={() => focusOn(root.name)}>Focus</button>
            {:else if section.kind === "unlinked"}
              <span class="role-badge unlinked">unlinked</span>
              <b class="group-name">No registered parent</b>
              <code class="raw-parent" title="Parent session {section.rawParent}">{section.rawParent}</code>
              <span class="group-meta">
                {section.nodes.length} agent{section.nodes.length === 1 ? "" : "s"} · that session is not on the roster
              </span>
            {:else}
              <b class="group-name">Independent agents</b>
              <span class="group-meta">{section.nodes.length} with no parent session</span>
            {/if}
          </header>

          {#if section.kind === "orchestrator" && section.root}
            {@const root = section.root}
            {#if section.rootRendered}
              <div class="root-card">{@render agentCard(root)}</div>
            {/if}
            {#if section.nodes[0]?.children.length}
              <div class="inset root-inset">
                <div class="cardgrid">
                  {#each section.nodes[0].children as child (child.group.id)}
                    {@render subtree(child)}
                  {/each}
                </div>
              </div>
            {/if}
          {:else}
            <div class="cardgrid">
              {#each section.nodes as node (node.group.id)}
                {@render subtree(node)}
              {/each}
            </div>
          {/if}
        </section>
      {/each}
    </div>
  {/if}
</div>

{#if selected}
  <div class="scrim" role="presentation" transition:fade={{ duration: 120 }} onclick={(e) => e.target === e.currentTarget && closeAgent()}>
    <div
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-label="Agent {selected.name}"
      tabindex="-1"
      bind:this={panelEl}
      style="--agent:{agentColor(selected.name)}"
      transition:fly={{ y: 8, duration: 160 }}
    >
      <header class="mhead">
        <span class="avatar big" aria-hidden="true">
          <svg viewBox="0 0 16 16"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
        </span>
        <div class="mid">
          <div class="mname">
            {selected.name}
            {#if selected.login != null && selected.login !== store.me?.login}<span class="alogin">{selected.login}</span>{/if}
            {#if selected.hidden}<span class="hidden-badge">hidden</span>{/if}
            {#if selected.paused}<span class="paused-badge">paused</span>{/if}
          </div>
          {#if selected.message}<p class="mmessage">{selected.message}</p>{/if}
          <div class="msub">
            {selected.presence}
            {#if selected.hidden}· hidden{/if}
            {#if selected.work.length}· {selected.work.length} exclusive{/if}
            {#if selected.intents.length}· {selected.intents.length} queued{/if}
            {#if selected.stale}· <span class="bad">{selected.stale} stale claims</span>{/if}
            · {selected.lastSeen ? `last seen ${ago(selected.lastSeen, now)}` : "no session heartbeat"}
          </div>
        </div>
        {#if selected.work.length}
          <span class="completion big">
            <Ring done={selected.done} total={selected.work.length} color={agentColor(selected.name)} size={18} />
            <b>{selected.done}</b><span class="cslash">/</span>{selected.work.length} done
          </span>
        {/if}
        <button
          class="btn sm message-agent"
          title="Open the DM with {selected.name}"
          onclick={() => messageAgent(selected)}
        >Message</button>
        {#if selected.hidden}
          <button class="btn sm" onclick={() => void unhideGroup(selected)}>Unhide</button>
        {:else}
          <button class="btn sm" onclick={() => void hideGroup(selected)}>Hide</button>
        {/if}
        {#if selected.paused}
          <button class="btn sm" onclick={() => void resumeGroup(selected)}>Resume</button>
        {:else}
          <button class="btn sm" onclick={() => void pauseGroup(selected)}>Pause</button>
        {/if}
        <button class="btn sm danger" onclick={() => void killGroup(selected)}>Kill</button>
        <button class="close" aria-label="Close" onclick={closeAgent}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
        </button>
      </header>

      <div class="session">
        <div class="srow chat-row" data-dm-agent={selected.name} data-dm-unread={selectedDmUnread}>
          <span class="slabel">chat</span>
          <a
            class="dm-link"
            href={selectedDmHref}
            title="Open the DM with {selected.name}"
            onclick={(e) => {
              e.preventDefault();
              messageAgent(selected);
            }}
          >{selectedDmHref}</a>
          <span
            class="dm-unread"
            class:on={selectedDmUnread > 0}
            title={selectedDmUnread
              ? `${selectedDmUnread} unread message${selectedDmUnread === 1 ? "" : "s"} from ${selected.name}`
              : "No unread messages"}
          >{selectedDmUnread ? `${selectedDmUnread} unread` : "no unread"}</span>
          <button class="btn ghost sm" onclick={() => void copyDmLink()}>
            {copied === selectedDmUrl ? "Copied" : "Copy link"}
          </button>
        </div>
        <div class="srow">
          <span class="slabel">session</span>
          {#if selected.sessionId}
            <code class="sid" title="Select and copy">{selected.sessionId}</code>
            <button class="btn ghost sm" onclick={() => void copySession(selected.sessionId)}>
              {copied === selected.sessionId ? "Copied" : "Copy"}
            </button>
          {:else}
            <span class="muted">not captured yet</span>
          {/if}
        </div>
        <div class="srow">
          <span class="slabel">host</span><span>{selected.host ?? "unknown"}</span>
          <span class="slabel">pid</span><span>{selected.pid ?? "unknown"}</span>
          <span class="slabel">last seen</span>
          <span>{selected.lastSeen ? ago(selected.lastSeen, now) : "never"}</span>
        </div>
        {#if selected.orchestrator}
          <div class="srow">
            <span class="slabel">orchestrator</span>
            <button class="linklike" onclick={() => focusOn(selected.orchestrator)}>
              {selected.orchestrator}
            </button>
          </div>
        {:else if selected.parentSessionId}
          <div class="srow">
            <span class="slabel">parent session</span>
            <code class="sid" title="Parent session is not registered yet">{selected.parentSessionId}</code>
          </div>
        {/if}
        {#if selectedSubagents?.total}
          <div class="srow">
            <span class="slabel">subagents</span>
            <span>{selectedSubagents.live} live of {selectedSubagents.total}</span>
          </div>
        {/if}
        <div class="srow">
          <span class="slabel">claims</span>
          <span>{selected.work.length} of {selected.exclusiveLimit} exclusive</span>
          {#if selected.orchestrator || selectedSubagents?.total}
            <span class="slabel">tree</span>
            <span>{selected.treeExclusive} of {selected.treeLimit}</span>
          {/if}
        </div>
        {#if selected.hidden}
          <div class="hidden-note">
            Hidden{selected.hiddenBy ? ` by ${selected.hiddenBy}` : ""}{selected.hiddenAt ? ` ${ago(selected.hiddenAt, now)}` : ""}.
            Claims, chat identity and history are untouched.
          </div>
        {/if}
        {#if selected.paused}
          <div class="paused-note">
            Paused{selected.pausedBy ? ` by ${selected.pausedBy}` : ""}{selected.pausedAt ? ` ${ago(selected.pausedAt, now)}` : ""}.
            Claims are kept; new claims are blocked until resumed.
          </div>
        {/if}
      </div>

      <UsageStats agent={selected.name} settingsEnabled />

      <div class="mbody">
        {#if selected.claims.length}
          {#each selected.claims as c (c.id)}
            {@render claimRow(c)}
          {/each}
        {:else}
          <div class="no-claims">No claims. This is an idle session on the roster.</div>
        {/if}
        <ActivityPanel agent={selected.name} onOpenIssue={openIssue} onOpenChat={openActivityChat} />
      </div>
    </div>
  </div>
{/if}

<style>
  .wrap {
    height: 100%;
    overflow-y: auto;
    padding: 18px 20px 36px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* --- summary ------------------------------------------------------- */
  .summary {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 20px;
    align-items: center;
    padding: 14px 16px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: linear-gradient(180deg, var(--surface), var(--sunk));
  }
  .progress {
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-width: 0;
  }
  .ptop {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .plabel {
    font: 650 15px/1 var(--font-display);
    letter-spacing: -0.01em;
  }
  .plive {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: 500 10px/1 var(--font-mono);
    color: var(--ok);
  }
  .plive i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 16%, transparent);
    animation: pulse 2.4s ease-in-out infinite;
  }
  .plive.off {
    color: var(--ink-3);
  }
  .plive.off i {
    animation: none;
    box-shadow: none;
  }
  @keyframes pulse {
    50% { box-shadow: 0 0 0 5px transparent; }
  }
  .track {
    height: 6px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    border-radius: 99px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--ok) 75%, var(--accent)), var(--ok));
    transition: width 0.3s var(--ease);
  }
  .widget-line {
    font: 400 10.5px/1.4 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .stat {
    min-width: 54px;
    padding: 7px 9px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border: 1px solid var(--line);
    text-align: center;
  }
  .stat b {
    display: block;
    font: 650 17px/1 var(--font-display);
    font-variant-numeric: tabular-nums;
  }
  .stat span {
    display: block;
    margin-top: 4px;
    font: 500 9.5px/1 var(--font-mono);
    letter-spacing: 0.04em;
    color: var(--ink-3);
  }
  .stat.bad {
    border-color: color-mix(in srgb, var(--danger) 40%, transparent);
    background: color-mix(in srgb, var(--danger) 9%, transparent);
  }
  .stat.bad b {
    color: var(--danger);
  }
  .stat.live b {
    color: var(--ok);
  }
  .stat.warn {
    border-color: color-mix(in srgb, var(--status-review) 38%, transparent);
    background: color-mix(in srgb, var(--status-review) 8%, transparent);
  }
  .stat.warn b {
    color: var(--status-review);
  }
  /* --- bar ----------------------------------------------------------- */
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .seg {
    display: inline-flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--sunk);
    border: 1px solid var(--line);
  }
  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    background: none;
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 500;
    color: var(--ink-3);
  }
  .seg button.on {
    background: var(--raise);
    color: var(--ink);
  }
  .seg span {
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .seg span.bad {
    color: var(--danger);
  }
  .hidden-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--sunk);
    padding: 5px 10px;
    font: 500 12px/1 var(--font-ui);
    color: var(--ink-3);
    cursor: pointer;
  }
  .hidden-toggle.on {
    background: color-mix(in srgb, var(--status-review) 14%, var(--raise));
    border-color: color-mix(in srgb, var(--status-review) 45%, var(--line));
    color: var(--ink);
  }
  .hidden-toggle span {
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .focus-banner {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px 4px 10px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--line));
    background: color-mix(in srgb, var(--accent) 9%, transparent);
    font: 400 11px/1.3 var(--font-ui);
    color: var(--ink-dim);
  }
  .focus-banner b {
    color: var(--ink);
    font-weight: 650;
  }
  .focus-banner .btn {
    height: 22px;
  }
  .legend {
    display: flex;
    align-items: center;
    gap: 9px;
    flex-wrap: wrap;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .lg {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    display: inline-block;
  }
  .intent-dot {
    background: transparent;
    border: 1px dashed var(--ink-3);
  }
  .live-note {
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .grow {
    flex: 1;
  }
  .btn.sm {
    height: 26px;
    padding: 0 9px;
    font-size: 11.5px;
  }

  /* --- hierarchy sections ---------------------------------------------- */
  /* Sections stack in roster order; masonry only applies inside one section,
     so a heartbeat cannot reshuffle the tree. */
  .sections {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    padding: 10px 12px 12px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: color-mix(in srgb, var(--surface) 55%, var(--sunk));
  }
  .group.orch-section {
    border-color: color-mix(in srgb, var(--accent) 26%, var(--line));
    background: linear-gradient(
      180deg,
      color-mix(in srgb, var(--accent) 5%, var(--surface)),
      var(--surface) 180px
    );
  }
  .group.unlinked-section {
    border-style: dashed;
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .group-name {
    max-width: 46ch;
    font: 650 13px/1.2 var(--font-display);
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group-meta {
    font: 400 10px/1.4 var(--font-mono);
    color: var(--ink-3);
  }
  .raw-parent {
    max-width: 100%;
    padding: 2px 6px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background: var(--raise);
    font: 400 10px/1.45 var(--font-mono);
    color: var(--ink-dim);
    overflow-wrap: anywhere;
    word-break: break-all;
  }
  .cardgrid {
    columns: 240px;
    column-gap: 10px;
    column-fill: balance;
  }
  .node {
    break-inside: avoid;
  }
  .root-card {
    break-inside: avoid;
  }
  /* The connector line is the relationship: children read as nested under the
     card above instead of as unrelated cards with a truncated chip. */
  .inset {
    margin-left: 10px;
    padding-left: 12px;
    border-left: 2px solid color-mix(in srgb, var(--accent) 30%, var(--line));
  }
  .root-inset {
    margin-left: 14px;
  }
  .hidden-group {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-width: 0;
    padding: 9px 11px;
    border: 1px dashed color-mix(in srgb, var(--status-review) 45%, var(--line));
    border-radius: 10px;
    background: color-mix(in srgb, var(--status-review) 4%, var(--surface));
  }
  .overflow-note {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin: 0 0 10px;
    padding: 7px 10px;
    border: 1px dashed var(--line-2);
    border-radius: 8px;
    font: 400 10.5px/1.4 var(--font-mono);
    color: var(--ink-3);
  }
  .overflow-note b {
    color: var(--ink-dim);
    font-weight: 600;
  }
  .role-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 700 8.5px/1 var(--font-mono);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    padding: 3px 6px;
    border-radius: 4px;
  }
  .role-badge.orch {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
  }
  .role-badge.sub {
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 6%, transparent);
  }
  .role-badge.unlinked {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 11%, transparent);
  }
  .agent-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 100%;
    margin: 0 0 10px;
    padding: 11px 12px 9px;
    border-radius: 12px;
    border: 1px solid var(--line);
    background: var(--surface);
    text-align: left;
    cursor: pointer;
    break-inside: avoid;
    font: inherit;
    color: inherit;
    transition: border-color 0.15s, background-color 0.15s, transform 0.15s;
  }
  .agent-card:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--agent) 70%, var(--ink));
    outline-offset: 2px;
  }
  .agent-card:hover {
    background: var(--raise);
    border-color: color-mix(in srgb, var(--agent) 45%, var(--line));
    transform: translateY(-1px);
  }
  .agent-card.stale {
    border-color: color-mix(in srgb, var(--danger) 32%, var(--line));
  }
  .agent-card.paused {
    border-color: color-mix(in srgb, var(--status-doing) 55%, var(--line));
    background: color-mix(in srgb, var(--status-doing) 5%, var(--surface));
  }
  .agent-card.hidden {
    border-style: dashed;
    border-color: color-mix(in srgb, var(--status-review) 45%, var(--line));
    background: color-mix(in srgb, var(--status-review) 3%, var(--surface));
  }
  .agent-card.orchestrator {
    border-left: 3px solid var(--agent);
    background: color-mix(in srgb, var(--agent) 5%, var(--surface));
  }
  .agent-card.subagent-card {
    background: color-mix(in srgb, var(--sunk) 45%, var(--surface));
  }
  .acheck {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .avatar {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    flex: none;
    border-radius: 8px;
    color: var(--agent);
    background: color-mix(in srgb, var(--agent) 13%, transparent);
    border: 1px solid color-mix(in srgb, var(--agent) 30%, transparent);
  }
  .avatar svg {
    width: 15px;
    height: 15px;
  }
  .avatar.big {
    width: 32px;
    height: 32px;
    border-radius: 9px;
  }
  .avatar.big svg {
    width: 18px;
    height: 18px;
  }
  .aid {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .aid .role-badge {
    align-self: flex-start;
  }
  .aname {
    font-size: 13px;
    font-weight: 650;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .alogin {
    font: 400 9.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .amessage {
    font-size: 11px;
    line-height: 1.35;
    color: var(--ink-dim);
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .rel-link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    align-self: flex-start;
    max-width: 100%;
    padding: 3px 6px;
    border: 1px solid color-mix(in srgb, var(--status-review) 30%, var(--line));
    border-radius: 5px;
    background: color-mix(in srgb, var(--status-review) 8%, transparent);
    font: 400 9.5px/1.2 var(--font-mono);
    color: var(--status-review);
    cursor: pointer;
  }
  .rel-link:hover {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--line));
    color: var(--ink);
  }
  .rel-link svg {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .rel-raw {
    display: block;
    align-self: stretch;
    font: 400 9.5px/1.5 var(--font-mono);
    color: var(--ink-3);
    overflow-wrap: anywhere;
    word-break: break-all;
  }
  .rel-raw code {
    font: inherit;
    color: var(--ink-dim);
  }
  .mmessage {
    margin: 3px 0 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--ink-dim);
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .badges {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
    flex-wrap: wrap;
    flex: none;
    max-width: 64%;
  }
  .stale-badge,
  .hidden-badge,
  .paused-badge,
  .idle-badge,
  .queued-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 3px 6px;
    border-radius: 4px;
  }
  .stale-badge {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 11%, transparent);
  }
  .paused-badge {
    color: var(--status-doing);
    background: color-mix(in srgb, var(--status-doing) 16%, transparent);
    border: 1px solid color-mix(in srgb, var(--status-doing) 40%, transparent);
  }
  .hidden-badge {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--status-review) 35%, transparent);
  }
  .subagent-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 3px 6px;
    border-radius: 4px;
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--ok) 30%, transparent);
  }
  .idle-badge {
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 7%, transparent);
  }
  .queued-badge {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 12%, transparent);
  }
  .tiles {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    break-inside: avoid;
  }
  .idle-line {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 32px;
    font: 400 10.5px/1.4 var(--font-mono);
    color: var(--ink-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .idle-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
    background: var(--ink-3);
  }
  .idle-dot.live {
    background: var(--ok);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 15%, transparent);
  }
  .tile {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    font: 600 10px/1 var(--font-mono);
    color: #15110c;
    background: var(--status-backlog);
    transition: transform 0.12s var(--ease);
  }
  .agent-card:hover .tile {
    transform: scale(1.04);
  }
  .t-backlog { background: var(--status-backlog); }
  .t-todo { background: var(--status-todo); }
  .t-doing { background: var(--status-doing); }
  .t-review { background: var(--status-review); }
  .t-done { background: var(--status-done); }
  .tile.intent {
    background: transparent;
    border: 1.5px dashed color-mix(in srgb, var(--status-review) 75%, transparent);
    color: var(--ink-dim);
  }
  .tile.stale {
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--danger) 65%, transparent);
  }

  .afoot {
    display: flex;
    align-items: center;
    gap: 7px;
    padding-top: 2px;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .counts {
    color: var(--ink-3);
  }
  .budget {
    padding: 1px 5px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--ink-3) 35%, transparent);
    font-variant-numeric: tabular-nums;
    color: var(--ink-3);
  }
  .budget.tree {
    border-style: dashed;
  }
  .budget.warn {
    border-color: color-mix(in srgb, #d69e2e 70%, transparent);
    color: #d69e2e;
  }
  .budget.full {
    border-color: color-mix(in srgb, var(--danger) 70%, transparent);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    color: var(--danger);
  }
  .completion {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 500 10.5px/1 var(--font-mono);
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
  }
  .completion b {
    color: var(--ink);
    font-weight: 600;
  }
  .completion .cslash {
    color: var(--ink-3);
  }
  .completion.big {
    font-size: 11.5px;
    margin-right: 8px;
  }
  .completion.muted {
    color: var(--ink-3);
    font-style: italic;
  }
  .gtick {
    color: var(--ink-3);
  }

  /* --- modal ---------------------------------------------------------- */
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 55;
    display: grid;
    place-items: center;
    padding: 24px;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(2px);
  }
  .modal {
    width: min(820px, 100%);
    max-height: min(80vh, 760px);
    display: flex;
    flex-direction: column;
    border-radius: 14px;
    border: 1px solid var(--line-2);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    outline: none;
    overflow: hidden;
  }
  .mhead {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--line);
    background: linear-gradient(90deg, color-mix(in srgb, var(--agent) 9%, transparent), transparent 55%);
  }
  .mid {
    min-width: 0;
  }
  .mname {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font: 650 16px/1.2 var(--font-display);
  }
  .msub {
    margin-top: 3px;
    font: 400 10.5px/1.3 var(--font-mono);
    color: var(--ink-3);
  }
  .msub .bad {
    color: var(--danger);
  }
  .close {
    margin-left: 4px;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    flex: none;
    border: 0;
    border-radius: 7px;
    background: none;
    color: var(--ink-3);
  }
  .close:hover {
    background: color-mix(in srgb, var(--ink) 7%, transparent);
    color: var(--ink);
  }
  .close svg {
    width: 14px;
    height: 14px;
  }
  .session {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 9px 14px;
    border-bottom: 1px solid var(--line);
    background: color-mix(in srgb, var(--ink) 2.5%, transparent);
    font: 400 10.5px/1.55 var(--font-mono);
    color: var(--ink-dim);
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .slabel {
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .sid {
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--raise);
    border: 1px solid var(--line);
    color: var(--ink);
    user-select: all;
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--ink-3);
  }
  .linklike {
    border: 0;
    background: none;
    padding: 0;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
  }
  .linklike:hover {
    text-decoration: underline;
  }
  .dm-link {
    color: var(--accent);
    text-decoration: none;
    overflow-wrap: anywhere;
  }
  .dm-link:hover {
    text-decoration: underline;
  }
  .dm-unread {
    padding: 1px 6px;
    border-radius: 99px;
    border: 1px solid var(--line-2);
    color: var(--ink-3);
    font: 500 9.5px/1.5 var(--font-mono);
  }
  .dm-unread.on {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .paused-note {
    color: var(--status-doing);
    font-size: 11px;
  }
  .hidden-note {
    color: var(--status-review);
    font-size: 11px;
  }
  .no-claims {
    padding: 22px 16px;
    text-align: center;
    font-size: 12.5px;
    color: var(--ink-3);
  }
  .mbody {
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 14px;
    min-width: 0;
    border-top: 1px solid color-mix(in srgb, var(--ink) 5%, transparent);
  }
  .row:first-child {
    border-top: 0;
  }
  .row:hover {
    background: color-mix(in srgb, var(--ink) 3%, transparent);
  }
  .row.intent {
    background: color-mix(in srgb, var(--surface) 92%, var(--bg));
  }
  .key {
    border: 0;
    background: none;
    padding: 0;
    flex: none;
    font: 600 10.5px/1 var(--font-mono);
    color: var(--accent);
    cursor: pointer;
  }
  .key:hover {
    text-decoration: underline;
  }
  .title {
    flex: 1;
    min-width: 0;
    font-size: 12.5px;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .state {
    flex: none;
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    padding: 3px 6px;
    border-radius: 4px;
  }
  .state.idle { color: var(--ink-dim); background: color-mix(in srgb, var(--ink) 7%, transparent); }
  .state.warn { color: var(--status-review); background: color-mix(in srgb, var(--status-review) 13%, transparent); }
  .state.ok { color: var(--ok); background: color-mix(in srgb, var(--ok) 13%, transparent); }
  .state.bad { color: var(--danger); background: color-mix(in srgb, var(--danger) 11%, transparent); }
  .queued-pill {
    flex: none;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 13%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .stale-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 11%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .chip-pr,
  .chip-branch {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    max-width: 200px;
    padding: 2px 7px;
    border-radius: 5px;
    font: 400 10px/1.5 var(--font-mono);
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border: 1px solid var(--line);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip-pr {
    color: var(--status-review);
    text-decoration: none;
  }
  .chip-pr:hover {
    border-color: color-mix(in srgb, var(--status-review) 45%, transparent);
  }
  .chip-pr svg,
  .chip-branch svg {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .hb {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .hb.stale {
    color: var(--danger);
  }
  .hb svg,
  .clock {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .acts {
    display: flex;
    gap: 5px;
    flex: none;
  }

  /* --- empty --------------------------------------------------------- */
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 44px 20px;
    border: 1px dashed var(--line-2);
    border-radius: 12px;
    text-align: center;
  }
  .empty svg {
    width: 30px;
    height: 30px;
    color: var(--ink-3);
  }
  .empty p {
    margin: 0;
    font-size: 13px;
    color: var(--ink-dim);
  }
  .empty .hint {
    max-width: 52ch;
    font-size: 12px;
    color: var(--ink-3);
    line-height: 1.6;
  }
  .empty code {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-dim);
    background: var(--raise);
    padding: 1px 5px;
    border-radius: 4px;
  }

  @media (max-width: 900px) {
    .summary {
      grid-template-columns: minmax(0, 1fr);
    }
    .stats {
      flex-wrap: wrap;
    }
    .legend {
      order: 3;
    }
    .scrim {
      padding: 10px;
    }
    .row {
      flex-wrap: wrap;
    }
    .title {
      flex-basis: 100%;
      order: 2;
    }
    .group-name {
      max-width: 100%;
      white-space: normal;
    }
    .inset {
      margin-left: 4px;
      padding-left: 9px;
    }
  }

  @media (max-width: 640px) {
    .wrap {
      padding: 12px 12px 30px;
    }
    .group {
      padding: 9px 9px 10px;
    }
    .root-inset {
      margin-left: 6px;
    }
    .badges {
      max-width: 100%;
    }
  }
</style>
