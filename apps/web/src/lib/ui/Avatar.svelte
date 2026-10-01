<script lang="ts">
  import { store } from "../store.svelte";

  // Only the signed-in user's profile is known client-side; anyone else is
  // drawn as a neutral initial-less disc with a tooltip.
  let { userId, size = 20 }: { userId: number | null; size?: number } = $props();
  const isMe = $derived(userId != null && userId === store.me?.id);
  const initial = $derived((store.me?.display_name || store.me?.login || "?")[0].toUpperCase());
</script>

{#if userId != null}
  {#if isMe && store.me?.avatar_url}
    <img class="av" src={store.me.avatar_url} alt="" title="Assigned to you" width={size} height={size} />
  {:else if isMe}
    <span class="av me" style="width:{size}px;height:{size}px;font-size:{Math.round(size * 0.45)}px" title="Assigned to you">{initial}</span>
  {:else}
    <span class="av other" style="width:{size}px;height:{size}px" title="Assigned to user #{userId}"></span>
  {/if}
{/if}

<style>
  .av {
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 50%;
    object-fit: cover;
  }
  .me {
    font: 700 10px/1 var(--font-display);
    color: var(--on-accent);
    background: linear-gradient(135deg, #f0b46a, #c96f2a);
  }
  .other {
    background: var(--raise);
    box-shadow: inset 0 0 0 1.5px var(--line-2);
  }
</style>
