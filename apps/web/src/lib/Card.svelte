<script lang="ts">
  import type { Issue } from "@breadboard/api-client";
  import { ago, claimStale, store } from "./store.svelte";
  import PriorityGlyph from "./ui/PriorityGlyph.svelte";
  import Avatar from "./ui/Avatar.svelte";
  import { agentColor, labelColor, projectColor } from "./ui/colors";

  let { issue, keyboardSelected = false }: { issue: Issue; keyboardSelected?: boolean } = $props();

  const claim = $derived(store.claimForIssue(issue.id));
  const intents = $derived(
    store.claimsForIssue(issue.id).filter((c) => c.mode === "intent"),
  );
  const stale = $derived(claim ? claimStale(claim.last_heartbeat, store.clock) : false);
  const claimAge = $derived(claim ? ago(claim.last_heartbeat, store.clock) : "");

  const MAX_LABELS = 3;

  let el: HTMLButtonElement | undefined = $state();
  let dragging = $state(false);
  $effect(() => {
    if (keyboardSelected) el?.scrollIntoView({ block: "nearest" });
  });

  // PRs that close this issue don't get their own card (see store.filtered):
  // their state surfaces here instead, so the board shows one thing, not two.
  const closingPrs = $derived(
    issue.links
      .filter((l) => l.kind === "closed-by")
      .map((l) => store.findIssue(l.target_id))
      .filter((t): t is Issue => !!t && t.type === "pr"),
  );
  const pkey = $derived(store.projectKey(issue.project_id));
</script>

<button
  bind:this={el}
  class="card"
  class:done={issue.status === "done"}
  class:claimed={!!claim}
  class:stale={stale}
  style={claim ? `--agent:${agentColor(claim.agent_name)}` : undefined}
  class:selected={store.selectedKey === issue.key}
  class:kb={keyboardSelected}
  class:dragging
  draggable="true"
  ondragstart={(e) => {
    e.dataTransfer?.setData("text/plain", issue.key);
    dragging = true;
  }}
  ondragend={() => (dragging = false)}
  onclick={() => (store.selectedKey = issue.key)}
