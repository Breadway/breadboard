<script lang="ts">
  import { store, type View } from "./store.svelte";
  import Ring from "./ui/Ring.svelte";
  import Avatar from "./ui/Avatar.svelte";
  import { projectColor } from "./ui/colors";

  const views: { v: View; label: string }[] = [
    { v: "board", label: "Board" },
    { v: "backlog", label: "Backlog" },
    { v: "roadmap", label: "Roadmap" },
    { v: "agents", label: "Agents" },
    { v: "chat", label: "Chat" },
    { v: "inbox", label: "Inbox" },
  ];

  const CLOSED_SHOWN = 2;

  // Open cycles soonest first, then the most recently closed few.
  const cycles = $derived.by(() => {
    const open = store.cycles.filter((c) => !c.closed).sort((a, b) => a.starts_on.localeCompare(b.starts_on));
    const closed = store.cycles
      .filter((c) => c.closed)
      .sort((a, b) => b.ends_on.localeCompare(a.ends_on))
      .slice(0, CLOSED_SHOWN);
    return [...open, ...closed];
  });

  const cycleStats = $derived.by(() => {
    const m = new Map<number, { done: number; total: number }>();
    for (const i of store.issues) {
      if (!i.cycle_id || i.type === "pr") continue;
      const s = m.get(i.cycle_id) ?? { done: 0, total: 0 };
      s.total++;
      if (i.status === "done") s.done++;
      m.set(i.cycle_id, s);
    }
    return m;
  });

  const openCounts = $derived.by(() => {
    const m = new Map<number, number>();
    for (const i of store.issues) {
      if (i.status === "done" || i.type === "pr") continue;
      m.set(i.project_id, (m.get(i.project_id) ?? 0) + 1);
    }
    return m;
  });

  const projects = $derived(store.projects.filter((p) => !p.archived));
  const totalOpen = $derived(store.issues.filter((i) => i.status !== "done" && i.type !== "pr").length);

  function go(v: View) {
    store.view = v;
  }

  function pickCycle(id: number) {
    store.toggleFilter("cycles", id);
    if (store.view !== "board" && store.view !== "backlog") store.view = "board";
  }

  function pickProject(key: string) {
    store.toggleFilter("projects", key);
    if (store.view !== "board" && store.view !== "backlog") store.view = "board";
  }
</script>

