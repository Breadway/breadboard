<script lang="ts">
  import type { HandoffNote, IssueSession } from "@breadboard/api-client";
  import { ago } from "./store.svelte";

  let {
    sessions,
    handoff = null,
  }: { sessions: IssueSession[]; handoff?: HandoffNote | null } = $props();
</script>

<section class="sec sessions-sec">
  <h3>
    Sessions
    <span class="grow"></span>
    <span class="count" title="Sessions that have touched this issue">{sessions.length}</span>
  </h3>

  {#if handoff}
    <div class="handoff-card" title="Latest release handoff">
      <div class="handoff-top">
        <span class="tag">Handoff</span>
        <span class="agent">{handoff.agent_name}</span>
        <span class="state">{handoff.state}</span>
        <span class="grow"></span>
        <span class="when" title={new Date(handoff.created_at).toLocaleString()}>{ago(handoff.created_at)}</span>
      </div>
      <p class="handoff-note">{handoff.note}</p>
    </div>
  {/if}

  {#if sessions.length}
    <ul class="session-list">
      {#each sessions as s (s.session_id)}
        <li>
          <span class="agent">{s.agent_name}</span>
          <span class="mono sid" title={s.session_id}>{s.session_id.slice(0, 10)}</span>
          {#if s.last_state}<span class="state">{s.last_state}</span>{/if}
          <span class="grow"></span>
          <span class="when" title="First seen {new Date(s.first_seen).toLocaleString()}">from {ago(s.first_seen)}</span>
          <span class="when strong" title="Last acted {new Date(s.last_seen).toLocaleString()}">last {ago(s.last_seen)}</span>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="dim small">No sessions recorded yet. A claim with a session id starts the link.</p>
  {/if}
</section>

<style>
  .sessions-sec h3 {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .count {
    font: 600 10px/1 var(--font-mono);
    color: var(--muted);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 3px 7px;
  }
  .handoff-card {
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--line));
    background: color-mix(in srgb, var(--accent) 6%, var(--surface));
    border-radius: 8px;
    padding: 9px 10px;
    margin-bottom: 10px;
  }
  .handoff-top {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-bottom: 5px;
  }
  .tag {
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--accent);
  }
  .handoff-note {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text);
    white-space: pre-wrap;
  }
  .session-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .session-list li {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--surface);
    font-size: 11px;
    min-width: 0;
  }
  .agent {
    font-weight: 600;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sid {
    font-size: 10px;
    color: var(--muted);
  }
  .state {
    font: 600 9px/1 var(--font-mono);
    color: var(--muted);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 2px 4px;
  }
  .when {
    font-size: 10px;
    color: var(--muted);
    white-space: nowrap;
  }
  .when.strong {
    color: var(--text);
  }
  .grow {
    flex: 1;
  }
  .small {
    font-size: 11px;
  }
</style>
