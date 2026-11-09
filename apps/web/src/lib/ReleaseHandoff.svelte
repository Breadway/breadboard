<script lang="ts">
  import { fly } from "svelte/transition";
  import type { AgentClaim } from "@breadboard/api-client";
  import { store } from "./store.svelte";

  let note = $state("");
  let noNote = $state(false);
  let busy = $state(false);

  const claim = $derived(store.pendingRelease);

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
      tabindex="-1"
      transition:fly={{ y: 8, duration: 150, opacity: 0.4 }}
      onkeydown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          close();
        }
      }}
    >
      <header>
        <h2>Release {claim.key}</h2>
        <button class="ib" aria-label="Cancel" onclick={close}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
        </button>
      </header>
      <p class="lede">
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
