<script lang="ts">
  import { onMount } from "svelte";
  import { api, store } from "./store.svelte";
  import { renderMarkdown } from "./markdown";
  import Mermaid from "./Mermaid.svelte";
  import ApprovalWindowEditor from "./ApprovalWindowEditor.svelte";
  import MaintenanceCalendar from "./MaintenanceCalendar.svelte";
  import type {
    ApprovalAsk,
    ApprovalCounts,
    ApprovalDecision,
    ApprovalKind,
    ApprovalStatus,
    ApprovalUrgency,
  } from "@breadboard/api-client";

  type Filter = "needs" | "all" | "approval" | "maintenance" | "info" | "changes" | "answered";

  const FILTERS: { id: Filter; label: string }[] = [
    { id: "needs", label: "Needs you" },
    { id: "all", label: "All" },
    { id: "approval", label: "Approvals" },
    { id: "maintenance", label: "Maintenance" },
    { id: "info", label: "Info" },
    { id: "changes", label: "Changes requested" },
    { id: "answered", label: "Answered" },
  ];

  const DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

  let view = $state<"inbox" | "calendar">("inbox");
  let filter = $state<Filter>("needs");
  let asks = $state<ApprovalAsk[]>([]);
  let counts = $state<ApprovalCounts | null>(null);
  let selected = $state<ApprovalAsk | null>(null);
  let selectedId = $state<number | null>(null);
  let loading = $state(true);
  let detailLoading = $state(false);
  let error = $state("");
  let deciding = $state<ApprovalDecision | null>(null);
  let decidingError = $state("");
  let note = $state("");
  let search = $state("");
  let project = $state("");
  let agent = $state("");
  let urgency = $state<"" | ApprovalUrgency>("");
  let age = $state<"any" | "1h" | "24h" | "7d">("any");
  let showDetail = $state(false);
  let lightbox = $state<{ src: string; caption: string } | null>(null);
  let tick = $state(0);
  let searchTimer: ReturnType<typeof setTimeout> | null = null;

  let editStart = $state<Date | null>(null);
  let editEnd = $state<Date | null>(null);
  let syncedWindow = $state("");

  const projectList = $derived(store.projects.filter((p) => !p.archived));
  const agentOptions = $derived.by(() => {
    const names = new Set<string>();
    for (const claim of store.agents) names.add(claim.agent_name);
    for (const ask of asks) names.add(ask.agent_name);
    return [...names].sort((a, b) => a.localeCompare(b));
  });

  function countFor(id: Filter): number {
    if (!counts) return 0;
    return counts[id];
  }

  function errMsg(e: unknown) {
    return e instanceof Error ? e.message : String(e);
  }

  function ageCutoff(): number | null {
    if (age === "any") return null;
    const hours = age === "1h" ? 1 : age === "24h" ? 24 : 24 * 7;
    return Date.now() - hours * 3_600_000;
  }

  const visibleAsks = $derived.by(() => {
    const cutoff = ageCutoff();
    if (cutoff === null) return asks;
    return asks.filter((a) => new Date(a.created_at).getTime() >= cutoff);
  });

  async function load() {
    loading = true;
    error = "";
    try {
      const [rows, freshCounts] = await Promise.all([
        api.listApprovals({
          filter,
          project: project || undefined,
          agent: agent || undefined,
          urgency: urgency || undefined,
          q: search.trim() || undefined,
        }),
        api.approvalCounts(),
      ]);
      asks = rows;
      counts = freshCounts;
      store.approvalUnread = freshCounts.needs;
    } catch (e) {
      error = errMsg(e);
    } finally {
      loading = false;
    }
  }

  async function open(id: number) {
    selectedId = id;
    detailLoading = true;
    decidingError = "";
    showDetail = true;
    try {
      selected = await api.getApproval(id);
      note = "";
      if (selected.window) {
        editStart = new Date(selected.window.starts_at);
        editEnd = new Date(selected.window.ends_at);
      } else {
        editStart = null;
        editEnd = null;
      }
      // Reading it advances the operator's cursor; refresh the badge quietly.
      void load();
    } catch (e) {
      decidingError = errMsg(e);
    } finally {
      detailLoading = false;
    }
  }

  function pickFilter(next: Filter) {
    filter = next;
    void load();
  }

  function onSearch(value: string) {
    search = value;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void load(), 250);
  }

  function windowPatch() {
    if (!selected?.window || !editStart || !editEnd) return undefined;
    return { starts_at: editStart.toISOString(), ends_at: editEnd.toISOString() };
  }

  async function decide(decision: ApprovalDecision) {
    if (!selected || deciding) return;
    deciding = decision;
    decidingError = "";
    try {
      const updated = await api.decideApproval(selected.id, {
        decision,
        note_md: note.trim() || undefined,
        window: decision === "approve" || decision === "changes" ? windowPatch() : undefined,
      });
      selected = updated;
      selectedId = updated.id;
      note = "";
      store.flash(
        decision === "approve"
          ? "Approved. The agent has your decision."
          : decision === "decline"
            ? "Declined. The agent will stop and record the reason."
            : decision === "changes"
              ? "Sent back. It stays out of Needs you until the agent resubmits."
              : "Instructions sent. The ask is closed.",
        "info",
      );
      await load();
    } catch (e) {
      decidingError = errMsg(e);
    } finally {
      deciding = null;
    }
  }

  function statusTag(status: ApprovalStatus) {
    switch (status) {
      case "pending":
        return "needs you";
      case "changes":
        return "changes requested";
      case "approved":
        return status;
      case "answered":
        return "answered";
      case "declined":
        return "declined";
      case "withdrawn":
        return "withdrawn";
      case "expired":
        return "expired";
    }
  }

  function relAge(iso: string) {
    const minutes = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 60_000));
    if (minutes < 1) return "just now";
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.round(minutes / 60);
    if (hours < 48) return `${hours}h ago`;
    return `${Math.round(hours / 24)}d ago`;
  }

  function hm(d: Date) {
    return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  }

  function windowLabel(startIso: string, endIso: string) {
    const start = new Date(startIso);
    const end = new Date(endIso);
    return `${DAY_NAMES[(start.getDay() + 6) % 7]} ${start.getDate()} ${start.toLocaleDateString("en-AU", { month: "short" })}, ${hm(start)}-${hm(end)}`;
  }

  type Segment = { kind: "md" | "mermaid"; text: string };
  function segments(md: string): Segment[] {
    const out: Segment[] = [];
    const re = /```mermaid[^\n]*\n([\s\S]*?)```/g;
    let last = 0;
    let match: RegExpExecArray | null;
    while ((match = re.exec(md))) {
      if (match.index > last) out.push({ kind: "md", text: md.slice(last, match.index) });
      out.push({ kind: "mermaid", text: match[1].trim() });
      last = match.index + match[0].length;
    }
    if (last < md.length) out.push({ kind: "md", text: md.slice(last) });
    return out.filter((s) => s.text.trim().length > 0);
  }

  function eventBody(ask: ApprovalAsk) {
    return ask.body_md.trim();
  }

  function summaryLine(ask: ApprovalAsk) {
    return ask.summary || ask.body_md.replace(/[#*`>]/g, "").split("\n").find((l) => l.trim()) || "";
  }

  function isOpen(ask: ApprovalAsk) {
    return ask.status === "pending";
  }

  function cycleView(next: "inbox" | "calendar") {
    view = next;
    if (next === "inbox") void load();
  }

  onMount(() => {
    void load();
  });

  $effect(() => {
    // A live approval event refetches the list and open detail without losing
    // the operator's place.
    void store.approvalTick;
    void tick;
    void load();
    if (selectedId) {
      void api
        .getApproval(selectedId)
        .then((fresh) => {
          if (selectedId === fresh.id) selected = fresh;
        })
        .catch(() => {});
    }
  });

  $effect(() => {
    // A decision or a fresh revision replaces the stored window; resync the
    // draft only when that stored window actually changed, never while the
    // operator is dragging.
    const w = selected?.window;
    if (!w) return;
    const key = `${w.id}:${w.updated_at}:${w.starts_at}:${w.ends_at}`;
    if (key === syncedWindow) return;
    syncedWindow = key;
    editStart = new Date(w.starts_at);
    editEnd = new Date(w.ends_at);
  });

  function runbookSteps(md: string) {
    return md
      .split(/\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => line.replace(/^\s*(?:\d+[.)]|[-*])\s*/, ""))
      .filter(Boolean);
  }

  function kindLabel(kind: ApprovalKind) {
    return kind === "maintenance" ? "maintenance" : kind === "info" ? "info" : "approval";
  }

  function eventLabel(kind: string) {
    return kind === "created"
      ? "opened"
      : kind === "revised"
        ? "resubmitted"
        : kind === "comment"
          ? "commented"
          : kind === "approved"
            ? "approved"
            : kind === "declined"
              ? "declined"
              : kind === "changes"
                ? "requested changes"
                : kind === "info"
                  ? "sent instructions"
                  : kind === "withdrawn"
                    ? "withdrawn"
                    : kind === "expired"
                      ? "expired"
                      : kind;
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && lightbox) lightbox = null;
  }}
