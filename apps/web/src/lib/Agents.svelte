<script lang="ts">
  import { onMount } from "svelte";
  import { ago, claimStale, store } from "./store.svelte";
  import { safeHttpUrl } from "@breadboard/api-client";
  import StatusGlyph from "./ui/StatusGlyph.svelte";
  import Ring from "./ui/Ring.svelte";
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

  const claims = $derived(store.agents);
  const work = $derived(claims.filter((c) => c.mode === "exclusive"));
  const queued = $derived(claims.filter((c) => c.mode === "intent"));
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

  interface Group {
    id: string;
    name: string;
    login: string;
    claims: typeof claims;
    work: typeof claims;
    intents: typeof claims;
    done: number;
    live: number;
    stale: number;
    lastHeartbeat: string;
  }

  // One agent = (owner, agent name). An agent can hold many issues; this
  // groups them so each row of the tab is an agent, not a single claim.
  // Stats are always computed from every claim the agent holds; the stale
  // view only narrows which rows are rendered, so completion does not jump.
  const allGroups = $derived.by(() => {
    const map = new Map<string, Group>();
    for (const c of claims) {
      const id = `${c.claimed_by_login}:${c.agent_name}`;
      let g = map.get(id);
      if (!g) {
        g = {
          id,
          name: c.agent_name,
          login: c.claimed_by_login,
          claims: [],
          work: [],
          intents: [],
          done: 0,
          live: 0,
          stale: 0,
          lastHeartbeat: c.last_heartbeat,
        };
        map.set(id, g);
      }
      g.claims.push(c);
      if (c.mode === "intent") g.intents.push(c);
      else {
        g.work.push(c);
        if (c.state === "done") g.done++;
        if (claimStale(c.last_heartbeat, now)) g.stale++;
        else g.live++;
      }
      if (c.last_heartbeat > g.lastHeartbeat) g.lastHeartbeat = c.last_heartbeat;
    }
    for (const g of map.values()) {
      g.claims.sort((a, b) =>
        a.mode === b.mode ? a.key.localeCompare(b.key) : a.mode === "exclusive" ? -1 : 1,
      );
    }
    return [...map.values()].sort(
      (a, b) =>
        b.live - a.live ||
        b.work.length - a.work.length ||
        b.intents.length - a.intents.length ||
        a.name.localeCompare(b.name),
    );
  });

  const groups = $derived(
    staleOnly
      ? allGroups
          .filter((g) => g.stale > 0)
          .map((g) => ({
            ...g,
            claims: g.claims.filter(
              (c) => c.mode === "exclusive" && claimStale(c.last_heartbeat, now),
            ),
          }))
      : allGroups,
  );

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
        All <span>{allGroups.length} agents</span>
      </button>
      <button class:on={staleOnly} onclick={() => (staleOnly = true)}>
        Stale <span class:bad={staleCount > 0}>{staleCount}</span>
      </button>
    </div>
    <span class="live-note">{liveCount} issues heartbeating · {groups.length} agents</span>
    <span class="grow"></span>
    <button class="btn ghost sm" onclick={() => void store.loadAgents()}>Refresh</button>
  </div>

  {#if !groups.length}
    <div class="empty">
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <rect x="4" y="7" width="16" height="12" rx="3" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path d="M12 3v4M8.5 12.5h.01M15.5 12.5h.01M9.5 16h5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
      </svg>
      <p>
        {staleOnly ? "No stale claims. Every agent heartbeated in the last 5 minutes." : "No agent is working an issue right now."}
      </p>
      <p class="hint">
        Agents claim with <code>breadboard claim KEY --agent NAME</code> or the <code>claim_issue</code> MCP tool.
        Claims tint the card on the board until released.
      </p>
    </div>
  {:else}
    <div class="groups">
      {#each groups as g (g.id)}
        {@const ownerDiffers = g.login !== store.me?.login}
        <section class="group" class:stale={g.stale > 0} style="--agent:{agentColor(g.name)}">
          <header class="ghead">
            <span class="avatar" aria-hidden="true">
              <svg viewBox="0 0 16 16"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
            </span>
            <div class="gid">
              <span class="aname">{g.name}</span>
              {#if ownerDiffers}<span class="alogin" title="Claims use this user's token">{g.login}</span>{/if}
            </div>
            <span class="gbadge live" class:off={g.live === 0}>{g.live} live</span>
            {#if g.stale > 0}
              <span class="gbadge stale" title="No heartbeat for a while; these claims may be abandoned">
                <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
                {g.stale} stale
              </span>
            {/if}
            {#if g.intents.length > 0}<span class="gbadge queued">{g.intents.length} queued</span>{/if}
            <span class="grow"></span>
            {#if g.work.length}
              <span class="completion" title="{g.done} of {g.work.length} exclusive issues done">
                <Ring done={g.done} total={g.work.length} color={agentColor(g.name)} size={16} />
                <b>{g.done}</b><span class="cslash">/</span>{g.work.length} done
              </span>
            {:else}
              <span class="completion muted">queued only</span>
            {/if}
            <span class="gtick" title="Last heartbeat {g.lastHeartbeat}">pulse {ago(g.lastHeartbeat, now)}</span>
          </header>

          <div class="gbody">
            {#each g.claims as c (c.id)}
              {@const stale = c.mode === "exclusive" && claimStale(c.last_heartbeat, now)}
              {@const pr = safeHttpUrl(c.pr_url)}
              <div class="row" class:stale class:intent={c.mode === "intent"}>
                <StatusGlyph status={c.board_status} size={12} />
                <button class="key" onclick={() => (store.selectedKey = c.key)} title="Open {c.key}">{c.key}</button>
                {#if c.mode === "intent"}<span class="queued-pill">queued</span>{/if}
                <span class="title">{c.issue_title}</span>
                <span class="state {tone(c.state)}">{stateLabel(c.state)}</span>
                {#if stale}
                  <span class="stale-pill" title="Last heartbeat {ago(c.last_heartbeat, now)}; claim may be abandoned">
                    <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
                    stale
                  </span>
                {/if}
                {#if pr}
                  <a class="chip-pr" href={pr} target="_blank" rel="noreferrer" title="Pull request">
                    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="12" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4 5.1v5.8M12 10.9V6.5a2 2 0 0 0-2-2H7.5m1.5-2-2 2 2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
                    PR{c.pr_number ? ` #${c.pr_number}` : ""}
                  </a>
                {:else if c.branch}
                  <span class="chip-branch" title="Branch">
                    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4.5" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="11.5" cy="6" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.5 5.1v5.8M11.5 7.6c0 2.2-2 2.6-4.6 3.3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
                    {c.branch}
                  </span>
                {/if}
                <span
                  class="hb"
                  class:stale
                  title={stale
                    ? `Last heartbeat ${ago(c.last_heartbeat, now)}; claim may be abandoned`
                    : `Last heartbeat ${ago(c.last_heartbeat, now)} ago`}
                >
                  {#if stale}
                    <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
                  {:else}
                    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 8h3l1.7-4 2.6 8 1.7-4h4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
                  {/if}
                  {ago(c.last_heartbeat, now)}
                </span>
                <span class="acts">
                  <button class="btn sm" onclick={() => (store.selectedKey = c.key)}>Open</button>
                  {#if c.claimed_by === store.me?.id}
                    <button class="btn sm danger" onclick={() => void store.releaseById(c.id)}>Release</button>
                  {/if}
                </span>
              </div>
            {/each}
            {#if g.work.length === 0 && g.intents.length === 0}
              <div class="row muted-row">No claims.</div>
            {/if}
          </div>
        </section>
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

  /* --- agent panels --------------------------------------------------- */
  .groups {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .group {
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface);
    overflow: hidden;
    transition: border-color 0.15s;
  }
  .group:hover {
    border-color: color-mix(in srgb, var(--agent) 38%, var(--line));
  }
  .group.stale {
    border-color: color-mix(in srgb, var(--danger) 28%, var(--line));
  }
  .ghead {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
    background: linear-gradient(
      90deg,
      color-mix(in srgb, var(--agent) 7%, transparent),
      transparent 45%
    );
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
  .gid {
    display: flex;
    align-items: baseline;
    gap: 7px;
    min-width: 0;
  }
  .aname {
    font-size: 13px;
    font-weight: 650;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .alogin {
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .gbadge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 3px 6px;
    border-radius: 4px;
    white-space: nowrap;
  }
  .gbadge.live {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 12%, transparent);
  }
  .gbadge.live.off {
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 5%, transparent);
  }
  .gbadge.stale {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 11%, transparent);
  }
  .gbadge.queued {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 12%, transparent);
  }
  .completion {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
  }
  .completion b {
    color: var(--ink);
    font-weight: 600;
  }
  .completion .cslash {
    color: var(--ink-3);
  }
  .completion.muted {
    color: var(--ink-3);
    font-style: italic;
  }
  .gtick {
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
  }

  .gbody {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 12px;
    min-width: 0;
    border-top: 1px solid color-mix(in srgb, var(--ink) 5%, transparent);
  }
  .row:first-child {
    border-top: 0;
  }
  .row:hover {
    background: color-mix(in srgb, var(--ink) 3%, transparent);
  }
  .row.intent {
    background: color-mix(in srgb, var(--surface) 92%, var(--bg));
  }
  .key {
    border: 0;
    background: none;
    padding: 0;
    flex: none;
    font: 600 10.5px/1 var(--font-mono);
    color: var(--accent);
    cursor: pointer;
  }
  .key:hover {
    text-decoration: underline;
  }
  .title {
    flex: 1;
    min-width: 0;
    font-size: 12.5px;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .state {
    flex: none;
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.04em;
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
    flex: none;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 13%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .stale-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 11%, transparent);
    padding: 3px 6px;
    border-radius: 4px;
  }
  .chip-pr,
  .chip-branch {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    max-width: 220px;
    padding: 2px 7px;
    border-radius: 5px;
    font: 400 10px/1.5 var(--font-mono);
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border: 1px solid var(--line);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip-pr {
    color: var(--status-review);
    text-decoration: none;
  }
  .chip-pr:hover {
    border-color: color-mix(in srgb, var(--status-review) 45%, transparent);
  }
  .chip-pr svg,
  .chip-branch svg {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .hb {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .hb svg,
  .clock {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .hb.stale {
    color: var(--danger);
  }
  .acts {
    display: flex;
    gap: 5px;
    flex: none;
  }
  .muted-row {
    color: var(--ink-3);
    font-size: 12px;
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
    .row {
      flex-wrap: wrap;
    }
    .title {
      flex-basis: 100%;
      order: 2;
    }
  }
</style>