{#snippet icon(v: View)}
  <svg viewBox="0 0 16 16" aria-hidden="true">
    {#if v === "board"}
      <rect x="1.5" y="2" width="3.6" height="12" rx="1" fill="currentColor" />
      <rect x="6.2" y="2" width="3.6" height="8" rx="1" fill="currentColor" opacity=".7" />
      <rect x="10.9" y="2" width="3.6" height="10" rx="1" fill="currentColor" opacity=".45" />
    {:else if v === "backlog"}
      <path d="M2 3.5h12M2 8h12M2 12.5h8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    {:else if v === "roadmap"}
      <path d="M2 4h7M5 8h8M3 12h6" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" />
    {:else if v === "inbox"}
      <path d="M2 9.5 3.8 3h8.4L14 9.5V13H2zM2 9.5h3.5l1 1.7h3l1-1.7H14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" />
    {:else if v === "chat"}
      <path d="M2.2 3.4h11.6a1 1 0 0 1 1 1v6.2a1 1 0 0 1-1 1H7l-3.4 2.6v-2.6H2.2a1 1 0 0 1-1-1V4.4a1 1 0 0 1 1-1z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" />
    {:else if v === "agents"}
      <circle cx="5.5" cy="5" r="2.3" fill="none" stroke="currentColor" stroke-width="1.4" /><circle cx="11" cy="6" r="1.8" fill="none" stroke="currentColor" stroke-width="1.3" opacity=".65" /><path d="M1.5 13.5c0-2.2 1.8-3.5 4-3.5s4 1.3 4 3.5M10 13.5c.2-1.8 1.5-2.8 3-2.8 1 0 1.9.4 2.4 1.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    {:else}
      <circle cx="8" cy="8" r="2.2" fill="none" stroke="currentColor" stroke-width="1.5" />
      <path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M3.6 12.4 5 11M11 5l1.4-1.4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
    {/if}
  </svg>
{/snippet}

<aside class="side" aria-label="Navigation">
  <div class="logo">
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M4.5 10.2C2.6 9.4 2.4 5 6.6 4.1 9 3.6 10.5 4 12 4s3-.4 5.4.1c4.2.9 4 5.3 2.1 6.1V19a1.6 1.6 0 0 1-1.6 1.6H6.1A1.6 1.6 0 0 1 4.5 19z" fill="var(--accent)" />
      <path d="M7.5 10.6v7.4M12 10.6v7.4M16.5 10.6v7.4M5.6 14.3h12.8" stroke="var(--on-accent)" stroke-width="1.5" stroke-linecap="round" opacity=".55" />
    </svg>
    <b>breadboard</b>
    <span class="live" class:ok={store.connected} title={store.connected ? "Live: changes appear as they happen" : "Reconnecting to the live stream…"}>
      <i></i>{store.connected ? "live" : "offline"}
    </span>
  </div>

  <button class="find" onclick={() => store.openPalette()}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="1.6" d="M7 12.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11zm4-1.5 3.5 3.5" /></svg>
    Search or create
    <kbd>Ctrl K</kbd>
  </button>

  <nav>
    {#each views as { v, label } (v)}
      <button class="nav" class:on={store.view === v} aria-current={store.view === v ? "page" : undefined} onclick={() => go(v)}>
        {@render icon(v)}{label}
        {#if v === "backlog"}<span class="n">{totalOpen}</span>{/if}
        {#if v === "agents" && store.activeAgents > 0}<span class="agent-count">{store.activeAgents}</span>{/if}
        {#if v === "inbox" && store.unread > 0}<span class="badge">{store.unread > 99 ? "99+" : store.unread}</span>{/if}
      </button>
    {/each}
  </nav>

  {#if cycles.length}
    <div class="sh">Cycles</div>
    {#each cycles as c (c.id)}
      {@const s = cycleStats.get(c.id) ?? { done: 0, total: 0 }}
      <button class="nav" class:on={store.isFiltered("cycles", c.id)} class:closed={c.closed} onclick={() => pickCycle(c.id)} title="{c.starts_on} → {c.ends_on}">
        <Ring done={s.done} total={s.total} color={c.closed ? "var(--status-done)" : "var(--accent)"} />
        <span class="nm">{c.name}</span>
        <span class="n">{c.closed ? "closed" : `${s.done}/${s.total}`}</span>
      </button>
    {/each}
  {/if}

  <div class="sh">Projects</div>
  <div class="projects">
    {#each projects as p (p.id)}
      <button class="nav" class:on={store.isFiltered("projects", p.key)} onclick={() => pickProject(p.key)} title={p.forgejo_repo ?? "Planning project, not synced"}>
        <span class="pj" style="background:{projectColor(p.key)}">{p.key[0]}</span>
        <span class="nm">{p.name}</span>
        <span class="n">{openCounts.get(p.id) ?? 0}</span>
      </button>
    {/each}
  </div>

  <div class="foot">
    <button class="nav" class:on={store.view === "settings"} onclick={() => go("settings")}>{@render icon("settings")}Settings</button>
    <div class="me">
      <Avatar userId={store.me?.id ?? null} size={22} />
      <span class="nm">{store.me?.login}</span>
      <button class="out" onclick={() => store.logout()}>Sign out</button>
    </div>
  </div>
</aside>

<style>
  .side {
    background: var(--sunk);
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 14px 10px 10px;
    min-height: 0;
    overflow-y: auto;
  }
  .logo {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 2px 8px 14px;
  }
  .logo svg {
    width: 22px;
    height: 22px;
    flex: none;
  }
  .logo b {
    font: 800 17px/1 var(--font-display);
    letter-spacing: -0.025em;
  }
  .live {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 5px;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .live i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ink-3);
  }
  .live.ok i {
    background: var(--ok);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 15%, transparent);
    animation: pulse 2.4s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      box-shadow: 0 0 0 5px transparent;
    }
  }
  .find {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 10px;
    padding: 7px 9px;
    border-radius: 8px;
    background: var(--surface);
    border: 1px solid var(--line);
    color: var(--ink-3);
    font-size: 13px;
    text-align: left;
  }
  .find:hover {
    border-color: var(--line-2);
    color: var(--ink-dim);
  }
  .find svg {
    width: 14px;
    height: 14px;
  }
  .find kbd {
    margin-left: auto;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border: 0;
    border-radius: 7px;
    background: none;
    color: var(--ink-dim);
    font-size: 13.5px;
    text-align: left;
    width: 100%;
    min-width: 0;
  }
  .nav :global(svg) {
    width: 16px;
    height: 16px;
    flex: none;
  }
  .nav > svg {
    opacity: 0.8;
  }
  .nav:hover {
    background: color-mix(in srgb, var(--ink) 4.5%, transparent);
    color: var(--ink);
  }
  .nav.on {
    background: var(--raise);
    color: var(--ink);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .nav.closed .nm {
    color: var(--ink-3);
  }
  .nm {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .n {
    margin-left: auto;
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
  .badge {
    margin-left: auto;
    min-width: 16px;
    height: 16px;
    border-radius: 99px;
    background: var(--accent);
    color: var(--on-accent);
    font: 600 10px/16px var(--font-mono);
    text-align: center;
    padding: 0 4px;
  }
  .agent-count {
    margin-left: auto;
    min-width: 16px;
    height: 16px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--ok) 20%, transparent);
    color: var(--ok);
    border: 1px solid color-mix(in srgb, var(--ok) 45%, transparent);
    font: 600 10px/15px var(--font-mono);
    text-align: center;
    padding: 0 4px;
  }
  .sh {
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-3);
    padding: 16px 8px 6px;
  }
  .projects {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .pj {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    display: grid;
    place-items: center;
    font: 700 9px/1 var(--font-display);
    color: #0f0c09;
    flex: none;
  }
  .foot {
    margin-top: auto;
    padding-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .me {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 10px 8px 2px;
    margin-top: 6px;
    border-top: 1px solid var(--line);
    font-size: 13px;
    color: var(--ink-dim);
  }
  .out {
    margin-left: auto;
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 12px;
    white-space: nowrap;
  }
  .out:hover {
    color: var(--ink);
  }
</style>
