<script lang="ts">
  import { tick, onMount } from "svelte";
  import { fly } from "svelte/transition";
  import type { AgentClaim } from "@breadboard/api-client";
  import { store } from "./store.svelte";

  let note = $state("");
  let noNote = $state(false);
  let busy = $state(false);
  let dialogEl = $state<HTMLDivElement | null>(null);
  let noteEl = $state<HTMLTextAreaElement | null>(null);

  const claim = $derived(store.pendingRelease);

  // Elements the Tab trap may move through. Queried live because the note
  // checkbox and the error text appear and disappear while the dialog is open.
  const FOCUSABLE =
    'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
  let returnFocus: HTMLElement | null = null;
  let fallbackFocus: HTMLElement | null = null;
  let wasOpen = false;

  function focusables(): HTMLElement[] {
    if (!dialogEl) return [];
    return Array.from(dialogEl.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => el.getClientRects().length > 0,
    );
  }

  // BB-199: behave like a real modal. Remember the trigger, move focus to the
  // note field on open, and restore focus when the dialog closes. If the
  // trigger disappeared (the released claim row is gone), fall back to the
  // dialog that contained it.
  $effect(() => {
    const open = claim !== null;
    if (open === wasOpen) return;
    wasOpen = open;
    if (open) {
      const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      returnFocus = active;
      fallbackFocus = active?.closest<HTMLElement>('[role="dialog"]') ?? null;
      void tick().then(() => noteEl?.focus());
      return;
    }
    const target = returnFocus?.isConnected ? returnFocus : fallbackFocus?.isConnected ? fallbackFocus : null;
    returnFocus = null;
    fallbackFocus = null;
    void tick().then(() => target?.focus());
  });

  // Escape is taken at the window in the capture phase. This listener is
  // registered on mount, before the agent panel registers its own Escape
  // handler when a card opens, so it runs first and stops immediate
  // propagation: the panel cannot close underneath the modal and the app's
  // global handler cannot blur the field. Tab cycles inside the dialog and
  // cannot reach the page behind it.
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!claim || !dialogEl) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close();
        return;
      }
      if (e.key !== "Tab") return;
      const items = focusables();
      if (!items.length) {
        e.preventDefault();
        dialogEl.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      const inside = active instanceof Node && dialogEl.contains(active);
      if (e.shiftKey) {
        if (!inside || active === first) {
          e.preventDefault();
          last.focus();
        }
      } else if (!inside || active === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  /** Mirror of the server rule: an intent claim, or an exclusive claim still
   *  planned with no branch/worktree/repo/PR/commit, never held work. */
  function neverHeldWork(c: AgentClaim): boolean {
    if (c.mode === "intent") return true;
    return (
      c.state === "planned" &&
      !c.branch &&
      !c.worktree &&
      !c.repo &&
      !c.pr_url &&
      c.pr_number == null &&
      !c.head_sha
    );
  }
  const canNoNote = $derived(claim ? neverHeldWork(claim) : false);

  function close() {
    store.pendingRelease = null;
    store.releaseError = null;
    note = "";
    noNote = false;
    busy = false;
  }

  async function submit() {
    if (!claim) return;
    const text = note.trim();
    if (!noNote && !text) {
      store.releaseError =
        "A handoff note is required: one line covering what is done, what is next and what is risky.";
      return;
    }
    busy = true;
    const ok = await store.releaseById(
      claim.id,
      true,
      noNote ? undefined : text,
      noNote,
    );
    busy = false;
    if (ok) close();
  }
</script>

{#if claim}
  <div
    class="overlay"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget) close();
    }}
  >
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Release {claim.key}"
      aria-describedby="release-handoff-help"
      tabindex="-1"
      bind:this={dialogEl}
      transition:fly={{ y: 8, duration: 150, opacity: 0.4 }}
    >
      <header>
        <h2>Release {claim.key}</h2>
        <button class="ib" aria-label="Cancel" onclick={close}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
        </button>
      </header>
      <p class="lede" id="release-handoff-help">
        Leave one line for the next session: <b>what is done</b>, <b>what is next</b> and
        <b>what is risky</b>. It survives the release, appears on the card and in the
        agent timeline, and is returned when someone resumes this issue.
      </p>
      <textarea
        class="field"
        rows="3"
        maxlength="500"
        placeholder="done: ...; next: ...; risky: ..."
        bind:value={note}
        bind:this={noteEl}
        disabled={noNote}
        aria-label="Handoff note"
      ></textarea>
      {#if canNoNote}
        <label class="no-note">
          <input type="checkbox" bind:checked={noNote} />
          No work happened (claim never held work): release without a note
        </label>
      {/if}
      {#if store.releaseError}
        <p class="error">{store.releaseError}</p>
      {/if}
      <footer>
        <button class="btn" onclick={close}>Cancel</button>
        <button class="btn primary" disabled={busy || (!noNote && !note.trim())} onclick={() => void submit()}>
          {busy ? "Releasing…" : noNote ? "Release without note" : "Release with handoff"}
        </button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: grid;
    place-items: center;
    padding: 20px;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(2px);
  }
  .dialog {
    width: min(460px, 100%);
    background: var(--surface);
    border: 1px solid var(--line-2);
    border-radius: 12px;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.5);
    padding: 16px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    flex: 1;
  }
  .lede {
    margin: 0 0 10px;
    color: var(--muted);
    font-size: 12px;
    line-height: 1.45;
  }
  .field {
    width: 100%;
    resize: vertical;
    min-height: 74px;
  }
  .no-note {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin-top: 9px;
    font-size: 11px;
    color: var(--muted);
    line-height: 1.35;
  }
  .no-note input {
    margin-top: 2px;
  }
  .error {
    margin: 9px 0 0;
    color: var(--danger);
    font-size: 12px;
    line-height: 1.4;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }
  .ib {
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }
  .ib:hover {
    background: var(--raise);
    color: var(--text);
  }
  .ib svg {
    width: 14px;
    height: 14px;
  }
</style>
