<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { store } from "./store.svelte";

  let query = $state("");
  let cursor = $state(0);
  let inputEl = $state<HTMLInputElement | null>(null);

  type Item =
    | { kind: "create"; label: string }
    | { kind: "issue"; key: string; label: string }
    | { kind: "view"; label: string; go: () => void };

  const items = $derived.by<Item[]>(() => {
    const q = query.trim().toLowerCase();
    const out: Item[] = [];

    if (q) {
      const matches = store.issues
        .filter((i) => `${i.key} ${i.title}`.toLowerCase().includes(q))
        .slice(0, 8)
        .map<Item>((i) => ({ kind: "issue", key: i.key, label: `${i.key}  ${i.title}` }));
      const exact = store.issues.some((i) => i.key.toLowerCase() === q);
      if (!exact) out.push({ kind: "create", label: `Create issue: “${query.trim()}”` });
      out.push(...matches);
    } else {
      out.push(
        { kind: "view", label: "Go to board", go: () => (store.view = "board") },
        { kind: "view", label: "Go to backlog", go: () => (store.view = "backlog") },
        { kind: "view", label: "Go to roadmap", go: () => (store.view = "roadmap") },
        { kind: "view", label: "Go to settings", go: () => (store.view = "settings") },
        { kind: "view", label: store.filterMine ? "Show all issues" : "Show only my issues", go: () => (store.filterMine = !store.filterMine) },
      );
    }
    return out;
  });

  $effect(() => {
    if (store.paletteOpen) {
      query = "";
      cursor = 0;
      queueMicrotask(() => inputEl?.focus());
    }
  });

  $effect(() => {
    void items;
    cursor = 0;
  });

  async function run(item: Item) {
    if (item.kind === "create") {
      const issue = await store.create({ title: query.trim() });
      if (issue) store.selectedKey = issue.key;
    } else if (item.kind === "issue") {
      store.selectedKey = item.key;
    } else {
      item.go();
    }
    store.paletteOpen = false;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") store.paletteOpen = false;
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      cursor = (cursor + 1) % Math.max(items.length, 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      cursor = (cursor - 1 + items.length) % Math.max(items.length, 1);
    } else if (e.key === "Enter" && items[cursor]) {
      e.preventDefault();
      void run(items[cursor]);
    }
  }
</script>

{#if store.paletteOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="scrim"
    role="presentation"
    transition:fade={{ duration: 120 }}
    onclick={(e) => e.target === e.currentTarget && (store.paletteOpen = false)}
  >
    <div
      class="palette"
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
      tabindex="-1"
      transition:fly={{ y: -8, duration: 140 }}
    >
      <input
        bind:this={inputEl}
        bind:value={query}
        onkeydown={onKey}
        placeholder="Search issues, or type to create…"
      />
      <ul>
        {#each items as item, i (item.kind + item.label)}
          <li>
            <button
              class:active={i === cursor}
              onmouseenter={() => (cursor = i)}
              onclick={() => run(item)}
            >
              {#if item.kind === "create"}<span class="tag">new</span>{/if}
              {item.label}
            </button>
          </li>
        {:else}
          <li class="none">no matches</li>
        {/each}
      </ul>
      <div class="hint">↑↓ to navigate · ↵ to select · esc to close</div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 14vh;
    z-index: 40;
  }
  .palette {
    width: min(560px, 92vw);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.5);
  }
  input {
    width: 100%;
    border: none;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    color: var(--ink);
    padding: 12px 14px;
    font: inherit;
    font-size: 14px;
  }
  input:focus {
    outline: none;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 46vh;
    overflow-y: auto;
  }
  li button {
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--ink);
    padding: 8px 10px;
    border-radius: 6px;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    display: flex;
    gap: 8px;
    align-items: center;
  }
  li button.active {
    background: color-mix(in srgb, var(--accent) 16%, var(--surface));
  }
  .tag {
    font-size: 10px;
    text-transform: uppercase;
    background: var(--accent);
    color: #000;
    border-radius: 4px;
    padding: 1px 5px;
  }
  .none {
    padding: 10px;
    color: var(--ink-dim);
    font-size: 13px;
  }
  .hint {
    padding: 7px 12px;
    border-top: 1px solid var(--border);
    color: var(--ink-dim);
    font-size: 11px;
  }
</style>
