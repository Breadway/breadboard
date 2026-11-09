<script lang="ts">
  import type { CommitAttribution, CommitModelSource } from "@breadboard/api-client";
  import { ago, api, errMsg, store } from "./store.svelte";
  import { agentColor } from "./ui/colors";

  let { issueKey }: { issueKey: string } = $props();

  let commits = $state<CommitAttribution[]>([]);
  let loading = $state(false);
  let error = $state("");

  async function load(key = issueKey) {
    if (!key) return;
    loading = true;
    error = "";
    try {
      const rows = await api.listIssueCommits(key);
      if (key === issueKey) commits = rows;
    } catch (e) {
      if (key === issueKey) {
        commits = [];
        error = errMsg(e);
      }
    } finally {
      if (key === issueKey) loading = false;
    }
  }

  $effect(() => {
    const key = issueKey;
    commits = [];
    error = "";
    if (key) void load(key);
  });

  // Recording a commit publishes card.updated, which bumps issueTick.
  $effect(() => {
    const t = store.issueTick;
    if (t.n && t.key === issueKey) void load(t.key);
  });

  function short(value: string, n = 10): string {
    return value.length > n ? value.slice(0, n) : value;
  }

  function modelLabel(commit: CommitAttribution): string | null {
    if (!commit.model) return null;
    if (!commit.provider) return commit.model;
    if (commit.model.startsWith(`${commit.provider}/`) || commit.model.includes("/")) {
      return commit.model;
    }
    return `${commit.provider}/${commit.model}`;
  }

  function sourceTitle(source: CommitModelSource): string {
    switch (source) {
      case "reported":
        return "Model reported explicitly by the agent when the commit was recorded";
      case "session":
        return "Resolved from usage data for this exact session, which recorded a single model";
      default:
        return "No model was reported and this exact session's usage did not resolve to one model";
    }
  }
</script>

<section class="commits" aria-label="Commit attribution">
  <header class="chead">
    <h3>Commits</h3>
    {#if commits.length}<span class="count">{commits.length}</span>{/if}
    <span class="grow"></span>
    {#if commits.some((c) => !c.model)}
      <span class="warnchip" title="At least one commit has no known model">models unknown</span>
    {/if}
  </header>

  {#if error}
    <p class="err">{error}</p>
  {/if}

  {#if commits.length}
    <div class="list">
      {#each commits as commit (commit.id)}
        <article class="commit">
          <div class="top">
            <code title={commit.sha}>{short(commit.sha)}</code>
            <span class="repo" title={commit.repo}>{commit.repo}</span>
            <span class="grow"></span>
            {#if commit.model}
              <span class="model" title={sourceTitle(commit.model_source)}>{modelLabel(commit)}</span>
            {:else}
              <span class="model unknown" title={sourceTitle("unknown")}>model unknown</span>
            {/if}
          </div>
          <div class="meta">
            <span class="agent" style="--agent:{agentColor(commit.agent_name)}">{commit.agent_name}</span>
            {#if commit.orchestrator}
              <span title={commit.orchestrator_session_id ?? "orchestrator"}>via {commit.orchestrator}</span>
            {:else if commit.orchestrator_session_id}
              <span class="mono" title={commit.orchestrator_session_id}>
                orchestrator {short(commit.orchestrator_session_id)}
              </span>
            {/if}
            {#if commit.session_id}
              <span class="mono" title={commit.session_id}>session {short(commit.session_id)}</span>
            {/if}
            {#if commit.issue_key}<span class="mono">{commit.issue_key}</span>{/if}
            <span class="grow"></span>
            <time datetime={commit.created_at} title={new Date(commit.created_at).toLocaleString()}>
              {ago(commit.created_at)}
            </time>
          </div>
          {#if commit.note}<p class="note">{commit.note}</p>{/if}
          {#if commit.patch_id || commit.tree_sha}
            <p class="rewrite" title="Used to match a rebased or squashed commit back to this attribution">
              {#if commit.patch_id}<span>patch {short(commit.patch_id)}</span>{/if}
              {#if commit.tree_sha}<span>tree {short(commit.tree_sha)}</span>{/if}
            </p>
          {/if}
        </article>
      {/each}
    </div>
    <p class="foot">
      Only recorded commits appear here. A commit with no attribution is explicitly unknown, not guessed.
    </p>
  {:else if loading}
    <p class="muted">Loading commits…</p>
  {:else}
    <p class="muted">No commits recorded. Agents call <code>record_commit</code> after committing.</p>
  {/if}
</section>

<style>
  .commits {
    margin: 12px 0 0;
    padding: 12px 14px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: color-mix(in srgb, var(--ink) 2%, transparent);
  }
  .chead {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
  }
  .chead h3 {
    margin: 0;
    font: 600 13px/1 var(--font-display);
  }
  .count {
    padding: 2px 6px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    color: var(--ink-3);
    font: 600 10px/1 var(--font-mono);
  }
  .grow {
    flex: 1;
  }
  .warnchip {
    padding: 3px 7px;
    border: 1px solid color-mix(in srgb, var(--warn, #b7791f) 40%, transparent);
    border-radius: 99px;
    color: color-mix(in srgb, var(--warn, #b7791f) 80%, var(--ink));
    font: 500 9.5px/1 var(--font-mono);
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .commit {
    padding: 8px 9px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: color-mix(in srgb, var(--ink) 3%, transparent);
  }
  .top {
    display: flex;
    align-items: baseline;
    gap: 7px;
    min-width: 0;
  }
  .top code {
    font: 600 11px/1.2 var(--font-mono);
    color: var(--ink);
  }
  .repo {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-3);
    font: 400 10px/1.2 var(--font-mono);
  }
  .model {
    max-width: 55%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 2px 6px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--accent) 13%, transparent);
    color: var(--ink);
    font: 500 10px/1.2 var(--font-mono);
  }
  .model.unknown {
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    color: var(--ink-3);
    font-style: italic;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 9px;
    margin-top: 5px;
    color: var(--ink-3);
    font: 400 10px/1.3 var(--font-mono);
  }
  .agent {
    padding: 1px 6px;
    border: 1px solid color-mix(in srgb, var(--agent) 40%, var(--line));
    border-radius: 99px;
    color: color-mix(in srgb, var(--agent) 75%, var(--ink));
    font-weight: 600;
  }
  .mono {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .note {
    margin: 5px 0 0;
    color: var(--ink-2, var(--ink-3));
    font: 400 11px/1.4 var(--font-ui, sans-serif);
  }
  .rewrite {
    display: flex;
    gap: 10px;
    margin: 4px 0 0;
    color: var(--ink-3);
    font: 400 9.5px/1.3 var(--font-mono);
  }
  .foot {
    margin: 9px 0 0;
    color: var(--ink-3);
    font: 400 9.5px/1.4 var(--font-mono);
  }
  .muted,
  .err {
    margin: 0;
    color: var(--ink-3);
    font: 400 11px/1.4 var(--font-mono);
  }
  .err {
    color: var(--danger);
  }
</style>
