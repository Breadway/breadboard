<script lang="ts">
  let { source, caption = "Diagram" }: { source: string; caption?: string } = $props();

  // Loaded lazily so the approvals page does not pay for mermaid unless an ask
  // actually contains a diagram. Strict security keeps labels sanitized and
  // disables click handlers and raw HTML from the diagram source.
  let loader: Promise<typeof import("mermaid").default | null> | null = null;
  let svg = $state("");
  let failed = $state(false);
  let open = $state(false);
  let error = $state("");

  async function mermaidModule() {
    if (!loader) {
      loader = import("mermaid")
        .then((mod) => {
          const mermaid = mod.default;
          mermaid.initialize({
            startOnLoad: false,
            securityLevel: "strict",
            theme: "dark",
            fontFamily: "Instrument Sans, system-ui, sans-serif",
            themeVariables: {
              background: "#161310",
              primaryColor: "#2a2118",
              primaryTextColor: "#ede6dc",
              primaryBorderColor: "#4a3b2b",
              lineColor: "#d98a3d",
              secondaryColor: "#1b1713",
              tertiaryColor: "#161310",
              noteBkgColor: "#241e18",
              noteTextColor: "#d6cec4",
              fontSize: "15px",
            },
            flowchart: { htmlLabels: false, useMaxWidth: true, padding: 14 },
            gantt: {
              fontSize: 14,
              sectionFontSize: 14,
              barHeight: 26,
              barGap: 6,
              topPadding: 42,
              leftPadding: 110,
            },
          });
          return mermaid;
        })
        .catch(() => null);
    }
    return loader;
  }

  async function render() {
    failed = false;
    error = "";
    svg = "";
    if (!source.trim()) return;
    if (source.length > 20_000) {
      failed = true;
      error = "diagram source is too long to render";
      return;
    }
    const mermaid = await mermaidModule();
    if (!mermaid) {
      failed = true;
      return;
    }
    try {
      const id = `m${Math.random().toString(36).slice(2)}`;
      const rendered = await mermaid.render(id, source);
      svg = rendered.svg;
    } catch (e) {
      failed = true;
      error = e instanceof Error ? e.message : "could not render this diagram";
    }
  }

  $effect(() => {
    // Re-render when the ask swaps or a revision changes the diagram.
    void source;
    void render();
  });
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && open) open = false;
  }}
/>

<div
  class="mermaid-card"
  role="button"
  tabindex={svg ? 0 : -1}
  aria-label="Enlarge {caption}"
  data-mermaid-card
  onclick={() => {
    if (svg) open = true;
  }}
  onkeydown={(e) => {
    if (svg && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      open = true;
    }
  }}
>
  <div class="cap">{caption}<span class="zoom-hint">{svg ? "click to enlarge" : ""}</span></div>
  {#if svg}
    <div class="diagram">{@html svg}</div>
  {:else if failed}
    <pre class="fallback" data-mermaid-fallback>{error || source}</pre>
  {:else}
    <div class="loading">Rendering diagram…</div>
  {/if}
</div>

{#if open && svg}
  <div
    class="lightbox"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    aria-label={caption}
    data-lightbox="mermaid"
    onclick={() => (open = false)}
    onkeydown={(e) => {
      if (e.key === "Escape") open = false;
    }}
  >
    <div class="inner" role="document">
      {@html svg}
    </div>
  </div>
{/if}

<style>
  .mermaid-card {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    padding: 14px 16px 10px;
    margin: 12px 0 18px;
    overflow-x: auto;
    cursor: zoom-in;
    transition: border-color 140ms var(--ease);
  }
  .mermaid-card:hover {
    border-color: var(--line-2);
  }
  .cap {
    font: 500 10px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-3);
    margin-bottom: 8px;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .zoom-hint {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
    font-size: 10px;
  }
  .diagram :global(svg) {
    max-width: 100%;
    height: auto;
  }
  .fallback {
    margin: 0;
    font-size: 11px;
    color: var(--ink-3);
    white-space: pre-wrap;
  }
  .loading {
    font-size: 11px;
    color: var(--ink-3);
    padding: 10px 0;
  }
  .lightbox {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.85);
    z-index: 80;
    display: grid;
    place-items: center;
    padding: 30px;
    cursor: zoom-out;
  }
  .inner {
    max-width: 1100px;
    width: 100%;
    max-height: 90vh;
    overflow: auto;
    border: 1px solid var(--line-2);
    border-radius: 12px;
    background: var(--surface);
    padding: 18px;
  }
  .inner :global(svg) {
    display: block;
    width: 100%;
    height: auto;
  }
</style>
