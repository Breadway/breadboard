<script lang="ts">
  let {
    done,
    total,
    size = 16,
    color = "var(--accent)",
  }: { done: number; total: number; size?: number; color?: string } = $props();
  const r = 6;
  const C = 2 * Math.PI * r;
  const f = $derived(total ? Math.min(1, done / total) : 0);
</script>

<svg class="ring" viewBox="0 0 16 16" width={size} height={size} role="img" aria-label="{done} of {total} done">
  <circle cx="8" cy="8" r={r} fill="none" stroke="var(--line-2)" stroke-width="2" />
  {#if f > 0}
    <circle
      cx="8"
      cy="8"
      r={r}
      fill="none"
      stroke={color}
      stroke-width="2"
      stroke-linecap="round"
      stroke-dasharray="{(f * C).toFixed(2)} {C.toFixed(2)}"
      transform="rotate(-90 8 8)"
    />
  {/if}
</svg>

<style>
  .ring {
    flex: none;
    display: block;
  }
</style>
