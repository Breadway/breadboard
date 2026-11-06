<script lang="ts">
  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { bare, isTyping, store, type View } from "./lib/store.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import TopBar from "./lib/TopBar.svelte";
  import Board from "./lib/Board.svelte";
  import Backlog from "./lib/Backlog.svelte";
  import Roadmap from "./lib/Roadmap.svelte";
  import Agents from "./lib/Agents.svelte";
  import Chat from "./lib/Chat.svelte";
  import Inbox from "./lib/Inbox.svelte";
  import Settings from "./lib/Settings.svelte";
  import IssueDrawer from "./lib/IssueDrawer.svelte";
  import CommandPalette from "./lib/CommandPalette.svelte";
  import { closeOpenMenu } from "./lib/ui/Menu.svelte";

  let devLoginName = $state("riley");

  // #new (the desktop shell's quick-add hotkey) opens the palette; #KEY
  // (from "Copy link") opens that issue once the board has loaded.
  let pendingKey: string | null = null;
  function onHash() {
    const h = decodeURIComponent(location.hash.slice(1));
    if (!h) return;
    if (h === "new") store.paletteOpen = true;
    else if (/^[A-Za-z]+-\d+$/.test(h)) pendingKey = h.toUpperCase();
    history.replaceState(null, "", location.pathname + location.search);
  }
  $effect(() => {
    if (pendingKey && store.auth === "in" && store.issueByKey(pendingKey)) {
      store.selectedKey = pendingKey;
      pendingKey = null;
    }
  });

  onMount(() => {
    void store.init();
    onHash();
    window.addEventListener("hashchange", onHash);

    const onKey = (e: KeyboardEvent) => {
      if (store.auth !== "in") return;
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        store.paletteOpen = !store.paletteOpen;
        return;
      }
      // Esc closes the top layer only: palette (handled in the palette), then
      // an open menu, then a focused field, then the issue panel.
      if (e.key === "Escape") {
        if (store.paletteOpen) {
          store.paletteOpen = false;
          return;
        }
        if (closeOpenMenu()) return;
        if (isTyping(e)) {
          (e.target as HTMLElement).blur();
          return;
        }
        if (store.selectedKey) store.selectedKey = null;
        return;
      }
      // Bare C only, so Ctrl+C still copies.
      if (e.key === "c" && bare(e) && !store.paletteOpen && !isTyping(e)) {
        e.preventDefault();
        store.openPalette();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("hashchange", onHash);
    };
  });

  const tabs: { v: View; label: string }[] = [
    { v: "board", label: "Board" },
    { v: "backlog", label: "Backlog" },
    { v: "roadmap", label: "Roadmap" },
    { v: "agents", label: "Agents" },
    { v: "chat", label: "Chat" },
    { v: "inbox", label: "Inbox" },
    { v: "settings", label: "Settings" },
  ];
</script>

<svelte:head><title>{store.unread ? `(${store.unread}) ` : ""}breadboard</title></svelte:head>

