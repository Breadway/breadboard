<script lang="ts">
  import type { Priority } from "@breadboard/api-client";

  // Signal bars for low/medium/high, a red block for urgent, a dotted line
  // for none.
  let { priority, size = 14 }: { priority: Priority; size?: number } = $props();
  const level = $derived(({ low: 1, medium: 2, high: 3 } as Record<string, number>)[priority] ?? 0);
</script>

<svg class="pg" viewBox="0 0 14 14" width={size} height={size} role="img" aria-label="{priority} priority">
  {#if priority === "urgent"}
    <rect x="1" y="1" width="12" height="12" rx="3" fill="var(--danger)" />
    <path d="M7 3.6v4.2" stroke="#1a0807" stroke-width="1.8" stroke-linecap="round" />
    <circle cx="7" cy="10.2" r="1" fill="#1a0807" />
  {:else if priority === "none"}
    <path d="M2 7h2M6 7h2M10 7h2" stroke="var(--ink-3)" stroke-width="1.6" stroke-linecap="round" />
  {:else}
    {#each [0, 1, 2] as i (i)}
      <rect
        x={1.5 + i * 4}
        y={9 - i * 3}
        width="3"
        height={4 + i * 3}
        rx="1"
        fill={i < level ? "var(--ink)" : "var(--ink-3)"}
        opacity={i < level ? 0.9 : 0.35}
      />
    {/each}
  {/if}
</svg>

<style>
  .pg {
    flex: none;
    display: block;
  }
</style>
