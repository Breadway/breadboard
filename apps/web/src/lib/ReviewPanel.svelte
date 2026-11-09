<script lang="ts">
  import {
    safeHttpUrl,
    type ReviewRequest,
    type ReviewState,
  } from "@breadboard/api-client";
  import { ago, api, errMsg, store } from "./store.svelte";

  /**
   * Two modes, one component:
   * - `issueKey`: the requester's view on a card, including a create form.
   * - `reviewer`: that reviewer's queue, without a create form.
   * Both use the same records, so a human and an agent see identical rows.
   */
  let {
    issueKey = null,
    reviewer = null,
    compact = false,
  }: { issueKey?: string | null; reviewer?: string | null; compact?: boolean } = $props();

  let requests = $state<ReviewRequest[]>([]);
  let loading = $state(false);
  let busy = $state(0);
  let error = $state("");
  let showForm = $state(false);
  let createReviewer = $state("");
  let createKind = $state<"auto" | "human" | "agent">("auto");
  let createUrl = $state("");
  let createNumber = $state("");
  let createNote = $state("");
  let noteDrafts = $state<Record<number, string>>({});

  const STATE_LABEL: Record<ReviewState, string> = {
    requested: "Requested",
    accepted: "Accepted",
    changes_requested: "Changes requested",
    approved: "Approved",
  };

  const suggestions = $derived(
    [...new Set(store.roster.map((r) => r.agent_name).concat(store.me ? [store.me.login] : []))].sort(),
  );
  const outstanding = $derived(requests.filter((r) => r.state !== "approved"));

  async function load() {
    loading = true;
    try {
      requests = issueKey
        ? await api.listIssueReviews(issueKey)
        : await api.listReviews({ reviewer: reviewer ?? undefined, outstanding: true });
      error = "";
    } catch (e) {
      error = errMsg(e);
    } finally {
      loading = false;
    }
  }

  // Refetch when the target changes or a review event arrives.
  $effect(() => {
    void issueKey;
    void reviewer;
    void store.reviewTick;
    void load();
  });

  function prLabel(r: ReviewRequest) {
    if (r.pr_number != null) return r.pr_repo ? `${r.pr_repo}#${r.pr_number}` : `PR #${r.pr_number}`;
    return r.pr_url ? "PR link" : "no PR";
  }

  function prHref(r: ReviewRequest) {
    return r.pr_url ? safeHttpUrl(r.pr_url) : null;
  }

  async function act(id: number, state: ReviewState) {
    if (busy) return;
    busy = id;
    error = "";
    try {
      const note = (noteDrafts[id] ?? "").trim();
      await api.updateReview(id, {
        state,
        ...(note ? { note } : {}),
        ...(reviewer ? { agent_name: reviewer } : {}),
      });
      noteDrafts[id] = "";
      await load();
    } catch (e) {
      error = errMsg(e);
    } finally {
      busy = 0;
    }
  }

  async function create() {
    if (!issueKey || busy) return;
    const reviewerName = createReviewer.trim();
    if (!reviewerName) {
      error = "Name the reviewer";
      return;
    }
    const number = createNumber.trim() ? Number(createNumber.trim()) : undefined;
    if (!createUrl.trim() && number == null) {
      error = "Give the request a PR URL or number";
      return;
    }
    if (number != null && !Number.isInteger(number)) {
      error = "PR number must be a whole number";
      return;
    }
    busy = -1;
    error = "";
    try {
      await api.requestReview(issueKey, {
        reviewer: reviewerName,
        ...(createKind !== "auto" ? { reviewer_kind: createKind } : {}),
        ...(createUrl.trim() ? { pr_url: createUrl.trim() } : {}),
        ...(number != null ? { pr_number: number } : {}),
        ...(createNote.trim() ? { note: createNote.trim() } : {}),
      });
      showForm = false;
      createReviewer = "";
      createKind = "auto";
      createUrl = "";
      createNumber = "";
      createNote = "";
      await load();
    } catch (e) {
      error = errMsg(e);
    } finally {
      busy = 0;
    }
  }
</script>

