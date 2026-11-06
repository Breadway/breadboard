<script lang="ts">
  import { onMount } from "svelte";
  import { ago, claimStale, store } from "./store.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";
  import { agentColor } from "./ui/colors";

  // Tick every 15s so relative heartbeats and stale badges age live; SSE
  // pushes claim/heartbeat/release events straight into store.agents.
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

  // Exclusive claims are work; intents are queued reservations.
  const claims = $derived(
    [...store.agents].sort((a, b) =>
      a.mode === b.mode
        ? b.last_heartbeat.localeCompare(a.last_heartbeat)
        : a.mode === "exclusive"
          ? -1
          : 1,
    ),
  );
  const work = $derived(claims.filter((c) => c.mode === "exclusive"));
  const queued = $derived(claims.filter((c) => c.mode === "intent"));
  const shown = $derived(staleOnly ? work.filter((c) => claimStale(c.last_heartbeat, now)) : claims);
  const staleCount = $derived(work.filter((c) => claimStale(c.last_heartbeat, now)).length);
  const liveCount = $derived(work.length - staleCount);

  const tallies = $derived.by(() => {
    const t = { active: 0, review: 0, verifying: 0, blocked: 0, done: 0 };
    for (const c of work) {
      if (c.state === "done") t.done++;
      else if (c.state === "implementing" || c.state === "reworking" || c.state === "planned") t.active++;
      else if (c.state === "awaiting-review" || c.state === "awaiting-ci" || c.state === "awaiting-merge") t.review++;
      else if (c.state === "verifying") t.verifying++;
      else t.blocked++;
    }
    return t;
  });

  function tone(s: string): "idle" | "warn" | "ok" | "bad" {
    if (s === "blocked" || s === "needs-user") return "bad";
    if (s === "awaiting-review" || s === "awaiting-ci" || s === "awaiting-merge" || s === "verifying") return "warn";
    if (s === "done") return "ok";
    return "idle";
  }

  function stateLabel(s: string): string {
    return s.replace(/-/g, " ");
  }
</script>

