<script lang="ts">
  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { PRIORITIES, STATUSES, type AgentState } from "@breadboard/api-client";
  import { store, statusName } from "./store.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";
  import PriorityGlyph from "./ui/PriorityGlyph.svelte";
  import Avatar from "./ui/Avatar.svelte";
  import { agentColor, labelColor, projectColor } from "./ui/colors";

  // Every facet is optional and multi-select; facets combine with AND, values
  // inside one facet with OR. Nothing closes on pick, so any combination can
  // be built in one visit.

  const AGENT_STATES: AgentState[] = [
    "planned",
    "implementing",
    "reworking",
    "awaiting-review",
    "awaiting-ci",
    "awaiting-merge",
    "verifying",
    "blocked",
    "needs-user",
    "done",
  ];

  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  let firstEl: HTMLElement | undefined = $state();

  const labels = $derived(
    [...new Set(store.issues.flatMap((i) => i.labels))].sort((a, b) => a.localeCompare(b)),
  );
  const agentNames = $derived(
    [...new Set(store.agents.map((a) => a.agent_name))].sort((a, b) => a.localeCompare(b)),
  );
  const agentStates = $derived(
    AGENT_STATES.filter((s) => store.agents.some((a) => a.state === s)),
  );
  const assigneeIds = $derived(
    [...new Set(store.issues.map((i) => i.assignee_id).filter((v): v is number => v != null))].sort(
      (a, b) => a - b,
    ),
  );
  const assigneeLabel = (id: number) =>
    store.me?.id === id ? `${store.me.login} (you)` : `User #${id}`;

  const matchCount = $derived(store.filtered.length);

  // Explicit per-facet toggles keep the Svelte markup type-safe without
  // generic gymnastics at every call site.
  const toggleProject = (v: string) => store.toggleFilter("projects", v);
  const toggleStatus = (v: (typeof STATUSES)[number]) => store.toggleFilter("statuses", v);
  const togglePriority = (v: (typeof PRIORITIES)[number]) => store.toggleFilter("priorities", v);
  const toggleType = (v: "issue" | "pr") => store.toggleFilter("types", v);
  const toggleCycle = (v: number) => store.toggleFilter("cycles", v);
  const toggleEpic = (v: number) => store.toggleFilter("epics", v);
  const toggleAssignee = (v: number) => store.toggleFilter("assignees", v);
  const toggleAgent = (v: string) => store.toggleFilter("agents", v);
  const toggleAgentState = (v: AgentState) => store.toggleFilter("agentStates", v);
  const toggleLabel = (v: string) => store.toggleFilter("labels", v);

  function clearAll() {
    store.clearFilters();
  }

  function onDoc(e: MouseEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
  onMount(() => {
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  });

  // Focus the search box on open, and claim Escape before App's global
  // handler closes the issue drawer instead.
  $effect(() => {
    if (!open) return;
    queueMicrotask(() => firstEl?.focus());
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        open = false;
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  });

  function onPanelKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      open = false;
    }
  }

  // --- active chips -------------------------------------------------------
  type Chip = { id: string; label: string; remove: () => void; tint?: string };
  const chips = $derived.by(() => {
    const f = store.filters;
    const out: Chip[] = [];
    for (const key of f.projects) {
      const p = store.projects.find((x) => x.key === key);
      out.push({
        id: `p:${key}`,
        label: p?.name ?? key,
        tint: projectColor(key),
        remove: () => store.toggleFilter("projects", key),
      });
    }
    for (const id of f.cycles) {
      const c = store.cycles.find((x) => x.id === id);
      out.push({ id: `c:${id}`, label: c?.name ?? `Cycle ${id}`, remove: () => store.toggleFilter("cycles", id) });
    }
    for (const id of f.epics) {
      const e = store.epics.find((x) => x.id === id);
      out.push({ id: `e:${id}`, label: e?.key ?? `Epic ${id}`, remove: () => store.toggleFilter("epics", id) });
    }
    for (const s of f.statuses) {
      out.push({ id: `s:${s}`, label: statusName(s), remove: () => store.toggleFilter("statuses", s) });
    }
    for (const p of f.priorities) {
      out.push({ id: `pr:${p}`, label: `${statusName(p)} priority`, remove: () => store.toggleFilter("priorities", p) });
    }
    for (const l of f.labels) {
      out.push({ id: `l:${l}`, label: `#${l}`, tint: labelColor(l), remove: () => store.toggleFilter("labels", l) });
    }
    for (const id of f.assignees) {
      out.push({ id: `a:${id}`, label: assigneeLabel(id), remove: () => store.toggleFilter("assignees", id) });
    }
    if (f.unassigned) {
      out.push({ id: "unassigned", label: "Unassigned", remove: () => (store.filters.unassigned = false) });
    }
    for (const a of f.agents) {
      out.push({ id: `ag:${a}`, label: a, tint: agentColor(a), remove: () => store.toggleFilter("agents", a) });
    }
    for (const s of f.agentStates) {
      out.push({ id: `as:${s}`, label: `state: ${s}`, remove: () => store.toggleFilter("agentStates", s) });
    }
    if (f.claimed) {
      out.push({
        id: "claimed",
        label: f.claimed === "claimed" ? "Claimed" : "Unclaimed",
        remove: () => (store.filters.claimed = ""),
      });
    }
    for (const t of f.types) {
      out.push({ id: `t:${t}`, label: t === "pr" ? "Pull requests" : "Issues", remove: () => store.toggleFilter("types", t) });
    }
    if (f.text.trim()) {
      out.push({ id: "text", label: `"${f.text.trim()}"`, remove: () => (store.filters.text = "") });
    }
    return out as Chip[];
  });