/>

<div class="page" data-approval-page>
  <header class="page-head">
    <div>
      <h1>Approvals</h1>
      <p>
        Decisions, context and maintenance windows from your agents. Approve, decline, send work
        back, or answer with instructions. Requests gone back for changes stay out of your inbox
        until the agent resubmits.
      </p>
    </div>
    <div class="seg" role="tablist" aria-label="Approvals views">
      <button
        class:on={view === "inbox"}
        role="tab"
        aria-selected={view === "inbox"}
        data-view="inbox"
        onclick={() => cycleView("inbox")}
      >
        Inbox <span class="count">{counts?.needs ?? 0}</span>
      </button>
      <button
        class:on={view === "calendar"}
        role="tab"
        aria-selected={view === "calendar"}
        data-view="calendar"
        onclick={() => cycleView("calendar")}
      >
        Maintenance
      </button>
    </div>
  </header>

  {#if view === "calendar"}
    <MaintenanceCalendar
      refreshKey={store.approvalTick}
      onOpenAsk={(id) => {
        view = "inbox";
        void open(id);
      }}
    />
  {:else}
    <section class="inbox" class:show-detail={showDetail}>
      <div class="list-pane" data-approval-list>
        <div class="list-controls">
          <input
            class="search"
            type="search"
            placeholder="Search asks, agents, services"
            aria-label="Search approvals"
            value={search}
            oninput={(e) => onSearch(e.currentTarget.value)}
          />
          <div class="selects">
            <select aria-label="Project filter" bind:value={project} onchange={() => load()}>
              <option value="">All projects</option>
              {#each projectList as p (p.id)}
                <option value={p.key}>{p.key}</option>
              {/each}
            </select>
            <select aria-label="Agent filter" bind:value={agent} onchange={() => load()}>
              <option value="">All agents</option>
              {#each agentOptions as name (name)}
                <option value={name}>{name}</option>
              {/each}
            </select>
            <select aria-label="Urgency filter" bind:value={urgency} onchange={() => load()}>
              <option value="">Any urgency</option>
              <option value="high">High</option>
              <option value="normal">Normal</option>
              <option value="low">Low</option>
            </select>
            <select aria-label="Age filter" bind:value={age} onchange={() => (tick = tick + 1)}>
              <option value="any">Any age</option>
              <option value="1h">Last hour</option>
              <option value="24h">Last 24h</option>
              <option value="7d">Last 7 days</option>
            </select>
          </div>
          <div class="filters" role="group" aria-label="Approval filters">
            {#each FILTERS as f (f.id)}
              <button
                class="chip"
                class:on={filter === f.id}
                data-filter={f.id}
                aria-pressed={filter === f.id}
                onclick={() => pickFilter(f.id)}
              >
                {f.label}<span class="n">{countFor(f.id)}</span>
              </button>
            {/each}
          </div>
        </div>

        <div class="ask-list">
          {#if loading && !asks.length}
            <div class="empty-list">Loading asks…</div>
          {:else if error}
            <div class="empty-list err" role="alert">{error} <button class="btn" onclick={() => load()}>Retry</button></div>
          {:else if !visibleAsks.length}
            <div class="empty-list">
              Nothing here.<br /><span>Requests sent back for changes stay hidden until the agent resubmits.</span>
            </div>
          {:else}
            {#each visibleAsks as ask (ask.id)}
              <button
                class="ask-card"
                class:on={selectedId === ask.id}
                data-approval-card={ask.id}
                onclick={() => open(ask.id)}
              >
                <div class="ask-top">
                  <span class="avatar" data-agent={ask.agent_name}>{ask.agent_name.slice(0, 1).toUpperCase()}</span>
                  <span class="ask-agent"><b>{ask.agent_name}</b></span>
                  <span class="ask-age">{relAge(ask.created_at)}</span>
                </div>
                <div class="ask-title">{ask.title}</div>
                <div class="ask-sub">{summaryLine(ask)}</div>
                <div class="ask-meta">
                  <span class="tag {kindLabel(ask.kind)}">{kindLabel(ask.kind)}</span>
                  {#if ask.urgency === "high"}<span class="tag high">high</span>{/if}
                  <span class="tag {ask.status}">{statusTag(ask.status)}</span>
                  {#if ask.revision > 1}<span class="tag rev">rev {ask.revision}</span>{/if}
                  {#if ask.project_key}<span class="tag project">{ask.project_key}</span>{/if}
                  {#if ask.unread}<span class="unread-dot" aria-label="Unread"></span>{/if}
                </div>
              </button>
            {/each}
          {/if}
        </div>
      </div>

      <div class="detail-pane" data-approval-detail>
        {#if detailLoading && !selected}
          <div class="detail"><div class="empty-list">Loading ask…</div></div>
        {:else if !selected}
          <div class="detail"><div class="empty-list">Select an ask to see its context and decide.</div></div>
        {:else}
          {@const ask = selected}
          <article class="detail">
            <button class="back" onclick={() => (showDetail = false)}>‹ Inbox</button>
            <div class="detail-head">
              <span class="avatar lg">{ask.agent_name.slice(0, 1).toUpperCase()}</span>
              <div>
                <div class="ask-agent">
                  <b>{ask.agent_name}</b>
                  <span>asked {relAge(ask.created_at)}</span>
                </div>
                <h2 class="detail-title">{ask.title}</h2>
                <div class="ask-meta">
                  <span class="tag {kindLabel(ask.kind)}">{kindLabel(ask.kind)}</span>
                  {#if ask.urgency === "high"}<span class="tag high">high</span>{/if}
                  <span class="tag {ask.status}" data-approval-status={ask.status}>{statusTag(ask.status)}</span>
                  {#if ask.revision > 1}<span class="tag rev">revision {ask.revision}</span>{/if}
                  {#if ask.project_key}<span class="tag project">{ask.project_key}</span>{/if}
                </div>
              </div>
            </div>
            <p class="detail-sub">{ask.summary}</p>

            <div class="status-line">
              <i class="pulse" class:quiet={!isOpen(ask)}></i>
              {#if ask.status === "pending"}
                {ask.kind === "maintenance" ? "Waiting on your decision and window" : "Waiting on your decision"}
              {:else if ask.status === "approved"}
                Approved by {ask.decided_by_login ?? "the operator"}
              {:else if ask.status === "declined"}
                Declined by {ask.decided_by_login ?? "the operator"}
              {:else if ask.status === "answered"}
                Answered with instructions
              {:else if ask.status === "changes"}
                Sent back for changes
              {:else if ask.status === "withdrawn"}
                Withdrawn by the agent
              {:else}
                Expired before a decision
              {/if}
            </div>

            {#if isOpen(ask)}
              <div class="quick-actions">
                <button class="btn ok" data-decision="approve" disabled={deciding !== null} onclick={() => decide("approve")}>
                  {ask.kind === "maintenance" ? "Approve window" : "Approve"}
                </button>
                <button class="btn danger" data-decision="decline" disabled={deciding !== null} onclick={() => decide("decline")}>Decline</button>
                <button class="btn" data-decision="changes" disabled={deciding !== null} onclick={() => decide("changes")}>Request changes</button>
                <button class="btn primary" data-decision="info" disabled={deciding !== null} onclick={() => decide("info")}>
                  Send info / instructions
                </button>
              </div>
            {/if}

            {#if decidingError}
              <div class="resolution changes" role="alert">{decidingError}</div>
            {/if}

            {#if ask.status === "changes"}
              <div class="resolution changes" data-resolution="changes">
                <div>
                  <b>Hidden from your inbox until the agent reworks it.</b><br />
                  The agent will resubmit with new evidence, and this same ask will reappear as
                  revision {ask.revision + 1} with the full thread.
                </div>
              </div>
            {:else if ask.status === "approved"}
              <div class="resolution" data-resolution="approved">
                <div>
                  <b>Approved.</b>
                  The agent has your decision{ask.issue_key ? ` and will report the outcome on ${ask.issue_key}` : ""}.
                  {ask.kind === "maintenance" ? "The window is approved in the calendar." : ""}
                </div>
              </div>
            {:else if ask.status === "declined"}
              <div class="resolution changes" data-resolution="declined">
                <div><b>Declined.</b> The agent will stop and record the reason{ask.issue_key ? ` on ${ask.issue_key}` : ""}.</div>
              </div>
            {:else if ask.status === "answered"}
              <div class="resolution info" data-resolution="answered">
                <div><b>Answered.</b> Your instructions were delivered and the ask is closed.</div>
              </div>
            {:else if ask.status === "expired"}
              <div class="resolution changes" data-resolution="expired">
                <div><b>Expired.</b> The one-shot approval lapsed before a decision; the agent must open a new ask.</div>
              </div>
            {:else if ask.status === "withdrawn"}
              <div class="resolution changes" data-resolution="withdrawn">
                <div><b>Withdrawn.</b> The agent no longer needs this decision.</div>
              </div>
            {/if}

            <div class="meta-strip">
              <div class="meta-cell"><span>Issue</span><b>{ask.issue_key ?? "none"}</b></div>
              <div class="meta-cell"><span>Environment</span><b>{ask.environment ?? "unstated"}</b></div>
              <div class="meta-cell"><span>Deadline</span><b>{ask.deadline ?? "none"}</b></div>
              <div class="meta-cell">
                <span>{ask.expires_at ? "Expires" : "Category"}</span>
                <b>{ask.expires_at ? new Date(ask.expires_at).toLocaleString("en-AU") : kindLabel(ask.kind)}</b>
              </div>
            </div>

            {#if eventBody(ask)}
              <div class="md" data-approval-body>
                {#each segments(eventBody(ask)) as seg, i (i)}
                  {#if seg.kind === "mermaid"}
                    <Mermaid source={seg.text} caption="Diagram" />
                  {:else}
                    {@html renderMarkdown(seg.text)}
                  {/if}
                {/each}
              </div>
            {/if}

            {#if ask.attachments.length}
              <div class="gallery">
                {#each ask.attachments as shot (shot.id)}
                  <button
                    type="button"
                    class="shot"
                    data-screenshot={shot.artifact_id}
                    aria-label="Enlarge {shot.caption || shot.filename}"
                    onclick={() => (lightbox = { src: api.artifactUrl(shot.artifact_id), caption: shot.caption || shot.filename })}
                    onkeydown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        lightbox = { src: api.artifactUrl(shot.artifact_id), caption: shot.caption || shot.filename };
                      }
                    }}
                  >
                    <img src={api.artifactUrl(shot.artifact_id)} alt={shot.caption || shot.filename} loading="lazy" />
                    <span class="shot-cap"><b>{shot.caption || shot.filename}</b><span>click to enlarge</span></span>
                  </button>
                {/each}
              </div>
            {/if}

            {#if ask.window && isOpen(ask) && editStart && editEnd}
              <ApprovalWindowEditor
                start={editStart}
                end={editEnd}
                timezone={ask.window.timezone}
                onchange={(start, end) => {
                  editStart = start;
                  editEnd = end;
                }}
              />
              <div class="window-actions">
                <button class="btn ok" data-window-action="approve" disabled={deciding !== null} onclick={() => decide("approve")}>
                  Approve as proposed
                </button>
                <button class="btn primary" data-window-action="revise" disabled={deciding !== null} onclick={() => decide("changes")}>
                  Send revised window
                </button>
                <span class="hint">Drag the block or its edges. Snap 15 minutes.</span>
              </div>
            {:else if ask.window}
              <div class="window-summary" data-window-summary>
                <h3>Approved maintenance window</h3>
                <p>
                  {windowLabel(ask.window.starts_at, ask.window.ends_at)} {ask.window.timezone}
                  · <span class="status-badge {ask.window.status}">{ask.window.status}</span>
                </p>
                <div class="services">
                  {#each ask.window.services as svc (svc)}<span class="svc">{svc}</span>{/each}
                </div>
                <div class="two-col">
                  <div class="panel"><h5>Why</h5><p>{ask.window.reason}</p></div>
                  <div class="panel"><h5>Risk</h5><p>{ask.window.risks}</p></div>
                </div>
                <div class="panel"><h5>Rollback</h5><p>{ask.window.rollback}</p></div>
                <h3>Runbook</h3>
                <ol class="runbook">
                  {#each runbookSteps(ask.window.runbook_md) as step (step)}
                    <li>{step}</li>
                  {/each}
                </ol>
              </div>
            {/if}

            {#if ask.checklist.length}
              <div class="ask-box" data-approval-checklist>
                <h4>What the agent needs from you</h4>
                <ul>
                  {#each ask.checklist as point (point)}<li>{point}</li>{/each}
                </ul>
              </div>
            {/if}

            {#if ask.risk_md || ask.rollback_md}
              <div class="two-col">
                {#if ask.risk_md}
                  <div class="panel" data-panel="risk">
                    <h5>Risk</h5>
                    <div class="md compact">{@html renderMarkdown(ask.risk_md)}</div>
                  </div>
                {/if}
                {#if ask.rollback_md}
                  <div class="panel" data-panel="rollback">
                    <h5>Rollback</h5>
                    <div class="md compact">{@html renderMarkdown(ask.rollback_md)}</div>
                  </div>
                {/if}
              </div>
            {/if}

            {#if ask.events.length}
              <div class="md"><h3>Conversation</h3></div>
              <div class="thread" data-approval-thread>
                {#each ask.events as event (event.id)}
                  <div class="msg" class:me={event.actor_kind === "human"}>
                    <span class="avatar sm">{event.actor_name.slice(0, 1).toUpperCase()}</span>
                    <div class="bubble">
                      <div class="who">
                        <b>{event.actor_kind === "human" ? "You" : event.actor_name}</b>
                        <span>{relAge(event.created_at)}</span>
                        {#if event.kind === "revised"}
                          <span class="tag rev">revision {event.meta.revision ?? ""}</span>
                        {:else if event.kind !== "comment" && event.kind !== "created"}
                          <span class="tag {event.kind}">{event.kind}</span>
                        {/if}
                      </div>
                      {#if event.body_md}
                        <div class="md compact">{@html renderMarkdown(event.body_md)}</div>
                      {/if}
                    </div>
                  </div>
                {/each}
              </div>
            {/if}

            {#if ask.events.length}
              <div class="md"><h3>Audit</h3></div>
              <details class="audit" data-approval-audit>
                <summary>
                  Activity feed <span>{ask.events.length} transition{ask.events.length === 1 ? "" : "s"}</span>
                </summary>
                <ol>
                  {#each ask.events as event (event.id)}
                    <li data-audit-kind={event.kind}>
                      <b>{eventLabel(event.kind)}</b>
                      <span class="who">
                        {event.actor_kind === "human" ? "Operator" : event.actor_kind === "system" ? "System" : "Agent"}
                        · {event.actor_kind === "human" ? event.actor_name : event.actor_name}
                      </span>
                      <time datetime={event.created_at}>{new Date(event.created_at).toLocaleString("en-AU")}</time>
                      {#if event.meta.revision}
                        <span class="rev">rev {event.meta.revision}</span>
                      {/if}
                    </li>
                  {/each}
                </ol>
              </details>
            {/if}

            {#if isOpen(ask)}
              <div class="composer">
                <textarea
                  bind:value={note}
                  placeholder="Add context for the agent. Markdown supported, including code and links."
                  data-decision-note
                  onkeydown={(e) => {
                    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) decide("approve");
                  }}
                ></textarea>
                <div class="composer-row">
                  <button class="btn ok" data-decision="approve" disabled={deciding !== null} onclick={() => decide("approve")}>
                    {ask.kind === "maintenance" ? "Approve window" : "Approve"}
                  </button>
                  <button class="btn danger" data-decision="decline" disabled={deciding !== null} onclick={() => decide("decline")}>Decline</button>
                  <button class="btn" data-decision="changes" disabled={deciding !== null} onclick={() => decide("changes")}>Request changes</button>
                  <button class="btn primary" data-decision="info" disabled={deciding !== null} onclick={() => decide("info")}>
                    Send info / instructions
                  </button>
                  <span class="hint">Cmd+Enter sends the primary action</span>
                </div>
                <p class="note">
                  Request changes hides this ask from your inbox until the agent resubmits with new
                  evidence. Decline records a reason and stops the work. Send info answers a question
                  and closes the ask.
                </p>
              </div>
            {/if}
          </article>
        {/if}
      </div>
    </section>
  {/if}
</div>

{#if lightbox}
  <div
    class="lightbox"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    aria-label={lightbox.caption}
    data-lightbox="screenshot"
    onclick={() => (lightbox = null)}
    onkeydown={(e) => {
      if (e.key === "Escape") lightbox = null;
    }}
  >
    <div class="inner">
      <img src={lightbox.src} alt={lightbox.caption} />
      <p>{lightbox.caption}</p>
    </div>
  </div>
{/if}

<style>
  .page {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .page-head {
    padding: 22px 24px 14px;
    display: flex;
    align-items: flex-end;
    gap: 20px;
    flex: none;
    flex-wrap: wrap;
  }
  .page-head h1 {
    margin: 0;
    font: 600 26px/1.05 var(--font-display);
    letter-spacing: -0.03em;
  }
  .page-head p {
    margin: 5px 0 0;
    color: var(--ink-dim);
    font-size: 12.5px;
    max-width: 640px;
  }
  .seg {
    margin-left: auto;
    display: inline-flex;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 3px;
    gap: 2px;
  }
  .seg button {
    border: 0;
    background: none;
    color: var(--ink-dim);
    padding: 6px 12px;
    border-radius: 7px;
    font-size: 12.5px;
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .seg button.on {
    background: var(--accent-soft);
    color: var(--ink);
  }
  .seg .count {
    font-size: 10px;
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    border-radius: 999px;
    padding: 0 6px;
  }
  .inbox {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 372px minmax(0, 1fr);
    border-top: 1px solid var(--line);
  }
  .list-pane {
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--line);
    background: var(--sunk);
  }
  .list-controls {
    padding: 12px 12px 8px;
    display: grid;
    gap: 8px;
    border-bottom: 1px solid var(--line);
  }
  .search {
    width: 100%;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 7px 10px;
    font-size: 12.5px;
  }
  .search:focus {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .selects {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .selects select {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 7px;
    padding: 5px 6px;
    font-size: 11px;
    color: var(--ink-dim);
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink-dim);
    padding: 4px 9px;
    border-radius: 999px;
    font-size: 11.5px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .chip:hover {
    color: var(--ink);
    border-color: var(--line-2);
  }
  .chip.on {
    background: var(--accent-soft);
    color: var(--ink);
    border-color: color-mix(in srgb, var(--accent) 40%, transparent);
  }
  .chip .n {
    font: 10px var(--font-mono);
    color: var(--ink-3);
  }
  .chip.on .n {
    color: var(--accent);
  }
  .ask-list {
    flex: 1;
    overflow-y: auto;
    padding: 6px 10px 20px;
    display: grid;
    gap: 6px;
    align-content: start;
  }
  .ask-card {
    text-align: left;
    width: 100%;
    border: 1px solid transparent;
    background: transparent;
    border-radius: 10px;
    padding: 12px;
    display: grid;
    gap: 7px;
    transition: 120ms var(--ease);
    position: relative;
  }
  .ask-card:hover {
    background: color-mix(in srgb, var(--ink) 3%, transparent);
  }
  .ask-card.on {
    background: var(--surface);
    border-color: var(--line);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.25);
  }
  .ask-card.on::before {
    content: "";
    position: absolute;
    left: -1px;
    top: 12px;
    bottom: 12px;
    width: 2px;
    background: var(--accent);
    border-radius: 2px;
  }
  .ask-top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .avatar {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 10px;
    font-weight: 700;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    flex: none;
  }
  .avatar.lg {
    width: 28px;
    height: 28px;
    font-size: 11px;
  }
  .avatar.sm {
    width: 22px;
    height: 22px;
    font-size: 10px;
  }
  .ask-agent {
    font-size: 11.5px;
    color: var(--ink-dim);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .ask-agent b {
    color: var(--ink);
  }
  .ask-age {
    margin-left: auto;
    font-size: 10.5px;
    color: var(--ink-3);
  }
  .ask-title {
    font-size: 13.5px;
    font-weight: 600;
    line-height: 1.3;
    letter-spacing: -0.01em;
  }
  .ask-sub {
    font-size: 11.5px;
    color: var(--ink-3);
    line-height: 1.45;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .ask-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .tag {
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-weight: 700;
    padding: 2px 6px;
    border-radius: 4px;
    border: 1px solid transparent;
    white-space: nowrap;
  }
  .tag.approval {
    color: var(--accent);
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 28%, transparent);
  }
  .tag.maintenance {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 10%, transparent);
    border-color: color-mix(in srgb, var(--status-review) 30%, transparent);
  }
  .tag.info {
    color: var(--status-todo);
    background: color-mix(in srgb, var(--status-todo) 10%, transparent);
    border-color: color-mix(in srgb, var(--status-todo) 30%, transparent);
  }
  .tag.high {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    border-color: color-mix(in srgb, var(--danger) 30%, transparent);
  }
  .tag.pending {
    color: var(--status-doing);
    background: color-mix(in srgb, var(--status-doing) 10%, transparent);
    border-color: color-mix(in srgb, var(--status-doing) 28%, transparent);
  }
  .tag.changes,
  .tag.declined,
  .tag.expired {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    border-color: color-mix(in srgb, var(--danger) 28%, transparent);
  }
  .tag.approved,
  .tag.answered {
    color: var(--ok);
    background: color-mix(in srgb, var(--ok) 12%, transparent);
    border-color: color-mix(in srgb, var(--ok) 28%, transparent);
  }
  .tag.withdrawn {
    color: var(--ink-dim);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    border-color: var(--line);
  }
  .tag.rev {
    color: var(--status-review);
    background: color-mix(in srgb, var(--status-review) 10%, transparent);
    border-color: color-mix(in srgb, var(--status-review) 28%, transparent);
  }
  .tag.project {
    color: var(--ink-3);
    border-color: var(--line);
  }
  .unread-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    margin-left: auto;
  }
  .empty-list {
    padding: 28px 16px;
    color: var(--ink-3);
    font-size: 12.5px;
    text-align: center;
    line-height: 1.6;
  }
  .empty-list span {
    color: var(--ink-3);
  }
  .empty-list.err {
    color: var(--danger);
  }
  .detail-pane {
    min-height: 0;
    overflow-y: auto;
    background: var(--bg);
  }
  .detail {
    max-width: 880px;
    margin: 0 auto;
    padding: 26px 30px 60px;
  }
  .back {
    display: none;
    border: 0;
    background: none;
    color: var(--accent);
    padding: 0 0 10px;
    font-size: 12.5px;
  }
  .detail-head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }
  .detail-title {
    margin: 6px 0 8px;
    font: 600 24px/1.18 var(--font-display);
    letter-spacing: -0.03em;
  }
  .detail-sub {
    color: var(--ink-dim);
    font-size: 13px;
    margin: 12px 0 10px;
  }
  .status-line {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--status-doing);
    font-size: 12px;
    font-weight: 600;
  }
  .pulse {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--status-doing);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--status-doing) 14%, transparent);
  }
  .pulse.quiet {
    background: var(--ink-3);
    box-shadow: none;
  }
  .quick-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    margin: 14px 0 2px;
  }
  .meta-strip {
    margin: 16px 0 22px;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 1px;
    background: var(--line);
    border: 1px solid var(--line);
    border-radius: 10px;
    overflow: hidden;
  }
  .meta-cell {
    background: var(--surface);
    padding: 10px 12px;
  }
  .meta-cell span {
    display: block;
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-3);
    margin-bottom: 3px;
  }
  .meta-cell b {
    font-size: 12px;
    font-weight: 500;
  }
  .md :global(h3) {
    font: 600 13px/1.3 var(--font-display);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-dim);
    margin: 24px 0 9px;
  }
  .md :global(p) {
    margin: 0 0 10px;
    color: #d6cec4;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0 0 12px;
    padding-left: 20px;
    color: #d6cec4;
  }
  .md :global(li) {
    margin: 3px 0;
  }
  .md :global(strong) {
    color: var(--ink);
  }
  .md :global(code) {
    background: color-mix(in srgb, var(--accent) 8%, var(--surface));
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0 4px;
    font-size: 12px;
    color: #e9c08a;
  }
  .md :global(pre) {
    background: var(--sunk);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 12px 14px;
    overflow-x: auto;
    font-size: 12px;
    line-height: 1.6;
    color: #cbbfae;
  }
  .md :global(blockquote) {
    margin: 12px 0;
    border-left: 2px solid var(--accent);
    padding: 2px 0 2px 14px;
    color: var(--ink-dim);
  }
  .md :global(a) {
    color: var(--accent);
    text-decoration: none;
    border-bottom: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
  }
  .md.compact :global(h3) {
    display: none;
  }
  .md.compact :global(p) {
    margin: 0 0 6px;
    font-size: 12.5px;
  }
  .gallery {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin: 12px 0 18px;
  }
  .shot {
    margin: 0;
    padding: 0;
    text-align: left;
    border: 1px solid var(--line);
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface);
    cursor: zoom-in;
    transition: 140ms var(--ease);
  }
  .shot:hover {
    border-color: var(--line-2);
    transform: translateY(-1px);
  }
  .shot img {
    display: block;
    width: 100%;
    height: auto;
  }
  .shot-cap {
    padding: 8px 10px;
    font-size: 11px;
    color: var(--ink-3);
    border-top: 1px solid var(--line);
    display: flex;
    gap: 6px;
    align-items: center;
    text-align: left;
  }
  .shot-cap b {
    color: var(--ink-dim);
    font-weight: 500;
  }
  .shot-cap span {
    margin-left: auto;
  }
  .ask-box {
    border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
    background: linear-gradient(180deg, color-mix(in srgb, var(--accent) 7%, transparent), transparent);
    border-radius: 12px;
    padding: 16px 18px;
    margin: 8px 0 20px;
  }
  .ask-box h4 {
    margin: 0 0 8px;
    font: 600 11px/1 var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--accent);
  }
  .ask-box ul {
    margin: 0;
    padding-left: 18px;
    color: #ddd3c7;
  }
  .ask-box li {
    margin: 5px 0;
  }
  .two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin: 4px 0 18px;
  }
  .panel {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    padding: 13px 15px;
  }
  .panel h5 {
    margin: 0 0 6px;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-3);
  }
  .panel p {
    margin: 0;
    font-size: 12.5px;
    color: var(--ink-dim);
  }
  .thread {
    display: grid;
    gap: 12px;
  }
  .msg {
    display: grid;
    grid-template-columns: 30px 1fr;
    gap: 10px;
  }
  .bubble {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 10px 12px;
    font-size: 12.5px;
    color: #d3c9bd;
  }
  .msg.me .bubble {
    background: color-mix(in srgb, var(--accent) 6%, transparent);
    border-color: color-mix(in srgb, var(--accent) 25%, transparent);
  }
  .who {
    display: flex;
    gap: 8px;
    align-items: baseline;
    margin-bottom: 3px;
    flex-wrap: wrap;
  }
  .who b {
    font-size: 11.5px;
    color: var(--ink);
  }
  .who span {
    font-size: 10px;
    color: var(--ink-3);
  }
  .resolution {
    border: 1px solid color-mix(in srgb, var(--ok) 35%, transparent);
    background: color-mix(in srgb, var(--ok) 12%, transparent);
    border-radius: 10px;
    padding: 13px 15px;
    font-size: 12.5px;
    color: #bfe3cc;
    margin: 8px 0 18px;
  }
  .resolution.changes {
    border-color: color-mix(in srgb, var(--danger) 35%, transparent);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    color: #f0c0bc;
  }
  .resolution.info {
    border-color: color-mix(in srgb, var(--status-todo) 35%, transparent);
    background: color-mix(in srgb, var(--status-todo) 10%, transparent);
    color: #c3d6f4;
  }
  .resolution b {
    color: var(--ink);
  }
  .composer {
    margin-top: 26px;
    background: color-mix(in srgb, var(--bg) 92%, transparent);
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: 12px;
  }
  .composer textarea {
    width: 100%;
    min-height: 72px;
    resize: vertical;
    background: var(--sunk);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 10px 12px;
    color: var(--ink);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .composer textarea:focus {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .composer textarea::placeholder {
    color: var(--ink-3);
  }
  .composer-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 9px;
    flex-wrap: wrap;
  }
  .composer-row .hint {
    margin-left: auto;
    font-size: 10.5px;
    color: var(--ink-3);
  }
  .composer .note {
    font-size: 10.5px;
    color: var(--ink-3);
    margin: 8px 2px 0;
    line-height: 1.45;
  }
  .window-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    margin: -6px 0 16px;
  }
  .window-actions .hint {
    font-size: 10px;
    color: var(--ink-3);
  }
  .window-summary {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    padding: 14px 16px;
    margin: 12px 0 18px;
  }
  .window-summary h3 {
    margin: 0 0 6px;
    font: 600 13px/1.3 var(--font-display);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-dim);
  }
  .window-summary p {
    margin: 0 0 10px;
    font-size: 12.5px;
    color: var(--ink-dim);
  }
  .status-badge {
    display: inline-flex;
    align-items: center;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 2px 7px;
    border-radius: 999px;
    border: 1px solid var(--line);
  }
  .status-badge.proposed {
    color: var(--status-review);
  }
  .status-badge.approved {
    color: var(--ok);
  }
  .services {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 6px 0 12px;
  }
  .svc {
    font: 10.5px var(--font-mono);
    border: 1px solid var(--line);
    background: var(--bg);
    border-radius: 5px;
    padding: 3px 7px;
    color: var(--ink-dim);
  }
  .runbook {
    margin: 8px 0 0;
    padding-left: 20px;
    color: #d4cabd;
    font-size: 12.5px;
    display: grid;
    gap: 6px;
  }
  .audit {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    margin: 8px 0 18px;
  }
  .audit summary {
    cursor: pointer;
    padding: 10px 13px;
    font-size: 12px;
    color: var(--ink-dim);
    display: flex;
    gap: 8px;
    align-items: center;
    list-style: none;
  }
  .audit summary::-webkit-details-marker {
    display: none;
  }
  .audit summary::before {
    content: "▸";
    color: var(--accent);
    font-size: 10px;
  }
  .audit[open] summary::before {
    content: "▾";
  }
  .audit summary span {
    color: var(--ink-3);
    font-size: 10.5px;
  }
  .audit ol {
    margin: 0;
    padding: 2px 14px 12px 13px;
    list-style: none;
    display: grid;
    gap: 7px;
  }
  .audit li {
    display: grid;
    grid-template-columns: 110px minmax(0, 1fr) auto auto;
    gap: 8px;
    align-items: baseline;
    font-size: 11.5px;
    color: var(--ink-dim);
    border-top: 1px solid var(--line);
    padding-top: 7px;
  }
  .audit li b {
    color: var(--ink);
    font-weight: 600;
  }
  .audit li .who {
    color: var(--ink-3);
    margin: 0;
    font-size: 11px;
  }
  .audit li time {
    font: 10.5px var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .audit li .rev {
    font: 10px var(--font-mono);
    color: var(--status-review);
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
  .lightbox .inner {
    max-width: 1100px;
    width: 100%;
    max-height: 90vh;
    overflow: auto;
    border: 1px solid var(--line-2);
    border-radius: 12px;
    background: var(--surface);
    padding: 12px;
  }
  .lightbox img {
    display: block;
    width: 100%;
    height: auto;
  }
  .lightbox p {
    margin: 8px 4px 2px;
    color: var(--ink-dim);
    font-size: 12px;
  }
  .btn {
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink-dim);
    border-radius: 8px;
    padding: 7px 12px;
    font-size: 12px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    transition: 120ms var(--ease);
  }
  .btn:hover {
    color: var(--ink);
    border-color: var(--line-2);
    background: var(--raise);
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
  }
  .btn.ok {
    color: var(--ok);
    border-color: color-mix(in srgb, var(--ok) 40%, transparent);
  }
  .btn.danger {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .btn.ghost {
    background: none;
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  @media (max-width: 1250px) {
    .inbox {
      grid-template-columns: 330px minmax(0, 1fr);
    }
  }
  @media (max-width: 980px) {
    .inbox {
      grid-template-columns: 1fr;
    }
    .detail-pane {
      display: none;
    }
    .inbox.show-detail .list-pane {
      display: none;
    }
    .inbox.show-detail .detail-pane {
      display: block;
    }
    .back {
      display: inline-block;
    }
    .two-col,
    .gallery,
    .meta-strip {
      grid-template-columns: 1fr;
    }
    .audit li {
      grid-template-columns: 1fr auto;
    }
    .audit li .who {
      grid-column: 1 / -1;
    }
    .detail {
      padding: 20px 18px 140px;
    }
    .page-head {
      padding: 14px 14px 10px;
    }
  }
</style>
