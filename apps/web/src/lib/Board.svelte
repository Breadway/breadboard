<script lang="ts">
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

  function drop(e: DragEvent, status: Status) {
    e.preventDefault();
    dragOver = null;
    const key = e.dataTransfer?.getData("text/plain");
    if (key) void store.move(key, status);
  }
</script>

<div class="board">
  {#each STATUSES as status (status)}
    {@const issues = store.byStatus(status)}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section
      class="column"
      role="list"
      aria-label={labels[status]}
      class:over={dragOver === status}
      ondragover={(e) => {
        e.preventDefault();
        dragOver = status;
      }}
      ondragleave={() => (dragOver === status ? (dragOver = null) : null)}
      ondrop={(e) => drop(e, status)}
    >
      <h2>{labels[status]} <span class="count">{issues.length}</span></h2>
      <div class="cards">
        {#each issues as issue (issue.id)}
          <Card {issue} />
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
