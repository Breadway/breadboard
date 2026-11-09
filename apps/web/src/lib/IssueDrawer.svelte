<script lang="ts">
  import { fly } from "svelte/transition";
  import {
    PRIORITIES,
    safeHttpUrl,
    STATUSES,
    type ActivityEntry,
    type Comment,
    type Issue,
    type IssueLink,
    type IssuePatch,
    type LinkKind,
  } from "@breadboard/api-client";
  import { ago, api, claimStale, errMsg, store, statusName } from "./store.svelte";
  import { renderMarkdown } from "./markdown";
  import Menu, { type MenuItem } from "./ui/Menu.svelte";
  import StatusGlyph from "./ui/StatusGlyph.svelte";
  import PriorityGlyph from "./ui/PriorityGlyph.svelte";
  import Avatar from "./ui/Avatar.svelte";
  import Ring from "./ui/Ring.svelte";
  import UsageStats from "./UsageStats.svelte";
  import { agentColor, labelColor, projectColor } from "./ui/colors";

  // The issue's fields come straight from the store, so a drag on the board,
  // a live event from someone else, or a refused save (which leaves the store
  // untouched) all show here immediately. Only comments and activity, which
  // the store doesn't hold, are fetched per issue.
  let fetched = $state<Issue | null>(null);
  const issue = $derived(store.issueByKey(store.selectedKey) ?? (fetched?.key === store.selectedKey ? fetched : null));

  let comments = $state<Comment[]>([]);
  let activity = $state<ActivityEntry[]>([]);
  let loadedKey = $state<string | null>(null);
  let loading = $state(false);

  let titleDraft = $state("");
  let titleFocused = $state(false);
  let titleEl: HTMLTextAreaElement | undefined = $state();
  let editingBody = $state(false);
  let bodyDraft = $state("");
  let labelDraft = $state("");
  let commentDraft = $state("");
  let posting = $state(false);
  let confirmingDelete = $state(false);
  let addingLink = $state(false);
  let claiming = $state(false);
  let claimAgent = $state("");
  let claimNote = $state("");
  let claimMode = $state<"exclusive" | "intent">("exclusive");
  let claimBusy = $state(false);
  let linkKind = $state<LinkKind>("blocks");
  let linkTarget = $state("");
  let linkBusy = $state(false);

  const POINTS = [1, 2, 3, 5, 8, 13];
  const manualLinkKinds: LinkKind[] = ["blocks", "blocked-by", "relates-to"];
  const linkLabels: Record<string, string> = {
    closes: "Closes",
    "closed-by": "Closed by",
    blocks: "Blocks",
    "blocked-by": "Blocked by",
    "relates-to": "Relates to",
  };

  async function load(key: string) {
    loading = true;
    try {
      const [d, a] = await Promise.all([api.getIssue(key), api.getActivity(key).catch(() => [] as ActivityEntry[])]);
      // A slower response for an issue you've already left must not land
      // in this panel.
      if (store.selectedKey !== key) return;
      fetched = d;
      comments = d.comments;
      activity = a;
      loadedKey = key;
    } catch {
      if (store.selectedKey === key) store.flash(`Couldn't load ${key}`);
    } finally {
      if (store.selectedKey === key) loading = false;
    }
  }

  // New issue selected: reset local UI state and fetch its thread.
  $effect(() => {
    const key = store.selectedKey;
    editingBody = false;
    confirmingDelete = false;
    addingLink = false;
    labelDraft = "";
    if (!key) {
      comments = [];
      activity = [];
      loadedKey = null;
      return;
    }
    if (loadedKey !== key) {
      comments = [];
      activity = [];
    }
    void load(key);
  });

  // A live event touched the open issue: refresh comments and activity.
  $effect(() => {
    const t = store.issueTick;
    if (t.n && t.key === store.selectedKey && t.key === loadedKey) void load(t.key);
  });

  // Keep the title draft in step with the store unless you're typing in it.
  $effect(() => {
    const t = issue?.title ?? "";
    if (!titleFocused) titleDraft = t;
  });

  // Grow the title to fit. Measured a frame later: while the panel is still
  // sliding in, scrollHeight can read 0 and would collapse it.
  function fitTitle() {
    if (!titleEl) return;
    titleEl.style.height = "auto";
    if (titleEl.scrollHeight) titleEl.style.height = titleEl.scrollHeight + "px";
  }
  $effect(() => {
    void titleDraft;
    void titleEl;
    const id = requestAnimationFrame(fitTitle);
    return () => cancelAnimationFrame(id);
  });

  async function save(patch: IssuePatch) {
    if (!issue) return;
    await store.patch(issue.key, patch);
  }

  function saveTitle() {
    titleFocused = false;
    const t = titleDraft.trim();
    if (issue && t && t !== issue.title) void save({ title: t });
    else titleDraft = issue?.title ?? "";
  }

  function startBody() {
    bodyDraft = issue?.body ?? "";
    editingBody = true;
  }
  async function saveBody() {
    if (issue && bodyDraft !== issue.body) await save({ body: bodyDraft });
    editingBody = false;
  }

  function addLabel() {
    if (!issue) return;
    const add = labelDraft.split(",").map((s) => s.trim()).filter(Boolean);
    labelDraft = "";
    const next = [...new Set([...issue.labels, ...add])];
    if (next.length !== issue.labels.length) void save({ labels: next });
  }
  function removeLabel(l: string) {
    if (issue) void save({ labels: issue.labels.filter((x) => x !== l) });
  }

  const claim = $derived(issue ? store.claimForIssue(issue.id) : undefined);
  const safePrUrl = $derived(claim ? safeHttpUrl(claim.pr_url) : null);
  const intents = $derived(
    issue ? store.claimsForIssue(issue.id).filter((c) => c.mode === "intent") : [],
  );
  const claimStaleFlag = $derived(claim ? claimStale(claim.last_heartbeat, store.clock) : false);
  const claimAge = $derived(claim ? ago(claim.last_heartbeat, store.clock) : "");

  async function doClaim() {
    if (!issue) return;
    const agent_name = claimAgent.trim() || store.me?.login || "agent";
    claimBusy = true;
    const ok = await store.claim(issue.key, {
      agent_name,
      mode: claimMode,
      note: claimNote.trim() || undefined,
    });
    claimBusy = false;
    if (ok) {
      claiming = false;
      claimAgent = "";
      claimNote = "";
    }
  }

  const statusItems = $derived<MenuItem[]>(
    STATUSES.map((s, i) => ({
      label: statusName(s),
      glyph: { status: s },
      hint: String(i + 1),
      current: issue?.status === s,
      run: () => issue && void store.move(issue.key, s),
    })),
  );
  const priorityItems = $derived<MenuItem[]>(
    [...PRIORITIES].reverse().map((p) => ({
      label: p === "none" ? "No priority" : statusName(p),
      glyph: { priority: p },
      current: issue?.priority === p,
      run: () => void save({ priority: p }),
    })),
  );
  const assigneeItems = $derived<MenuItem[]>([
    { label: store.me ? `${store.me.login} (you)` : "Me", glyph: { dot: "#d98a3d" }, current: issue?.assignee_id === store.me?.id, run: () => void save({ assignee_id: store.me?.id ?? null }) },
    { label: "Unassigned", current: issue?.assignee_id == null, run: () => void save({ assignee_id: null }) },
  ]);
  const pointItems = $derived<MenuItem[]>([
    { label: "No estimate", current: issue?.estimate == null, run: () => void save({ estimate: null }) },
    ...[...new Set([...POINTS, ...(issue?.estimate != null ? [issue.estimate] : [])])]
      .sort((a, b) => a - b)
      .map((n) => ({ label: `${n} ${n === 1 ? "point" : "points"}`, current: issue?.estimate === n, run: () => void save({ estimate: n }) })),
  ]);
  const projectItems = $derived<MenuItem[]>(
    store.projects
      .filter((p) => !p.archived || p.id === issue?.project_id)
      .map((p) => ({
        label: `${p.key} · ${p.name}`,
        glyph: { dot: projectColor(p.key) },
        current: p.id === issue?.project_id,
        run: () => issue && p.id !== issue.project_id && void store.moveProject(issue.key, p.key),
      })),
  );
  const cycleItems = $derived<MenuItem[]>([
    { label: "No cycle", current: issue?.cycle_id == null, run: () => void save({ cycle_id: null }) },
    ...store.cycles.map((c) => ({
      label: `${c.name}${c.closed ? " (closed)" : ""}`,
      current: issue?.cycle_id === c.id,
      run: () => void save({ cycle_id: c.id }),
    })),
  ]);
  const epicItems = $derived<MenuItem[]>([
    { label: "No epic", current: issue?.epic_id == null, run: () => void save({ epic_id: null }) },
    ...store.epics
      .filter((e) => !e.archived || e.id === issue?.epic_id)
      .map((e) => ({ label: `${e.key} · ${e.title}`, glyph: { text: "◆" }, current: issue?.epic_id === e.id, run: () => void save({ epic_id: e.id }) })),
  ]);

  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      store.info(`Copied ${what}`);
    } catch {
      store.flash(`Couldn't copy ${what}`);
    }
  }
  const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);
  const moreItems = $derived<MenuItem[]>(
    issue
      ? [
          { label: "Copy key", run: () => void copy(issue.key, issue.key) },
          { label: "Copy link", run: () => void copy(`${store.serverBase}/#${issue.key}`, "link") },
          { label: "Copy branch name", run: () => void copy(`fix/${issue.key.toLowerCase()}-${slug(issue.title)}`, "branch name") },
          { label: "Delete issue…", danger: true, run: () => (confirmingDelete = true) },
        ]
      : [],
  );

  const selectedCycle = $derived(store.cycles.find((c) => c.id === issue?.cycle_id));
  const cycleDone = $derived(selectedCycle ? store.issues.filter((i) => i.cycle_id === selectedCycle.id && i.status === "done").length : 0);
  const cycleTotal = $derived(selectedCycle ? store.issues.filter((i) => i.cycle_id === selectedCycle.id).length : 0);
  const selectedEpic = $derived(store.epics.find((e) => e.id === issue?.epic_id));
  const project = $derived(issue ? store.project(issue.project_id) : undefined);

  async function resolveTargetId(keyOrId: string): Promise<number | null> {
    const t = keyOrId.trim();
    if (!t) return null;
    const known = store.issues.find((i) => i.key.toLowerCase() === t.toLowerCase());
    if (known) return known.id;
    if (/^\d+$/.test(t)) return Number(t);
    try {
      return (await api.getIssue(t.toUpperCase())).id;
    } catch {
      return null;
    }
  }

  async function addLink() {
    if (!issue || !linkTarget.trim()) return;
    linkBusy = true;
    try {
      const targetId = await resolveTargetId(linkTarget);
      if (targetId === null) return store.flash(`No issue called "${linkTarget.trim()}"`);
      if (targetId === issue.id) return store.flash("An issue can't link to itself");
      if (!issue.links.some((l) => l.kind === linkKind && l.target_id === targetId)) {
        await save({ links: [...issue.links, { kind: linkKind, target_id: targetId }] });
      }
      linkTarget = "";
      addingLink = false;
    } finally {
      linkBusy = false;
    }
  }

  function removeLink(link: IssueLink) {
    if (issue) void save({ links: issue.links.filter((l) => !(l.kind === link.kind && l.target_id === link.target_id)) });
  }

  async function postComment() {
    if (!issue || !commentDraft.trim() || posting) return;
    posting = true;
    try {
      const c = await api.addComment(issue.key, commentDraft.trim());
      comments = [...comments, c];
      commentDraft = "";
    } catch (e) {
      store.flash(`Couldn't post the comment: ${errMsg(e)}`);
    } finally {
      posting = false;
    }
  }

  type Entry = { at: string; c?: Comment; a?: ActivityEntry };
  const timeline = $derived<Entry[]>(
    [...activity.map((a) => ({ at: a.at, a })), ...comments.map((c) => ({ at: c.created_at, c }))].sort((x, y) =>
      x.at.localeCompare(y.at),
    ),
  );

  const FIELD: Record<string, string> = {
    assignee_id: "assignee",
    epic_id: "epic",
    cycle_id: "cycle",
    estimate: "points",
    agent_claim: "agent claim",
    agent_state: "agent state",
    agent_release: "agent release",
    agent_pr: "agent PR",
  };
  // Activity stores raw column values; turn ids back into names.
  function val(field: string, v: string | null): string {
    if (v == null || v === "") return "none";
    const n = Number(v);
    if (field === "assignee_id") return n === store.me?.id ? store.me.login : `user #${v}`;
    if (field === "cycle_id") return store.cycles.find((c) => c.id === n)?.name ?? `cycle #${v}`;
    if (field === "epic_id") return store.epics.find((e) => e.id === n)?.key ?? `epic #${v}`;
    return v;
  }
  function who(a: ActivityEntry) {
    return a.actor ?? (a.source === "breadboard" ? "Someone" : "Forgejo");
  }
  function author(c: Comment) {
    if (c.author_id != null && c.author_id === store.me?.id) return store.me.login;
    return c.external_id ? "Forgejo" : "Someone";
  }
  function when(ts: string) {
    const d = new Date(ts);
    const today = new Date().toDateString() === d.toDateString();
    return today
      ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
      : d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  }
