<script lang="ts">
  import { STATUSES, type Cycle } from "@breadboard/api-client";
  import { store, statusName } from "./store.svelte";
  import Ring from "./ui/Ring.svelte";

  let { cycle }: { cycle: Cycle } = $props();

  // Cycle-scoped, but still honours every other active filter so the numbers
  // match the cards below.
  const issues = $derived(
    store.issues.filter(
      (i) => i.cycle_id === cycle.id && i.type !== "pr" && store.matches(i, { ignoreCycles: true }),
    ),
  );
  const counts = $derived(STATUSES.map((s) => issues.filter((i) => i.status === s).length));
  const done = $derived(counts[4]);
  const pts = (list: typeof issues) => list.reduce((a, i) => a + (i.estimate ?? 0), 0);
  const ptsDone = $derived(pts(issues.filter((i) => i.status === "done")));
  const ptsAll = $derived(pts(issues));

  const DAY = 86_400_000;
  const daysLeft = $derived.by(() => {
    const end = new Date(cycle.ends_on + "T23:59:59");
    return Math.ceil((end.getTime() - Date.now()) / DAY);
  });
  const fmt = (d: string) => new Date(d + "T12:00:00").toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const isFilter = $derived(store.isFiltered("cycles", cycle.id));
</script>

<section class="cyc" aria-label="Cycle progress">
  <div class="name">
    <Ring done={done} total={issues.length} />
    <h2>{cycle.name}</h2>
    {#if !isFilter}
      <button class="focus" onclick={() => store.setOnlyFilter("cycles", cycle.id)}>Show only this cycle</button>
    {/if}
  </div>
  <div class="dates">
    {fmt(cycle.starts_on)} → {fmt(cycle.ends_on)} ·
    {#if cycle.closed}closed
    {:else if daysLeft < 0}<span class="late">{-daysLeft} {-daysLeft === 1 ? "day" : "days"} over</span>
    {:else}<span class="left">{daysLeft} {daysLeft === 1 ? "day" : "days"} left</span>{/if}
  </div>
  <div class="stack">
    <div class="sbar">
      {#each STATUSES as s, i (s)}
        {#if counts[i]}<i style="flex:{counts[i]};background:var(--status-{s})" title="{counts[i]} {s}"></i>{/if}
      {/each}
      {#if !issues.length}<i class="none"></i>{/if}
    </div>
    <div class="legend">
      {#each STATUSES as s, i (s)}
        {#if counts[i]}<span><i style="background:var(--status-{s})"></i>{statusName(s)} <em>{counts[i]}</em></span>{/if}
      {/each}
      {#if !issues.length}<span>No issues in this cycle yet</span>{/if}
    </div>
  </div>
  <div class="big">
    <b>{done}<small> / {issues.length}</small></b>
    <span>{ptsAll ? `${ptsDone} of ${ptsAll} pts done` : "done"}</span>
  </div>
</section>

<style>
  .cyc {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 6px 24px;
    align-items: center;
    margin: 16px 20px 2px;
    padding: 12px 16px;
    border-radius: 11px;
    background: var(--surface);
    border: 1px solid var(--line);
  }
  .name {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font: 700 15px/1.2 var(--font-display);
    letter-spacing: -0.01em;
    white-space: nowrap;
  }
  .focus {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 11px;
    text-decoration: underline;
    text-underline-offset: 2px;
    white-space: nowrap;
  }
  .focus:hover {
    color: var(--accent);
  }
  .dates {
    grid-column: 1;
    font: 400 11px/1.3 var(--font-mono);
    color: var(--ink-3);
  }
  .left {
    color: var(--accent);
  }
  .late {
    color: var(--danger);
  }
  .stack {
    grid-row: 1 / 3;
    grid-column: 2;
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-width: 0;
  }
  .sbar {
    display: flex;
    gap: 2px;
    height: 8px;
    border-radius: 4px;
    overflow: hidden;
    background: var(--sunk);
  }
  .sbar i {
    display: block;
    transition: flex 0.5s var(--ease);
  }
  .sbar .none {
    flex: 1;
    background: var(--line);
  }
  .legend {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .legend span {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .legend i {
    width: 7px;
    height: 7px;
    border-radius: 2px;
  }
  .legend em {
    font-style: normal;
    color: var(--ink-dim);
  }
  .big {
    grid-row: 1 / 3;
    grid-column: 3;
    text-align: right;
  }
  .big b {
    display: block;
    font: 800 26px/1 var(--font-display);
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }
  .big small {
    font-size: 15px;
    color: var(--ink-3);
  }
  .big span {
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }

  @media (max-width: 860px) {
    .cyc {
      grid-template-columns: minmax(0, 1fr) auto;
      margin: 12px 14px 2px;
    }
    .stack {
      grid-row: auto;
      grid-column: 1 / -1;
    }
    .big {
      grid-row: 1 / 3;
      grid-column: 2;
    }
  }
</style>
