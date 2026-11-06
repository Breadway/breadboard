<script lang="ts">
  import { store } from "./store.svelte";
  import Filters from "./Filters.svelte";

  const titles = { board: "Board", backlog: "Backlog", roadmap: "Roadmap", agents: "Agents", chat: "Chat", inbox: "Inbox", settings: "Settings" };
  const filterable = $derived(store.view === "board" || store.view === "backlog");
  const mine = $derived(!!store.me && store.isFiltered("assignees", store.me.id));
</script>

<div class="bar">
  <h1 class="crumb">{titles[store.view]}</h1>

  {#if filterable}
    <Filters />
    <button class="chip" class:on={mine} aria-pressed={mine} onclick={() => store.toggleMine()}>
      Assigned to me
    </button>
  {/if}

  <span class="grow"></span>
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
  .bar :global(.chip.on) {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 50%, transparent);
    color: var(--accent);
  }

  @media (max-width: 860px) {
    .bar {
      padding: 8px 12px;
      gap: 6px;
      min-height: 50px;
    }
    .crumb {
      font-size: 17px;
    }
    .bar :global(.btn kbd) {
      display: none;
    }
    .bar :global(.chip) {
      height: 26px;
      padding: 0 8px;
      font-size: 11.5px;
    }
  }
</style>
