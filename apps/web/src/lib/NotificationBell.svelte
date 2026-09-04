<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { store } from "./store.svelte";

  function open(issueId: number | null) {
    if (issueId == null) return;
    const issue = store.issues.find((i) => i.id === issueId);
    if (issue) {
      store.selectedKey = issue.key;
      store.bellOpen = false;
    }
  }
</script>

<div class="bell">
  <button class="trigger" onclick={() => (store.bellOpen = !store.bellOpen)} title="Notifications">
    <span class="ico">🔔</span>
    {#if store.unread > 0}<span class="badge">{store.unread > 9 ? "9+" : store.unread}</span>{/if}
  </button>

  {#if store.bellOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div
      class="scrim"
      role="presentation"
      transition:fade={{ duration: 100 }}
      onclick={() => (store.bellOpen = false)}
    ></div>
    <div class="panel" transition:fly={{ y: -6, duration: 120 }}>
      <div class="phead">
        <span>Notifications</span>
        {#if store.notifications.some((n) => !n.read)}
          <button class="link" onclick={() => store.markAllRead()}>mark all read</button>
        {/if}
      </div>
      <ul>
        {#each store.notifications.slice(0, 30) as n (n.id)}
          <li class:unread={!n.read}>
            <button
              onclick={() => {
                if (!n.read) store.markRead(n.id);
                open(n.issue_id);
              }}
            >
              <div class="body">{n.body}</div>
              <div class="when">{new Date(n.created_at).toLocaleString()}</div>
            </button>
          </li>
        {:else}
          <li class="empty">Nothing yet.</li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

<style>
  .bell {
    position: relative;
  }
  .trigger {
    background: none !important;
    border: none !important;
    cursor: pointer;
    position: relative;
    padding: 2px 4px !important;
    font-size: 14px;
  }
  .ico {
    filter: grayscale(1) opacity(0.75);
  }
  .badge {
    position: absolute;
    top: -2px;
    right: -3px;
    background: #e05252;
    color: #fff;
    font-size: 9px;
    line-height: 1;
    padding: 2px 3px;
    border-radius: 8px;
    font-variant-numeric: tabular-nums;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .panel {
    position: absolute;
    right: 0;
    top: 28px;
    width: 320px;
    max-height: 60vh;
    overflow-y: auto;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.5);
    z-index: 31;
  }
  .phead {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    color: var(--ink-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .link {
    background: none;
    border: none;
    color: var(--ink-dim);
    text-decoration: underline;
    cursor: pointer;
    font-size: 11px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li button {
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid var(--surface);
    padding: 9px 12px;
    color: var(--ink);
    cursor: pointer;
    font: inherit;
  }
  li button:hover {
    background: var(--bg);
  }
  li.unread button {
    border-left: 2px solid var(--accent);
  }
  .body {
    font-size: 13px;
  }
  .when {
    font-size: 11px;
    color: var(--ink-dim);
    margin-top: 2px;
  }
  .empty {
    padding: 16px;
    color: var(--ink-dim);
    font-size: 13px;
    text-align: center;
  }
</style>
