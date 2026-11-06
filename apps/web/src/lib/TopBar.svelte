<script lang="ts">
  import { store } from "./store.svelte";
  import Menu, { type MenuItem } from "./ui/Menu.svelte";
  import { projectColor } from "./ui/colors";

  const titles = { board: "Board", backlog: "Backlog", roadmap: "Roadmap", agents: "Agents", inbox: "Inbox", settings: "Settings" };
  const filterable = $derived(store.view === "board" || store.view === "backlog");

  const project = $derived(store.projects.find((p) => p.key === store.filterProject));
  const cycle = $derived(store.cycles.find((c) => c.id === store.filterCycle));
  const epic = $derived(store.epics.find((e) => e.id === store.filterEpic));

  // One searchable menu for everything you can filter by. On wide screens
  // projects and cycles are also in the sidebar; on phones this is the only way.
  const filterItems = $derived<MenuItem[]>([
    ...store.projects
      .filter((p) => !p.archived)
      .map((p) => ({
        label: `Project: ${p.name}`,
        glyph: { dot: projectColor(p.key) },
        current: store.filterProject === p.key,
        run: () => (store.filterProject = store.filterProject === p.key ? "" : p.key),
      })),
    ...store.cycles.map((c) => ({
      label: `Cycle: ${c.name}${c.closed ? " (closed)" : ""}`,
      glyph: { text: "◔" },
      current: store.filterCycle === c.id,
      run: () => (store.filterCycle = store.filterCycle === c.id ? 0 : c.id),
    })),
    ...store.epics
      .filter((e) => !e.archived)
      .map((e) => ({
        label: `Epic: ${e.key} · ${e.title}`,
        glyph: { text: "◆" },
        current: store.filterEpic === e.id,
        run: () => (store.filterEpic = store.filterEpic === e.id ? 0 : e.id),
      })),
  ]);
</script>

<div class="bar">
  <h1 class="crumb">
    {titles[store.view]}
    {#if filterable}<span>/ {project ? project.name : "all projects"}</span>{/if}
  </h1>

  {#if filterable}
    <div class="chips">
      {#if project}
        <span class="chip"><i style="background:{projectColor(project.key)}"></i>Project <b>{project.key}</b>
          <button class="x" aria-label="Clear project filter" onclick={() => (store.filterProject = "")}>×</button></span>
      {/if}
      {#if cycle}
        <span class="chip">Cycle <b>{cycle.name}</b>
          <button class="x" aria-label="Clear cycle filter" onclick={() => (store.filterCycle = 0)}>×</button></span>
      {/if}
      {#if epic}
        <span class="chip">Epic <b>{epic.key}</b>
          <button class="x" aria-label="Clear epic filter" onclick={() => (store.filterEpic = 0)}>×</button></span>
      {/if}
      <Menu items={filterItems} triggerClass="chip dash" filter title="Filter by project, cycle or epic">
        {#snippet trigger()}+ Filter{/snippet}
      </Menu>
    </div>
  {/if}

  <span class="grow"></span>

  {#if filterable}
    <button class="chip" class:on={store.filterMine} aria-pressed={store.filterMine} onclick={() => (store.filterMine = !store.filterMine)}>
      Assigned to me
    </button>
  {/if}
  <button class="btn primary" onclick={() => store.openPalette()}>New issue <kbd>C</kbd></button>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 20px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
    min-height: 56px;
  }
  .crumb {
    margin: 0 6px 0 0;
    font: 700 19px/1.2 var(--font-display);
    letter-spacing: -0.02em;
    white-space: nowrap;
  }
  .crumb span {
    color: var(--ink-3);
    font-weight: 500;
  }
  .chips {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .grow {
    flex: 1;
  }
  .bar :global(.chip) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: 7px;
    border: 1px solid var(--line-2);
    background: none;
    font-size: 12px;
    color: var(--ink-dim);
    white-space: nowrap;
  }
  .bar :global(.chip:hover) {
    color: var(--ink);
    border-color: color-mix(in srgb, var(--ink) 26%, transparent);
  }
  .bar :global(.chip.dash) {
    border-style: dashed;
    color: var(--ink-3);
  }
  .bar :global(.chip.on) {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 50%, transparent);
    color: var(--accent);
  }
  .chip b {
    color: var(--ink);
    font-weight: 500;
  }
  .chip i {
    width: 7px;
    height: 7px;
    border-radius: 2px;
  }
  .x {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 14px;
    line-height: 1;
    padding: 0 0 0 2px;
  }
  .x:hover {
    color: var(--danger);
  }

  @media (max-width: 860px) {
    .bar {
      padding: 10px 14px;
    }
    .crumb span {
      display: none;
    }
  }
</style>
