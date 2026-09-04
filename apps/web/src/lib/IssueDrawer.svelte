<script lang="ts">
  import { PRIORITIES, STATUSES, type IssueDetail, type Priority, type Status } from "@breadboard/api-client";
  import { api, store } from "./store.svelte";

  let detail = $state<IssueDetail | null>(null);
  let loading = $state(false);
  let titleDraft = $state("");
  let bodyDraft = $state("");
  let labelsDraft = $state("");

  $effect(() => {
    const key = store.selectedKey;
    if (!key) {
      detail = null;
      return;
    }
    loading = true;
    api
      .getIssue(key)
      .then((d) => {
        detail = d;
        titleDraft = d.title;
        bodyDraft = d.body;
        labelsDraft = d.labels.join(", ");
      })
      .catch(() => store.flash(`couldn't load ${key}`))
      .finally(() => (loading = false));
  });

  async function save(patch: Parameters<typeof store.patch>[1]) {
    if (!detail) return;
    const updated = await store.patch(detail.key, patch);
    detail = { ...detail, ...updated };
  }

  function saveTitle() {
    if (detail && titleDraft.trim() && titleDraft !== detail.title) {
      void save({ title: titleDraft.trim() });
    }
  }
  function saveBody() {
    if (detail && bodyDraft !== detail.body) void save({ body: bodyDraft });
  }
  function saveLabels() {
    if (!detail) return;
    const next = labelsDraft
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    if (next.join(",") !== detail.labels.join(",")) void save({ labels: next });
  }

  const assignedToMe = $derived(detail?.assignee_id === store.me?.id);
</script>

{#if store.selectedKey}
  <aside class="drawer">
    <header>
      <span class="key">{store.selectedKey}</span>
      <div class="spacer"></div>
      {#if detail?.forgejo_ref}
        <span class="fg">⇄ {detail.forgejo_ref.repo}#{detail.forgejo_ref.number}</span>
      {/if}
      <button class="icon" title="Close" onclick={() => (store.selectedKey = null)}>✕</button>
    </header>

    {#if loading && !detail}
      <p class="dim pad">loading…</p>
    {:else if detail}
      <div class="body">
        <input
          class="title"
          bind:value={titleDraft}
          onblur={saveTitle}
          onkeydown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        />

        <div class="row">
          <label>
            Status
            <select
              value={detail.status}
              onchange={(e) => save({ status: e.currentTarget.value as Status })}
            >
              {#each STATUSES as s (s)}<option value={s}>{s}</option>{/each}
            </select>
          </label>
          <label>
            Priority
            <select
              value={detail.priority}
              onchange={(e) => save({ priority: e.currentTarget.value as Priority })}
            >
              {#each PRIORITIES as p (p)}<option value={p}>{p}</option>{/each}
            </select>
          </label>
          <label>
            Estimate
            <input
              type="number"
              min="0"
              value={detail.estimate ?? ""}
              onchange={(e) =>
                save({ estimate: e.currentTarget.value === "" ? null : Number(e.currentTarget.value) })}
            />
          </label>
        </div>

        <div class="row">
          <label class="grow">
            Labels
            <input bind:value={labelsDraft} onblur={saveLabels} placeholder="comma, separated" />
          </label>
          <button
            class="btn"
            onclick={() => save({ assignee_id: assignedToMe ? null : store.me?.id })}
          >
            {assignedToMe ? "Unassign" : "Assign to me"}
          </button>
        </div>

        {#if store.epics.length}
          <label class="block">
            Epic
            <select
              value={detail.epic_id ?? ""}
              onchange={(e) =>
                save({ epic_id: e.currentTarget.value === "" ? null : Number(e.currentTarget.value) })}
            >
              <option value="">—</option>
              {#each store.epics as ep (ep.id)}<option value={ep.id}>{ep.key} · {ep.title}</option>{/each}
            </select>
          </label>
        {/if}

        <label class="block">
          Description
          <textarea rows="6" bind:value={bodyDraft} onblur={saveBody}></textarea>
        </label>

        <section class="comments">
          <h3>Comments {detail.comments.length ? `(${detail.comments.length})` : ""}</h3>
          {#each detail.comments as c (c.id)}
            <div class="comment">
              <div class="meta dim">{new Date(c.created_at).toLocaleString()}</div>
              <div>{c.body}</div>
            </div>
          {:else}
            <p class="dim">No comments. (Comment sync lands with the Forgejo engine.)</p>
          {/each}
        </section>

        <footer>
          <button class="btn danger" onclick={() => detail && store.remove(detail.key)}>
            Delete issue
          </button>
        </footer>
      </div>
    {/if}
  </aside>
{/if}

<style>
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    width: min(440px, 100vw);
    height: 100vh;
    background: var(--surface);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    z-index: 20;
    box-shadow: -12px 0 40px rgba(0, 0, 0, 0.4);
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .key {
    font-variant-numeric: tabular-nums;
    color: var(--ink-dim);
  }
  .fg {
    font-size: 11px;
    color: var(--ink-dim);
  }
  .icon {
    background: none;
    border: none;
    color: var(--ink-dim);
    cursor: pointer;
    font-size: 14px;
  }
  .body {
    padding: 12px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .title {
    font-size: 16px;
    font-weight: 600;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    color: var(--ink);
  }
  .row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-dim);
  }
  label.block {
    width: 100%;
  }
  label.grow {
    flex: 1;
  }
  select,
  input,
  textarea {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 6px;
    color: var(--ink);
    font: inherit;
    text-transform: none;
    letter-spacing: normal;
  }
  textarea {
    resize: vertical;
  }
  .btn {
    align-self: flex-end;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 10px;
    color: var(--ink);
    cursor: pointer;
    font-size: 12px;
  }
  .btn:hover {
    border-color: var(--accent);
  }
  .btn.danger:hover {
    border-color: #e05252;
    color: #e05252;
  }
  .comments h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-dim);
    margin: 4px 0 8px;
  }
  .comment {
    border-top: 1px solid var(--border);
    padding: 8px 0;
    font-size: 13px;
  }
  .meta {
    font-size: 11px;
    margin-bottom: 2px;
  }
  .dim {
    color: var(--ink-dim);
  }
  .pad {
    padding: 12px;
  }
  footer {
    border-top: 1px solid var(--border);
    padding-top: 12px;
  }
</style>