<section class="sec reviews" class:compact>
  <h3>
    Reviews
    {#if outstanding.length}<span class="count" title="Outstanding requests block done">{outstanding.length} open</span>{/if}
    <span class="grow"></span>
    {#if issueKey}
      <button onclick={() => (showForm = !showForm)}>{showForm ? "Cancel" : "+ Request"}</button>
    {/if}
  </h3>

  {#if error}<p class="err">{error}</p>{/if}

  {#if showForm && issueKey}
    <div class="form">
      <div class="row">
        <input
          class="field"
          list="reviewer-names"
          placeholder="Reviewer (human login or agent name)"
          bind:value={createReviewer}
          aria-label="Reviewer"
        />
        <select class="field kind-sel" bind:value={createKind} aria-label="Reviewer kind">
          <option value="auto">Auto</option>
          <option value="human">Human</option>
          <option value="agent">Agent</option>
        </select>
      </div>
      <datalist id="reviewer-names">
        {#each suggestions as name (name)}<option value={name}></option>{/each}
      </datalist>
      <div class="row">
        <input class="field" placeholder="PR URL" bind:value={createUrl} aria-label="PR URL" />
        <input class="field num" placeholder="PR #" bind:value={createNumber} aria-label="PR number" />
      </div>
      <input class="field" placeholder="What should they look at? (optional)" bind:value={createNote} aria-label="Review note" />
      <div class="row end">
        <button class="btn primary" disabled={busy !== 0} onclick={create}>
          {busy === -1 ? "Requesting…" : "Request review"}
        </button>
      </div>
    </div>
  {/if}

  {#each requests as r (r.id)}
    <div class="req" class:done={r.state === "approved"} class:esc={!!r.escalated_at}>
      <div class="top">
        <span class="chip state-{r.state}">{STATE_LABEL[r.state]}</span>
        <b class="who">{r.reviewer}</b>
        <span class="kind">{r.reviewer_kind}</span>
        {#if r.escalated_at}<span class="chip esc-chip" title="Untouched past the escalation threshold">escalated</span>{/if}
        <span class="grow"></span>
        {#if prHref(r)}
          <a class="pr" href={prHref(r)} target="_blank" rel="noreferrer">{prLabel(r)}</a>
        {:else}
          <span class="pr dim">{prLabel(r)}</span>
        {/if}
        <time title={new Date(r.requested_at).toLocaleString()}>{ago(r.requested_at)}</time>
      </div>
      {#if !compact}
        <div class="meta">
          <button class="key" onclick={() => (store.selectedKey = r.issue_key)}>{r.issue_key}</button>
          <span class="tx">{r.issue_title}</span>
          {#if r.requested_by_login}<span class="dim">asked by {r.requested_by_login}</span>{/if}
        </div>
      {/if}
      {#if r.note}<p class="note">{r.note}</p>{/if}
      {#if r.state !== "approved"}
        <div class="actions">
          <input
            class="field note-in"
            placeholder="Outcome note (optional)"
            bind:value={noteDrafts[r.id]}
            aria-label="Review outcome note"
          />
          <button class="btn sm" disabled={busy === r.id} onclick={() => act(r.id, "accepted")}>Accept</button>
          <button class="btn sm warn" disabled={busy === r.id} onclick={() => act(r.id, "changes_requested")}>Changes</button>
          <button class="btn sm primary" disabled={busy === r.id} onclick={() => act(r.id, "approved")}>Approve</button>
        </div>
      {:else if r.responded_at}
        <div class="meta"><span class="dim">resolved {ago(r.responded_at)}</span></div>
      {/if}
    </div>
  {:else}
    <p class="dim small">{loading ? "Loading…" : issueKey ? "No review requests on this issue." : "No open review requests."}</p>
  {/each}
</section>

<style>
  .reviews {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .grow {
    flex: 1;
  }
  .count {
    font: 500 10px/1 var(--font-mono);
    color: var(--warn);
    border: 1px solid color-mix(in oklab, var(--warn), transparent 45%);
    border-radius: 999px;
    padding: 3px 7px;
  }
  .err {
    margin: 0;
    color: var(--bad);
    font-size: 12px;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 10px;
    background: var(--bg);
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .row.end {
    justify-content: flex-end;
  }
  .num {
    width: 90px;
    flex: none;
  }
  .kind-sel {
    width: 86px;
    flex: none;
  }
  .req {
    border: 1px solid var(--line);
    border-left: 3px solid var(--warn);
    border-radius: 7px;
    padding: 9px 10px;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .req.done {
    border-left-color: var(--ok);
    opacity: 0.78;
  }
  .req.esc {
    border-left-color: var(--bad);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }
  .who {
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind {
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
    text-transform: uppercase;
  }
  .chip {
    font: 500 10px/1 var(--font-mono);
    border-radius: 999px;
    padding: 3px 7px;
    border: 1px solid var(--line-2);
    color: var(--ink-dim);
    white-space: nowrap;
  }
  .state-requested {
    color: var(--warn);
    border-color: color-mix(in oklab, var(--warn), transparent 45%);
  }
  .state-accepted {
    color: var(--info, #6aa9ff);
    border-color: color-mix(in oklab, var(--info, #6aa9ff), transparent 45%);
  }
  .state-changes_requested {
    color: var(--bad);
    border-color: color-mix(in oklab, var(--bad), transparent 45%);
  }
  .state-approved {
    color: var(--ok);
    border-color: color-mix(in oklab, var(--ok), transparent 45%);
  }
  .esc-chip {
    color: var(--bad);
    border-color: color-mix(in oklab, var(--bad), transparent 40%);
  }
  .pr {
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-dim);
    text-decoration: none;
    white-space: nowrap;
  }
  .pr:hover {
    color: var(--ink);
  }
  time {
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .meta {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    font-size: 12px;
  }
  .meta .key {
    border: 0;
    background: none;
    padding: 0;
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-dim);
  }
  .meta .tx {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .note {
    margin: 0;
    font-size: 12px;
    color: var(--ink-dim);
    white-space: pre-wrap;
  }
  .actions {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .note-in {
    flex: 1;
    min-width: 0;
  }
  .small {
    font-size: 11px;
  }
  .dim {
    color: var(--ink-3);
  }
</style>