{#snippet logo(size: number)}
  <svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
    <path d="M4.5 10.2C2.6 9.4 2.4 5 6.6 4.1 9 3.6 10.5 4 12 4s3-.4 5.4.1c4.2.9 4 5.3 2.1 6.1V19a1.6 1.6 0 0 1-1.6 1.6H6.1A1.6 1.6 0 0 1 4.5 19z" fill="var(--accent)" />
    <path d="M7.5 10.6v7.4M12 10.6v7.4M16.5 10.6v7.4M5.6 14.3h12.8" stroke="var(--on-accent)" stroke-width="1.5" stroke-linecap="round" opacity=".55" />
  </svg>
{/snippet}

{#if store.auth === "loading"}
  <div class="splash">
    {@render logo(40)}
    <p class="dim">Connecting…</p>
  </div>
{:else if store.auth === "error"}
  <div class="splash">
    {@render logo(40)}
    <h1>Can't reach breadboard</h1>
    <p class="dim">{store.authError}. Retrying on its own; you can also try now.</p>
    <button class="btn primary" onclick={() => store.retryNow()}>Retry now</button>
  </div>
{:else if store.auth === "out"}
  <div class="splash signin">
    <div class="glow" aria-hidden="true"></div>
    {@render logo(52)}
    <h1>breadboard</h1>
    <p class="dim">Planning for the bread ecosystem, synced both ways with Forgejo.</p>
    <a class="btn primary big" href="/auth/login">Sign in with git.breadway.dev</a>
    {#if store.isDev || store.devLoginAllowed}
      <div class="dev">
        <span class="dim">Dev login</span>
        <input class="field" bind:value={devLoginName} aria-label="Dev login name" />
        <button class="btn" onclick={() => store.devLogin(devLoginName)}>Sign in</button>
      </div>
    {/if}
  </div>
{:else}
  <div class="app">
    <Sidebar />
    <main class="main">
      <TopBar />
      <div class="view">
        {#if store.view === "board"}
          <Board />
        {:else if store.view === "backlog"}
          <Backlog />
        {:else if store.view === "roadmap"}
          <Roadmap />
        {:else if store.view === "agents"}
          <Agents />
        {:else if store.view === "chat"}
          <Chat />
        {:else if store.view === "inbox"}
          <Inbox />
        {:else}
          <Settings />
        {/if}
      </div>
      <nav class="tabbar" aria-label="Views">
        {#each tabs as t (t.v)}
          <button class:on={store.view === t.v} onclick={() => (store.view = t.v)}>
            {t.label}{#if t.v === "inbox" && store.unread}<span class="dot"></span>{/if}{#if t.v === "agents" && store.activeAgents}<span class="pill">{store.activeAgents}</span>{/if}
          </button>
        {/each}
      </nav>
    </main>
  </div>

  <IssueDrawer />
  <CommandPalette />

  {#if store.toast}
    {#key store.toast.id}
      <div class="toast" class:err={store.toast.kind === "error"} role="status" transition:fly={{ y: 12, duration: 200 }}>
        <i></i>
        <span>{store.toast.msg}</span>
        {#if store.toast.undo}
          <button
            onclick={() => {
              const u = store.toast?.undo;
              store.toast = null;
              u?.();
            }}>Undo</button>
        {/if}
      </div>
    {/key}
  {/if}
{/if}

<style>
  .app {
    height: 100%;
    display: grid;
    grid-template-columns: 236px minmax(0, 1fr);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .view {
    flex: 1;
    min-height: 0;
  }
  .tabbar {
    display: none;
  }

  .splash {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 24px;
    text-align: center;
    overflow: hidden;
  }
  .splash h1 {
    margin: 4px 0 0;
    font: 800 30px/1.1 var(--font-display);
    letter-spacing: -0.03em;
  }
  .signin h1 {
    font-size: 44px;
  }
  .splash p {
    margin: 0;
    max-width: 44ch;
  }
  .glow {
    position: absolute;
    inset: auto 0 -40%;
    height: 80%;
    background: radial-gradient(50% 50% at 50% 50%, color-mix(in srgb, var(--accent) 18%, transparent), transparent 70%);
    pointer-events: none;
  }
  .big {
    height: 40px;
    padding: 0 18px;
    font-size: 14px;
    margin-top: 10px;
    text-decoration: none;
  }
  .dev {
    margin-top: 24px;
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 12px;
  }
  .dim {
    color: var(--ink-dim);
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: 22px;
    translate: -50% 0;
    z-index: 60;
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: calc(100vw - 32px);
    padding: 9px 12px 9px 11px;
    border-radius: 10px;
    background: var(--raise);
    border: 1px solid var(--line-2);
    box-shadow: var(--shadow-pop);
    font-size: 13px;
  }
  .toast i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
    background: var(--ok);
  }
  .toast.err {
    border-color: color-mix(in srgb, var(--danger) 50%, transparent);
  }
  .toast.err i {
    background: var(--danger);
  }
  .toast button {
    border: 0;
    background: none;
    color: var(--accent);
    font-weight: 600;
    font-size: 12.5px;
    padding: 0 0 0 4px;
  }

  @media (max-width: 860px) {
    .app {
      grid-template-columns: minmax(0, 1fr);
    }
    .app :global(.side) {
      display: none;
    }
    .tabbar {
      display: flex;
      justify-content: space-around;
      border-top: 1px solid var(--line);
      background: var(--sunk);
      padding: 4px 4px calc(6px + env(safe-area-inset-bottom, 0px));
    }
    .tabbar button {
      position: relative;
      border: 0;
      background: none;
      padding: 10px 6px;
      font-size: 12px;
      color: var(--ink-3);
    }
    .tabbar button.on {
      color: var(--accent);
    }
    .tabbar .dot {
      position: absolute;
      top: 8px;
      right: 0;
      width: 6px;
      height: 6px;
      border-radius: 50%;
      background: var(--accent);
    }
    .tabbar .pill {
      display: inline-block;
      margin-left: 4px;
      min-width: 15px;
      height: 15px;
      line-height: 15px;
      border-radius: 99px;
      background: var(--ok);
      color: #0c0f0c;
      font: 600 9px/15px var(--font-mono);
      text-align: center;
      padding: 0 3px;
    }
    .toast {
      bottom: 70px;
    }
  }
</style>
