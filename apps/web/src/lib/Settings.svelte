<script lang="ts">
  import { onMount } from "svelte";
  import type { NotificationPrefs } from "@breadboard/api-client";
  import { api, store } from "./store.svelte";

  let loading = $state(true);
  let busy = $state(false);
  let prefs = $state<NotificationPrefs | null>(null);

  // new project form
  let npKey = $state("");
  let npName = $state("");

  // add repo form
  let arRepo = $state("");
  let arKey = $state("");
  let arName = $state("");

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
      await api.createProject({ key: npKey.trim().toUpperCase(), name: npName.trim() });
      npKey = "";
      npName = "";
    });

  const rename = (id: number, name: string) =>
    run(() => api.updateProject(id, { name }));

  const toggleArchive = (id: number, archived: boolean) =>
    run(() => api.updateProject(id, { archived }));

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
</script>

<div class="wrap">
  {#if loading}
    <p class="dim">loading…</p>
  {:else}
    <section class="card">
      <h2>Projects</h2>
      <table>
        <thead>
          <tr><th>Key</th><th>Name</th><th>Repo</th><th></th></tr>
        </thead>
        <tbody>
          {#each store.projects as p (p.id)}
            <tr>
              <td class="mono">{p.key}</td>
              <td>
                <input
                  value={p.name}
                  onblur={(e) => e.currentTarget.value.trim() && e.currentTarget.value !== p.name && rename(p.id, e.currentTarget.value.trim())}
                />
              </td>
              <td class="dim mono">{p.forgejo_repo ?? "—"}</td>
              <td class="right">
                <span class="badge" class:on={!p.archived} class:off={p.archived}>
                  {p.archived ? "archived" : "active"}
                </span>
                <button disabled={busy} onclick={() => toggleArchive(p.id, !p.archived)}>
                  {p.archived ? "Unarchive" : "Archive"}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="add-form">
        <span class="add-label">Add project</span>
        <div class="add-row">
          <input placeholder="KEY" bind:value={npKey} maxlength="8" style="width:80px" />
          <input placeholder="Name" bind:value={npName} />
          <button class="primary" disabled={busy || !npKey.trim() || !npName.trim()} onclick={createProject}>
            Add project
          </button>
        </div>
        <p class="hint">Repo-less projects are for planning / roadmap items. To sync a Forgejo repo, add it below.</p>
      </div>
    </section>

    <section class="card">
      <h2>
        Synced repos
        <button class="ghost" disabled={busy} onclick={syncNow}>Sync now</button>
      </h2>
      <table>
        <thead>
          <tr><th>Repo</th><th>Project</th><th>Status</th><th>Last synced</th><th></th></tr>
        </thead>
        <tbody>
          {#each store.repos as r (r.repo)}
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
            <tr><td colspan="5" class="dim">No repos synced yet.</td></tr>
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
  input {
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
