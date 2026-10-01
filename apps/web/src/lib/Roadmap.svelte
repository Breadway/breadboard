<script lang="ts">
  import { STATUSES, type Cycle, type EpicView, type Issue } from "@breadboard/api-client";
  import { api, errMsg, store } from "./store.svelte";
  import Menu, { type MenuItem } from "./ui/Menu.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";

  // Epics are rows, cycles are columns: the last couple of shipped cycles,
  // every open one, then a column for work not in any cycle.
  const SHIPPED_SHOWN = 2;
  const CELL_MAX = 6;

  let busy = $state(false);
  let neKey = $state("");
  let neTitle = $state("");
  let showArchived = $state(false);
  let confirming = $state<number | null>(null);

  const columns = $derived.by(() => {
    const shipped = store.cycles
      .filter((c) => c.closed)
      .sort((a, b) => b.ends_on.localeCompare(a.ends_on))
      .slice(0, SHIPPED_SHOWN)
      .reverse();
    const open = store.cycles.filter((c) => !c.closed).sort((a, b) => a.starts_on.localeCompare(b.starts_on));
    return [...shipped, ...open, null] as (Cycle | null)[];
  });
  const active = $derived(store.activeCycle);
  const epics = $derived(store.epics.filter((e) => showArchived || !e.archived));
  const archivedCount = $derived(store.epics.filter((e) => e.archived).length);

  function cell(epicId: number, cycle: Cycle | null): Issue[] {
    return store.issues
      .filter((i) => i.epic_id === epicId && i.type !== "pr" && (cycle ? i.cycle_id === cycle.id : !i.cycle_id || !columns.some((c) => c?.id === i.cycle_id)))
      .sort((a, b) => STATUSES.indexOf(b.status) - STATUSES.indexOf(a.status));
  }

  const fmt = (d: string) => new Date(d + "T12:00:00").toLocaleDateString(undefined, { month: "short", day: "numeric" });

  async function run(fn: () => Promise<unknown>) {
    busy = true;
    try {
      await fn();
      store.epics = await api.listEpics();
    } catch (e) {
      store.flash(errMsg(e));
    } finally {
      busy = false;
    }
  }

  const create = () =>
    run(async () => {
      await api.createEpic({ key: neKey.trim().toUpperCase(), title: neTitle.trim() });
      store.info(`Created epic ${neKey.trim().toUpperCase()}`);
      neKey = "";
      neTitle = "";
    });

  function rename(e: EpicView, title: string) {
    const t = title.trim();
    if (t && t !== e.title) void run(() => api.updateEpic(e.id, { title: t }));
  }

  function epicMenu(e: EpicView): MenuItem[] {
    return [
      { label: "Show its issues on the board", run: () => { store.filterEpic = e.id; store.view = "board"; } },
      { label: e.archived ? "Unarchive" : "Archive", run: () => void run(() => api.updateEpic(e.id, { archived: !e.archived })) },
      { label: "Delete epic…", danger: true, run: () => (confirming = e.id) },
    ];
  }
</script>

