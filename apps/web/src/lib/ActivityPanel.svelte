<script lang="ts">
  import { onMount } from "svelte";
  import {
    AGENT_EVENT_KINDS,
    safeHttpUrl,
    type AgentEvent,
    type AgentEventKind,
  } from "@breadboard/api-client";
  import { ago, api, errMsg } from "./store.svelte";

  // One agent's typed timeline, loaded newest first and filtered server-side
  // by kind and time range. The parent owns the modal and the navigation
  // callbacks so this panel never has to know how views are switched.
  let {
    agent,
    onOpenIssue,
    onOpenChat,
  }: {
    agent: string;
    onOpenIssue: (key: string) => void;
    onOpenChat: (room: number, message: number | null) => void;
  } = $props();

  type Range = "1h" | "24h" | "7d" | "30d" | "all";
  const RANGES: { value: Range; label: string; hours: number | null }[] = [
    { value: "1h", label: "1h", hours: 1 },
    { value: "24h", label: "24h", hours: 24 },
    { value: "7d", label: "7d", hours: 24 * 7 },
    { value: "30d", label: "30d", hours: 24 * 30 },
    { value: "all", label: "all", hours: null },
  ];

  const KIND_LABELS: Record<AgentEventKind, string> = {
    registered: "registered",
    claimed: "claimed",
    released: "released",
    state_change: "state",
    paused: "paused",
    removed: "removed",
    pr_opened: "PR opened",
    pr_merged: "PR merged",
    ci_result: "CI",
    chat_sent: "chat",
    handoff: "handoff",
  };

  // ci_result and handoff have no emit source yet; a chip that can only ever
  // be empty is misleading, so keep them out of the filter UI. The API still
  // accepts every kind. paused and removed are emitted by pause/remove.
  const CHIP_KINDS = AGENT_EVENT_KINDS.filter(
    (k) => k !== "ci_result" && k !== "handoff",
  );

  let kind = $state<AgentEventKind | "all">("all");
  let range = $state<Range>("24h");
  let events = $state<AgentEvent[]>([]);
  let loading = $state(true);
  let loadingMore = $state(false);
  let error = $state<string | null>(null);
  let hasMore = $state(false);
  let nextBefore = $state<number | null>(null);
  let now = $state(Date.now());
  let generation = 0;

  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });

  function sinceFor(value: Range): string | undefined {
    const hours = RANGES.find((r) => r.value === value)?.hours;
    if (hours == null) return undefined;
    return new Date(Date.now() - hours * 3_600_000).toISOString();
  }

  async function load(
    reset: boolean,
    name = agent,
    selectedKind = kind,
    selectedRange = range,
  ) {
    const token = ++generation;
    if (reset) {
      loading = true;
      error = null;
    } else {
      loadingMore = true;
    }
    try {
      const page = await api.agentEvents(name, {
        kinds: selectedKind === "all" ? undefined : [selectedKind],
        since: sinceFor(selectedRange),
        limit: 50,
        before: reset ? undefined : nextBefore ?? undefined,
      });
      if (token !== generation) return;
      if (reset) events = page.events;
      else events = [...events, ...page.events];
      hasMore = page.has_more;
      nextBefore = page.next_before ?? null;
    } catch (e) {
      if (token !== generation) return;
      error = errMsg(e);
      if (reset) events = [];
    } finally {
      if (token === generation) {
        loading = false;
        loadingMore = false;
      }
    }
  }

  // Refetch on agent, kind or range change. Only those inputs are read here,
  // so appending a page does not restart the query.
  $effect(() => {
    void agent;
    void kind;
    void range;
    void load(true);
  });

  function absolute(iso: string): string {
    return new Date(iso).toLocaleString();
  }

  function initial(kind: AgentEventKind): string {
    return KIND_LABELS[kind].slice(0, 1).toUpperCase();
  }
</script>