<div class="wrap">
  <header class="summary">
    <div class="progress">
      <div class="ptop">
        <span class="plabel">{tallies.done} of {work.length} done</span>
        <span class="plive" class:off={!store.connected}>
          <i></i>{store.connected ? "live" : "reconnecting"}
        </span>
      </div>
      <div class="track" role="progressbar" aria-valuenow={tallies.done} aria-valuemin={0} aria-valuemax={work.length}>
        {#if work.length}
          <span class="fill" style="width:{(tallies.done / work.length) * 100}%"></span>
        {/if}
      </div>
      <span class="widget-line" title="Server roll-up, same shape as the opencode bb-status widget">{store.agentLine || "No active agent claims"}</span>
    </div>

    <div class="stats">
      <div class="stat"><b>{tallies.active}</b><span>active</span></div>
      <div class="stat"><b>{tallies.review}</b><span>review</span></div>
      <div class="stat"><b>{tallies.verifying}</b><span>verifying</span></div>
      <div class="stat" class:bad={tallies.blocked > 0}><b>{tallies.blocked}</b><span>blocked</span></div>
      <div class="stat" class:bad={staleCount > 0}><b>{staleCount}</b><span>stale</span></div>
      <div class="stat" class:queued={queued.length > 0}><b>{queued.length}</b><span>queued</span></div>
    </div>
  </header>

  <div class="bar">
    <div class="seg">
      <button class:on={!staleOnly} onclick={() => (staleOnly = false)}>
        All <span>{claims.length}</span>
      </button>
      <button class:on={staleOnly} onclick={() => (staleOnly = true)}>
        Stale <span class:bad={staleCount > 0}>{staleCount}</span>
      </button>
    </div>
    <span class="live-note">{liveCount} heartbeating now</span>
    <span class="grow"></span>
    <button class="btn ghost sm" onclick={() => void store.loadAgents()}>Refresh</button>
  </div>

  {#if !shown.length}
    <div class="empty">
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <rect x="4" y="7" width="16" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path d="M12 3v4M8.5 12.5h.01M15.5 12.5h.01M9.5 16h5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
      </svg>
      <p>
        {staleOnly ? "Nothing is stale. Every claim heartbeated in the last 5 minutes." : "No agent is working an issue right now."}
      </p>
      <p class="hint">
        Agents claim with <code>breadboard claim KEY --agent NAME</code> or the <code>claim_issue</code> MCP tool.
        Claims tint the card on the board until released.
      </p>
    </div>
  {:else}
    <div class="list">
      {#each shown as c (c.issue_id)}
        {@const stale = claimStale(c.last_heartbeat, now)}
        {@const ownerDiffers = c.claimed_by_login !== store.me?.login}
        {@const session = c.session_id ? c.session_id.slice(0, 7) : ""}
        <article class="claim" class:stale class:intent={c.mode === "intent"} style="--agent:{agentColor(c.agent_name)}">
          <span class="stripe" aria-hidden="true"></span>

          <div class="who">
            <span class="avatar" aria-hidden="true">
              <svg viewBox="0 0 16 16"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
            </span>
            <div class="who-text">
              <span class="aname" title="Agent name">{c.agent_name}</span>
              {#if ownerDiffers || session}
                <span class="alogin" title={ownerDiffers ? `Claimed with ${c.claimed_by_login}'s token` : "Your claim"}>
                  {ownerDiffers ? c.claimed_by_login : ""}{ownerDiffers && session ? " · " : ""}{session}
                </span>
              {/if}
            </div>
          </div>

          <div class="what">
            <div class="head">
              <button class="key" onclick={() => (store.selectedKey = c.key)} title="Open {c.key}">
                <StatusGlyph status={c.board_status} size={12} />{c.key}
              </button>
              <span class="state {tone(c.state)}">{stateLabel(c.state)}</span>
              {#if c.mode === "intent"}<span class="queued-pill">queued</span>{/if}
              {#if stale && c.mode === "exclusive"}<span class="stale-pill">stale</span>{/if}
            </div>
            <span class="title">{c.issue_title}</span>
            {#if c.note}<p class="note">{c.note}</p>{/if}
            {#if c.branch || c.pr_url || c.pr_number || c.head_sha}
              <div class="meta">
                {#if c.branch}
                  <span class="mchip" title="Branch">
                    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4.5" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="11.5" cy="6" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.5 5.1v5.8M11.5 7.6c0 2.2-2 2.6-4.6 3.3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
                    {c.branch}
                  </span>
                {/if}
                {#if c.pr_url}
                  <a class="mchip link" href={c.pr_url} target="_blank" rel="noreferrer" title="Pull request">
                    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="12" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4 5.1v5.8M12 10.9V6.5a2 2 0 0 0-2-2H7.5m1.5-2-2 2 2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
                    PR{c.pr_number ? ` #${c.pr_number}` : ""}
                  </a>
                {:else if c.pr_number}
                  <span class="mchip">PR #{c.pr_number}</span>
                {/if}
                {#if c.head_sha}<span class="mchip sha" title="Head commit">{c.head_sha.slice(0, 7)}</span>{/if}
              </div>
            {/if}
          </div>

          <div class="side">
            <span class="hb" class:stale={stale && c.mode === "exclusive"} title="Last heartbeat: {c.last_heartbeat}">
              <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 8h3l1.7-4 2.6 8 1.7-4h4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
              {ago(c.last_heartbeat, now)}
            </span>
            <div class="actions">
              <button class="btn sm" onclick={() => (store.selectedKey = c.key)}>Open</button>
              {#if c.claimed_by === store.me?.id}
                <button class="btn sm danger" onclick={() => void store.releaseById(c.id)}>Release</button>
              {/if}
            </div>
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
    padding: 18px 20px 36px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* --- summary ------------------------------------------------------- */
  .summary {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 20px;
    align-items: center;
    padding: 14px 16px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: linear-gradient(180deg, var(--surface), var(--sunk));
  }
  .progress {
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-width: 0;
  }
  .ptop {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .plabel {
    font: 650 15px/1 var(--font-display);
    letter-spacing: -0.01em;
  }
  .plive {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: 500 10px/1 var(--font-mono);
    color: var(--ok);
    text-transform: lowercase;
  }
  .plive i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 16%, transparent);
    animation: pulse 2.4s ease-in-out infinite;
  }
  .plive.off {
    color: var(--ink-3);
  }
  .plive.off i {
    animation: none;
    box-shadow: none;
  }
  @keyframes pulse {
    50% { box-shadow: 0 0 0 5px transparent; }
  }
  .track {
    height: 6px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    border-radius: 99px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--ok) 75%, var(--accent)), var(--ok));
    transition: width 0.3s var(--ease);
  }
  .widget-line {
    font: 400 10.5px/1.4 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .stats {
    display: flex;
    gap: 6px;
  }
  .stat {
    min-width: 54px;
    padding: 7px 9px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border: 1px solid var(--line);
    text-align: center;
  }
  .stat b {
    display: block;
    font: 650 17px/1 var(--font-display);
    font-variant-numeric: tabular-nums;
    color: var(--ink);
  }
  .stat span {
    display: block;
    margin-top: 4px;
    font: 500 9.5px/1 var(--font-mono);
    letter-spacing: 0.04em;
    color: var(--ink-3);
  }
  .stat.bad {
    border-color: color-mix(in srgb, var(--danger) 40%, transparent);
    background: color-mix(in srgb, var(--danger) 9%, transparent);
  }
  .stat.bad b {
    color: var(--danger);
  }
  .stat.queued {
    border-color: color-mix(in srgb, var(--status-review) 35%, transparent);
  }
  .stat.queued b {
    color: var(--status-review);
  }

  /* --- filter bar ---------------------------------------------------- */
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .seg {
    display: inline-flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--sunk);
    border: 1px solid var(--line);
  }
  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    background: none;
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 500;
    color: var(--ink-3);
  }
  .seg button:hover {
    color: var(--ink-dim);
  }
  .seg button.on {
    background: var(--raise);
    color: var(--ink);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }
  .seg span {
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
  .seg span.bad {
    color: var(--danger);
  }
  .live-note {
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .grow {
    flex: 1;
  }
  .btn.sm {
    height: 26px;
    padding: 0 9px;
    font-size: 11.5px;
  }

  /* --- rows ---------------------------------------------------------- */
  .list {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .claim {
    position: relative;
    display: grid;
    grid-template-columns: 172px minmax(0, 1fr) auto;
    gap: 16px;
    align-items: start;
    padding: 12px 14px 12px 16px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    transition: border-color 0.15s, background-color 0.15s;
  }
  .claim:hover {
    background: var(--raise);
    border-color: color-mix(in srgb, var(--agent) 40%, var(--line));
  }
  .claim.stale {
    border-color: color-mix(in srgb, var(--danger) 30%, var(--line));
  }
  .claim.intent {
    border-style: dashed;
    background: color-mix(in srgb, var(--surface) 92%, var(--bg));
  }
  .claim.intent .stripe {
    background: color-mix(in srgb, var(--agent) 40%, transparent);
  }
  .claim.intent .avatar {
    opacity: 0.6;
  }
  .stripe {
    position: absolute;
    left: 0;
    top: 10px;
    bottom: 10px;
    width: 3px;
    border-radius: 99px;
    background: var(--agent);
  }
  .claim.stale .stripe {
    background: var(--danger);
  }

  .who {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    padding-top: 1px;
  }
  .avatar {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    flex: none;
    border-radius: 8px;
    color: var(--agent);
    background: color-mix(in srgb, var(--agent) 13%, transparent);
    border: 1px solid color-mix(in srgb, var(--agent) 30%, transparent);
  }
  .avatar svg {
    width: 15px;
    height: 15px;
  }
  .who-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .aname {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .alogin {
    font: 400 10px/1.3 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .what {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .key {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    background: none;
    padding: 0;
    font: 600 11px/1 var(--font-mono);
    color: var(--accent);
  }
  .key:hover {
    text-decoration: underline;
  }
  .state {
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 3px 6px;
    border-radius: 4px;
  }
  .state.idle {
    color: var(--ink-dim);
    background: color-mix(in srgb, var(--ink) 7%, transparent);
  }
  .state.warn {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 13%, transparent);
  }
  .state.ok {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 13%, transparent);
  }
  .state.bad {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 11%, transparent);
  }
  .queued-pill {
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 13%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .stale-pill {
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 11%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .title {
    font-size: 13.5px;
    font-weight: 500;
    line-height: 1.35;
    color: var(--ink);
    overflow-wrap: anywhere;
  }
  .note {
    margin: 1px 0 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--ink-dim);
    padding-left: 8px;
    border-left: 2px solid color-mix(in srgb, var(--agent) 45%, transparent);
    overflow-wrap: anywhere;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 2px;
  }
  .mchip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 260px;
    padding: 2px 7px;
    border-radius: 5px;
    font: 400 10.5px/1.5 var(--font-mono);
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border: 1px solid var(--line);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mchip svg {
    width: 11px;
    height: 11px;
    flex: none;
  }
  .mchip.link {
    color: var(--status-review);
    text-decoration: none;
  }
  .mchip.link:hover {
    border-color: color-mix(in srgb, var(--status-review) 45%, transparent);
  }
  .mchip.sha {
    color: var(--ink-dim);
    letter-spacing: 0.02em;
  }

  .side {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 9px;
    flex: none;
  }
  .hb {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .hb svg {
    width: 11px;
    height: 11px;
  }
  .hb.stale {
    color: var(--danger);
  }
  .actions {
    display: flex;
    gap: 5px;
  }

  /* --- empty --------------------------------------------------------- */
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 44px 20px;
    border: 1px dashed var(--line-2);
    border-radius: 12px;
    text-align: center;
  }
  .empty svg {
    width: 30px;
    height: 30px;
    color: var(--ink-3);
  }
  .empty p {
    margin: 0;
    font-size: 13px;
    color: var(--ink-dim);
  }
  .empty .hint {
    max-width: 52ch;
    font-size: 12px;
    color: var(--ink-3);
    line-height: 1.6;
  }
  .empty code {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-dim);
    background: var(--raise);
    padding: 1px 5px;
    border-radius: 4px;
  }

  @media (max-width: 1080px) {
    .summary {
      grid-template-columns: minmax(0, 1fr);
      gap: 14px;
    }
    .stats {
      flex-wrap: wrap;
    }
    .claim {
      grid-template-columns: 1fr;
      gap: 10px;
    }
    .who {
      padding-top: 0;
    }
    .side {
      flex-direction: row;
      align-items: center;
      justify-content: space-between;
    }
  }
</style>