>
  <div class="top">
    <span class="tick" style="background:{projectColor(pkey)}"></span>
    <span class="key">{issue.key}</span>
    {#if issue.type === "pr"}<span class="pr-tag">PR</span>{/if}
    {#if claim}
      <span
        class="agent"
        class:stale={stale}
        title={stale
          ? `Claimed by ${claim.agent_name} · last heartbeat ${claimAge} · claim may be abandoned`
          : `Claimed by ${claim.agent_name} · ${claim.state} · heartbeat ${claimAge} ago`}
      >
        {#if stale}
          <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
        {:else}
          <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
        {/if}
        {claim.agent_name}
      </span>
    {:else if intents.length}
      <span class="queued" title="{intents.map((i) => i.agent_name).join(', ')} queued for this">
        <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.4 2.2" /><path d="M8 4.6V8l2.4 1.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
        {intents.length} queued
      </span>
    {/if}
    <span class="prio" title="{issue.priority} priority"><PriorityGlyph priority={issue.priority} /></span>
  </div>
  <div class="title">{issue.title}</div>
  {#if issue.labels.length || closingPrs.length || issue.estimate != null || issue.assignee_id || issue.forgejo_ref}
    <div class="bot">
      {#each issue.labels.slice(0, MAX_LABELS) as label (label)}
        <span class="chip-label"><i style="background:{labelColor(label)}"></i>{label}</span>
      {/each}
      {#if issue.labels.length > MAX_LABELS}
        <span class="more" title={issue.labels.slice(MAX_LABELS).join(", ")}>+{issue.labels.length - MAX_LABELS}</span>
      {/if}
      {#each closingPrs as pr (pr.id)}
        <span class="prc" class:merged={pr.status === "done"} title="{pr.status === 'done' ? 'Merged' : 'Open'} pull request: {pr.title}">
          <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4" cy="3.5" r="1.8" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="4" cy="12.5" r="1.8" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="12" cy="12.5" r="1.8" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M4 5.3v5.4M12 10.7V6.5a2 2 0 0 0-2-2H7.5m1.5-2-2 2 2 2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
          #{pr.forgejo_ref?.number ?? pr.id}
        </span>
      {/each}
      {#if issue.estimate != null}<span class="est" title="{issue.estimate} points"><i></i>{issue.estimate}</span>{/if}
      <span class="grow"></span>
      {#if issue.forgejo_ref}
        <span class="sync" title="Synced with {issue.forgejo_ref.repo}#{issue.forgejo_ref.number}">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 6.5A5 5 0 0 1 12.2 4.5M13 9.5A5 5 0 0 1 3.8 11.5M12.5 2v2.8H9.7M3.5 14v-2.8h2.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </span>
      {/if}
      <Avatar userId={issue.assignee_id} size={20} />
    </div>
  {/if}
</button>

<style>
  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 7px;
    width: 100%;
    padding: 11px 12px 10px;
    border-radius: 10px;
    background: var(--surface);
    border: 1px solid var(--line);
    text-align: left;
    cursor: grab;
    transition:
      transform 0.16s var(--ease),
      border-color 0.15s,
      box-shadow 0.16s,
      background-color 0.15s;
  }
  .card:hover {
    border-color: var(--line-2);
    background: var(--raise);
    transform: translateY(-1px);
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.35);
  }
  .card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .card.kb {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .card.dragging {
    opacity: 0.4;
  }
  .card.claimed {
    border-color: color-mix(in srgb, var(--agent, var(--ok)) 55%, transparent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--agent, var(--ok)) 45%, transparent);
  }
  .card.claimed:hover {
    border-color: color-mix(in srgb, var(--agent, var(--ok)) 75%, transparent);
  }
  .card.claimed.stale {
    border-color: color-mix(in srgb, var(--danger) 50%, transparent);
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .agent {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.04em;
    padding: 3px 6px 3px 5px;
    border-radius: 4px;
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--agent, var(--ok));
    background: color-mix(in srgb, var(--agent, var(--ok)) 13%, transparent);
  }
  .agent svg {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .queued {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.04em;
    padding: 3px 6px 3px 5px;
    border-radius: 4px;
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 5%, transparent);
    border: 1px dashed var(--line-2);
    white-space: nowrap;
  }
  .queued svg {
    width: 10px;
    height: 10px;
    flex: none;
  }
  .agent.stale {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
  }
  .card:active {
    cursor: grabbing;
  }
  .card.done .title {
    color: var(--ink-dim);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .tick {
    width: 3px;
    height: 11px;
    border-radius: 2px;
    flex: none;
  }
  .key {
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .pr-tag {
    font: 600 9px/1 var(--font-mono);
    letter-spacing: 0.06em;
    padding: 2px 4px;
    border-radius: 3px;
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 14%, transparent);
  }
  .prio {
    margin-left: auto;
  }
  .title {
    font-size: 13.5px;
    font-weight: 500;
    line-height: 1.38;
    overflow-wrap: anywhere;
    text-wrap: pretty;
  }
  .bot {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    min-height: 20px;
  }
  .grow {
    flex: 1;
  }
  .more {
    font: 500 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .prc {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 500 10.5px/1 var(--font-mono);
    padding: 3px 6px;
    border-radius: 5px;
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 12%, transparent);
  }
  .prc.merged {
    color: var(--status-done);
    background: color-mix(in srgb, var(--status-done) 12%, transparent);
  }
  .prc svg {
    width: 11px;
    height: 11px;
  }
  .est {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font: 500 10.5px/1 var(--font-mono);
    color: var(--ink-dim);
  }
  .est i {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--ink-3);
  }
  .sync {
    display: grid;
    place-items: center;
    color: var(--ink-3);
  }
  .sync svg {
    width: 12px;
    height: 12px;
  }
</style>
