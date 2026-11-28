<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./store.svelte";
  import type { FreezePeriod, MaintenanceWindow } from "@breadboard/api-client";

  let {
    onOpenAsk,
    refreshKey = 0,
  }: { onOpenAsk: (id: number) => void; refreshKey?: number } = $props();

  const DAY_MS = 86_400_000;
  const HOUR_PX = 38;
  const DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

  let weekStart = $state(startOfWeek(new Date()));
  let windows = $state<MaintenanceWindow[]>([]);
  let freezes = $state<FreezePeriod[]>([]);
  let loading = $state(true);
  let error = $state("");
  let selected = $state<MaintenanceWindow | null>(null);
  let drag = $state<{
    id: number;
    mode: "move" | "top" | "bottom";
    x: number;
    y: number;
    day: number;
    start: number;
    end: number;
    moved: boolean;
    colWidth: number;
  } | null>(null);
  let deciding = $state(false);

  function startOfWeek(d: Date) {
    const out = new Date(d);
    out.setHours(0, 0, 0, 0);
    const weekday = (out.getDay() + 6) % 7;
    out.setDate(out.getDate() - weekday);
    return out;
  }

  function dayDate(i: number) {
    const d = new Date(weekStart);
    d.setDate(d.getDate() + i);
    return d;
  }

  function fmtDay(d: Date) {
    return d.toLocaleDateString("en-AU", { weekday: "short", day: "numeric", month: "short" });
  }

  function fmtTime(hours: number) {
    const h = Math.floor(hours);
    const m = Math.round((hours - h) * 60);
    return `${String(h % 24).padStart(2, "0")}:${String(m === 60 ? 0 : m).padStart(2, "0")}`;
  }

  function hoursOf(d: Date) {
    return d.getHours() + d.getMinutes() / 60;
  }

  function dayIndex(d: Date) {
    const base = new Date(weekStart).getTime();
    const day = new Date(d);
    day.setHours(0, 0, 0, 0);
    return Math.round((day.getTime() - base) / DAY_MS);
  }

  function inWeek(w: MaintenanceWindow) {
    const start = new Date(w.starts_at).getTime();
    const end = new Date(w.ends_at).getTime();
    return end > weekStart.getTime() && start < weekStart.getTime() + 7 * DAY_MS;
  }

  async function load() {
    loading = true;
    error = "";
    const from = weekStart.toISOString();
    const to = new Date(weekStart.getTime() + 7 * DAY_MS).toISOString();
    try {
      const [w, f] = await Promise.all([
        api.listMaintenanceWindows({ from, to }),
        api.listFreezes({ from, to }),
      ]);
      windows = w;
      freezes = f;
    } catch (e) {
      error = e instanceof Error ? e.message : "could not load the maintenance calendar";
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
  });

  $effect(() => {
    void refreshKey;
    void load();
  });

  function shiftWeek(days: number) {
    weekStart = new Date(weekStart.getTime() + days * DAY_MS);
    void load();
  }

  function today() {
    weekStart = startOfWeek(new Date());
    void load();
  }

  const weekWindows = $derived(windows.filter(inWeek));

  function frozenOn(date: Date) {
    const start = new Date(date);
    start.setHours(0, 0, 0, 0);
    const end = start.getTime() + DAY_MS;
    return freezes.filter((f) => {
      const fs = new Date(f.starts_at).getTime();
      const fe = new Date(f.ends_at).getTime();
      return fs < end && fe > start.getTime();
    });
  }

  function conflictFor(w: MaintenanceWindow) {
    if (w.status === "cancelled" || w.status === "completed") return false;
    const start = new Date(w.starts_at).getTime();
    const end = new Date(w.ends_at).getTime();
    if (freezes.some((f) => new Date(f.starts_at).getTime() < end && new Date(f.ends_at).getTime() > start)) {
      return true;
    }
    return weekWindows.some(
      (o) =>
        o.id !== w.id &&
        o.status !== "cancelled" &&
        new Date(o.starts_at).getTime() < end &&
        new Date(o.ends_at).getTime() > start,
    );
  }

  function blockStyle(w: MaintenanceWindow) {
    const start = hoursOf(new Date(w.starts_at));
    const end = hoursOf(new Date(w.ends_at));
    const hours = end > start ? end - start : 24 - start + end;
    return `top:${start * HOUR_PX}px;height:${Math.max(20, hours * HOUR_PX - 2)}px`;
  }

  function beginDrag(e: PointerEvent, w: MaintenanceWindow, mode: "move" | "top" | "bottom") {
    const col = (e.currentTarget as HTMLElement).closest(".day-col") as HTMLElement | null;
    drag = {
      id: w.id,
      mode,
      x: e.clientX,
      y: e.clientY,
      day: dayIndex(new Date(w.starts_at)),
      start: hoursOf(new Date(w.starts_at)),
      end: hoursOf(new Date(w.ends_at)),
      moved: false,
      colWidth: col?.getBoundingClientRect().width || 1,
    };
    try {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    } catch {
      /* capture is best-effort */
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function onDrag(e: PointerEvent) {
    if (!drag) return;
    const w = windows.find((x) => x.id === drag!.id);
    if (!w) return;
    const days = Math.round((e.clientX - drag.x) / drag.colWidth);
    const dh = Math.round(((e.clientY - drag.y) / HOUR_PX) * 4) / 4;
    if (days !== 0 || dh !== 0) drag.moved = true;
    const baseDay = dayDate(clamp(drag.day + days, 0, 6));
    const base = new Date(baseDay);
    base.setHours(0, 0, 0, 0);
    if (drag.mode === "move") {
      const duration = spanHours(drag.start, drag.end);
      const startHours = clamp(drag.start + dh, 0, 24 - Math.min(duration, 24));
      const start = new Date(base.getTime() + startHours * 3_600_000);
      const end = new Date(start.getTime() + duration * 3_600_000);
      w.starts_at = start.toISOString();
      w.ends_at = end.toISOString();
    } else if (drag.mode === "top") {
      const startHours = clamp(drag.start + dh, 0, 24);
      if (startHours >= spanHours(0, drag.end)) return;
      w.starts_at = new Date(base.getTime() + startHours * 3_600_000).toISOString();
      w.ends_at = new Date(base.getTime() + drag.end * 3_600_000).toISOString();
    } else {
      const endHours = clamp(drag.end + dh, 0, 24);
      if (endHours <= drag.start) return;
      w.starts_at = new Date(base.getTime() + drag.start * 3_600_000).toISOString();
      w.ends_at = new Date(base.getTime() + endHours * 3_600_000).toISOString();
    }
  }

  function spanHours(start: number, end: number) {
    return end > start ? end - start : 24 - start + end;
  }

  function clamp(n: number, lo: number, hi: number) {
    return Math.max(lo, Math.min(hi, n));
  }

  function finishDrag() {
    if (!drag) return;
    const w = windows.find((x) => x.id === drag!.id);
    const moved = drag.moved;
    drag = null;
    if (!moved || !w) return;
    selected = w;
  }

  async function decide(action: "approve" | "changes") {
    const w = selected;
    if (!w || !w.ask_id) return;
    deciding = true;
    try {
      await api.decideApproval(w.ask_id, {
        decision: action,
        note_md:
          action === "changes"
            ? `Revised window: ${fmtDay(new Date(w.starts_at))} ${fmtTime(hoursOf(new Date(w.starts_at)))}-${fmtTime(
                hoursOf(new Date(w.ends_at)),
              )}. Approve that slot and proceed.`
            : "Approved from the maintenance calendar.",
        window: { starts_at: w.starts_at, ends_at: w.ends_at },
      });
      selected = null;
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : "the decision was refused";
    } finally {
      deciding = false;
    }
  }

  function runbookSteps(md: string) {
    return md
      .split(/\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => line.replace(/^\s*(?:\d+[.)]|[-*])\s*/, ""))
      .filter(Boolean);
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && selected) selected = null;
  }}
