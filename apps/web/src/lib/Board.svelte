<script lang="ts">
  import { onMount } from "svelte";
  import { STATUSES, type Status } from "@breadboard/api-client";
  import { bare, isTyping, store, statusName } from "./store.svelte";
  import Card from "./Card.svelte";
  import CycleStrip from "./CycleStrip.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";

  const DONE_LIMIT = 20;

  let dragOver = $state<Status | null>(null);
  let col = $state(0);
  let row = $state(0);
  // The keyboard cursor only shows once you've used the keys, and hides
  // again on a mouse click, so it doesn't read as a selection.
  let kbActive = $state(false);

  // byStatus() is sorted most-recently-updated first, so this keeps the
  // freshest completions visible. Done otherwise grows without bound since
  // issues never leave it; the full history is still one click away in Backlog.
  const all = $derived(STATUSES.map((s) => store.byStatus(s)));
  const columns = $derived(all.map((list, i) => (STATUSES[i] === "done" ? list.slice(0, DONE_LIMIT) : list)));
  const points = $derived(all.map((list) => list.reduce((a, i) => a + (i.estimate ?? 0), 0)));

  // A single explicitly selected cycle drives the strip; with several
  // selected, one strip would be misleading, so hide it.
  const stripCycle = $derived(
    store.filters.cycles.length === 0
      ? store.activeCycle
      : store.filters.cycles.length === 1
        ? store.cycles.find((c) => c.id === store.filters.cycles[0])
        : undefined,
  );

  function clamp() {
    col = Math.max(0, Math.min(col, 4));
    const len = columns[col].length;
    row = len === 0 ? 0 : Math.max(0, Math.min(row, len - 1));
  }

  const selected = $derived(columns[col]?.[row]);

  function drop(e: DragEvent, status: Status) {
    e.preventDefault();
    dragOver = null;
    const key = e.dataTransfer?.getData("text/plain");
    if (key) void store.move(key, status);
  }

  async function moveSelected(delta: number) {
    if (!selected) return;
    const target = Math.max(0, Math.min(col + delta, 4));
    if (target === col) return;
    const key = selected.key;
    await store.move(key, STATUSES[target]);
    col = target;
    row = Math.max(0, columns[target].findIndex((i) => i.key === key));
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      // Bare keys only: Ctrl+K, Ctrl+L and friends belong to the browser and
      // the palette, not the board cursor.
      if (!bare(e) || store.paletteOpen || store.selectedKey || isTyping(e)) return;
      switch (e.key) {
        case "ArrowLeft":
        case "h": col = Math.max(0, col - 1); row = 0; clamp(); break;
        case "ArrowRight":
        case "l": col = Math.min(4, col + 1); row = 0; clamp(); break;
        case "ArrowUp":
        case "k": row = Math.max(0, row - 1); break;
        case "ArrowDown":
        case "j": row = Math.min(columns[col].length - 1, row + 1); break;
        case "[": void moveSelected(-1); break;
        case "]": void moveSelected(1); break;
        case "Enter":
        case "e":
          if (!selected) return;
          store.selectedKey = selected.key;
          break;
        default: return;
      }
      kbActive = true;
      e.preventDefault();
    };
    const onMouse = () => (kbActive = false);
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onMouse);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onMouse);
    };
  });

  $effect(() => {
    void columns;
    clamp();
  });
</script>

<div class="wrap">
  {#if stripCycle}<CycleStrip cycle={stripCycle} />{/if}

  <div class="board">
    {#each STATUSES as status, ci (status)}
      {@const issues = columns[ci]}
      <section
        class="col"
        role="list"
        aria-label={statusName(status)}
        class:over={dragOver === status}
        ondragover={(e) => { e.preventDefault(); dragOver = status; }}
        ondragleave={(e) => { if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) dragOver = null; }}
        ondrop={(e) => drop(e, status)}
      >
        <h2>
          <StatusGlyph {status} />
          {statusName(status)}
          <span class="ct">{all[ci].length}</span>
          {#if points[ci]}<span class="pts">{points[ci]} pts</span>{/if}
          <button class="add" title="New issue in {statusName(status)}" aria-label="New issue in {statusName(status)}" onclick={() => store.openPalette(status)}>+</button>
        </h2>
        <div class="cards">
          {#each issues as issue, ri (issue.id)}
            <Card {issue} keyboardSelected={kbActive && ci === col && ri === row} />
          {:else}
            <p class="empty">{dragOver === status ? "Drop to move here" : "Nothing here"}</p>
          {/each}
          {#if status === "done" && all[ci].length > DONE_LIMIT}
            <button class="more" onclick={() => (store.view = "backlog")}>
              {all[ci].length - DONE_LIMIT} more in Backlog
            </button>
          {/if}
        </div>
      </section>
    {/each}
  </div>
</div>

<style>
  .wrap {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .board {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: repeat(5, minmax(220px, 1fr));
    gap: 12px;
    padding: 14px 20px 0;
    overflow-x: auto;
  }
  .col {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-radius: 12px;
    transition: background-color 0.15s, box-shadow 0.15s;
  }
  .col.over {
    background: color-mix(in srgb, var(--accent) 7%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 6px 6px 8px;
    font: 600 13px/1.2 var(--font-ui);
  }
  .ct {
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-dim);
    background: var(--raise);
    border-radius: 5px;
    padding: 3px 6px;
    font-variant-numeric: tabular-nums;
  }
  .pts {
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .add {
    margin-left: auto;
    width: 22px;
    height: 22px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--ink-3);
    display: grid;
    place-items: center;
    font-size: 16px;
    line-height: 1;
  }
  .add:hover {
    background: var(--raise);
    color: var(--ink);
  }
  .cards {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    /* room so the hover lift and focus rings aren't clipped by overflow */
    padding: 3px 4px 20px;
    margin: -3px -4px 0;
  }
  .empty {
    margin: 0;
    padding: 18px 8px;
    text-align: center;
    font-size: 12px;
    color: var(--ink-3);
    border: 1px dashed var(--line);
    border-radius: 10px;
  }
  .more {
    padding: 10px;
    border-radius: 9px;
    border: 1px dashed var(--line-2);
    background: none;
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .more:hover {
    color: var(--accent);
    border-color: var(--accent);
  }

  @media (max-width: 860px) {
    .board {
      display: flex;
      gap: 10px;
      padding: 12px 14px 0;
      scroll-snap-type: x mandatory;
    }
    .col {
      flex: 0 0 85%;
      scroll-snap-align: start;
    }
  }
</style>
