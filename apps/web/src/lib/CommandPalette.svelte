<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { PRIORITIES, type NewIssue, type Priority } from "@breadboard/api-client";
  import { store, statusName, type View } from "./store.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";

  let query = $state("");
  let cursor = $state(0);
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLElement | null>(null);

  // Extra fields for issue creation, collapsed by default (Tab opens them).
  // Quick fields typed into the title cover the common ones.
  let showOptions = $state(false);
  let optBody = $state("");
  let optPriority = $state<Priority | "">("");
  let optLabels = $state("");
  let optProject = $state("");
  let optEpic = $state<number | 0>(0);
  let optCycle = $state<number | 0>(0);
  let optEstimate = $state<number | "">("");

  interface Quick {
    title: string;
    priority?: Priority;
    labels: string[];
    mine: boolean;
    project?: string;
    estimate?: number;
  }

  /** Pull `!high #bug @me ^BAR ~3` out of the typed text. Anything that
   *  doesn't parse as a quick field stays in the title. */
  function parse(q: string): Quick {
    const out: Quick = { title: "", labels: [], mine: false };
    const words: string[] = [];
    for (const w of q.split(/\s+/)) {
      const lw = w.toLowerCase();
      if (/^!(none|low|medium|high|urgent)$/.test(lw)) out.priority = lw.slice(1) as Priority;
      else if (/^#[\w.-]+$/.test(w)) out.labels.push(w.slice(1));
      else if (lw === "@me") out.mine = true;
      else if (/^\^\w+$/.test(w) && store.projects.some((p) => p.key === w.slice(1).toUpperCase())) out.project = w.slice(1).toUpperCase();
      else if (/^~\d+$/.test(w)) out.estimate = Number(w.slice(1));
      else if (w) words.push(w);
    }
    out.title = words.join(" ");
    return out;
  }

  type Item =
    | { kind: "create"; quick: Quick }
    | { kind: "issue"; key: string }
    | { kind: "view"; label: string; hint?: string; go: () => void };

  const quick = $derived(parse(query.trim()));
  const hasQuick = $derived(!!(quick.priority || quick.labels.length || quick.mine || quick.project || quick.estimate != null));

  const groups = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const g: { name: string; items: Item[] }[] = [];
    if (q) {
      const t = quick.title.toLowerCase();
      const exact = store.issues.some((i) => i.key.toLowerCase() === q);
      if (quick.title && !exact) g.push({ name: "Create", items: [{ kind: "create", quick }] });
      const needle = t || q;
      const matches = store.issues
        .filter((i) => `${i.key} ${i.title}`.toLowerCase().includes(needle))
        .sort((a, b) => Number(b.key.toLowerCase() === needle) - Number(a.key.toLowerCase() === needle) || b.updated_at.localeCompare(a.updated_at))
        .slice(0, 8)
        .map<Item>((i) => ({ kind: "issue", key: i.key }));
      if (matches.length) g.push({ name: "Issues", items: matches });
      const views = goItems().filter((v) => v.kind === "view" && v.label.toLowerCase().includes(q));
      if (views.length) g.push({ name: "Go to", items: views });
    } else {
      const recent = [...store.issues]
        .filter((i) => i.type !== "pr")
        .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
        .slice(0, 4)
        .map<Item>((i) => ({ kind: "issue", key: i.key }));
      if (recent.length) g.push({ name: "Recently updated", items: recent });
      g.push({ name: "Go to", items: goItems() });
    }
    return g;
  });
  const flat = $derived(groups.flatMap((g) => g.items));

  function goItems(): Item[] {
    const v = (label: string, view: View): Item => ({ kind: "view", label, go: () => (store.view = view) });
    return [
      v("Board", "board"),
      v("Backlog", "backlog"),
      v("Roadmap", "roadmap"),
      v("Agents", "agents"),
      v("Inbox", "inbox"),
      v("Settings", "settings"),
      { kind: "view", label: store.me && store.isFiltered("assignees", store.me.id) ? "Show everyone's issues" : "Show only issues assigned to me", go: () => store.toggleMine() },
    ];
  }

  $effect(() => {
    if (store.paletteOpen) {
      query = "";
      cursor = 0;
      showOptions = false;
      optBody = "";
      optPriority = "";
      optLabels = "";
      optProject = store.filters.projects.length === 1 ? store.filters.projects[0] : "";
      optEpic = store.filters.epics.length === 1 ? store.filters.epics[0] : 0;
      optCycle = store.filters.cycles.length === 1 ? store.filters.cycles[0] : 0;
      optEstimate = "";
      queueMicrotask(() => inputEl?.focus());
    }
  });

  $effect(() => {
    void query;
    cursor = 0;
  });

  $effect(() => {
    void cursor;
    listEl?.querySelector(".active")?.scrollIntoView({ block: "nearest" });
  });

  function close() {
    store.paletteOpen = false;
  }

  async function run(item: Item) {
    if (item.kind === "create") {
      const q = item.quick;
      const input: NewIssue = { title: q.title };
      if (store.paletteStatus) input.status = store.paletteStatus;
      if (optBody.trim()) input.body = optBody.trim();
      const priority = q.priority ?? optPriority;
      if (priority) input.priority = priority;
      const project = q.project ?? optProject;
      // No silent default when there is a choice: a title-only card must not
      // land in whichever project happens to be first (or in a repo tracker
      // it doesn't belong to). Ask instead.
      if (!project && store.projects.filter((p) => !p.archived).length > 1) {
        showOptions = true;
        store.flash("Choose a project for this issue", "error");
        return;
      }
      if (project) input.project = project;
      if (optEpic) input.epic_id = optEpic;
      if (optCycle) input.cycle_id = optCycle;
      const est = q.estimate ?? optEstimate;
      if (est !== "" && est != null) input.estimate = Math.max(0, Math.round(Number(est)));
      const labels = [...q.labels, ...optLabels.split(",").map((s) => s.trim()).filter(Boolean)];
      if (labels.length) input.labels = [...new Set(labels)];
      if (q.mine && store.me) input.assignee_id = store.me.id;
      close();
      const issue = await store.create(input);
      if (issue) store.selectedKey = issue.key;
      return;
    }
    close();
    if (item.kind === "issue") store.selectedKey = item.key;
    else item.go();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      // Handled here so the open issue behind the palette stays open.
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      cursor = (cursor + 1) % Math.max(flat.length, 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      cursor = (cursor - 1 + flat.length) % Math.max(flat.length, 1);
    } else if (e.key === "Enter" && flat[cursor]) {
      e.preventDefault();
      void run(flat[cursor]);
    } else if (e.key === "Tab" && quick.title && !e.shiftKey && !showOptions) {
      e.preventDefault();
      showOptions = true;
    }
  }

  function hl(text: string): string {
    const t = (quick.title || query).trim();
    const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
    if (!t) return esc(text);
    const i = text.toLowerCase().indexOf(t.toLowerCase());
    if (i < 0) return esc(text);
    return esc(text.slice(0, i)) + "<mark>" + esc(text.slice(i, i + t.length)) + "</mark>" + esc(text.slice(i + t.length));
  }

  function insert(token: string) {
    query = (query.trimEnd() ? query.trimEnd() + " " : "") + token;
    inputEl?.focus();
  }

  const quickDesc = $derived(
    [
      quick.priority && `!${quick.priority}`,
      ...quick.labels.map((l) => `#${l}`),
      quick.mine && "@me",
      quick.project && `^${quick.project}`,
      quick.estimate != null && `~${quick.estimate}`,
      store.paletteStatus && `in ${statusName(store.paletteStatus)}`,
    ]
      .filter(Boolean)
      .join(" "),
  );