/>

<section class="calendar-wrap">
  <div class="calendar-main">
    <div class="cal-bar">
      <button class="btn ghost" aria-label="Previous week" data-cal-prev onclick={() => shiftWeek(-7)}>‹</button>
      <h2 data-cal-title>{fmtDay(weekStart)} - {fmtDay(dayDate(6))}</h2>
      <button class="btn ghost" aria-label="Next week" data-cal-next onclick={() => shiftWeek(7)}>›</button>
      <button class="btn ghost" data-cal-today onclick={today}>Today</button>
      <div class="spacer"></div>
      <div class="legend">
        <span><i class="approved"></i>approved</span>
        <span><i class="proposed"></i>proposed</span>
        <span><i class="freeze"></i>freeze</span>
        <span><i class="conflict"></i>conflict</span>
      </div>
    </div>

    {#if loading}
      <div class="state">Loading maintenance windows…</div>
    {:else if error}
      <div class="state err" role="alert">{error} <button class="btn" onclick={() => load()}>Retry</button></div>
    {/if}

    <div class="cal-scroll">
      <div class="week">
        <div class="week-head">
          <div></div>
          {#each DAY_NAMES as name, i (name)}
            {@const date = dayDate(i)}
            <div class:today={date.toDateString() === new Date().toDateString()}>
              {name}<b>{date.getDate()}</b>
            </div>
          {/each}
        </div>
        <div class="week-body">
          <div class="hours">
            {#each Array.from({ length: 24 }, (_, h) => h) as h (h)}
              <span style="top:{h * HOUR_PX}px">{String(h).padStart(2, "0")}:00</span>
            {/each}
          </div>
          {#each DAY_NAMES as _name, i (i)}
            {@const date = dayDate(i)}
            <div
              class="day-col"
              class:weekend={i >= 5}
              data-cal-day={i}
            >
              {#each frozenOn(date) as freeze (freeze.id)}
                <div class="freeze" title={freeze.label} data-cal-freeze={freeze.id}>
                  <span>{freeze.label}</span>
                </div>
              {/each}
              {#each weekWindows.filter((w) => dayIndex(new Date(w.starts_at)) === i) as w (w.id)}
                <div
                  class="win {w.status}"
                  class:conflict={conflictFor(w)}
                  class:dragging={drag?.id === w.id}
                  data-cal-win={w.id}
                  style={blockStyle(w)}
                  role="button"
                  tabindex="0"
                  aria-label="{w.title}, {w.status}"
                  onpointerdown={(e) => w.status === "proposed" && beginDrag(e, w, "move")}
                  onpointermove={onDrag}
                  onpointerup={finishDrag}
                  onpointercancel={() => (drag = null)}
                  onclick={() => (selected = w)}
                  onkeydown={(e) => {
                    if (e.key === "Enter" || e.key === " ") selected = w;
                  }}
                >
                  {#if w.status === "proposed"}
                    <div
                      class="handle top"
                      data-cal-handle="top"
                      role="presentation"
                      onpointerdown={(e) => beginDrag(e, w, "top")}
                    ></div>
                  {/if}
                  <b>{w.title}</b>
                  <span>
                    {fmtTime(hoursOf(new Date(w.starts_at)))}-{fmtTime(hoursOf(new Date(w.ends_at)))} · {w.agent_name}{w.status === "proposed" ? " · proposed" : ""}
                  </span>
                  {#if w.status === "proposed"}
                    <div
                      class="handle bottom"
                      data-cal-handle="bottom"
                      role="presentation"
                      onpointerdown={(e) => beginDrag(e, w, "bottom")}
                    ></div>
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
        </div>
      </div>
    </div>
  </div>

  <aside class="cal-side">
    <h3>Windows this week</h3>
    <p>Click a window for services, risk, rollback and the runbook. Drag a proposed window, or its edges, to move it; conflicts are shown but never block.</p>
    {#if !weekWindows.length}
      <div class="empty-side">No maintenance windows this week.</div>
    {/if}
    {#each weekWindows as w (w.id)}
      <button class="win-item" data-cal-open={w.id} onclick={() => (selected = w)}>
        <div class="row"><i class="status-badge {w.status}">{w.status}</i><span class="who">{w.agent_name}</span></div>
        <b>{w.title}</b>
        <span>{fmtDay(new Date(w.starts_at))} · {fmtTime(hoursOf(new Date(w.starts_at)))}-{fmtTime(hoursOf(new Date(w.ends_at)))}</span>
        <span>{w.services.join(", ")}</span>
      </button>
    {/each}
    {#each freezes as f (f.id)}
      <div class="freeze-note"><b>{f.label}</b><br />{fmtDay(new Date(f.starts_at))} to {fmtDay(new Date(f.ends_at))}. Windows inside a freeze are allowed and shown as a conflict.</div>
    {/each}
  </aside>
</section>

{#if selected}
  {@const w = selected}
  <div class="modal-scrim" role="presentation" data-cal-scrim onclick={(e) => e.target === e.currentTarget && (selected = null)}>
    <div class="modal" role="dialog" aria-modal="true" tabindex="-1" aria-label={w.title} data-cal-popup>
      <div class="modal-head">
        <div>
          <div class="badge-row">
            <span class="status-badge {w.status}">{w.status}</span>
            <span class="meta">{w.agent_name}{w.ask_id ? ` · ask ${w.ask_id}` : ""}</span>
            {#if conflictFor(w)}<span class="conflict-pill">overlaps a freeze or another window</span>{/if}
          </div>
          <h2>{w.title}</h2>
          <div class="sub">
            {fmtDay(new Date(w.starts_at))} · {fmtTime(hoursOf(new Date(w.starts_at)))}-{fmtTime(hoursOf(new Date(w.ends_at)))} {w.timezone}
            · {(spanHours(hoursOf(new Date(w.starts_at)), hoursOf(new Date(w.ends_at)))).toFixed(1)} hours
          </div>
        </div>
        <button class="modal-close" aria-label="Close" data-cal-close onclick={() => (selected = null)}>×</button>
      </div>
      <div class="modal-body">
        <div class="panel">
          <h5>Services affected</h5>
          <div class="services">{#each w.services as s (s)}<span class="svc">{s}</span>{/each}</div>
        </div>
        <div class="two-col">
          <div class="panel"><h5>Why</h5><p>{w.reason || "No reason recorded."}</p></div>
          <div class="panel"><h5>Risk</h5><p>{w.risks || "No risk recorded."}</p></div>
        </div>
        <div class="panel"><h5>Rollback plan</h5><p>{w.rollback || "No rollback recorded."}</p></div>
        <h3 class="runbook-title">Runbook</h3>
        {#if runbookSteps(w.runbook_md).length}
          <div class="runbook">
            {#each runbookSteps(w.runbook_md) as step, i (i)}
              <div class="step"><div>{step}</div></div>
            {/each}
          </div>
        {:else}
          <p class="dim">No runbook recorded.</p>
        {/if}
        <div class="modal-actions">
          {#if w.status === "proposed" && w.ask_id}
            <button class="btn ok" data-cal-action="approve" disabled={deciding} onclick={() => decide("approve")}>Approve this window</button>
            <button class="btn" data-cal-action="changes" disabled={deciding} onclick={() => decide("changes")}>Send revised slot</button>
          {:else if w.ask_id}
            <button class="btn" data-cal-open-ask onclick={() => { const id = w.ask_id!; selected = null; onOpenAsk(id); }}>Open the ask</button>
          {/if}
          <button class="btn ghost" onclick={() => (selected = null)}>Close</button>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .calendar-wrap {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 330px;
    border-top: 1px solid var(--line);
    overflow: hidden;
  }
  .calendar-main {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .cal-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 18px;
    border-bottom: 1px solid var(--line);
    flex: none;
    flex-wrap: wrap;
  }
  .cal-bar h2 {
    margin: 0;
    font: 600 16px/1 var(--font-display);
    letter-spacing: -0.02em;
  }
  .spacer {
    flex: 1;
  }
  .legend {
    display: flex;
    gap: 14px;
    font-size: 10.5px;
    color: var(--ink-3);
  }
  .legend i {
    display: inline-block;
    width: 10px;
    height: 6px;
    border-radius: 2px;
    margin-right: 5px;
    vertical-align: middle;
  }
  .legend i.approved {
    background: color-mix(in srgb, var(--ok) 50%, transparent);
  }
  .legend i.proposed {
    background: color-mix(in srgb, var(--status-review) 55%, transparent);
  }
  .legend i.freeze {
    background: repeating-linear-gradient(45deg, color-mix(in srgb, var(--danger) 60%, transparent), color-mix(in srgb, var(--danger) 60%, transparent) 3px, transparent 3px, transparent 6px);
  }
  .legend i.conflict {
    background: transparent;
    box-shadow: inset 0 0 0 2px var(--danger);
  }
  .state {
    padding: 16px 18px;
    color: var(--ink-3);
    font-size: 12.5px;
  }
  .state.err {
    color: var(--danger);
  }
  .cal-scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 14px 18px;
  }
  .week {
    position: relative;
    display: grid;
    grid-template-columns: 56px repeat(7, minmax(110px, 1fr));
    min-width: 860px;
  }
  .week-head {
    position: sticky;
    top: 0;
    z-index: 6;
    background: var(--bg);
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: 56px repeat(7, minmax(110px, 1fr));
    border-bottom: 1px solid var(--line);
  }
  .week-head div {
    padding: 9px 8px;
    font-size: 11px;
    color: var(--ink-3);
    border-left: 1px solid var(--line);
  }
  .week-head div:first-child {
    border-left: 0;
  }
  .week-head b {
    display: block;
    font-size: 14px;
    color: var(--ink-dim);
    font-weight: 600;
  }
  .week-head .today b {
    color: var(--accent);
  }
  .week-body {
    position: relative;
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: 56px repeat(7, minmax(110px, 1fr));
    height: 912px;
  }
  .hours {
    position: relative;
  }
  .hours span {
    position: absolute;
    right: 8px;
    font: 9.5px var(--font-mono);
    color: var(--ink-3);
    transform: translateY(-50%);
  }
  .day-col {
    position: relative;
    border-left: 1px solid var(--line);
  }
  .day-col::before {
    content: "";
    position: absolute;
    inset: 0;
    background: repeating-linear-gradient(to bottom, transparent 0, transparent 37px, color-mix(in srgb, var(--ink) 3.5%, transparent) 37px, color-mix(in srgb, var(--ink) 3.5%, transparent) 38px);
    pointer-events: none;
  }
  .day-col.weekend {
    background: color-mix(in srgb, var(--ink) 1.2%, transparent);
  }
  .freeze {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    bottom: 0;
    background: repeating-linear-gradient(45deg, color-mix(in srgb, var(--danger) 5%, transparent), color-mix(in srgb, var(--danger) 5%, transparent) 7px, transparent 7px, transparent 14px);
    border-top: 1px dashed color-mix(in srgb, var(--danger) 30%, transparent);
    border-bottom: 1px dashed color-mix(in srgb, var(--danger) 30%, transparent);
    pointer-events: none;
  }
  .freeze span {
    position: sticky;
    top: 2px;
    display: block;
    padding: 2px 5px;
    font: 9px var(--font-mono);
    color: color-mix(in srgb, var(--danger) 75%, var(--ink));
  }
  .win {
    position: absolute;
    left: 6px;
    right: 6px;
    border-radius: 7px;
    padding: 6px 7px;
    overflow: hidden;
    font-size: 10.5px;
    line-height: 1.35;
    border: 1px solid color-mix(in srgb, var(--accent) 50%, transparent);
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    color: #f0d3ae;
    cursor: grab;
    user-select: none;
    z-index: 2;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
    touch-action: none;
  }
  .win:hover {
    filter: brightness(1.1);
  }
  .win.dragging {
    cursor: grabbing;
    z-index: 9;
    box-shadow: 0 12px 28px rgba(0, 0, 0, 0.55);
  }
  .win b {
    display: block;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .win span {
    color: var(--ink-dim);
    font-size: 9.5px;
  }
  .win.proposed {
    border-style: dashed;
    background: color-mix(in srgb, var(--status-review) 14%, transparent);
    border-color: color-mix(in srgb, var(--status-review) 60%, transparent);
    color: #ded1f5;
  }
  .win.proposed b {
    color: #f1e9ff;
  }
  .win.approved {
    border-color: color-mix(in srgb, var(--ok) 55%, transparent);
    background: color-mix(in srgb, var(--ok) 13%, transparent);
    color: #c9e8d5;
  }
  .win.approved b {
    color: #e4f7ea;
  }
  .win.in_progress {
    border-color: color-mix(in srgb, var(--status-doing) 55%, transparent);
    background: color-mix(in srgb, var(--status-doing) 13%, transparent);
  }
  .win.conflict {
    border-color: var(--danger);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--danger) 30%, transparent);
  }
  .win .handle {
    position: absolute;
    left: 0;
    right: 0;
    height: 7px;
    cursor: ns-resize;
  }
  .win .handle.top {
    top: 0;
  }
  .win .handle.bottom {
    bottom: 0;
  }
  .cal-side {
    border-left: 1px solid var(--line);
    min-height: 0;
    overflow-y: auto;
    padding: 16px 16px 24px;
    background: var(--sunk);
  }
  .cal-side h3 {
    margin: 0 0 4px;
    font: 600 12px/1 var(--font-display);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-3);
  }
  .cal-side p {
    margin: 0 0 14px;
    color: var(--ink-dim);
    font-size: 11.5px;
  }
  .empty-side {
    color: var(--ink-3);
    font-size: 12px;
    padding: 8px 0;
  }
  .win-item {
    width: 100%;
    text-align: left;
    border: 1px solid var(--line);
    background: var(--surface);
    border-radius: 9px;
    padding: 10px 11px;
    margin-bottom: 8px;
    display: grid;
    gap: 4px;
  }
  .win-item:hover {
    border-color: var(--line-2);
  }
  .win-item .row {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .win-item .who {
    margin-left: auto;
    font-size: 10.5px;
    color: var(--ink-3);
  }
  .win-item b {
    font-size: 12px;
  }
  .win-item span {
    font-size: 10.5px;
    color: var(--ink-3);
  }
  .freeze-note {
    border: 1px dashed var(--line);
    border-radius: 9px;
    padding: 10px 11px;
    font-size: 11px;
    color: var(--ink-3);
    line-height: 1.5;
    margin-top: 14px;
  }
  .status-badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 10.5px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 3px 8px;
    border-radius: 999px;
    border: 1px solid var(--line);
  }
  .status-badge.proposed {
    color: var(--status-review);
    border-color: color-mix(in srgb, var(--status-review) 40%, transparent);
    background: color-mix(in srgb, var(--status-review) 10%, transparent);
  }
  .status-badge.approved {
    color: var(--ok);
    border-color: color-mix(in srgb, var(--ok) 40%, transparent);
    background: color-mix(in srgb, var(--ok) 12%, transparent);
  }
  .status-badge.in_progress {
    color: var(--status-doing);
    border-color: color-mix(in srgb, var(--status-doing) 40%, transparent);
    background: color-mix(in srgb, var(--status-doing) 10%, transparent);
  }
  .modal-scrim {
    position: fixed;
    inset: 0;
    background: rgba(5, 4, 3, 0.7);
    backdrop-filter: blur(3px);
    z-index: 70;
    display: grid;
    place-items: center;
    padding: 28px;
  }
  .modal {
    width: min(880px, 100%);
    max-height: min(86vh, 900px);
    overflow-y: auto;
    background: var(--surface);
    border: 1px solid var(--line-2);
    border-radius: 14px;
    box-shadow: var(--shadow-pop);
  }
  .modal-head {
    padding: 20px 24px 14px;
    border-bottom: 1px solid var(--line);
    display: flex;
    gap: 12px;
    align-items: flex-start;
    position: sticky;
    top: 0;
    background: var(--surface);
    z-index: 2;
  }
  .modal-head h2 {
    margin: 0 0 5px;
    font: 600 21px/1.15 var(--font-display);
    letter-spacing: -0.03em;
  }
  .badge-row {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 7px;
    flex-wrap: wrap;
  }
  .meta {
    font-size: 11px;
    color: var(--ink-3);
  }
  .conflict-pill {
    font-size: 10px;
    color: var(--danger);
    border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
    border-radius: 999px;
    padding: 1px 7px;
  }
  .sub {
    color: var(--ink-dim);
    font-size: 12px;
  }
  .modal-body {
    padding: 18px 24px 24px;
  }
  .modal-close {
    margin-left: auto;
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 22px;
    line-height: 1;
    padding: 2px 6px;
    border-radius: 6px;
  }
  .modal-close:hover {
    background: color-mix(in srgb, var(--ink) 5%, transparent);
    color: var(--ink);
  }
  .services {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 6px 0 0;
  }
  .svc {
    font: 10.5px var(--font-mono);
    border: 1px solid var(--line);
    background: var(--bg);
    border-radius: 5px;
    padding: 3px 7px;
    color: var(--ink-dim);
  }
  .two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin: 12px 0;
  }
  .panel {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    padding: 13px 15px;
    margin-bottom: 12px;
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
  .runbook-title {
    font: 600 13px/1.3 var(--font-display);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-dim);
    margin: 18px 0 8px;
  }
  .runbook {
    counter-reset: step;
    display: grid;
    gap: 8px;
    margin-top: 8px;
  }
  .runbook .step {
    display: grid;
    grid-template-columns: 26px 1fr;
    gap: 10px;
    align-items: start;
    color: #d4cabd;
    font-size: 12.5px;
  }
  .runbook .step::before {
    counter-increment: step;
    content: counter(step);
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font: 10px var(--font-mono);
    background: var(--accent-soft);
    color: var(--accent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
  }
  .modal-actions {
    display: flex;
    gap: 8px;
    margin-top: 20px;
    padding-top: 16px;
    border-top: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .dim {
    color: var(--ink-3);
    font-size: 12px;
  }
  @media (max-width: 1250px) {
    .calendar-wrap {
      grid-template-columns: minmax(0, 1fr);
    }
    .cal-side {
      display: none;
    }
  }
  @media (max-width: 980px) {
    .two-col {
      grid-template-columns: 1fr;
    }
  }
</style>
