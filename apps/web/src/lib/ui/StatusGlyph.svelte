<script lang="ts">
  import type { Status } from "@breadboard/api-client";

  // Backlog is a dashed ring, todo an empty ring, doing half full, review
  // three quarters, done a filled check: the progress reads without colour.
  let { status, size = 14 }: { status: Status; size?: number } = $props();

  function wedge(f: number) {
    const a = f * 2 * Math.PI - Math.PI / 2;
    const x = 7 + 4 * Math.cos(a);
    const y = 7 + 4 * Math.sin(a);
    return `M7 7V3A4 4 0 ${f > 0.5 ? 1 : 0} 1 ${x.toFixed(2)} ${y.toFixed(2)}Z`;
  }
</script>

<svg class="sg" viewBox="0 0 14 14" width={size} height={size} role="img" aria-label={status} style="--c: var(--status-{status})">
  {#if status === "done"}
    <circle cx="7" cy="7" r="6.6" fill="var(--c)" />
    <path d="m4.2 7.2 1.9 1.9 3.7-3.9" fill="none" stroke="var(--bg)" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
  {:else}
    <circle cx="7" cy="7" r="5.8" fill="none" stroke="var(--c)" stroke-width="1.5" stroke-dasharray={status === "backlog" ? "2 2.2" : undefined} />
    {#if status === "doing"}<path d={wedge(0.5)} fill="var(--c)" />{/if}
    {#if status === "review"}<path d={wedge(0.75)} fill="var(--c)" />{/if}
  {/if}
</svg>

<style>
  .sg {
    flex: none;
    display: block;
  }
</style>