</script>

{#if store.paletteOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="scrim" role="presentation" transition:fade={{ duration: 120 }} onclick={(e) => e.target === e.currentTarget && close()}>
    <div class="pal" role="dialog" aria-modal="true" aria-label="Search or create" tabindex="-1" transition:fly={{ y: -8, duration: 160 }}>
      <div class="pin">
        <svg viewBox="0 0 16 16" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="1.6" d="M7 12.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11zm4-1.5 3.5 3.5" /></svg>
        <input
          bind:this={inputEl}
          bind:value={query}
          onkeydown={onKey}
          placeholder={store.paletteStatus ? `New issue in ${statusName(store.paletteStatus)}…` : "Search issues, jump to a view, or type a new title"}
          aria-label="Search or create"
          autocomplete="off"
          spellcheck="false"
        />
        <kbd>esc</kbd>
      </div>
      <div class="toks">
        <span class="lbl">Quick fields</span>
        <button class:hit={!!quick.priority} onclick={() => insert("!high")}>!high</button>
        <button class:hit={quick.labels.length > 0} onclick={() => insert("#")}>#label</button>
        <button class:hit={quick.mine} onclick={() => insert("@me")}>@me</button>
        <button class:hit={!!quick.project} onclick={() => insert("^" + (store.projects[0]?.key ?? "KEY"))}>^PROJECT</button>
        <button class:hit={quick.estimate != null} onclick={() => insert("~3")}>~3 pts</button>
        {#if quick.title}
          <button class="opts" onclick={() => (showOptions = !showOptions)}>{showOptions ? "Fewer fields" : "More fields"} <kbd>tab</kbd></button>
        {/if}
      </div>

      {#if quick.title && showOptions}
        <div class="options" transition:fly={{ y: -4, duration: 120 }}>
          <div class="opt-row">
            <select class="field" bind:value={optPriority} aria-label="Priority">
              <option value="">Priority…</option>
              {#each PRIORITIES as p (p)}<option value={p}>{statusName(p)}</option>{/each}
            </select>
            <select class="field" bind:value={optProject} aria-label="Project">
              <option value="">Project…</option>
              {#each store.projects.filter((p) => !p.archived) as p (p.id)}<option value={p.key}>{p.key} · {p.name}</option>{/each}
            </select>
            <select class="field" bind:value={optCycle} aria-label="Cycle">
              <option value={0}>Cycle…</option>
              {#each store.cycles.filter((c) => !c.closed) as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
            </select>
            <select class="field" bind:value={optEpic} aria-label="Epic">
              <option value={0}>Epic…</option>
              {#each store.epics.filter((e) => !e.archived) as ep (ep.id)}<option value={ep.id}>{ep.key}</option>{/each}
            </select>
            <input
              class="field est"
              type="number"
              min="0"
              step="1"
              placeholder="Pts"
              aria-label="Points"
              value={optEstimate}
              oninput={(e) => (optEstimate = e.currentTarget.value === "" ? "" : Math.round(Number(e.currentTarget.value)))}
            />
          </div>
          <input class="field" placeholder="Labels, comma separated" bind:value={optLabels} />
          <textarea class="field" rows="3" placeholder="Description (markdown)" bind:value={optBody}></textarea>
        </div>
      {/if}

      <div class="list" bind:this={listEl}>
        {#each groups as g (g.name)}
          <div class="gh">{g.name}</div>
          {#each g.items as item (item.kind + (item.kind === "issue" ? item.key : item.kind === "view" ? item.label : "c"))}
            {@const idx = flat.indexOf(item)}
            <button class="it" class:active={idx === cursor} onmouseenter={() => (cursor = idx)} onclick={() => run(item)}>
              {#if item.kind === "create"}
                <span class="plus">+</span>
                <span class="t">New issue: <b>{item.quick.title}</b>{#if quickDesc}<span class="qd">{quickDesc}</span>{/if}</span>
                <span class="go">↵ create</span>
              {:else if item.kind === "issue"}
                {@const i = store.issueByKey(item.key)}
                {#if i}<StatusGlyph status={i.status} />{/if}
                <span class="key">{item.key}</span>
                <span class="t">{@html hl(i?.title ?? "")}</span>
                <span class="go">↵ open</span>
              {:else}
                <span class="arrow">→</span>
                <span class="t">{item.label}</span>
              {/if}
            </button>
          {/each}
        {:else}
          <p class="none">No matches. Type a title to create an issue.</p>
        {/each}
      </div>
      <div class="foot"><span>↑↓ move</span><span>↵ open or create</span><span>tab more fields</span><span>esc close</span></div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 13vh 16px 16px;
    background: rgba(5, 4, 3, 0.55);
    backdrop-filter: blur(3px);
  }
  .pal {
    width: min(640px, 100%);
    border-radius: 14px;
    background: var(--surface);
    border: 1px solid var(--line-2);
    box-shadow: 0 30px 90px rgba(0, 0, 0, 0.6);
    overflow: hidden;
    outline: none;
  }
  .pin {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--line);
  }
  .pin svg {
    width: 17px;
    height: 17px;
    color: var(--ink-3);
    flex: none;
  }
  .pin input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    font-size: 16px;
    outline: none;
  }
  .pin input::placeholder {
    color: var(--ink-3);
  }
  .toks {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .toks .lbl {
    margin-right: 2px;
  }
  .toks button {
    padding: 3px 6px;
    border-radius: 5px;
    background: var(--bg);
    border: 1px solid var(--line);
    font: inherit;
    color: var(--ink-dim);
  }
  .toks button:hover {
    border-color: var(--line-2);
    color: var(--ink);
  }
  .toks button.hit {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 50%, transparent);
    background: var(--accent-soft);
  }
  .toks .opts {
    margin-left: auto;
    display: inline-flex;
    gap: 6px;
    align-items: center;
    background: none;
    border-color: transparent;
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--bg);
  }
  .opt-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .options .field {
    background: var(--surface);
    font-size: 12px;
  }
  .options textarea {
    resize: vertical;
  }
  .est {
    width: 70px;
  }
  .list {
    max-height: 46vh;
    overflow-y: auto;
    padding: 6px;
  }
  .gh {
    padding: 10px 10px 6px;
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .it {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: none;
    font-size: 13.5px;
    text-align: left;
  }
  .it.active {
    background: color-mix(in srgb, var(--ink) 6.5%, transparent);
  }
  .key {
    width: 64px;
    flex: none;
    font: 500 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .t {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .t :global(mark) {
    background: none;
    color: var(--accent);
  }
  .qd {
    margin-left: 8px;
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .go {
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
    visibility: hidden;
  }
  .it.active .go {
    visibility: visible;
  }
  .plus {
    width: 18px;
    height: 18px;
    border-radius: 5px;
    flex: none;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 700;
  }
  .arrow {
    width: 14px;
    color: var(--ink-3);
  }
  .none {
    margin: 0;
    padding: 14px 10px;
    color: var(--ink-3);
    font-size: 13px;
  }
  .foot {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    padding: 9px 16px;
    border-top: 1px solid var(--line);
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  @media (max-width: 860px) {
    .scrim {
      padding-top: 24px;
    }
    .foot {
      display: none;
    }
  }
</style>