<div class="wrap">
  <div class="head">
    <p class="lede">Each row is an epic; each column is a cycle. Issues sit where they're scheduled.</p>
    <div class="new">
      <input class="field" placeholder="KEY" bind:value={neKey} maxlength="12" style="width:90px" aria-label="New epic key" />
      <input class="field" placeholder="Epic title" bind:value={neTitle} aria-label="New epic title" onkeydown={(e) => e.key === "Enter" && neKey.trim() && neTitle.trim() && create()} />
      <button class="btn" disabled={busy || !neKey.trim() || !neTitle.trim()} onclick={create}>New epic</button>
    </div>
  </div>

  {#if epics.length}
    <div class="scroll">
      <div class="grid" style="grid-template-columns: 260px repeat({columns.length}, minmax(190px, 1fr))">
        <div class="ch corner">Epic</div>
        {#each columns as c (c?.id ?? "none")}
          <div class="ch" class:now={c && c.id === active?.id}>
            <span class="kind">{c ? (c.closed ? "Shipped" : c.id === active?.id ? "Current" : "Planned") : "Unscheduled"}</span>
            <b>{c ? c.name : "Not in a cycle"}</b>
            <span class="dates">{c ? `${fmt(c.starts_on)} → ${fmt(c.ends_on)}` : " "}</span>
          </div>
        {/each}

        {#each epics as e (e.id)}
          {@const done = e.progress.done ?? 0}
          <div class="epic" class:archived={e.archived}>
            <div class="ek-row">
              <span class="ek">{e.key}</span>
              {#if e.archived}<span class="arch">archived</span>{/if}
              <span class="grow"></span>
              <Menu items={epicMenu(e)} triggerClass="ib" title="Epic actions" align="right">
                {#snippet trigger()}<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3.5" cy="8" r="1.3" fill="currentColor" /><circle cx="8" cy="8" r="1.3" fill="currentColor" /><circle cx="12.5" cy="8" r="1.3" fill="currentColor" /></svg>{/snippet}
              </Menu>
            </div>
            <input class="et" value={e.title} aria-label="Epic title" onblur={(ev) => rename(e, ev.currentTarget.value)} onkeydown={(ev) => ev.key === "Enter" && ev.currentTarget.blur()} />
            {#if e.description?.trim()}<p class="ed">{e.description}</p>{/if}
            <div class="sbar" title="{done} of {e.total} done">
              {#each STATUSES as s (s)}
                {#if e.progress[s]}<i style="flex:{e.progress[s]};background:var(--status-{s})"></i>{/if}
              {/each}
              {#if !e.total}<i class="none"></i>{/if}
            </div>
            <span class="pct">{done}/{e.total} done</span>
            {#if confirming === e.id}
              <div class="confirm">
                <span>Delete {e.key}? Its issues stay, unlinked.</span>
                <button class="btn ghost" onclick={() => (confirming = null)}>Cancel</button>
                <button class="btn danger confirm" onclick={() => { confirming = null; void run(() => api.deleteEpic(e.id)); }}>Delete</button>
              </div>
            {/if}
          </div>
          {#each columns as c (c?.id ?? "none")}
            {@const list = cell(e.id, c)}
            <div class="cell" class:now={c && c.id === active?.id}>
              {#if list.length}
                <div class="sbar thin">
                  {#each STATUSES as s (s)}
                    {@const n = list.filter((i) => i.status === s).length}
                    {#if n}<i style="flex:{n};background:var(--status-{s})"></i>{/if}
                  {/each}
                </div>
                {#each list.slice(0, CELL_MAX) as i (i.id)}
                  <button class="mini" onclick={() => (store.selectedKey = i.key)} title="{i.key}: {i.title}">
                    <StatusGlyph status={i.status} size={12} /><span>{i.title}</span>
                  </button>
                {/each}
                {#if list.length > CELL_MAX}
                  <button class="more" onclick={() => { store.filterEpic = e.id; store.filterCycle = c?.id ?? 0; store.view = "backlog"; }}>{list.length - CELL_MAX} more</button>
                {/if}
              {:else}
                <span class="empty">·</span>
              {/if}
            </div>
          {/each}
        {/each}
      </div>
    </div>
  {:else}
    <div class="blank">
      <p>No epics yet. Create one above, then set an issue's epic from its panel.</p>
    </div>
  {/if}

  {#if archivedCount}
    <button class="btn ghost toggle" onclick={() => (showArchived = !showArchived)}>
      {showArchived ? "Hide" : "Show"} {archivedCount} archived {archivedCount === 1 ? "epic" : "epics"}
    </button>
  {/if}
</div>

<style>
  .wrap {
    height: 100%;
    overflow-y: auto;
    padding: 16px 20px 32px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  .lede {
    margin: 0;
    color: var(--ink-3);
    font-size: 13px;
  }
  .new {
    display: flex;
    gap: 6px;
    margin-left: auto;
    flex-wrap: wrap;
  }
  .scroll {
    overflow-x: auto;
    border: 1px solid var(--line);
    border-radius: 12px;
  }
  .grid {
    display: grid;
    min-width: max-content;
  }
  .grid > div {
    padding: 12px 14px;
    border-right: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }
  .ch {
    background: var(--surface);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .ch.corner {
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-3);
    justify-content: flex-end;
  }
  .ch.now {
    background: linear-gradient(var(--accent-soft), var(--surface));
  }
  .kind {
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .now .kind {
    color: var(--accent);
  }
  .ch b {
    font: 700 14px/1.2 var(--font-display);
    letter-spacing: -0.01em;
  }
  .dates {
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
    white-space: pre;
  }
  .epic {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .epic.archived {
    opacity: 0.55;
  }
  .ek-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: -4px;
  }
  .ek {
    font: 500 10.5px/1 var(--font-mono);
    letter-spacing: 0.06em;
    color: var(--accent);
  }
  .arch {
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .grow {
    flex: 1;
  }
  .ek-row :global(.ib) {
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 6px;
    background: none;
    display: grid;
    place-items: center;
    color: var(--ink-3);
  }
  .ek-row :global(.ib:hover) {
    background: var(--raise);
    color: var(--ink);
  }
  .ek-row :global(.ib svg) {
    width: 14px;
    height: 14px;
  }
  .et {
    margin-left: -6px;
    padding: 3px 6px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: none;
    font: 700 15px/1.25 var(--font-display);
    letter-spacing: -0.01em;
    outline: none;
  }
  .et:hover {
    border-color: var(--line);
  }
  .et:focus {
    border-color: var(--line-2);
    background: var(--bg);
  }
  .ed {
    margin: 0;
    font-size: 12px;
    line-height: 1.4;
    color: var(--ink-3);
  }
  .sbar {
    display: flex;
    gap: 2px;
    height: 6px;
    border-radius: 3px;
    overflow: hidden;
    background: var(--sunk);
    margin-top: 2px;
  }
  .sbar.thin {
    height: 5px;
    margin: 0 0 4px;
  }
  .sbar i {
    display: block;
    min-width: 2px;
  }
  .sbar .none {
    flex: 1;
    background: var(--line);
  }
  .pct {
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-dim);
  }
  .confirm {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    padding: 8px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--danger) 8%, var(--bg));
    font-size: 12px;
  }
  .confirm span {
    flex-basis: 100%;
  }
  .cell {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .cell.now {
    background: color-mix(in srgb, var(--accent) 3%, transparent);
  }
  .mini {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    padding: 3px 4px;
    margin: 0 -4px;
    border: 0;
    border-radius: 5px;
    background: none;
    font-size: 12.5px;
    color: var(--ink-dim);
    text-align: left;
  }
  .mini span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mini:hover {
    background: var(--raise);
    color: var(--ink);
  }
  .more {
    align-self: flex-start;
    border: 0;
    background: none;
    padding: 3px 0;
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .more:hover {
    color: var(--accent);
  }
  .empty {
    color: var(--ink-3);
    opacity: 0.5;
  }
  .blank {
    padding: 60px 0;
    text-align: center;
    color: var(--ink-3);
  }
  .toggle {
    margin-top: 12px;
  }
</style>
