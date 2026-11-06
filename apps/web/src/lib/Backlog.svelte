<script lang="ts">
  import { PRIORITIES, STATUSES, type Status } from "@breadboard/api-client";
  import { store, statusName } from "./store.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";
  import PriorityGlyph from "./ui/PriorityGlyph.svelte";
  import Avatar from "./ui/Avatar.svelte";
  import { labelColor, projectColor } from "./ui/colors";

  // One group per status; inside a group, most urgent first, then by key.
  const groups = $derived(
    STATUSES.map((s) => ({
      status: s,
      rows: store.filtered
        .filter((i) => i.status === s)
        .sort((a, b) => {
          const p = PRIORITIES.indexOf(b.priority) - PRIORITIES.indexOf(a.priority);
          return p !== 0 ? p : a.key.localeCompare(b.key, undefined, { numeric: true });
        }),
    })).filter((g) => g.rows.length),
  );

  let collapsed = $state<Partial<Record<Status, boolean>>>({ done: true });
  const cycleName = (id: number | null) => (id ? (store.cycles.find((c) => c.id === id)?.name ?? "") : "");
</script>

<div class="wrap">
  {#each groups as g (g.status)}
    <section>
      <button class="gh" aria-expanded={!collapsed[g.status]} onclick={() => (collapsed[g.status] = !collapsed[g.status])}>
        <svg class="caret" class:shut={collapsed[g.status]} viewBox="0 0 10 10" aria-hidden="true"><path d="m2.5 3.5 2.5 3 2.5-3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
        <StatusGlyph status={g.status} />
        {statusName(g.status)}
        <span class="ct">{g.rows.length}</span>
      </button>
      {#if !collapsed[g.status]}
        {#each g.rows as issue (issue.id)}
          {@const pkey = store.projectKey(issue.project_id)}
          <button class="row" class:sel={store.selectedKey === issue.key} onclick={() => (store.selectedKey = issue.key)}>
            <PriorityGlyph priority={issue.priority} />
            <span class="key"><i style="background:{projectColor(pkey)}"></i>{issue.key}</span>
            <StatusGlyph status={issue.status} />
            <span class="t">{issue.title}</span>
            <span class="labs">
              {#each issue.labels.slice(0, 3) as l (l)}<span class="chip-label"><i style="background:{labelColor(l)}"></i>{l}</span>{/each}
            </span>
            <span class="cy">{cycleName(issue.cycle_id)}</span>
            <span class="who"><Avatar userId={issue.assignee_id} size={20} /></span>
          </button>
        {/each}
      {/if}
    </section>
  {:else}
    <div class="none">
      <p>No issues match these filters.</p>
      <button class="btn" onclick={() => store.clearFilters()}>Clear filters</button>
    </div>
  {/each}
</div>

<style>
  .wrap {
    height: 100%;
    overflow-y: auto;
    padding: 4px 20px 32px;
  }
  .gh {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 16px 8px 8px;
    border: 0;
    background: var(--bg);
    font-size: 13px;
    font-weight: 600;
    text-align: left;
  }
  .caret {
    width: 10px;
    height: 10px;
    color: var(--ink-3);
    transition: transform 0.15s;
  }
  .caret.shut {
    transform: rotate(-90deg);
  }
  .ct {
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .row {
    display: grid;
    grid-template-columns: 14px 84px 14px minmax(0, 1fr) auto 110px 20px;
    gap: 12px;
    align-items: center;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: none;
    font-size: 13px;
    text-align: left;
  }
  .row:hover {
    background: var(--surface);
  }
  .row.sel {
    background: var(--accent-soft);
  }
  .key {
    display: flex;
    align-items: center;
    gap: 6px;
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .key i {
    width: 3px;
    height: 11px;
    border-radius: 2px;
  }
  .t {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .labs {
    display: flex;
    gap: 5px;
  }
  .cy {
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
    text-align: right;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .who {
    display: grid;
    place-items: center;
  }
  .none {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 60px 0;
    color: var(--ink-3);
  }
  .none p {
    margin: 0;
  }

  @media (max-width: 860px) {
    .wrap {
      padding: 4px 10px 24px;
    }
    .row {
      grid-template-columns: 14px 72px minmax(0, 1fr) 20px;
    }
    .row > :global(.sg),
    .labs,
    .cy {
      display: none;
    }
  }
</style>