<section class="activity" aria-label="Agent activity">
  <header class="ahead">
    <h3>Activity</h3>
    <div class="filters">
      <div class="kinds" role="group" aria-label="Filter by event kind">
        <button class="chip" class:on={kind === "all"} onclick={() => (kind = "all")}>all</button>
        {#each CHIP_KINDS as k (k)}
          <button class="chip" class:on={kind === k} onclick={() => (kind = k)}>
            {KIND_LABELS[k]}
          </button>
        {/each}
      </div>
      <div class="ranges" role="group" aria-label="Filter by time range">
        {#each RANGES as r (r.value)}
          <button class="rng" class:on={range === r.value} onclick={() => (range = r.value)}>
            {r.label}
          </button>
        {/each}
      </div>
    </div>
  </header>

  {#if loading}
    <p class="state">Loading activity...</p>
  {:else if error}
    <p class="state bad">
      Couldn't load activity: {error}
      <button class="btn ghost sm" onclick={() => void load(true)}>Retry</button>
    </p>
  {:else if !events.length}
    <p class="state">No {kind === "all" ? "" : `${KIND_LABELS[kind]} `}events in this range.</p>
  {:else}
    <ol class="feed">
      {#each events as e (e.id)}
        {@const pr = safeHttpUrl(e.pr_url)}
        <li class="ev">
          <span class="badge k-{e.kind}" aria-hidden="true">{initial(e.kind)}</span>
          <div class="evbody">
            <div class="evtop">
              <span class="ekind">{KIND_LABELS[e.kind]}</span>
              <time datetime={e.at} title={absolute(e.at)}>{ago(e.at, now)}</time>
              {#if e.actor}<span class="actor">{e.actor}</span>{/if}
            </div>
            <p class="esummary">{e.summary || KIND_LABELS[e.kind]}</p>
            {#if e.detail}<p class="edetail">{e.detail}</p>{/if}
            {#if e.issue_key || pr || e.room_id != null}
              <div class="elinks">
                {#if e.issue_key}
                  <button class="linklike" onclick={() => onOpenIssue(e.issue_key!)}>
                    {e.issue_key}
                  </button>
                {/if}
                {#if pr}
                  <a class="linklike" href={pr} target="_blank" rel="noreferrer">
                    PR{e.pr_number ? ` #${e.pr_number}` : ""}
                  </a>
                {/if}
                {#if e.room_id != null}
                  <button
                    class="linklike"
                    onclick={() => onOpenChat(e.room_id!, e.message_id ?? null)}
                  >
                    {e.message_id ? "message" : "chat room"}
                  </button>
                {/if}
              </div>
            {/if}
          </div>
        </li>
      {/each}
    </ol>
    {#if hasMore}
      <button class="btn sm more" disabled={loadingMore} onclick={() => void load(false)}>
        {loadingMore ? "Loading..." : "Load older"}
      </button>
    {/if}
  {/if}
</section>

<style>
  .activity {
    border-top: 1px solid var(--line);
    padding-top: 12px;
    margin-top: 14px;
  }
  .ahead {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .ahead h3 {
    margin: 0;
    font-size: 12px;
    font-weight: 650;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .kinds {
    display: flex;
    gap: 4px;
    overflow-x: auto;
    max-width: 100%;
    padding-bottom: 2px;
  }
  .chip,
  .rng {
    border: 1px solid var(--line);
    background: transparent;
    color: var(--ink-3);
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 11px;
    white-space: nowrap;
    cursor: pointer;
  }
  .chip.on,
  .rng.on {
    border-color: var(--accent);
    color: var(--ink);
    background: color-mix(in oklab, var(--accent) 14%, transparent);
  }
  .ranges {
    display: flex;
    gap: 2px;
  }
  .state {
    margin: 6px 0;
    color: var(--ink-3);
    font-size: 12px;
  }
  .state.bad {
    color: var(--bad, #c0392b);
  }
  .feed {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .ev {
    display: grid;
    grid-template-columns: 22px minmax(0, 1fr);
    gap: 8px;
    align-items: start;
  }
  .badge {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 10px;
    font-weight: 700;
    color: var(--bg);
    background: var(--ink-3);
  }
  .k-registered,
  .k-pr_merged {
    background: var(--ok, #2e7d32);
  }
  .k-claimed,
  .k-pr_opened {
    background: var(--accent);
  }
  .k-released,
  .k-removed {
    background: var(--ink-3);
  }
  .k-state_change,
  .k-paused {
    background: var(--warn, #b7791f);
  }
  .k-ci_result {
    background: #2c7a7b;
  }
  .k-chat_sent,
  .k-handoff {
    background: #6b46c1;
  }
  .evbody {
    min-width: 0;
  }
  .evtop {
    display: flex;
    gap: 6px;
    align-items: baseline;
    font-size: 11px;
    color: var(--ink-3);
  }
  .ekind {
    font-weight: 650;
    color: var(--ink-2, var(--ink-3));
  }
  .actor::before {
    content: "· ";
  }
  .esummary {
    margin: 1px 0 0;
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .edetail {
    margin: 1px 0 0;
    font-size: 11px;
    color: var(--ink-3);
    overflow-wrap: anywhere;
  }
  .elinks {
    display: flex;
    gap: 10px;
    margin-top: 2px;
    font-size: 11px;
  }
  .linklike {
    border: 0;
    background: none;
    color: var(--accent);
    padding: 0;
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .more {
    margin-top: 10px;
  }
</style>
