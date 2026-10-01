<script lang="ts">
  import type { Notification } from "@breadboard/api-client";
  import { store } from "./store.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";

  const unread = $derived(store.notifications.filter((n) => !n.read));
  const read = $derived(store.notifications.filter((n) => n.read).slice(0, 50));

  const KIND: Record<string, string> = {
    assigned: "Assigned",
    mention: "Mention",
    cycle_started: "Cycle",
    cycle_ended: "Cycle",
    digest: "Digest",
  };

  function ago(ts: string) {
    const s = (Date.now() - new Date(ts).getTime()) / 1000;
    if (s < 60) return "now";
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    if (s < 86400 * 7) return `${Math.floor(s / 86400)}d`;
    return new Date(ts).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }

  function open(n: Notification) {
    if (!n.read) void store.markRead(n.id);
    const issue = n.issue_id != null ? store.findIssue(n.issue_id) : undefined;
    if (issue) store.selectedKey = issue.key;
  }
</script>

{#snippet row(n: Notification)}
  {@const issue = n.issue_id != null ? store.findIssue(n.issue_id) : undefined}
  <button class="row" class:unread={!n.read} onclick={() => open(n)}>
    <span class="dot"></span>
    {#if issue}<StatusGlyph status={issue.status} />{:else}<span class="sp"></span>{/if}
    <span class="kind">{KIND[n.kind] ?? n.kind}</span>
    <span class="body">{n.body}</span>
    <time datetime={n.created_at} title={new Date(n.created_at).toLocaleString()}>{ago(n.created_at)}</time>
  </button>
{/snippet}

<div class="wrap">
  <div class="gh">
    Unread <span class="ct">{unread.length}</span>
    {#if unread.length}<button class="btn ghost" onclick={() => store.markAllRead()}>Mark all read</button>{/if}
  </div>
  {#each unread as n (n.id)}{@render row(n)}{:else}<p class="none">You're all caught up.</p>{/each}

  {#if read.length}
    <div class="gh">Earlier</div>
    {#each read as n (n.id)}{@render row(n)}{/each}
  {/if}

  <p class="hint">Choose what notifies you in Settings.</p>
</div>

<style>
  .wrap {
    height: 100%;
    overflow-y: auto;
    padding: 4px 20px 32px;
    max-width: 900px;
  }
  .gh {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 18px 8px 8px;
    font-size: 13px;
    font-weight: 600;
  }
  .gh .btn {
    margin-left: auto;
    height: 26px;
  }
  .ct {
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .row {
    display: grid;
    grid-template-columns: 6px 14px 74px minmax(0, 1fr) auto;
    gap: 12px;
    align-items: center;
    width: 100%;
    padding: 9px 10px;
    border: 0;
    border-radius: 8px;
    background: none;
    text-align: left;
    font-size: 13px;
    color: var(--ink-dim);
  }
  .row:hover {
    background: var(--surface);
  }
  .row.unread {
    color: var(--ink);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  .unread .dot {
    background: var(--accent);
  }
  .kind {
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .body {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  time {
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .none,
  .hint {
    margin: 0;
    padding: 12px 10px;
    color: var(--ink-3);
    font-size: 13px;
  }
  .hint {
    padding-top: 24px;
    font-size: 12px;
  }
</style>