</script>

{#if store.selectedKey}
  <aside class="drawer" aria-label="Issue {store.selectedKey}" transition:fly={{ x: 60, duration: 220, opacity: 0.4 }}>
    <header>
      {#if issue}<StatusGlyph status={issue.status} />{/if}
      <span class="key">{store.selectedKey}</span>
      {#if issue?.forgejo_ref}
        <span class="sync" title="Synced with Forgejo">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 6.5A5 5 0 0 1 12.2 4.5M13 9.5A5 5 0 0 1 3.8 11.5M12.5 2v2.8H9.7M3.5 14v-2.8h2.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
          {issue.forgejo_ref.repo}#{issue.forgejo_ref.number}
        </span>
      {/if}
      {#if issue?.milestone}<span class="sync" title="Forgejo milestone">◆ {issue.milestone}</span>{/if}
      <span class="grow"></span>
      {#if issue}
        <Menu items={moreItems} triggerClass="ib" title="More actions" align="right">
          {#snippet trigger()}<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3.5" cy="8" r="1.3" fill="currentColor" /><circle cx="8" cy="8" r="1.3" fill="currentColor" /><circle cx="12.5" cy="8" r="1.3" fill="currentColor" /></svg>{/snippet}
        </Menu>
      {/if}
      <button class="ib" title="Close (Esc)" aria-label="Close" onclick={() => (store.selectedKey = null)}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
      </button>
    </header>

    {#if !issue}
      <p class="dim pad">{loading ? "Loading…" : "This issue isn't available."}</p>
    {:else}
      <div class="body">
        <textarea
          class="title"
          rows="1"
          bind:this={titleEl}
          bind:value={titleDraft}
          aria-label="Title"
          onfocus={() => (titleFocused = true)}
          onblur={saveTitle}
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              e.currentTarget.blur();
            }
          }}
        ></textarea>

        <div class="props">
          <span class="pl">Status</span>
          <Menu items={statusItems} triggerClass="pv">
            {#snippet trigger()}<StatusGlyph status={issue.status} />{statusName(issue.status)}{/snippet}
          </Menu>
          <span class="pl">Priority</span>
          <Menu items={priorityItems} triggerClass="pv">
            {#snippet trigger()}<PriorityGlyph priority={issue.priority} />{#if issue.priority === "none"}<span class="dim">No priority</span>{:else}{statusName(issue.priority)}{/if}{/snippet}
          </Menu>

          <span class="pl">Assignee</span>
          <Menu items={assigneeItems} triggerClass="pv">
            {#snippet trigger()}
              {#if issue.assignee_id == null}<span class="dim">Unassigned</span>
              {:else}<Avatar userId={issue.assignee_id} size={18} />{issue.assignee_id === store.me?.id ? store.me?.login : `User #${issue.assignee_id}`}{/if}
            {/snippet}
          </Menu>
          <span class="pl">Points</span>
          <Menu items={pointItems} triggerClass="pv">
            {#snippet trigger()}{#if issue.estimate == null}<span class="dim">No estimate</span>{:else}{issue.estimate} {issue.estimate === 1 ? "pt" : "pts"}{/if}{/snippet}
          </Menu>

          <span class="pl">Project</span>
          <Menu items={projectItems} triggerClass="pv" filter>
            {#snippet trigger()}<span class="pj" style="background:{projectColor(project?.key ?? '?')}">{(project?.key ?? "?")[0]}</span><span class="tx">{project?.name ?? "?"}</span>{/snippet}
          </Menu>
          <span class="pl">Cycle</span>
          <Menu items={cycleItems} triggerClass="pv">
            {#snippet trigger()}{#if selectedCycle}<Ring done={cycleDone} total={cycleTotal} size={15} /><span class="tx">{selectedCycle.name}</span>{:else}<span class="dim">No cycle</span>{/if}{/snippet}
          </Menu>

          <span class="pl">Epic</span>
          <div class="span3">
            <Menu items={epicItems} triggerClass="pv" filter>
              {#snippet trigger()}{#if selectedEpic}<span class="ek">{selectedEpic.key}</span><span class="tx">{selectedEpic.title}</span>{:else}<span class="dim">No epic</span>{/if}{/snippet}
            </Menu>
          </div>

          <span class="pl">Labels</span>
          <div class="span3 labels">
            {#each issue.labels as l (l)}
              <span class="chip-label lab"><i style="background:{labelColor(l)}"></i>{l}<button aria-label="Remove label {l}" onclick={() => removeLabel(l)}>×</button></span>
            {/each}
            <input
              class="addlab"
              placeholder={issue.labels.length ? "+ label" : "Add labels"}
              bind:value={labelDraft}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === ",") {
                  e.preventDefault();
                  addLabel();
                }
              }}
              onblur={addLabel}
            />
          </div>
        </div>

        <section class="sec agent-sec" class:claimed={!!claim} style={claim ? `--agent:${agentColor(claim.agent_name)}` : undefined}>
          <h3>
            Agent
            {#if claim}
              <span
                class="agent-pill"
                class:stale={claimStaleFlag}
                style="--agent:{agentColor(claim.agent_name)}"
                title={claimStaleFlag
                  ? `Last heartbeat ${claimAge}; claim may be abandoned`
                  : `Last heartbeat ${claimAge} ago`}
              >
                {#if claimStaleFlag}
                  <svg class="clock" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 4.8V8l2.2 1.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
                {:else}
                  <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
                {/if}
                {claim.agent_name} · {claim.state}{claimStaleFlag ? " · stale" : ""}
              </span>
            {/if}
            <span class="grow"></span>
            {#if claim}
              {#if claim.claimed_by === store.me?.id}
                <button onclick={() => claim && void store.releaseById(claim.id)}>Release</button>
              {/if}
            {:else}
              <button onclick={() => (claiming = !claiming)}>{claiming ? "Cancel" : "+ Claim"}</button>
            {/if}
          </h3>
          {#if claim}
            <div class="agent-chips">
              {#if claim.claimed_by_login !== store.me?.login}
                <span class="achip" title="Claimed with this user's token">{claim.claimed_by_login}</span>
              {/if}
              {#if claim.session_id}
                <span class="achip mono" title="Agent session">{claim.session_id.slice(0, 8)}</span>
              {/if}
              {#if claim.branch}
                <span class="achip mono" title="Branch">
                  <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4.5" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="11.5" cy="6" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.5 5.1v5.8M11.5 7.6c0 2.2-2 2.6-4.6 3.3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
                  {claim.branch}
                </span>
              {/if}
              {#if claim.pr_url}
                <a class="achip mono link" href={safePrUrl} target="_blank" rel="noreferrer" title="Pull request">
                  {#if !safePrUrl}<span>{claim.pr_url}</span>{:else}
                  <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="4" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="12" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4 5.1v5.8M12 10.9V6.5a2 2 0 0 0-2-2H7.5m1.5-2-2 2 2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
                  PR{claim.pr_number ? ` #${claim.pr_number}` : ""}{/if}
                </a>
              {:else if claim.pr_number}
                <span class="achip mono">PR #{claim.pr_number}</span>
              {/if}
              {#if claim.head_sha}<span class="achip mono" title="Head commit">{claim.head_sha.slice(0, 7)}</span>{/if}
            </div>
            {#if claim.note}<p class="agent-note">{claim.note}</p>{/if}
            {#if claim.claimed_by !== store.me?.id}
              <p class="dim small">Claimed by someone else. Only {claim.claimed_by_login} can heartbeat or release it.</p>
            {/if}
          {:else if claiming}
            <div class="claim-form">
              <div class="mode-seg" role="group" aria-label="Claim mode">
                <button class:on={claimMode === "exclusive"} onclick={() => (claimMode = "exclusive")} title="Take the issue: first claim wins and the card moves to doing">
                  Work it now
                </button>
                <button class:on={claimMode === "intent"} onclick={() => (claimMode = "intent")} title="Queue your interest without moving the card or blocking another agent">
                  Queue intent
                </button>
              </div>
              <input
                class="field"
                placeholder="Fun agent name (e.g. Toasty Pretzel)"
                bind:value={claimAgent}
                aria-label="Agent name"
              />
              <input
                class="field"
                placeholder="What are you starting? (optional note)"
                bind:value={claimNote}
                aria-label="Claim note"
                onkeydown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void doClaim();
                  }
                }}
              />
              <button class="btn primary" disabled={claimBusy} onclick={() => void doClaim()}>
                {claimBusy
                  ? "Claiming…"
                  : claimMode === "intent"
                    ? `Queue intent for ${issue.key}`
                    : `Claim ${issue.key}`}
              </button>
            </div>
            <p class="dim small">Claiming moves backlog/todo → doing and assigns the card to you.</p>
            <p class="dim small">Planning to take it later? Ask for an <b>intent</b> instead: it shows as queued without blocking anyone.</p>
          {:else}
            <p class="dim small">No agent is working this. Claim it before starting so the board shows it live.</p>
          {/if}

          {#if intents.length}
            <div class="intents">
              <span class="intents-label">Queued</span>
              {#each intents as it (it.id)}
                <span class="achip intent" style="--agent:{agentColor(it.agent_name)}" title="Soft reservation by {it.claimed_by_login}">
                  <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="3.2" y="5" width="9.6" height="7.4" rx="2" fill="none" stroke="currentColor" stroke-width="1.4" /><path d="M8 2.2v2.6M6.1 8.2h.01M9.9 8.2h.01M6.4 10.4h3.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
                  {it.agent_name} · {it.state}
                  {#if it.claimed_by === store.me?.id}
                    <button class="intent-x" aria-label="Release queued claim {it.agent_name}" onclick={() => void store.releaseById(it.id)}>×</button>
                  {/if}
                </span>
              {/each}
            </div>
          {/if}
        </section>

        <UsageStats issueKey={issue.key} compact />

        {#if selectedEpic?.description?.trim()}
          <p class="epic-desc"><span class="ek">{selectedEpic.key}</span> {selectedEpic.description}</p>
        {/if}

        <section class="sec">
          <h3>Relations <button onclick={() => (addingLink = !addingLink)}>{addingLink ? "Cancel" : "+ Add"}</button></h3>
          {#each issue.links as l (l.kind + l.target_id)}
            {@const t = store.findIssue(l.target_id)}
            <div class="rel">
              <button class="rel-main" title={t ? `${t.key}: ${t.title}` : `#${l.target_id}`} onclick={() => t && (store.selectedKey = t.key)} disabled={!t}>
                <span class="rk k-{l.kind}">{linkLabels[l.kind] ?? l.kind}</span>
                {#if t}<StatusGlyph status={t.status} />{/if}
                <span class="key">{t?.key ?? `#${l.target_id}`}</span>
                <span class="tx">{t?.title ?? ""}</span>
              </button>
              {#if manualLinkKinds.includes(l.kind)}
                <button class="rel-x" title="Remove link" aria-label="Remove link" onclick={() => removeLink(l)}>×</button>
              {/if}
            </div>
          {:else}
            {#if !addingLink}<p class="dim small">No related issues.</p>{/if}
          {/each}
          {#if addingLink}
            <div class="link-add">
              <select class="field" bind:value={linkKind} aria-label="Link type">
                {#each manualLinkKinds as k (k)}<option value={k}>{linkLabels[k]}</option>{/each}
              </select>
              <!-- svelte-ignore a11y_autofocus -->
              <input class="field" autofocus placeholder="Issue key, e.g. BB-12" bind:value={linkTarget} onkeydown={(e) => e.key === "Enter" && addLink()} />
              <button class="btn" disabled={linkBusy || !linkTarget.trim()} onclick={addLink}>Link</button>
            </div>
          {/if}
        </section>

        <section class="sec">
          <h3>Description {#if !editingBody}<button onclick={startBody}>Edit</button>{/if}</h3>
          {#if editingBody}
            <!-- svelte-ignore a11y_autofocus -->
            <textarea
              class="field desc"
              rows="8"
              autofocus
              bind:value={bodyDraft}
              placeholder="Markdown works here."
              onkeydown={(e) => {
                if ((e.ctrlKey || e.metaKey) && e.key === "Enter") void saveBody();
                if (e.key === "Escape") {
                  e.stopPropagation();
                  editingBody = false;
                }
              }}
            ></textarea>
            <div class="row-end">
              <span class="dim small">Ctrl ↵ to save · Esc to cancel</span>
              <button class="btn ghost" onclick={() => (editingBody = false)}>Cancel</button>
              <button class="btn primary" onclick={saveBody}>Save</button>
            </div>
          {:else if issue.body.trim()}
            <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
            <div class="md" role="presentation" ondblclick={startBody}>{@html renderMarkdown(issue.body)}</div>
          {:else}
            <button class="empty-desc" onclick={startBody}>Add a description…</button>
          {/if}
        </section>

        <section class="sec">
          <h3>Activity</h3>
          <div class="tl">
            {#each timeline as e, i (i)}
              {#if e.c}
                <div class="cm">
                  <div class="cm-h"><b>{author(e.c)}</b>{#if e.c.external_id}<span class="via">via Forgejo</span>{/if}<time title={new Date(e.at).toLocaleString()}>{when(e.at)}</time></div>
                  <div class="md">{@html renderMarkdown(e.c.body)}</div>
                </div>
              {:else if e.a}
                <div class="ev">
                  {#if e.a.field === "created"}
                    <b>{who(e.a)}</b> created this issue
                  {:else}
                    <b>{who(e.a)}</b> changed {FIELD[e.a.field] ?? e.a.field}
                    {#if e.a.old_value}from <em>{val(e.a.field, e.a.old_value)}</em>{/if}
                    to <em>{val(e.a.field, e.a.new_value)}</em>
                  {/if}
                  <time title={new Date(e.at).toLocaleString()}>{when(e.at)}</time>
                </div>
              {/if}
            {:else}
              <p class="dim small">{loading ? "Loading…" : "No activity yet."}</p>
            {/each}
          </div>
          <div class="compose">
            <textarea
              bind:value={commentDraft}
              placeholder="Leave a comment. Markdown and @mentions work."
              aria-label="Comment"
              onkeydown={(e) => (e.metaKey || e.ctrlKey) && e.key === "Enter" && postComment()}
            ></textarea>
            <div class="cf">
              <span>Ctrl ↵ to send</span>
              <button class="btn" onclick={postComment} disabled={!commentDraft.trim() || posting}>Comment</button>
            </div>
          </div>
        </section>
      </div>

      {#if confirmingDelete}
        <div class="confirm" transition:fly={{ y: 10, duration: 140 }}>
          <span>Delete <b>{issue.key}</b> permanently?</span>
          <button class="btn ghost" onclick={() => (confirmingDelete = false)}>Cancel</button>
          <button class="btn danger confirm" onclick={() => { confirmingDelete = false; void store.remove(issue.key); }}>Delete</button>
        </div>
      {/if}
    {/if}
  </aside>
{/if}

<style>
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(560px, 100vw);
    z-index: 20;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border-left: 1px solid var(--line-2);
    box-shadow: -30px 0 60px rgba(0, 0, 0, 0.45);
  }
  header {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 12px 14px 12px 18px;
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }
  .key {
    font: 500 12px/1 var(--font-mono);
    color: var(--ink-dim);
    white-space: nowrap;
  }
  .sync {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-dim);
    padding: 5px 8px;
    border-radius: 6px;
    background: var(--bg);
    border: 1px solid var(--line);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sync svg {
    width: 12px;
    height: 12px;
    flex: none;
    color: var(--ok);
  }
  .grow {
    flex: 1;
  }
  header :global(.ib),
  .ib {
    width: 28px;
    height: 28px;
    border: 0;
    border-radius: 7px;
    background: none;
    display: grid;
    place-items: center;
    color: var(--ink-3);
  }
  header :global(.ib:hover) {
    background: var(--raise);
    color: var(--ink);
  }
  header :global(.ib svg) {
    width: 15px;
    height: 15px;
  }
  .body {
    flex: 1;
    overflow-y: auto;
    padding: 18px 22px 28px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }
  .title {
    width: 100%;
    min-height: 38px;
    margin: 0 0 -6px -8px;
    padding: 4px 8px;
    resize: none;
    overflow: hidden;
    border: 1px solid transparent;
    border-radius: 8px;
    background: none;
    font: 700 23px/1.2 var(--font-display);
    letter-spacing: -0.02em;
    outline: none;
  }
  .title:hover {
    border-color: var(--line);
  }
  .title:focus {
    border-color: var(--line-2);
    background: var(--bg);
  }
  .props {
    display: grid;
    grid-template-columns: 76px minmax(0, 1fr) 64px minmax(0, 1fr);
    gap: 2px 10px;
    align-items: center;
  }
  .pl {
    font-size: 12px;
    color: var(--ink-3);
  }
  .span3 {
    grid-column: 2 / -1;
    min-width: 0;
  }
  .props :global(.pv) {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    min-height: 30px;
    margin-left: -8px;
    padding: 0 8px;
    border: 0;
    border-radius: 7px;
    background: none;
    font-size: 13px;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
  }
  .props :global(.pv:hover) {
    background: var(--raise);
  }
  .tx {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dim {
    color: var(--ink-3);
  }
  .small {
    font-size: 12.5px;
    margin: 0;
  }
  .pad {
    padding: 18px 22px;
  }
  .pj {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    display: grid;
    place-items: center;
    font: 700 9px/1 var(--font-display);
    color: #0f0c09;
    flex: none;
  }
  .ek {
    font: 500 11px/1 var(--font-mono);
    color: var(--accent);
  }
  .labels {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 5px;
    padding: 4px 0;
  }
  .lab button {
    border: 0;
    background: none;
    padding: 0 0 0 2px;
    color: var(--ink-3);
    font-size: 12px;
    line-height: 1;
  }
  .lab button:hover {
    color: var(--danger);
  }
  .addlab {
    flex: 1;
    min-width: 80px;
    padding: 3px 4px;
    border: 0;
    background: none;
    font-size: 12px;
    outline: none;
  }
  .addlab::placeholder {
    color: var(--ink-3);
  }
  .epic-desc {
    margin: -10px 0 0;
    font-size: 12.5px;
    color: var(--ink-dim);
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .sec {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .agent-sec {
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 10px 12px;
  }
  .agent-sec.claimed {
    border-color: color-mix(in srgb, var(--agent, var(--ok)) 45%, transparent);
  }
  .agent-sec .grow { flex: 1; }
  .agent-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font: 600 9.5px/1 var(--font-mono);
    letter-spacing: 0.03em;
    color: var(--agent, var(--ok));
    background: color-mix(in srgb, var(--agent, var(--ok)) 14%, transparent);
    padding: 3px 7px;
    border-radius: 5px;
    text-transform: none;
  }
  .agent-pill svg {
    width: 11px;
    height: 11px;
    flex: none;
  }
  .agent-pill.stale {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
  }
  .intents {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding-top: 8px;
    margin-top: 2px;
    border-top: 1px dashed var(--line);
  }
  .intents-label {
    font: 500 9px/1 var(--font-mono);
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .achip.intent {
    border-style: dashed;
    color: var(--agent, var(--ink-dim));
    background: color-mix(in srgb, var(--agent, var(--ink)) 6%, transparent);
  }
  .intent-x {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 12px;
    line-height: 1;
    padding: 0 0 0 2px;
  }
  .intent-x:hover {
    color: var(--danger);
  }
  .agent-chips {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
  }
  .achip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 7px;
    border-radius: 5px;
    font-size: 10.5px;
    color: var(--ink-3);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border: 1px solid var(--line);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .achip svg {
    width: 11px;
    height: 11px;
    flex: none;
  }
  .achip.link {
    color: var(--status-review);
    text-decoration: none;
  }
  .achip.link:hover {
    border-color: color-mix(in srgb, var(--status-review) 45%, transparent);
  }
  .agent-note {
    margin: 0;
    font-size: 12.5px;
    color: var(--ink-dim);
    border-left: 2px solid color-mix(in srgb, var(--agent, var(--ok)) 50%, transparent);
    padding-left: 8px;
  }
  .claim-form {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .mode-seg {
    display: inline-flex;
    align-self: flex-start;
    padding: 2px;
    border-radius: 7px;
    background: var(--bg);
    border: 1px solid var(--line);
    gap: 2px;
  }
  .mode-seg button {
    border: 0;
    background: none;
    padding: 4px 9px;
    border-radius: 5px;
    font-size: 11.5px;
    color: var(--ink-3);
  }
  .mode-seg button.on {
    background: var(--raise);
    color: var(--ink);
  }
  .claim-form .field {
    background: var(--bg);
    border: 1px solid var(--line-2);
    border-radius: 7px;
    padding: 7px 9px;
    font-size: 12.5px;
    color: var(--ink);
  }
  .small { font-size: 11.5px; }
  h3 {
    margin: 0;
    display: flex;
    justify-content: space-between;
    align-items: center;
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  h3 button {
    border: 0;
    background: none;
    font: inherit;
    letter-spacing: inherit;
    color: var(--ink-3);
  }
  h3 button:hover {
    color: var(--accent);
  }
  .rel {
    display: flex;
    align-items: stretch;
    border-radius: 8px;
    background: var(--bg);
    border: 1px solid var(--line);
    overflow: hidden;
  }
  .rel:hover {
    border-color: var(--line-2);
  }
  .rel-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 7px 10px;
    border: 0;
    background: none;
    font-size: 13px;
    text-align: left;
  }
  .rel-main .tx {
    white-space: nowrap;
  }
  .rk {
    width: 78px;
    flex: none;
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .k-blocks,
  .k-blocked-by {
    color: var(--danger);
  }
  .k-closed-by,
  .k-closes {
    color: var(--status-review);
  }
  .rel-x {
    border: 0;
    border-left: 1px solid var(--line);
    background: none;
    padding: 0 11px;
    color: var(--ink-3);
  }
  .rel-x:hover {
    color: var(--danger);
  }
  .link-add {
    display: flex;
    gap: 6px;
  }
  .link-add input {
    flex: 1;
    min-width: 0;
  }
  .desc {
    width: 100%;
    resize: vertical;
    font: 400 13px/1.55 var(--font-mono);
  }
  .row-end {
    display: flex;
    align-items: center;
    gap: 6px;
    justify-content: flex-end;
  }
  .row-end .small {
    margin-right: auto;
  }
  .empty-desc {
    padding: 12px;
    border: 1px dashed var(--line-2);
    border-radius: 8px;
    background: none;
    color: var(--ink-3);
    font-size: 13px;
    text-align: left;
  }
  .empty-desc:hover {
    color: var(--ink-dim);
    border-color: color-mix(in srgb, var(--ink) 26%, transparent);
  }
  .tl {
    position: relative;
    display: flex;
    flex-direction: column;
    padding-left: 22px;
  }
  .tl::before {
    content: "";
    position: absolute;
    left: 6px;
    top: 8px;
    bottom: 8px;
    width: 1px;
    background: var(--line-2);
  }
  .ev {
    position: relative;
    padding: 5px 0;
    font-size: 12.5px;
    color: var(--ink-dim);
    overflow-wrap: anywhere;
  }
  .ev::before {
    content: "";
    position: absolute;
    left: -19px;
    top: 11px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--surface);
    border: 1.5px solid var(--ink-3);
  }
  .ev b {
    color: var(--ink);
    font-weight: 500;
  }
  .ev em {
    font-style: normal;
    color: var(--ink);
  }
  time {
    margin-left: 6px;
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .cm {
    position: relative;
    margin: 6px 0;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--bg);
    border: 1px solid var(--line);
  }
  .cm::before {
    content: "";
    position: absolute;
    left: -22px;
    top: 10px;
    width: 13px;
    height: 13px;
    border-radius: 50%;
    background: linear-gradient(135deg, #f0b46a, #c96f2a);
    box-shadow: 0 0 0 3px var(--surface);
  }
  .cm-h {
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 12.5px;
    margin-bottom: 4px;
  }
  .via {
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ok);
  }
  .cm .md {
    font-size: 13px;
  }
  .compose {
    margin-top: 6px;
    border-radius: 10px;
    border: 1px solid var(--line-2);
    background: var(--bg);
    overflow: hidden;
  }
  .compose:focus-within {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
  }
  .compose textarea {
    display: block;
    width: 100%;
    min-height: 66px;
    padding: 10px 12px;
    border: 0;
    background: none;
    resize: vertical;
    font-size: 13px;
    outline: none;
  }
  .compose textarea::placeholder {
    color: var(--ink-3);
  }
  .cf {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px 6px 12px;
    border-top: 1px solid var(--line);
    font: 400 11px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .cf .btn {
    margin-left: auto;
    height: 28px;
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 18px;
    border-top: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
    background: color-mix(in srgb, var(--danger) 8%, var(--surface));
    font-size: 13px;
  }
  .confirm span {
    margin-right: auto;
  }

  @media (max-width: 860px) {
    .props {
      grid-template-columns: 76px minmax(0, 1fr);
    }
    .span3 {
      grid-column: 2;
    }
    .body {
      padding: 16px 16px 28px;
    }
  }
</style>
