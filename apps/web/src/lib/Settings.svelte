<script lang="ts">
  import { onMount } from "svelte";
  import type { ImportResult, NotificationPrefs } from "@breadboard/api-client";
  import { api, store } from "./store.svelte";

  let loading = $state(true);
  let busy = $state(false);
  let prefs = $state<NotificationPrefs | null>(null);
  let projectFilter = $state("");
  let repoFilter = $state("");

  const filteredProjects = $derived.by(() => {
    const q = projectFilter.trim().toLowerCase();
    if (!q) return store.projects;
    return store.projects.filter(
      (p) =>
        p.key.toLowerCase().includes(q) ||
        p.name.toLowerCase().includes(q) ||
        (p.forgejo_repo ?? "").toLowerCase().includes(q),
    );
  });

  const filteredRepos = $derived.by(() => {
    const q = repoFilter.trim().toLowerCase();
    if (!q) return store.repos;
    return store.repos.filter(
      (r) => r.repo.toLowerCase().includes(q) || (r.project_key ?? "").toLowerCase().includes(q),
    );
  });

  // new project form
  let npKey = $state("");
  let npName = $state("");
  let npRepo = $state("");

  // add repo form
  let arRepo = $state("");
  let arKey = $state("");
  let arName = $state("");

  // new cycle form
  let ncName = $state("");
  let ncStart = $state("");
  let ncEnd = $state("");

  // markdown import
  let imProject = $state("");
  let imMarkdown = $state("");
  let imBusy = $state(false);
  let imResult = $state<ImportResult | null>(null);
  let imError = $state<string | null>(null);

  function cycleCounts(id: number) {
    const all = store.issues.filter((i) => i.cycle_id === id);
    return { total: all.length, done: all.filter((i) => i.status === "done").length };
  }

  onMount(() => {
    Promise.all([
      store.loadSettings(),
      api.getNotificationPrefs().then((p) => (prefs = p)),
    ]).finally(() => (loading = false));
  });

  async function savePrefs() {
    if (!prefs) return;
    try {
      prefs = await api.setNotificationPrefs(prefs);
    } catch (e) {
      store.flash(e instanceof Error ? e.message : String(e));
    }
  }

  async function run(fn: () => Promise<unknown>) {
    busy = true;
    try {
      await fn();
      await store.loadSettings();
    } catch (e) {
      store.flash(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
    }
  }

  const createProject = () =>
    run(async () => {
      await api.createProject({
        key: npKey.trim().toUpperCase(),
        name: npName.trim(),
        forgejo_repo: npRepo.trim() || undefined,
      });
      npKey = "";
      npName = "";
      npRepo = "";
    });

  const rename = (id: number, name: string) =>
    run(() => api.updateProject(id, { name }));

  const toggleArchive = (id: number, archived: boolean) =>
    run(() => api.updateProject(id, { archived }));

  const setProjectRepo = (id: number, forgejo_repo: string) =>
    run(() => api.updateProject(id, { forgejo_repo: forgejo_repo.trim() || null }));

  const addRepo = () =>
    run(async () => {
      await api.addRepo(arRepo.trim(), arKey.trim().toUpperCase(), arName.trim() || undefined);
      arRepo = "";
      arKey = "";
      arName = "";
    });

  const removeRepo = (repo: string) =>
    run(() => api.removeRepo(repo));

  const syncNow = () =>
    run(async () => {
      await api.reconcile();
      store.flash("reconcile queued");
    });

  const createCycle = () =>
    run(async () => {
      await api.createCycle({ name: ncName.trim(), starts_on: ncStart, ends_on: ncEnd });
      ncName = "";
      ncStart = "";
      ncEnd = "";
    });

  const toggleCycle = (id: number, closed: boolean) =>
    run(() => (closed ? api.startCycle(id) : api.endCycle(id)));

  async function runImport(dryRun: boolean) {
    if (!imProject || !imMarkdown.trim()) return;
    imBusy = true;
    imError = null;
    try {
      imResult = await api.importMarkdown({ project: imProject, markdown: imMarkdown, dry_run: dryRun });
      if (!dryRun) {
        imMarkdown = "";
        await store.loadAll();
      }
    } catch (e) {
      imError = e instanceof Error ? e.message : String(e);
      imResult = null;
    } finally {
      imBusy = false;
    }
  }
</script>

<div class="wrap">
  {#if loading}
    <p class="dim">loading…</p>
  {:else}
    <section class="card">
      <h2>
        Projects
        <span class="count">{filteredProjects.length}/{store.projects.length}</span>
        <input
          class="search"
          type="search"
          placeholder="Filter…"
          bind:value={projectFilter}
        />
      </h2>
      <table>
        <thead>
          <tr><th>Key</th><th>Name</th><th>Repo</th><th></th></tr>
        </thead>
        <tbody>
          {#each filteredProjects as p (p.id)}
            <tr>
              <td class="mono">{p.key}</td>
              <td>
                <input
                  value={p.name}
                  onblur={(e) => e.currentTarget.value.trim() && e.currentTarget.value !== p.name && rename(p.id, e.currentTarget.value.trim())}
                />
              </td>
              <td class="mono">
                <input
                  class="repo-input"
                  value={p.forgejo_repo ?? ""}
                  placeholder="owner/name"
                  onblur={(e) => e.currentTarget.value.trim() !== (p.forgejo_repo ?? "") && setProjectRepo(p.id, e.currentTarget.value)}
                />
              </td>
              <td class="right">
                <span class="badge" class:on={!p.archived} class:off={p.archived}>
                  {p.archived ? "archived" : "active"}
                </span>
                <button disabled={busy} onclick={() => toggleArchive(p.id, !p.archived)}>
                  {p.archived ? "Unarchive" : "Archive"}
                </button>
              </td>
            </tr>
          {:else}
            <tr><td colspan="4" class="dim">No projects match "{projectFilter}".</td></tr>
          {/each}
        </tbody>
      </table>
      <div class="add-form">
        <span class="add-label">Add project</span>
        <div class="add-row">
          <input placeholder="KEY" bind:value={npKey} maxlength="8" style="width:80px" />
          <input placeholder="Name" bind:value={npName} />
          <input placeholder="owner/name (optional)" bind:value={npRepo} style="width:180px" />
          <button class="primary" disabled={busy || !npKey.trim() || !npName.trim()} onclick={createProject}>
            Add project
          </button>
        </div>
        <p class="hint">
          Repo-less projects are for planning / roadmap items. Setting a repo here just attaches the
          mapping; use "Add &amp; sync a repo" below to also enable sync and import its issues.
        </p>
      </div>
    </section>

    <section class="card">
      <h2>
        Synced repos
        <span class="count">{filteredRepos.length}/{store.repos.length}</span>
        <input
          class="search"
          type="search"
          placeholder="Filter…"
          bind:value={repoFilter}
        />
        <button class="ghost" disabled={busy} onclick={syncNow}>Sync now</button>
      </h2>
      <table>
        <thead>
          <tr><th>Repo</th><th>Project</th><th>Status</th><th>Last synced</th><th></th></tr>
        </thead>
        <tbody>
          {#each filteredRepos as r (r.repo)}
            <tr>
              <td class="mono">{r.repo}</td>
              <td class="mono">{r.project_key ?? "—"}</td>
              <td><span class="badge" class:on={r.enabled} class:off={!r.enabled}>{r.enabled ? "enabled" : "disabled"}</span></td>
              <td class="dim">{r.last_synced_at ? new Date(r.last_synced_at).toLocaleString() : "never"}</td>
              <td class="right">
                {#if r.enabled}
                  <button disabled={busy} onclick={() => removeRepo(r.repo)}>Disable</button>
                {/if}
              </td>
            </tr>
          {:else}
            <tr><td colspan="5" class="dim">{store.repos.length ? `No repos match "${repoFilter}".` : "No repos synced yet."}</td></tr>
          {/each}
        </tbody>
      </table>
      <div class="add-form">
        <span class="add-label">Add &amp; sync a repo</span>
        <div class="add-row">
          <input placeholder="owner/name" bind:value={arRepo} style="width:180px" />
          <input placeholder="KEY" bind:value={arKey} maxlength="8" style="width:80px" />
          <input placeholder="display name (optional)" bind:value={arName} />
          <button class="primary" disabled={busy || !arRepo.includes('/') || !arKey.trim()} onclick={addRepo}>
            Add &amp; sync
          </button>
        </div>
        <p class="hint">
          Creates the project if the key is free, enables sync, and imports the repo's issues.
          Register a webhook at <code>{store.serverBase}/webhooks/forgejo</code> for real-time updates
          (the reconcile pass is the fallback).
        </p>
      </div>
    </section>

    <section class="card">
      <h2>Cycles &amp; releases</h2>
      <table>
        <thead>
          <tr><th>Name</th><th>Dates</th><th>Issues</th><th>Status</th><th></th></tr>
        </thead>
        <tbody>
          {#each store.cycles as c (c.id)}
            {@const counts = cycleCounts(c.id)}
            <tr>
              <td>{c.name}</td>
              <td class="dim mono">{c.starts_on} → {c.ends_on}</td>
              <td class="dim">{counts.done}/{counts.total} done</td>
              <td><span class="badge" class:on={!c.closed} class:off={c.closed}>{c.closed ? "closed" : "open"}</span></td>
              <td class="right">
                <button disabled={busy} onclick={() => toggleCycle(c.id, c.closed)}>
                  {c.closed ? "Reopen" : "Close & archive"}
                </button>
              </td>
            </tr>
          {:else}
            <tr><td colspan="5" class="dim">No cycles yet.</td></tr>
          {/each}
        </tbody>
      </table>
      <div class="add-form">
        <span class="add-label">New cycle</span>
        <div class="add-row">
          <input placeholder="Name (e.g. BOS 0.6.2)" bind:value={ncName} style="width:200px" />
          <input type="date" bind:value={ncStart} />
          <input type="date" bind:value={ncEnd} />
          <button
            class="primary"
            disabled={busy || !ncName.trim() || !ncStart || !ncEnd}
            onclick={createCycle}
          >
            Add cycle
          </button>
        </div>
        <p class="hint">
          A cycle spans every project: use it for a release like "BOS 0.6.2" that pulls in work
          from several repos. Closing one archives its done issues off the default board/backlog
          view; they're still there by picking that cycle from the header filter.
        </p>
      </div>
    </section>

    <section class="card">
      <h2>Import markdown outline</h2>
      <p class="hint">
        Paste an outline; "##" headings become epics and "- [ ]" / "- [x]" items become issues,
        filed under the project you pick.
      </p>
      <div class="add-row">
        <select bind:value={imProject}>
          <option value="">Select project…</option>
          {#each store.projects as p (p.id)}<option value={p.key}>{p.key} · {p.name}</option>{/each}
        </select>
      </div>
      <textarea
        class="import-area"
        rows="8"
        bind:value={imMarkdown}
        placeholder={"## Epic title\n- [ ] An issue\n- [x] A done issue"}
      ></textarea>
      <div class="add-row">
        <button disabled={imBusy || !imProject || !imMarkdown.trim()} onclick={() => runImport(true)}>
          Preview (dry run)
        </button>
        <button
          class="primary"
          disabled={imBusy || !imProject || !imMarkdown.trim()}
          onclick={() => runImport(false)}
        >
          Import
        </button>
      </div>
      {#if imError}
        <p class="err">{imError}</p>
      {/if}
      {#if imResult}
        <p class="hint">
          {imResult.epics_created.length} epic(s), {imResult.issues_created.length} issue(s)
          {imResult.skipped.length ? `, ${imResult.skipped.length} skipped` : ""}
        </p>
        {#if imResult.epics_created.length}<p class="hint">Epics: {imResult.epics_created.join(", ")}</p>{/if}
        {#if imResult.issues_created.length}<p class="hint">Issues: {imResult.issues_created.join(", ")}</p>{/if}
        {#if imResult.skipped.length}<p class="hint">Skipped: {imResult.skipped.join(", ")}</p>{/if}
      {/if}
    </section>

    {#if prefs}
      <section class="card">
        <h2>Notifications</h2>
        <label class="toggle">
          <input type="checkbox" bind:checked={prefs.assigned} onchange={savePrefs} />
          <span class="track"><span class="thumb"></span></span>
          <span>When an issue is assigned to me</span>
        </label>
        <label class="toggle">
          <input type="checkbox" bind:checked={prefs.mention} onchange={savePrefs} />
          <span class="track"><span class="thumb"></span></span>
          <span>When I'm @mentioned in a comment or description</span>
        </label>
        <label class="toggle">
          <input type="checkbox" bind:checked={prefs.cycle} onchange={savePrefs} />
          <span class="track"><span class="thumb"></span></span>
          <span>When a cycle I have issues in starts or ends</span>
        </label>
        <p class="hint">
          Desktop pop-ups use your browser's notification permission. For pop-ups on the bread
          desktop, run <code>modules/breadboard.lua</code> in breadd.
        </p>
      </section>
    {/if}
  {/if}
</div>

<style>
  .wrap {
    height: calc(100vh - 46px);
    overflow-y: auto;
    padding: 20px 24px;
    max-width: 900px;
    margin: 0 auto;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px 18px;
    margin-bottom: 20px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 13px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-dim);
    border-bottom: 1px solid var(--border);
    padding-bottom: 10px;
    margin-bottom: 8px;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
    margin: 4px 0;
  }
  th {
    text-align: left;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-dim);
    padding: 6px 8px;
  }
  td {
    padding: 8px;
    border-top: 1px solid var(--bg);
  }
  tbody tr:hover td {
    background: color-mix(in srgb, var(--bg) 50%, transparent);
  }
  .mono {
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }
  .repo-input {
    width: 100%;
    box-sizing: border-box;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }
  .dim {
    color: var(--ink-dim);
  }
  .right {
    text-align: right;
    white-space: nowrap;
  }
  .badge {
    display: inline-block;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--ink-dim);
    margin-right: 8px;
  }
  .badge.on {
    color: var(--status-done);
    border-color: var(--status-done);
  }
  .badge.off {
    color: var(--ink-dim);
    border-color: var(--border);
  }
  input,
  select,
  textarea {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 7px;
    color: var(--ink);
    font: inherit;
  }
  .add-form {
    margin-top: 14px;
    padding: 12px 14px;
    background: var(--bg);
    border: 1px dashed var(--border);
    border-radius: 8px;
  }
  .add-label {
    display: block;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-dim);
    margin-bottom: 8px;
  }
  .add-row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
  button {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 12px;
    color: var(--ink);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  button:hover:not(:disabled) {
    border-color: var(--accent);
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button.ghost {
    margin-left: auto;
  }
  .count {
    font-weight: 400;
    text-transform: none;
    letter-spacing: normal;
    font-variant-numeric: tabular-nums;
    color: var(--ink-dim);
  }
  .search {
    margin-left: auto;
    width: 160px;
    font-size: 12px;
    text-transform: none;
    letter-spacing: normal;
  }
  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #000;
    font-weight: 600;
  }
  button.primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  button.primary:disabled {
    background: var(--surface);
    border-color: var(--border);
    color: var(--ink-dim);
  }
  .hint {
    font-size: 12px;
    color: var(--ink-dim);
    margin-top: 8px;
  }
  .err {
    font-size: 12px;
    color: #e05252;
    margin-top: 8px;
  }
  .import-area {
    width: 100%;
    box-sizing: border-box;
    font-family: ui-monospace, monospace;
    font-size: 12px;
    resize: vertical;
    margin-top: 10px;
  }
  code {
    background: var(--surface);
    padding: 1px 4px;
    border-radius: 3px;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    padding: 8px 0;
    cursor: pointer;
  }
  .toggle input {
    position: absolute;
    opacity: 0;
    width: 0;
    height: 0;
  }
  .toggle .track {
    position: relative;
    width: 34px;
    height: 20px;
    flex-shrink: 0;
    border-radius: 999px;
    background: var(--bg);
    border: 1px solid var(--border);
    transition: background 0.15s, border-color 0.15s;
  }
  .toggle .thumb {
    position: absolute;
    top: 1px;
    left: 1px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--ink-dim);
    transition: transform 0.15s, background 0.15s;
  }
  .toggle input:checked + .track {
    background: color-mix(in srgb, var(--accent) 30%, var(--bg));
    border-color: var(--accent);
  }
  .toggle input:checked + .track .thumb {
    transform: translateX(14px);
    background: var(--accent);
  }
</style>
