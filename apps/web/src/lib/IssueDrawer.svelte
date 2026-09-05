<script lang="ts">
  import { fly } from "svelte/transition";
  import {
    PRIORITIES,
    STATUSES,
    type ActivityEntry,
    type IssueDetail,
    type Priority,
    type Status,
  } from "@breadboard/api-client";
  import { api, store } from "./store.svelte";
  import { renderMarkdown } from "./markdown";

  let detail = $state<IssueDetail | null>(null);
  let activity = $state<ActivityEntry[]>([]);
  let tab = $state<"comments" | "activity">("comments");
  let loading = $state(false);
  let titleDraft = $state("");
  let bodyDraft = $state("");
  let labelsDraft = $state("");
  let commentDraft = $state("");
  let confirmingDelete = $state(false);
  let confirmTimer: ReturnType<typeof setTimeout> | undefined;
  let editingBody = $state(false);

  function requestDelete() {
    if (confirmingDelete) {
      clearTimeout(confirmTimer);
      confirmingDelete = false;
      if (detail) void store.remove(detail.key);
    } else {
      confirmingDelete = true;
      confirmTimer = setTimeout(() => (confirmingDelete = false), 3000);
    }
  }

  $effect(() => {
    const key = store.selectedKey;
    clearTimeout(confirmTimer);
    confirmingDelete = false;
    editingBody = false;
    if (!key) {
      detail = null;
      activity = [];
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
    api.getActivity(key).then((a) => (activity = a)).catch(() => {});
  });

  const linkLabels: Record<string, string> = {
    closes: "Closes",
    "closed-by": "Closed by",
    blocks: "Blocks",
    "blocked-by": "Blocked by",
    "relates-to": "Relates to",
  };

  function describe(a: ActivityEntry): string {
    const who = a.actor ?? (a.source === "breadboard" ? "someone" : "Forgejo");
    if (a.field === "created") return `${who} created this (${a.new_value ?? ""})`;
    const from = a.old_value || "—";
    const to = a.new_value || "—";
    return `${who} changed ${a.field}: ${from} → ${to}`;
  }

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
  function changeProject(project: string) {
    if (detail) void store.moveProject(detail.key, project);
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

  async function postComment() {
    if (!detail || !commentDraft.trim()) return;
    try {
      const c = await api.addComment(detail.key, commentDraft.trim());
      detail = { ...detail, comments: [...detail.comments, c] };
      commentDraft = "";
    } catch (e) {
      store.flash(`comment failed: ${e instanceof Error ? e.message : e}`);
    }
  }
</script>

{#if store.selectedKey}
  <aside class="drawer" transition:fly={{ x: 40, duration: 150 }}>
    <header>
      <span class="key">{store.selectedKey}</span>
      <div class="spacer"></div>
      {#if detail?.milestone}
        <span class="fg">◆ {detail.milestone}</span>
      {/if}
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

        {#if detail.links.length}
          <div class="links">
            {#each detail.links as l (l.kind + l.target_id)}
              {@const t = store.findIssue(l.target_id)}
              <button class="link-chip" onclick={() => t && (store.selectedKey = t.key)}>
                {linkLabels[l.kind] ?? l.kind} {t?.key ?? `#${l.target_id}`}{t ? ` · ${t.title}` : ""}
              </button>
            {/each}
          </div>
        {/if}

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

        <div class="row">
          <label class="grow">
            Project
            <select
              value={store.projectKey(detail.project_id)}
              onchange={(e) => changeProject(e.currentTarget.value)}
            >
              {#each store.projects as p (p.id)}<option value={p.key}>{p.key} · {p.name}</option>{/each}
            </select>
          </label>
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

        <div class="block">
          <div class="field-head">
            <span>Description</span>
            <button class="link-btn" onclick={() => (editingBody = !editingBody)}>
              {editingBody ? "Preview" : "Edit"}
            </button>
          </div>
          {#if editingBody}
            <textarea
              rows="6"
              bind:value={bodyDraft}
              onblur={() => {
                saveBody();
                editingBody = false;
              }}
            ></textarea>
          {:else}
            <div class="md">{@html renderMarkdown(detail.body || "*No description.*")}</div>
          {/if}
        </div>

        <section class="comments">
          <div class="tabs">
            <button class:on={tab === "comments"} onclick={() => (tab = "comments")}>
              Comments {detail.comments.length ? `(${detail.comments.length})` : ""}
            </button>
            <button class:on={tab === "activity"} onclick={() => (tab = "activity")}>
              Activity {activity.length ? `(${activity.length})` : ""}
            </button>
          </div>

          {#if tab === "comments"}
            {#each detail.comments as c (c.id)}
              <div class="comment">
                <div class="meta dim">
                  {new Date(c.created_at).toLocaleString()}{c.external_id ? " · synced" : ""}
                </div>
                <div class="md">{@html renderMarkdown(c.body)}</div>
              </div>
            {:else}
              <p class="dim">No comments yet.</p>
            {/each}
            <textarea
              rows="2"
              bind:value={commentDraft}
              placeholder="Add a comment…"
              onkeydown={(e) => (e.metaKey || e.ctrlKey) && e.key === "Enter" && postComment()}
            ></textarea>
            <button class="btn" onclick={postComment} disabled={!commentDraft.trim()}>Comment</button>
          {:else}
            {#each activity as a (a.at + a.field)}
              <div class="event">
                <div class="meta dim">{new Date(a.at).toLocaleString()}</div>
                <div>{describe(a)}</div>
              </div>
            {:else}
              <p class="dim">No recorded activity.</p>
            {/each}
          {/if}
        </section>

        <footer>
          <button class="btn danger" class:confirm={confirmingDelete} onclick={requestDelete}>
            {confirmingDelete ? "Click again to confirm" : "Delete issue"}
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
  .links {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .link-chip {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 3px 10px;
    color: var(--ink-dim);
    font-size: 12px;
    cursor: pointer;
  }
  .link-chip:hover {
    border-color: var(--accent);
    color: var(--ink);
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
  .block {
    width: 100%;
  }
  .field-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-dim);
    margin-bottom: 3px;
  }
  .link-btn {
    background: none;
    border: none;
    color: var(--ink-dim);
    text-decoration: underline;
    cursor: pointer;
    font-size: 11px;
    text-transform: none;
    letter-spacing: normal;
    padding: 0;
  }
  .link-btn:hover {
    color: var(--accent);
  }
  .md {
    font-size: 13px;
    line-height: 1.5;
  }
  :global(.md p) {
    margin: 0 0 8px;
  }
  :global(.md p:last-child) {
    margin-bottom: 0;
  }
  :global(.md ul),
  :global(.md ol) {
    margin: 4px 0 8px;
    padding-left: 20px;
  }
  :global(.md code) {
    background: var(--bg);
    padding: 1px 4px;
    border-radius: 3px;
    font-size: 12px;
    font-family: ui-monospace, monospace;
  }
  :global(.md pre) {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    overflow-x: auto;
  }
  :global(.md pre code) {
    background: none;
    padding: 0;
  }
  :global(.md a) {
    color: var(--accent);
  }
  :global(.md blockquote) {
    border-left: 2px solid var(--border);
    margin: 4px 0;
    padding-left: 10px;
    color: var(--ink-dim);
  }
  :global(.md h1),
  :global(.md h2),
  :global(.md h3) {
    font-size: 1em;
    font-weight: 600;
    margin: 8px 0 4px;
  }
  :global(.md img) {
    max-width: 100%;
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
  .btn.danger.confirm {
    background: #e05252;
    border-color: #e05252;
    color: #fff;
  }
  .tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 8px;
  }
  .tabs button {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--ink-dim);
    padding: 4px 2px;
    margin-right: 10px;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
  }
  .tabs button.on {
    color: var(--ink);
    border-bottom-color: var(--accent);
  }
  .comment,
  .event {
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
