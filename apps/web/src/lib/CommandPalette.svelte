<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { PRIORITIES, type NewIssue, type Priority } from "@breadboard/api-client";
  import { store } from "./store.svelte";

  let query = $state("");
  let cursor = $state(0);
  let inputEl = $state<HTMLInputElement | null>(null);

  // Optional extra fields for issue creation, collapsed by default; the
  // command palette otherwise only ever sets a title.
  let showOptions = $state(false);
  let optBody = $state("");
  let optPriority = $state<Priority | "">("");
  let optLabels = $state("");
  let optProject = $state("");
  let optEpic = $state<number | 0>(0);
  let optCycle = $state<number | 0>(0);
  let optEstimate = $state<number | "">("");

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

  const hasCreate = $derived(items.some((i) => i.kind === "create"));

  $effect(() => {
    if (store.paletteOpen) {
      query = "";
      cursor = 0;
      showOptions = false;
      optBody = "";
      optPriority = "";
      optLabels = "";
      optProject = "";
      optEpic = 0;
      optCycle = 0;
      optEstimate = "";
      queueMicrotask(() => inputEl?.focus());
    }
  });

  $effect(() => {
    void items;
    cursor = 0;
  });

  async function run(item: Item) {
    if (item.kind === "create") {
      const input: NewIssue = { title: query.trim() };
      if (optBody.trim()) input.body = optBody.trim();
      if (optPriority) input.priority = optPriority;
      if (optProject) input.project = optProject;
      if (optEpic) input.epic_id = optEpic;
      if (optCycle) input.cycle_id = optCycle;
      if (optEstimate !== "") input.estimate = optEstimate;
      const labels = optLabels
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      if (labels.length) input.labels = labels;
      const issue = await store.create(input);
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
      <div class="input-row">
        <input
          class="search-input"
          bind:this={inputEl}
          bind:value={query}
          onkeydown={onKey}
          placeholder="Search issues, or type to create…"
        />
        {#if hasCreate}
          <button class="opts-toggle" onclick={() => (showOptions = !showOptions)}>
            {showOptions ? "fewer options" : "more options"}
          </button>
        {/if}
      </div>

      {#if hasCreate && showOptions}
        <div class="options">
          <div class="opt-row">
            <select bind:value={optPriority}>
              <option value="">Priority…</option>
              {#each PRIORITIES as p (p)}<option value={p}>{p}</option>{/each}
            </select>
            <select bind:value={optProject}>
              <option value="">Project…</option>
              {#each store.projects as p (p.id)}<option value={p.key}>{p.key}</option>{/each}
            </select>
            <select bind:value={optCycle}>
              <option value={0}>Cycle…</option>
              {#each store.cycles as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
            </select>
            <select bind:value={optEpic}>
              <option value={0}>Epic…</option>
              {#each store.epics as ep (ep.id)}<option value={ep.id}>{ep.key}</option>{/each}
            </select>
            <input
              class="estimate"
              type="number"
              min="0"
              placeholder="Est."
              value={optEstimate}
              oninput={(e) => (optEstimate = e.currentTarget.value === "" ? "" : Number(e.currentTarget.value))}
            />
          </div>
          <input placeholder="Labels, comma separated" bind:value={optLabels} />
          <textarea rows="3" placeholder="Description (markdown)" bind:value={optBody}></textarea>
        </div>
      {/if}

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
  .input-row {
    display: flex;
    align-items: center;
    border-bottom: 1px solid var(--border);
  }
  .search-input {
    flex: 1;
    width: 100%;
    border: none;
    background: var(--bg);
    color: var(--ink);
    padding: 12px 14px;
    font: inherit;
    font-size: 14px;
  }
  .search-input:focus {
    outline: none;
  }
  .opts-toggle {
    background: none;
    border: none;
    color: var(--ink-dim);
    font-size: 11px;
    white-space: nowrap;
    padding: 0 14px;
    cursor: pointer;
  }
  .opts-toggle:hover {
    color: var(--accent);
  }
  .options {
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .opt-row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .options select,
  .options input,
  .options textarea {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 7px;
    color: var(--ink);
    font: inherit;
    font-size: 12px;
  }
  .options textarea {
    resize: vertical;
    width: 100%;
    box-sizing: border-box;
  }
  .options .estimate {
    width: 64px;
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
