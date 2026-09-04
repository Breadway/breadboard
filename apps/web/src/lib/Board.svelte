<script lang="ts">
  import { onMount } from "svelte";
  import { STATUSES, type Status } from "@breadboard/api-client";
  import { store } from "./store.svelte";
  import Card from "./Card.svelte";

  const labels: Record<Status, string> = {
    backlog: "Backlog",
    todo: "Todo",
    doing: "Doing",
    review: "Review",
    done: "Done",
  };

  let dragOver = $state<Status | null>(null);
  let col = $state(0);
  let row = $state(0);

  const columns = $derived(STATUSES.map((s) => store.byStatus(s)));

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
    await store.move(selected.key, STATUSES[target]);
    col = target;
    row = 0;
  }

  function isTyping(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (store.paletteOpen || store.selectedKey || isTyping(e)) return;
      switch (e.key) {
        case "ArrowLeft":
        case "h": col = Math.max(0, col - 1); row = 0; clamp(); break;
        case "ArrowRight":
        case "l": col = Math.min(4, col + 1); row = 0; clamp(); break;
        case "ArrowUp":
        case "k": row = Math.max(0, row - 1); break;
        case "ArrowDown":
        case "j": row = Math.min(columns[col].length - 1, row + 1); break;
        case "[": e.preventDefault(); void moveSelected(-1); break;
        case "]": e.preventDefault(); void moveSelected(1); break;
        case "Enter":
        case "e":
          if (selected) { e.preventDefault(); store.selectedKey = selected.key; }
          break;
        default: return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  $effect(() => {
    void columns;
    clamp();
  });
</script>

<div class="board">
  {#each STATUSES as status, ci (status)}
    {@const issues = columns[ci]}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section
      class="column"
      role="list"
      aria-label={labels[status]}
      class:over={dragOver === status}
      class:focused={ci === col}
      ondragover={(e) => { e.preventDefault(); dragOver = status; }}
      ondragleave={() => (dragOver === status ? (dragOver = null) : null)}
      ondrop={(e) => drop(e, status)}
    >
      <h2>{labels[status]} <span class="count">{issues.length}</span></h2>
      <div class="cards">
        {#each issues as issue, ri (issue.id)}
          <Card {issue} keyboardSelected={ci === col && ri === row} />
        {/each}
      </div>
    </section>
  {/each}
</div>

<style>
  .board {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(240px, 1fr);
    gap: 10px;
    padding: 12px 14px;
    overflow-x: auto;
    height: calc(100vh - 46px);
    align-content: start;
  }
  .column {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .column.over {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--surface));
  }
  .column.focused {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  h2 {
    margin: 2px 4px 8px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-dim);
    display: flex;
    gap: 6px;
  }
  .count {
    color: var(--border);
  }
  .cards {
    overflow-y: auto;
    flex: 1;
  }
</style>
