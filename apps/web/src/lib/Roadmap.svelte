<script lang="ts">
  import { STATUSES, type EpicView } from "@breadboard/api-client";
  import { api, store } from "./store.svelte";

  let busy = $state(false);
  let expanded = $state<number | null>(null);
  let neKey = $state("");
  let neTitle = $state("");
  let confirmingDelete = $state<number | null>(null);
  let confirmTimer: ReturnType<typeof setTimeout> | undefined;

  async function reload() {
    store.epics = await api.listEpics();
  }

  async function run(fn: () => Promise<unknown>) {
    busy = true;
    try {
      await fn();
      await reload();
    } catch (e) {
      store.flash(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  const create = () =>
    run(async () => {
      await api.createEpic({ key: neKey.trim().toUpperCase(), title: neTitle.trim() });
      neKey = "";
      neTitle = "";
    });

  const rename = (e: EpicView, title: string) =>
    run(() => api.updateEpic(e.id, { title }));

  const archive = (e: EpicView) => run(() => api.updateEpic(e.id, { archived: !e.archived }));

  function requestDelete(e: EpicView) {
    if (confirmingDelete === e.id) {
      clearTimeout(confirmTimer);
      confirmingDelete = null;
      void run(() => api.deleteEpic(e.id));
    } else {
      confirmingDelete = e.id;
      confirmTimer = setTimeout(() => (confirmingDelete = null), 3000);
    }
  }

  function pct(e: EpicView) {
    return e.total ? Math.round(((e.progress.done ?? 0) / e.total) * 100) : 0;
  }

  function childIssues(epicId: number) {
    return store.issues
      .filter((i) => i.epic_id === epicId)
      .sort((a, b) => STATUSES.indexOf(a.status) - STATUSES.indexOf(b.status));
  }
</script>

<div class="wrap">
  <div class="head">
    <h1>Roadmap</h1>
    <div class="new">
      <input placeholder="KEY" bind:value={neKey} maxlength="12" style="width:90px" />
      <input placeholder="Epic title" bind:value={neTitle} />
      <button disabled={busy || !neKey.trim() || !neTitle.trim()} onclick={create}>New epic</button>
    </div>
  </div>

  {#each store.epics as e (e.id)}
    <section class="epic" class:archived={e.archived}>
      <div class="epic-head">
        <span class="key">{e.key}</span>
        <input
          class="title"
          value={e.title}
          onblur={(ev) => ev.currentTarget.value.trim() && ev.currentTarget.value !== e.title && rename(e, ev.currentTarget.value.trim())}
        />
        <span class="count">{e.progress.done ?? 0}/{e.total}</span>
        <button class="ghost" onclick={() => (expanded = expanded === e.id ? null : e.id)}>
          {expanded === e.id ? "hide" : "issues"}
        </button>
        <button class="ghost" disabled={busy} onclick={() => archive(e)}>
          {e.archived ? "unarchive" : "archive"}
        </button>
        <button class="ghost danger" class:confirm={confirmingDelete === e.id} disabled={busy} onclick={() => requestDelete(e)}>
          {confirmingDelete === e.id ? "confirm delete" : "delete"}
        </button>
      </div>

      {#if e.description?.trim()}
        <p class="epic-description">{e.description}</p>
      {/if}

      <div class="bar" title="{pct(e)}% done">
        {#each STATUSES as s (s)}
          {@const n = e.progress[s] ?? 0}
          {#if n > 0}
            <div class="seg s-{s}" style="flex: {n}" title="{n} {s}"></div>
          {/if}
        {/each}
        {#if e.total === 0}<div class="seg empty" style="flex:1"></div>{/if}
      </div>

      {#if expanded === e.id}
        <div class="children">
          {#each childIssues(e.id) as i (i.id)}
            <button class="child" onclick={() => (store.selectedKey = i.key)}>
              <span class="pill s-{i.status}">{i.status}</span>
              <span class="ckey">{i.key}</span>
              {i.title}
            </button>
          {:else}
            <p class="dim">No issues in this epic yet.</p>
          {/each}
        </div>
      {/if}
    </section>
  {:else}
    <p class="dim">No epics yet. Create one above, then set an issue's epic from its detail panel.</p>
  {/each}
</div>

<style>
  .wrap { height: calc(100vh - 46px); overflow-y: auto; padding: 18px 22px; max-width: 900px; }
  .head { display: flex; align-items: baseline; gap: 16px; margin-bottom: 14px; }
  h1 { margin: 0; font-size: 20px; font-weight: 700; letter-spacing: -0.01em; }
  .new { display: flex; gap: 6px; margin-left: auto; }
  .epic { border: 1px solid var(--border); border-radius: 8px; padding: 12px; margin-bottom: 10px; background: var(--surface); }
  .epic.archived { opacity: 0.55; }
  .epic-head { display: flex; align-items: center; gap: 10px; }
  .key { font: 600 12px/1 ui-monospace, monospace; color: var(--ink-dim); }
  .title { flex: 1; background: transparent; border: 1px solid transparent; border-radius: 5px; padding: 4px 6px; color: var(--ink); font: 600 14px/1.2 inherit; }
  .title:hover, .title:focus { border-color: var(--border); background: var(--bg); }
  .count { font-variant-numeric: tabular-nums; color: var(--ink-dim); font-size: 12px; }
  .epic-description { margin: 8px 0 0; color: var(--ink-dim); font-size: 12px; line-height: 1.4; white-space: pre-wrap; }
  .bar { display: flex; height: 6px; border-radius: 3px; overflow: hidden; margin-top: 10px; background: var(--bg); }
  .seg { min-width: 2px; }
  .seg.s-backlog { background: var(--status-backlog); }
  .seg.s-todo { background: var(--status-todo); }
  .seg.s-doing { background: var(--status-doing); }
  .seg.s-review { background: var(--status-review); }
  .seg.s-done { background: var(--status-done); }
  .seg.empty { background: var(--border); }
  .children { margin-top: 10px; display: flex; flex-direction: column; gap: 3px; }
  .child { text-align: left; background: var(--bg); border: 1px solid var(--border); border-radius: 5px; padding: 5px 8px; color: var(--ink); font: inherit; font-size: 12px; cursor: pointer; display: flex; gap: 8px; align-items: center; }
  .child:hover { border-color: var(--accent); }
  .ckey { color: var(--ink-dim); font-family: ui-monospace, monospace; }
  .pill { font-size: 10px; padding: 0 6px; border-radius: 999px; border: 1px solid var(--border); text-transform: capitalize; }
  .s-backlog { color: var(--status-backlog); } .s-todo { color: var(--status-todo); }
  .s-doing { color: var(--status-doing); } .s-review { color: var(--status-review); }
  .s-done { color: var(--status-done); }
  button { background: var(--surface); border: 1px solid var(--border); border-radius: 5px; padding: 4px 10px; color: var(--ink); font: inherit; font-size: 12px; cursor: pointer; }
  button:hover:not(:disabled) { border-color: var(--accent); }
  button:disabled { opacity: 0.5; }
  .ghost { background: transparent; }
  .ghost.danger { color: #e05252; border-color: transparent; }
  .ghost.danger:hover { border-color: #e05252; }
  .ghost.danger.confirm { background: #e05252; border-color: #e05252; color: #fff; }
  input { background: var(--bg); border: 1px solid var(--border); border-radius: 5px; padding: 5px 7px; color: var(--ink); font: inherit; }
  .dim { color: var(--ink-dim); }
</style>