</script>

<div class="filters">
  <div class="anchor" bind:this={root}>
  <button
    class="trigger"
    class:on={store.hasFilters}
    aria-haspopup="dialog"
    aria-expanded={open}
    onclick={() => (open = !open)}
    title="Filter by project, cycle, epic, status, priority, label, assignee, agent and more"
  >
    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 3.5h12l-4.6 5.4v4.2L6.6 15v-6.1z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
    Filter
    {#if store.filterCount}<span class="count">{store.filterCount}</span>{/if}
    <svg class="caret" viewBox="0 0 10 10" aria-hidden="true"><path d="m2.5 3.5 2.5 3 2.5-3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
  </button>

  {#if open}
    <div class="panel" role="dialog" aria-label="Filters" tabindex="-1" onkeydown={onPanelKey} transition:fly={{ y: -4, duration: 110 }}>
      <div class="search">
        <svg viewBox="0 0 16 16" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="1.5" d="M7 12.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11zm4-1.5 3.5 3.5" /></svg>
        <input
          bind:this={firstEl}
          bind:value={store.filters.text}
          placeholder="Search key, title, description, labels"
          aria-label="Text filter"
        />
        {#if store.filters.text}<button class="clear-x" aria-label="Clear text" onclick={() => (store.filters.text = "")}>×</button>{/if}
      </div>

      <div class="grid">
        <section>
          <h4>Project</h4>
          {#each store.projects.filter((p) => !p.archived) as p (p.id)}
            <button class="opt" class:on={store.isFiltered("projects", p.key)} onclick={() => toggleProject(p.key)}>
              <span class="box"></span>
              <i class="swatch" style="background:{projectColor(p.key)}"></i>
              <span class="lbl">{p.key} <span class="sub">{p.name}</span></span>
            </button>
          {/each}
        </section>

        <section>
          <h4>Status</h4>
          {#each STATUSES as s (s)}
            <button class="opt" class:on={store.isFiltered("statuses", s)} onclick={() => toggleStatus(s)}>
              <span class="box"></span>
              <StatusGlyph status={s} size={12} />
              <span class="lbl">{statusName(s)}</span>
            </button>
          {/each}
        </section>

        <section>
          <h4>Priority</h4>
          {#each [...PRIORITIES].reverse() as p (p)}
            <button class="opt" class:on={store.isFiltered("priorities", p)} onclick={() => togglePriority(p)}>
              <span class="box"></span>
              <PriorityGlyph priority={p} />
              <span class="lbl">{p === "none" ? "None" : statusName(p)}</span>
            </button>
          {/each}
        </section>

        <section>
          <h4>Type</h4>
          {#each ["issue", "pr"] as const as t (t)}
            <button class="opt" class:on={store.isFiltered("types", t)} onclick={() => toggleType(t)}>
              <span class="box"></span>
              <span class="lbl">{t === "pr" ? "Pull requests" : "Issues"}</span>
            </button>
          {/each}
        </section>

        {#if store.cycles.length}
          <section>
            <h4>Cycle</h4>
            {#each store.cycles as c (c.id)}
              <button class="opt" class:on={store.isFiltered("cycles", c.id)} onclick={() => toggleCycle(c.id)}>
                <span class="box"></span>
                <span class="lbl">{c.name}{#if c.closed}<span class="sub">closed</span>{/if}</span>
              </button>
            {/each}
          </section>
        {/if}

        {#if store.epics.length}
          <section>
            <h4>Epic</h4>
            {#each store.epics.filter((e) => !e.archived) as e (e.id)}
              <button class="opt" class:on={store.isFiltered("epics", e.id)} onclick={() => toggleEpic(e.id)}>
                <span class="box"></span>
                <span class="ek">{e.key}</span>
                <span class="lbl ell">{e.title}</span>
              </button>
            {/each}
          </section>
        {/if}

        <section>
          <h4>Assignee</h4>
          <button class="opt" class:on={store.filters.unassigned} onclick={() => (store.filters.unassigned = !store.filters.unassigned)}>
            <span class="box"></span>
            <span class="lbl"><span class="sub">—</span> Unassigned</span>
          </button>
          {#each assigneeIds as id (id)}
            <button class="opt" class:on={store.isFiltered("assignees", id)} onclick={() => toggleAssignee(id)}>
              <span class="box"></span>
              <Avatar userId={id} size={16} />
              <span class="lbl">{assigneeLabel(id)}</span>
            </button>
          {/each}
        </section>

        <section>
          <h4>Agent</h4>
          <button class="opt" class:on={store.filters.claimed === "claimed"} onclick={() => (store.filters.claimed = store.filters.claimed === "claimed" ? "" : "claimed")}>
            <span class="box"></span>
            <span class="lbl">Claimed by an agent</span>
          </button>
          <button class="opt" class:on={store.filters.claimed === "unclaimed"} onclick={() => (store.filters.claimed = store.filters.claimed === "unclaimed" ? "" : "unclaimed")}>
            <span class="box"></span>
            <span class="lbl">Unclaimed</span>
          </button>
          {#each agentNames as a (a)}
            <button class="opt" class:on={store.isFiltered("agents", a)} onclick={() => toggleAgent(a)}>
              <span class="box"></span>
              <i class="swatch" style="background:{agentColor(a)}"></i>
              <span class="lbl">{a}</span>
            </button>
          {/each}
          {#if agentStates.length}
            <p class="divider">Agent state</p>
            {#each agentStates as s (s)}
              <button class="opt" class:on={store.isFiltered("agentStates", s)} onclick={() => toggleAgentState(s)}>
                <span class="box"></span>
                <span class="lbl">{s}</span>
              </button>
            {/each}
          {/if}
        </section>

        {#if labels.length}
          <section class="wide">
            <h4>Label</h4>
            <div class="wrap-chips">
              {#each labels as l (l)}
                <button class="chip-label pick" class:on={store.isFiltered("labels", l)} onclick={() => toggleLabel(l)}>
                  <i style="background:{labelColor(l)}"></i>{l}
                </button>
              {/each}
            </div>
          </section>
        {/if}
      </div>

      <div class="foot">
        <span class="hint">{matchCount} of {store.issues.length} issues match</span>
        <span class="grow"></span>
        <button class="foot-btn" disabled={!store.hasFilters} onclick={clearAll}>Clear all</button>
        <button class="foot-btn primary" onclick={() => (open = false)}>Done</button>
      </div>
    </div>
  {/if}
  </div>

  {#if chips.length}
    <div class="active" aria-label="Active filters">
      {#each chips as chip (chip.id)}
        <button class="chip" onclick={chip.remove} title="Remove {chip.label}">
          {#if chip.tint}<i style="background:{chip.tint}"></i>{/if}
          {chip.label}
          <span class="x" aria-hidden="true">×</span>
        </button>
      {/each}
      <button class="clear-all" onclick={clearAll}>Clear all</button>
    </div>
  {/if}
</div>

<style>
  .filters {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .anchor {
    position: relative;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 9px 0 10px;
    border-radius: 7px;
    border: 1px dashed var(--line-2);
    background: none;
    font-size: 12px;
    color: var(--ink-3);
    white-space: nowrap;
  }
  .trigger:hover {
    color: var(--ink);
    border-color: color-mix(in srgb, var(--ink) 26%, transparent);
  }
  .trigger.on {
    border-style: solid;
    border-color: color-mix(in srgb, var(--accent) 50%, transparent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .trigger svg {
    width: 12px;
    height: 12px;
  }
  .trigger .caret {
    width: 9px;
    height: 9px;
  }
  .count {
    min-width: 15px;
    height: 15px;
    padding: 0 4px;
    border-radius: 99px;
    background: var(--accent);
    color: var(--on-accent);
    font: 600 9.5px/15px var(--font-mono);
    text-align: center;
  }

  .panel {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 40;
    width: min(680px, calc(100vw - 40px));
    max-height: min(72vh, 640px);
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    background: var(--raise);
    border: 1px solid var(--line-2);
    box-shadow: var(--shadow-pop);
    outline: none;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
  }
  .search svg {
    width: 13px;
    height: 13px;
    color: var(--ink-3);
    flex: none;
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    outline: none;
    font-size: 13px;
  }
  .search input::placeholder {
    color: var(--ink-3);
  }
  .clear-x {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 15px;
    line-height: 1;
    padding: 0 2px;
  }
  .clear-x:hover {
    color: var(--danger);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 4px 10px;
    padding: 10px;
    overflow-y: auto;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    break-inside: avoid;
  }
  section.wide {
    grid-column: 1 / -1;
  }
  h4 {
    margin: 6px 0 4px;
    padding: 0 6px;
    font: 500 9.5px/1 var(--font-mono);
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .divider {
    margin: 8px 0 3px;
    padding: 0 6px;
    font: 500 9.5px/1 var(--font-mono);
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    padding: 5px 6px;
    border: 0;
    border-radius: 6px;
    background: none;
    font-size: 12.5px;
    text-align: left;
    color: var(--ink-dim);
    min-width: 0;
  }
  .opt:hover {
    background: color-mix(in srgb, var(--ink) 5%, transparent);
    color: var(--ink);
  }
  .opt.on {
    color: var(--ink);
  }
  .box {
    width: 13px;
    height: 13px;
    flex: none;
    border-radius: 4px;
    border: 1.5px solid var(--line-2);
    transition: background-color 0.12s, border-color 0.12s;
  }
  .opt.on .box {
    background: var(--accent);
    border-color: var(--accent);
    box-shadow: inset 0 0 0 2px var(--raise);
  }
  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    flex: none;
  }
  .lbl {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .lbl.ell {
    white-space: nowrap;
  }
  .sub {
    margin-left: 4px;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .ek {
    font: 600 10px/1 var(--font-mono);
    color: var(--accent);
    flex: none;
  }
  .wrap-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    padding: 0 4px 4px;
  }
  .pick {
    border: 1px solid var(--line);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    color: var(--ink-dim);
    cursor: pointer;
    padding: 2px 8px;
  }
  .pick.on {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
    background: var(--accent-soft);
    color: var(--accent);
  }

  .foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 9px 12px;
    border-top: 1px solid var(--line);
  }
  .hint {
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .grow {
    flex: 1;
  }
  .foot-btn {
    height: 26px;
    padding: 0 10px;
    border-radius: 7px;
    border: 1px solid var(--line-2);
    background: none;
    color: var(--ink-dim);
    font-size: 11.5px;
    font-weight: 500;
  }
  .foot-btn:hover:not(:disabled) {
    color: var(--ink);
    border-color: color-mix(in srgb, var(--ink) 26%, transparent);
  }
  .foot-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .foot-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }

  /* active chips, inline in the top bar */
  .active {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 5px 0 8px;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--line));
    background: color-mix(in srgb, var(--accent) 8%, transparent);
    color: var(--ink);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .chip:hover {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .chip i {
    width: 7px;
    height: 7px;
    border-radius: 2px;
    flex: none;
  }
  .chip .x {
    margin-left: 1px;
    color: var(--ink-3);
    font-size: 13px;
    line-height: 1;
  }
  .chip:hover .x {
    color: var(--danger);
  }
  .clear-all {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 11px;
    padding: 0 4px;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .clear-all:hover {
    color: var(--ink);
  }

  @media (max-width: 900px) {
    .panel {
      left: 0;
      width: calc(100vw - 28px);
    }
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @media (max-width: 620px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
