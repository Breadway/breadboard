<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "./lib/store.svelte";
  import Board from "./lib/Board.svelte";
  import Backlog from "./lib/Backlog.svelte";
  import Roadmap from "./lib/Roadmap.svelte";
  import Settings from "./lib/Settings.svelte";
  import NotificationBell from "./lib/NotificationBell.svelte";
  import IssueDrawer from "./lib/IssueDrawer.svelte";
  import CommandPalette from "./lib/CommandPalette.svelte";

  let devLoginName = $state("riley");

  onMount(() => {
    void store.init();
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        store.paletteOpen = !store.paletteOpen;
      } else if (e.key === "c" && !store.paletteOpen && !isTyping(e)) {
        e.preventDefault();
        store.paletteOpen = true;
      } else if (e.key === "Escape" && store.selectedKey) {
        store.selectedKey = null;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function isTyping(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    return t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
  }
</script>

<svelte:head><title>breadboard</title></svelte:head>

{#if store.auth === "loading"}
  <div class="center dim">connecting…</div>
{:else if store.auth === "out"}
  <div class="center signin">
    <h1>breadboard</h1>
    <p class="dim">Issue tracker for the bread ecosystem.</p>
    <a class="primary" href="/auth/login">Sign in with git.breadway.dev</a>
    {#if store.isDev}
      <div class="dev">
        <span class="dim">dev:</span>
        <input bind:value={devLoginName} />
        <button onclick={() => store.devLogin(devLoginName)}>dev login</button>
      </div>
    {/if}
  </div>
{:else}
  <header>
    <strong>breadboard</strong>

    <div class="seg">
      <button class:on={store.view === "board"} onclick={() => (store.view = "board")}>Board</button>
      <button class:on={store.view === "backlog"} onclick={() => (store.view = "backlog")}>Backlog</button>
      <button class:on={store.view === "roadmap"} onclick={() => (store.view = "roadmap")}>Roadmap</button>
      <button class:on={store.view === "settings"} onclick={() => (store.view = "settings")}>Settings</button>
    </div>

    {#if store.view === "board" || store.view === "backlog"}
      <select bind:value={store.filterProject} title="Filter by project">
        <option value="">All projects</option>
        {#each store.projects as p (p.id)}<option value={p.key}>{p.key}</option>{/each}
      </select>

      <select bind:value={store.filterCycle} title="Filter by cycle">
        <option value={0}>All cycles</option>
        {#each store.cycles as c (c.id)}<option value={c.id}>{c.name}{c.closed ? " (closed)" : ""}</option>{/each}
      </select>

      <button class:on={store.filterMine} onclick={() => (store.filterMine = !store.filterMine)}>Mine</button>
      <button class="new" onclick={() => (store.paletteOpen = true)}>+ New <kbd>C</kbd></button>
    {/if}

    <div class="right">
      <span class="dot" class:ok={store.connected} title={store.connected ? "live" : "reconnecting"}></span>
      <NotificationBell />
      <span class="dim">{store.me?.login}</span>
      <button class="link" onclick={() => store.logout()}>sign out</button>
    </div>
  </header>

  {#if store.view === "board"}
    <Board />
  {:else if store.view === "backlog"}
    <Backlog />
  {:else if store.view === "roadmap"}
    <Roadmap />
  {:else}
    <Settings />
  {/if}

  <IssueDrawer />
  <CommandPalette />

  {#if store.toast}
    <div class="toast">{store.toast}</div>
  {/if}
{/if}

<style>
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 14px;
    border-bottom: 1px solid var(--border);
    height: 46px;
    box-sizing: border-box;
  }
  header select,
  header button {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--ink);
    padding: 4px 8px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .seg {
    display: flex;
  }
  .seg button {
    border-radius: 0;
  }
  .seg button:first-child {
    border-radius: 5px 0 0 5px;
  }
  .seg button:last-child {
    border-radius: 0 5px 5px 0;
    border-left: none;
  }
  button.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .new kbd {
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 3px;
    font-size: 10px;
    color: var(--ink-dim);
  }
  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .link {
    background: none !important;
    border: none !important;
    color: var(--ink-dim) !important;
    text-decoration: underline;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ink-dim);
  }
  .dot.ok {
    background: #4caf50;
  }
  .center {
    height: 100vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
  }
  .signin h1 {
    margin: 0;
  }
  .primary {
    background: var(--accent);
    color: #000;
    text-decoration: none;
    padding: 9px 16px;
    border-radius: 6px;
    font-weight: 600;
  }
  .dev {
    margin-top: 20px;
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .dev input {
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--ink);
    border-radius: 5px;
    padding: 4px 6px;
  }
  .dev button {
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--ink);
    border-radius: 5px;
    padding: 4px 10px;
    cursor: pointer;
  }
  .dim {
    color: var(--ink-dim);
  }
  .toast {
    position: fixed;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    background: #2a1a1a;
    border: 1px solid #e05252;
    color: #f0c0c0;
    padding: 8px 14px;
    border-radius: 6px;
    font-size: 13px;
    z-index: 50;
  }
</style>
