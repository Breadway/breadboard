<script lang="ts">
  import { onMount } from "svelte";
  import { ago, claimStale, store } from "./store.svelte";
  import { agentColor } from "./ui/colors";

  // Tick every 15s so "xm ago" and stale badges go stale live, the same way
  // the opencode bb-status widget re-renders on its stamp. SSE also pushes
  // claim/heartbeat/release events straight into store.agents.
  let now = $state(Date.now());
  let staleOnly = $state(false);

  onMount(() => {
    void store.loadAgents();
    const t = setInterval(() => {
      now = Date.now();
      void store.loadAgents();
    }, 15_000);
    return () => clearInterval(t);
  });

  const claims = $derived(
    [...store.agents].sort((a, b) => b.last_heartbeat.localeCompare(a.last_heartbeat)),
  );
  const shown = $derived(staleOnly ? claims.filter((c) => claimStale(c.last_heartbeat, now)) : claims);
  const staleCount = $derived(claims.filter((c) => claimStale(c.last_heartbeat, now)).length);

  function stateTone(s: string): string {
    if (s === "blocked" || s === "needs-user") return "bad";
    if (s === "awaiting-review" || s === "awaiting-ci" || s === "awaiting-merge" || s === "verifying") return "warn";
    if (s === "done") return "ok";
    return "info";
  }
</script>

<div class="wrap">
  <div class="head">
    <div class="line" title="Live roll-up, same shape as the opencode bb-status widget">
      <span class="dot" class:ok={store.connected}></span>
      {store.agentLine || "No active agent claims"}
    </div>
    <div class="meta">
      <span>{claims.length} active · {staleCount} stale</span>
      <label class="stale-toggle">
        <input type="checkbox" bind:checked={staleOnly} /> stale only
      </label>
      <button class="btn" onclick={() => void store.loadAgents()}>Refresh</button>
    </div>
  </div>

  {#if !shown.length}
    <p class="empty">
      {staleOnly ? "No stale claims. Every agent heartbeated in the last 5 minutes." : "Nothing is claimed right now."}
      Agents claim with <code>breadboard claim KEY --agent NAME</code> or the <code>claim_issue</code> MCP tool;
      claims tint the card on the board until released.
    </p>
  {:else}
    <div class="list">
      {#each shown as c (c.issue_id)}
        {@const stale = claimStale(c.last_heartbeat, now)}
        <article class="row" class:stale style="--agent:{agentColor(c.agent_name)}">
          <div class="main">
            <div class="top">
              <button class="key" onclick={() => (store.selectedKey = c.key)}>{c.key}</button>
              <span class="state {stateTone(c.state)}">{c.state}</span>
              <span class="board">{c.board_status}</span>
              {#if stale}<span class="stale-badge">stale</span>{/if}
              <span class="hb" title="Last heartbeat: {c.last_heartbeat}">♥ {ago(c.last_heartbeat, now)}</span>
            </div>
            <div class="title">{c.issue_title}</div>
            <div class="sub">
              <span class="agent" title="Claimed by {c.claimed_by_login}{c.session_id ? ` · session ${c.session_id}` : ''}">
                🤖 {c.agent_name} · {c.claimed_by_login}{c.session_id ? ` · ${c.session_id.slice(0, 8)}` : ""}
              </span>              {#if c.branch}<span class="mono" title="Branch">⑂ {c.branch}</span>{/if}
              {#if c.pr_url}
                <a class="mono link" href={c.pr_url} target="_blank" rel="noreferrer" title="Pull request">
                  PR{c.pr_number ? ` #${c.pr_number}` : ""}
                </a>
              {:else if c.pr_number}<span class="mono">PR #{c.pr_number}</span>{/if}
              {#if c.head_sha}<span class="mono" title="Head SHA">{c.head_sha.slice(0, 7)}</span>{/if}
            </div>
            {#if c.note}<p class="note">{c.note}</p>{/if}
          </div>
          <div class="actions">
            {#if c.claimed_by === store.me?.id}
              <button class="btn small" onclick={() => (store.selectedKey = c.key)}>Open</button>
              <button class="btn small danger" onclick={() => void store.release(c.key)}>Release</button>
            {:else}
              <button class="btn small" onclick={() => (store.selectedKey = c.key)}>Open</button>
            {/if}
          </div>
        </article>
      {/each}
    </div>
  {/if}
</div>

<style>
  .wrap {
    height: 100%;
    overflow-y: auto;
    padding: 16px 20px 32px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    font: 600 13px/1.4 var(--font-mono);
    color: var(--ink);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ink-3);
    flex: none;
  }
  .dot.ok {
    background: var(--ok);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 15%, transparent);
    animation: pulse 2.4s ease-in-out infinite;
  }
  @keyframes pulse {
    50% { box-shadow: 0 0 0 5px transparent; }
  }
  .meta {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 12px;
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .stale-toggle {
    display: flex;
    align-items: center;
    gap: 5px;
    cursor: pointer;
  }
  .btn {
    border: 1px solid var(--line-2);
    background: var(--raise);
    border-radius: 7px;
    padding: 5px 10px;
    font-size: 12px;
    color: var(--ink-dim);
  }
  .btn:hover {
    color: var(--ink);
    border-color: color-mix(in srgb, var(--ink) 26%, transparent);
  }
  .btn.small {
    padding: 4px 9px;
    font-size: 11.5px;
  }
  .btn.danger:hover {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 50%, transparent);
  }
  .empty {
    color: var(--ink-dim);
    font-size: 13px;
    line-height: 1.6;
    border: 1px dashed var(--line-2);
    border-radius: 10px;
    padding: 20px;
  }
  .empty code {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--raise);
    padding: 1px 5px;
    border-radius: 4px;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .row {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    background: var(--surface);
    border: 1px solid color-mix(in srgb, var(--agent, var(--ok)) 35%, var(--line));
    border-radius: 10px;
    padding: 11px 12px;
  }
  .row.stale {
    border-color: color-mix(in srgb, var(--danger) 35%, var(--line));
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .key {
    border: 0;
    background: none;
    padding: 0;
    font: 600 11px/1 var(--font-mono);
    color: var(--accent);
    cursor: pointer;
  }
  .key:hover { text-decoration: underline; }
  .state {
    font: 600 9.5px/1 var(--font-mono);
    letter-spacing: 0.04em;
    padding: 3px 6px;
    border-radius: 4px;
  }
  .state.info { color: var(--accent); background: var(--accent-soft); }
  .state.warn { color: var(--status-review); background: color-mix(in srgb, var(--status-review) 14%, transparent); }
  .state.ok { color: var(--ok); background: color-mix(in srgb, var(--ok) 14%, transparent); }
  .state.bad { color: var(--danger); background: color-mix(in srgb, var(--danger) 12%, transparent); }
  .board {
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .stale-badge {
    font: 600 9px/1 var(--font-mono);
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .hb {
    margin-left: auto;
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .title {
    font-size: 13.5px;
    font-weight: 550;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 11.5px;
    color: var(--ink-3);
  }
  .agent {
    color: var(--agent, var(--ink-dim));
    font-weight: 600;
  }
  .mono {
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .link { color: var(--accent); text-decoration: none; }
  .link:hover { text-decoration: underline; }
  .note {
    margin: 0;
    font-size: 12.5px;
    color: var(--ink-dim);
    border-left: 2px solid var(--line-2);
    padding-left: 8px;
  }
  .actions {
    display: flex;
    gap: 6px;
    flex: none;
  }
</style>
