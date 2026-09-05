<script lang="ts">
  import type { Issue } from "@breadboard/api-client";
  import { store } from "./store.svelte";

  let { issue, keyboardSelected = false }: { issue: Issue; keyboardSelected?: boolean } = $props();

  let el: HTMLButtonElement | undefined = $state();
  $effect(() => {
    if (keyboardSelected) el?.scrollIntoView({ block: "nearest" });
  });

  const prioClass = $derived(
    issue.priority === "none" ? "" : `prio-${issue.priority}`,
  );
  const closeLinks = $derived(
    issue.links.filter((l) => l.kind === "closes" || l.kind === "closed-by"),
  );
</script>

<button
  bind:this={el}
  class="card"
  class:selected={store.selectedKey === issue.key}
  class:kb={keyboardSelected}
  draggable="true"
  ondragstart={(e) => e.dataTransfer?.setData("text/plain", issue.key)}
  onclick={() => (store.selectedKey = issue.key)}
>
  <div class="card-top">
    <span class="key">{issue.key}</span>
    {#if issue.priority !== "none"}
      <span class="prio {prioClass}" title={issue.priority}>{issue.priority}</span>
    {/if}
    {#if issue.forgejo_ref}
      <span class="fg" title="synced with {issue.forgejo_ref.repo}#{issue.forgejo_ref.number}">⇄</span>
    {/if}
  </div>
  <div class="title">{issue.title}</div>
  {#if closeLinks.length}
    <div class="link-line">
      {#each closeLinks as l (l.kind + l.target_id)}
        {@const t = store.findIssue(l.target_id)}
        <span title={t?.title ?? ""}>{l.kind === "closes" ? "→" : "←"} {t?.key ?? `#${l.target_id}`}</span>
      {/each}
    </div>
  {/if}
  {#if issue.labels.length || issue.estimate != null || issue.assignee_id}
    <div class="card-bot">
      {#each issue.labels as label (label)}
        <span class="label">{label}</span>
      {/each}
      {#if issue.estimate != null}<span class="est">{issue.estimate}</span>{/if}
      {#if issue.assignee_id === store.me?.id}
        {#if store.me?.avatar_url}
          <img class="avatar" src={store.me.avatar_url} alt="" title="assigned to you" width="14" height="14" />
        {:else}
          <span class="me" title="assigned to you">●</span>
        {/if}
      {:else if issue.assignee_id}
        <span class="assignee" title="assigned">◐</span>
      {/if}
    </div>
  {/if}
</button>

<style>
  .card {
    display: block;
    width: 100%;
    text-align: left;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 9px;
    margin-bottom: 6px;
    cursor: grab;
    color: var(--ink);
    font: inherit;
    transition: border-color 0.12s, transform 0.12s, box-shadow 0.12s;
  }
  .card:hover {
    /* inset only — same reason as .kb below: an outward drop shadow bleeds
       past the card's box and .cards' overflow-y:auto clips it there. */
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent) inset;
  }
  .card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent) inset;
  }
  .card.kb {
    border-color: var(--accent);
    /* inset, not outline+offset — the parent .cards has overflow-y:auto, which
       per spec forces overflow-x to compute as auto too, clipping anything
       that bleeds outside the card's own border box. */
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent) inset;
  }
  .card:active {
    cursor: grabbing;
  }
  .card-top {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 3px;
  }
  .key {
    font-size: 11px;
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
  }
  .title {
    font-size: 13px;
    font-weight: 500;
    line-height: 1.35;
  }
  .link-line {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 3px;
    font-size: 11px;
    color: var(--ink-dim);
  }
  .card-bot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
  }
  .label {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 999px;
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--ink-dim);
  }
  .prio {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-dim);
    margin-left: auto;
  }
  .prio-high {
    color: #e0a052;
  }
  .prio-urgent {
    color: #e05252;
    font-weight: 600;
  }
  .fg {
    color: var(--ink-dim);
    font-size: 11px;
  }
  .est {
    font-size: 10px;
    color: var(--ink-dim);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
  }
  .me {
    color: #4caf50;
    margin-left: auto;
  }
  .avatar {
    border-radius: 50%;
    object-fit: cover;
    margin-left: auto;
    display: block;
  }
  .assignee {
    color: var(--ink-dim);
    margin-left: auto;
  }
</style>
