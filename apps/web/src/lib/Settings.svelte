<script lang="ts">
  import { onMount } from "svelte";
  import { api, store } from "./store.svelte";

  let loading = $state(true);
  let busy = $state(false);

  // new project form
  let npKey = $state("");
  let npName = $state("");

  // add repo form
  let arRepo = $state("");
  let arKey = $state("");
  let arName = $state("");

  onMount(() => {
    store.loadSettings().finally(() => (loading = false));
  });

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
    <section>
      <h2>Projects</h2>
      <table>
        <thead>
          <tr><th>Key</th><th>Name</th><th>Repo</th><th></th></tr>
        </thead>
        <tbody>
          {#each store.projects as p (p.id)}
            <tr class:archived={p.archived}>
              <td class="mono">{p.key}</td>
              <td>
                <input
                  value={p.name}
                  onblur={(e) => e.currentTarget.value.trim() && e.currentTarget.value !== p.name && rename(p.id, e.currentTarget.value.trim())}
                />
              </td>
              <td class="dim mono">{p.forgejo_repo ?? "—"}</td>
              <td class="right">
                <button disabled={busy} onclick={() => toggleArchive(p.id, !p.archived)}>
                  {p.archived ? "Unarchive" : "Archive"}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="form">
        <input placeholder="KEY" bind:value={npKey} maxlength="8" style="width:80px" />
        <input placeholder="Name" bind:value={npName} />
        <button disabled={busy || !npKey.trim() || !npName.trim()} onclick={createProject}>
          Add project
        </button>
      </div>
      <p class="hint">Repo-less projects are for planning / roadmap items. To sync a Forgejo repo, add it below.</p>
    </section>

    <section>
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
            <tr class:off={!r.enabled}>
              <td class="mono">{r.repo}</td>
              <td class="mono">{r.project_key ?? "—"}</td>
              <td>{r.enabled ? "enabled" : "disabled"}</td>
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
      <div class="form">
        <input placeholder="owner/name" bind:value={arRepo} style="width:180px" />
        <input placeholder="KEY" bind:value={arKey} maxlength="8" style="width:80px" />
        <input placeholder="display name (optional)" bind:value={arName} />
        <button disabled={busy || !arRepo.includes('/') || !arKey.trim()} onclick={addRepo}>
          Add &amp; sync
        </button>
      </div>
      <p class="hint">
        Creates the project if the key is free, enables sync, and imports the repo's issues.
        Register a webhook at <code>{store.serverBase}/webhooks/forgejo</code> for real-time updates
        (the reconcile pass is the fallback).
      </p>
    </section>
  {/if}
</div>

<style>
  .wrap {
    height: calc(100vh - 46px);
    overflow-y: auto;
    padding: 20px 24px;
    max-width: 900px;
  }
  section {
    margin-bottom: 32px;
  }
  h2 {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-dim);
    border-bottom: 1px solid var(--border);
    padding-bottom: 6px;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
    margin: 8px 0;
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
    padding: 5px 8px;
    border-top: 1px solid var(--surface);
  }
  tr.archived td,
  tr.off td {
    opacity: 0.5;
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
  }
  input {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 7px;
    color: var(--ink);
    font: inherit;
  }
  .form {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
    margin-top: 8px;
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
  .hint {
    font-size: 12px;
    color: var(--ink-dim);
    margin-top: 4px;
  }
  code {
    background: var(--surface);
    padding: 1px 4px;
    border-radius: 3px;
  }
</style>
