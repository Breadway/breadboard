<script lang="ts" module>
  import type { Snippet } from "svelte";
  import type { Priority, Status } from "@breadboard/api-client";

  export type MenuGlyph =
    | { status: Status }
    | { priority: Priority }
    | { dot: string }
    | { text: string };

  export interface MenuItem {
    label: string;
    hint?: string;
    current?: boolean;
    danger?: boolean;
    glyph?: MenuGlyph;
    run: () => void;
  }

  // Only one menu is open at a time; App's Esc handler asks here first so Esc
  // closes the menu and not the panel underneath it.
  let openClose: (() => void) | null = null;
  export function closeOpenMenu(): boolean {
    if (!openClose) return false;
    openClose();
    return true;
  }
</script>

<script lang="ts">
  import { fly } from "svelte/transition";
  import StatusGlyph from "./StatusGlyph.svelte";
  import PriorityGlyph from "./PriorityGlyph.svelte";


  let {
    items,
    trigger,
    triggerClass = "",
    title = "",
    align = "left",
    filter = false,
  }: {
    items: MenuItem[];
    trigger: Snippet;
    triggerClass?: string;
    title?: string;
    align?: "left" | "right";
    filter?: boolean;
  } = $props();

  let open = $state(false);
  let q = $state("");
  let cursor = $state(0);
  let root: HTMLElement | undefined = $state();
  let filterEl: HTMLInputElement | undefined = $state();

  const shown = $derived(
    q.trim() ? items.filter((i) => i.label.toLowerCase().includes(q.trim().toLowerCase())) : items,
  );

  function close() {
    open = false;
    q = "";
    if (openClose === close) openClose = null;
  }

  function toggle() {
    if (open) return close();
    closeOpenMenu();
    open = true;
    openClose = close;
    cursor = Math.max(0, items.findIndex((i) => i.current));
    queueMicrotask(() => (filter ? filterEl?.focus() : root?.querySelector<HTMLElement>(".mi.cur, .mi")?.focus()));
  }

  function pick(item: MenuItem) {
    close();
    item.run();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      cursor = (cursor + 1) % Math.max(shown.length, 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      cursor = (cursor - 1 + shown.length) % Math.max(shown.length, 1);
    } else if (e.key === "Enter" && shown[cursor]) {
      e.preventDefault();
      pick(shown[cursor]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else return;
    queueMicrotask(() => root?.querySelectorAll<HTMLElement>(".mi")[cursor]?.focus({ preventScroll: false }));
    if (filter) queueMicrotask(() => filterEl?.focus());
  }

  $effect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (root && !root.contains(e.target as Node)) close();
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  });
</script>

<div class="wrap" bind:this={root}>
  <button class={triggerClass} {title} aria-haspopup="menu" aria-expanded={open} onclick={toggle}>
    {@render trigger()}
  </button>
  {#if open}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div class="menu {align}" role="menu" tabindex="-1" onkeydown={onKey} transition:fly={{ y: -4, duration: 110 }}>
      {#if filter}
        <input class="mf" bind:this={filterEl} bind:value={q} placeholder="Filter…" oninput={() => (cursor = 0)} />
      {/if}
      {#each shown as item, i (item.label)}
        <button
          class="mi"
          class:cur={i === cursor}
          class:danger={item.danger}
          role="menuitemradio"
          aria-checked={!!item.current}
          onmouseenter={() => (cursor = i)}
          onclick={() => pick(item)}
        >
          {#if item.glyph}
            {#if "status" in item.glyph}<StatusGlyph status={item.glyph.status} />
            {:else if "priority" in item.glyph}<PriorityGlyph priority={item.glyph.priority} />
            {:else if "dot" in item.glyph}<i class="dot" style="background:{item.glyph.dot}"></i>
            {:else}<span class="gt">{item.glyph.text}</span>{/if}
          {/if}
          <span class="ml">{item.label}</span>
          {#if item.current}<span class="check">✓</span>{/if}
          {#if item.hint}<kbd>{item.hint}</kbd>{/if}
        </button>
      {:else}
        <p class="none">No matches</p>
      {/each}
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    min-width: 0;
  }
  .wrap > button {
    max-width: 100%;
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    z-index: 35;
    min-width: 190px;
    max-width: 320px;
    max-height: 320px;
    overflow-y: auto;
    padding: 5px;
    border-radius: 10px;
    background: var(--raise);
    border: 1px solid var(--line-2);
    box-shadow: var(--shadow-pop);
    outline: none;
  }
  .menu.left {
    left: 0;
  }
  .menu.right {
    right: 0;
  }
  .mf {
    width: 100%;
    margin-bottom: 4px;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid var(--line);
    background: var(--bg);
    font-size: 12.5px;
    outline: none;
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 7px 9px;
    border: 0;
    border-radius: 6px;
    background: none;
    font-size: 13px;
    text-align: left;
    outline: none;
  }
  .mi.cur {
    background: color-mix(in srgb, var(--ink) 7%, transparent);
  }
  .mi.danger {
    color: var(--danger);
  }
  .ml {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    flex: none;
  }
  .gt {
    font: 500 10px/1 var(--font-mono);
    color: var(--accent);
    min-width: 14px;
  }
  .check {
    color: var(--accent);
    font-size: 12px;
  }
  .none {
    margin: 0;
    padding: 8px 9px;
    font-size: 12px;
    color: var(--ink-3);
  }
</style>
