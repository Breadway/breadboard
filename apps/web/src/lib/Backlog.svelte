<script lang="ts">
  import { STATUSES } from "@breadboard/api-client";
  import { store } from "./store.svelte";

  const rows = $derived(
    [...store.filtered].sort((a, b) => {
      const s = STATUSES.indexOf(a.status) - STATUSES.indexOf(b.status);
      return s !== 0 ? s : a.key.localeCompare(b.key, undefined, { numeric: true });
    }),
  );
</script>

<div class="wrap">
  <table>
    <thead>
      <tr>
        <th>Key</th>
        <th>Title</th>
        <th>Status</th>
        <th>Priority</th>
        <th>Labels</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as issue (issue.id)}
        <tr
          class:selected={store.selectedKey === issue.key}
          onclick={() => (store.selectedKey = issue.key)}
        >
          <td class="key">{issue.key}</td>
          <td>{issue.title}</td>
          <td><span class="pill s-{issue.status}">{issue.status}</span></td>
          <td class="dim">{issue.priority === "none" ? "" : issue.priority}</td>
          <td class="dim">{issue.labels.join(", ")}</td>
        </tr>
      {/each}
      {#if rows.length === 0}
        <tr><td colspan="5" class="dim empty">No issues match the current filter.</td></tr>
      {/if}
    </tbody>
  </table>
</div>

<style>
  .wrap {
    height: calc(100vh - 46px);
    overflow: auto;
    padding: 12px 14px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  th {
    text-align: left;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-dim);
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    background: var(--bg);
  }
  td {
    padding: 7px 10px;
    border-bottom: 1px solid var(--surface);
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover {
    background: var(--surface);
  }
  tr.selected {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
  }
  .key {
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .dim {
    color: var(--ink-dim);
  }
  .empty {
    text-align: center;
    padding: 30px;
  }
  .pill {
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid var(--border);
    text-transform: capitalize;
  }
  .s-doing {
    color: #e0a052;
    border-color: #e0a052;
  }
  .s-done {
    color: #4caf50;
    border-color: #4caf50;
  }
</style>
