<script lang="ts">
  let {
    start,
    end,
    timezone = "UTC",
    onchange,
  }: {
    start: Date;
    end: Date;
    timezone?: string;
    onchange: (start: Date, end: Date) => void;
  } = $props();

  const RANGE_HOURS = 12;
  const SNAP = 0.25;

  let dragging = $state<"move" | "l" | "r" | null>(null);
  let drag = $state<{ x: number; start: number; end: number; width: number } | null>(null);
  let barEl = $state<HTMLDivElement | null>(null);

  const DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

  function hourOf(d: Date) {
    return d.getHours() + d.getMinutes() / 60 + d.getSeconds() / 3600;
  }

  function dayBase(d: Date) {
    const out = new Date(d);
    out.setHours(0, 0, 0, 0);
    return out;
  }

  function clamp(n: number, lo: number, hi: number) {
    return Math.max(lo, Math.min(hi, n));
  }

  function round(n: number, step = SNAP) {
    return Math.round(n / step) * step;
  }

  // The timeline zooms to the twelve hours around the proposal, so a two hour
  // window stays draggable instead of being a two pixel stripe on a full day.
  const rangeStart = $derived(
    clamp(Math.floor(hourOf(start)) - 1, 0, 24 - RANGE_HOURS),
  );
  const rangeEnd = $derived(rangeStart + RANGE_HOURS);
  const startHour = $derived(hourOf(start));
  const endHour = $derived(hourOf(end));

  const left = $derived(clamp(((startHour - rangeStart) / RANGE_HOURS) * 100, 0, 100));
  const width = $derived(
    clamp(((Math.min(endHour, rangeEnd) - Math.max(startHour, rangeStart)) / RANGE_HOURS) * 100, 0, 100),
  );

  const ticks = $derived(
    Array.from({ length: RANGE_HOURS + 1 }, (_, i) => ({
      hour: (rangeStart + i) % 24,
      left: (i / RANGE_HOURS) * 100,
    })),
  );

  const week = $derived.by(() => {
    const base = dayBase(start);
    const monday = new Date(base);
    const weekday = (monday.getDay() + 6) % 7;
    monday.setDate(monday.getDate() - weekday);
    return DAY_NAMES.map((name, i) => {
      const date = new Date(monday);
      date.setDate(monday.getDate() + i);
      return { name, date };
    });
  });

  function two(n: number) {
    return String(n).padStart(2, "0");
  }

  function hm(hours: number) {
    const h = Math.floor(hours);
    const m = Math.round((hours - h) * 60);
    return `${two(h % 24)}:${two(m === 60 ? 0 : m)}`;
  }

  function duration() {
    const h = (end.getTime() - start.getTime()) / 3_600_000;
    return `${Number.isInteger(h) ? h : h.toFixed(2).replace(/0$/, "")}h`;
  }

  function fmtDay(d: Date) {
    try {
      return d.toLocaleDateString("en-AU", {
        weekday: "short",
        day: "numeric",
        month: "short",
        timeZone: timezone,
      });
    } catch {
      return d.toLocaleDateString("en-AU", { weekday: "short", day: "numeric", month: "short" });
    }
  }

  function emit(startHours: number, endHours: number) {
    const base = dayBase(start);
    const s = new Date(base.getTime() + startHours * 3_600_000);
    const e = new Date(base.getTime() + endHours * 3_600_000);
    if (e.getTime() - s.getTime() < 15 * 60_000) return;
    onchange(s, e);
  }

  function move(newStart: number, newEnd: number) {
    const lo = rangeStart;
    const hi = rangeEnd;
    let s = clamp(round(newStart), lo, hi);
    let e = clamp(round(newEnd), lo, hi);
    if (e - s < SNAP) {
      if (dragging === "l") s = e - SNAP;
      else e = s + SNAP;
    }
    emit(s, e);
  }

  function setStartHours(h: number) {
    const s = clamp(round(h), 0, 24 - SNAP);
    if (s >= endHour) return;
    emit(s, endHour);
  }

  function setEndHours(h: number) {
    const e = clamp(round(h), SNAP, 24);
    if (e <= startHour) return;
    emit(startHour, e);
  }

  function setDate(date: Date) {
    const s = new Date(date);
    s.setHours(start.getHours(), start.getMinutes(), 0, 0);
    const e = new Date(date);
    e.setHours(end.getHours(), end.getMinutes(), 0, 0);
    if (e <= s) e.setDate(e.getDate() + 1);
    onchange(s, e);
  }

  function shiftStart(minutes: number) {
    const s = new Date(start.getTime() + minutes * 60_000);
    if (s >= end) return;
    onchange(s, end);
  }

  function shiftEnd(minutes: number) {
    const e = new Date(end.getTime() + minutes * 60_000);
    if (e <= start) return;
    onchange(start, e);
  }

  function preset(hours: number) {
    const e = new Date(start.getTime() + hours * 3_600_000);
    onchange(start, e);
  }

  function begin(e: PointerEvent, mode: "move" | "l" | "r") {
    if (!barEl) return;
    dragging = mode;
    drag = {
      x: e.clientX,
      start: startHour,
      end: endHour,
      width: barEl.parentElement?.getBoundingClientRect().width || 1,
    };
    barEl.setPointerCapture(e.pointerId);
    e.preventDefault();
  }

  function onmove(e: PointerEvent) {
    if (!dragging || !drag) return;
    const dh = round(((e.clientX - drag.x) / drag.width) * RANGE_HOURS);
    if (dragging === "move") {
      const d = clamp(dh, -drag.start + rangeStart, rangeEnd - drag.end);
      move(drag.start + d, drag.end + d);
    } else if (dragging === "l") {
      move(drag.start + dh, drag.end);
    } else {
      move(drag.start, drag.end + dh);
    }
  }

  function finish() {
    dragging = null;
    drag = null;
  }

  function onkey(e: KeyboardEvent) {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    const step = e.shiftKey ? 1 : SNAP;
    const d = e.key === "ArrowLeft" ? -step : step;
    const delta = clamp(d, rangeStart - startHour, rangeEnd - endHour);
    move(startHour + delta, endHour + delta);
    e.preventDefault();
  }
</script>

<div class="win-editor" data-window-editor>
  <div class="we-head">
    <svg viewBox="0 0 16 16" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="1.4" d="M2.5 4h11v9.5h-11zM5.5 2v3m5-3v3M2.5 7h11" /></svg>
    <b>Proposed maintenance window</b>
    <span data-we-range-label>{fmtDay(start)} · {hm(startHour)}-{hm(endHour)}</span>
  </div>

  <div class="we-days" role="group" aria-label="Move the window to another day">
    {#each week as d, i (i)}
      <button
        type="button"
        data-we-day={i}
        class:on={d.date.toDateString() === start.toDateString()}
        aria-pressed={d.date.toDateString() === start.toDateString()}
        title={fmtDay(d.date)}
        onclick={() => setDate(d.date)}
      >
        {d.name}<b>{d.date.getDate()}</b>
      </button>
    {/each}
  </div>

  <div class="we-track-wrap">
    <div class="we-track">
      {#each ticks as t (t.hour + "-" + t.left)}
        <div class="we-tick" style="left:{t.left}%"><span>{two(t.hour)}:00</span></div>
      {/each}
      <div
        class="we-bar"
        class:dragging={dragging !== null}
        data-we-bar
        role="slider"
        aria-label="Maintenance window range"
        aria-valuemin={rangeStart}
        aria-valuemax={rangeEnd}
        aria-valuenow={startHour}
        aria-valuetext={`${hm(startHour)} to ${hm(endHour)}`}
        tabindex="0"
        style="left:{left}%;width:{width}%"
        bind:this={barEl}
        onpointerdown={(e) => begin(e, "move")}
        onpointermove={onmove}
        onpointerup={finish}
        onpointercancel={finish}
        onkeydown={onkey}
      >
        <div
          class="rh l"
          role="presentation"
          data-we-handle="left"
          onpointerdown={(e) => {
            e.stopPropagation();
            begin(e, "l");
          }}
        ></div>
        <b data-we-bar-label>{hm(startHour)} - {hm(endHour)}</b>
        <span data-we-duration>{duration()}</span>
        <div
          class="rh r"
          role="presentation"
          data-we-handle="right"
          onpointerdown={(e) => {
            e.stopPropagation();
            begin(e, "r");
          }}
        ></div>
      </div>
    </div>
    <span class="we-zoom">{two(rangeStart)}:00 to {two(rangeEnd)}:00 · drag the block or its edges · 15 minute snap</span>
  </div>

  <details class="fine-tune">
    <summary>Fine-tune times <span>hour : minute steppers</span></summary>
    <div class="fine-body">
      <div class="we-steps">
        <div class="we-step">
          <span>Starts</span>
          <div class="row">
            <button type="button" aria-label="Start hour earlier" data-we-step="start-hour-minus" onclick={() => shiftStart(-60)}>−</button>
            <div class="num" data-we-start-hour>{two(start.getHours())}</div>
            <button type="button" aria-label="Start hour later" data-we-step="start-hour-plus" onclick={() => shiftStart(60)}>+</button>
            <div class="colon">:</div>
            <button type="button" aria-label="Start 15 minutes earlier" data-we-step="start-minute-minus" onclick={() => shiftStart(-15)}>−</button>
            <div class="num" data-we-start-minute>{two(start.getMinutes())}</div>
            <button type="button" aria-label="Start 15 minutes later" data-we-step="start-minute-plus" onclick={() => shiftStart(15)}>+</button>
          </div>
        </div>
        <div class="we-step">
          <span>Ends</span>
          <div class="row">
            <button type="button" aria-label="End hour earlier" data-we-step="end-hour-minus" onclick={() => shiftEnd(-60)}>−</button>
            <div class="num" data-we-end-hour>{two(end.getHours())}</div>
            <button type="button" aria-label="End hour later" data-we-step="end-hour-plus" onclick={() => shiftEnd(60)}>+</button>
            <div class="colon">:</div>
            <button type="button" aria-label="End 15 minutes earlier" data-we-step="end-minute-minus" onclick={() => shiftEnd(-15)}>−</button>
            <div class="num" data-we-end-minute>{two(end.getMinutes())}</div>
            <button type="button" aria-label="End 15 minutes later" data-we-step="end-minute-plus" onclick={() => shiftEnd(15)}>+</button>
          </div>
        </div>
      </div>
      <div class="we-presets" role="group" aria-label="Window duration presets">
        {#each [1, 2, 3, 4, 6] as hours (hours)}
          <button type="button" data-we-preset={hours} onclick={() => preset(hours)}>{hours}h</button>
        {/each}
      </div>
    </div>
  </details>

  <div class="we-actions">
    <span class="hint">Conflicts with another window or a freeze are shown in the calendar but never block this ask.</span>
  </div>
</div>

<style>
  .win-editor {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    padding: 12px;
    margin: 12px 0 16px;
  }
  .we-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 9px;
  }
  .we-head svg {
    width: 14px;
    height: 14px;
    color: var(--status-review);
  }
  .we-head b {
    font-size: 12.5px;
  }
  .we-head span {
    margin-left: auto;
    font: 11px var(--font-mono);
    color: var(--accent);
  }
  .we-days {
    display: flex;
    gap: 4px;
    margin-bottom: 10px;
  }
  .we-days button {
    flex: 1;
    border: 1px solid var(--line);
    background: var(--bg);
    color: var(--ink-3);
    border-radius: 6px;
    padding: 5px 2px;
    font-size: 10.5px;
    display: grid;
    gap: 1px;
  }
  .we-days button b {
    font: 600 12px var(--font-ui);
    color: var(--ink-dim);
  }
  .we-days button:hover {
    color: var(--ink);
    border-color: var(--line-2);
  }
  .we-days button.on {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    color: var(--accent);
  }
  .we-days button.on b {
    color: var(--ink);
  }
  .we-track-wrap {
    position: relative;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--bg);
    padding: 22px 0 0;
    height: 104px;
    user-select: none;
  }
  .we-track {
    position: relative;
    height: 52px;
    margin: 0 12px;
    background: var(--sunk);
    border-top: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
  }
  .we-tick {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid var(--line);
  }
  .we-tick span {
    position: absolute;
    top: -18px;
    left: -13px;
    font: 9px var(--font-mono);
    color: var(--ink-3);
  }
  .we-bar {
    position: absolute;
    top: 5px;
    bottom: 5px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--status-review) 20%, transparent);
    border: 1px solid color-mix(in srgb, var(--status-review) 70%, transparent);
    color: #f0e6ff;
    font-size: 11px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    align-items: center;
    cursor: grab;
    z-index: 2;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
    touch-action: none;
  }
  .we-bar b {
    font-size: 11.5px;
    letter-spacing: 0.02em;
    white-space: nowrap;
  }
  .we-bar span {
    font-size: 9px;
    color: #cbb9ec;
  }
  .we-bar.dragging {
    cursor: grabbing;
  }
  .we-bar .rh {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 12px;
    cursor: ew-resize;
  }
  .we-bar .rh.l {
    left: 0;
    border-right: 1px solid rgba(255, 255, 255, 0.15);
  }
  .we-bar .rh.r {
    right: 0;
    border-left: 1px solid rgba(255, 255, 255, 0.15);
  }
  .we-bar .rh::after {
    content: "";
    position: absolute;
    top: 50%;
    left: 50%;
    width: 2px;
    height: 16px;
    margin: -8px 0 0 -1px;
    background: rgba(255, 255, 255, 0.55);
    border-radius: 2px;
  }
  .we-zoom {
    position: absolute;
    right: 12px;
    bottom: 4px;
    font: 9px var(--font-mono);
    color: var(--ink-3);
  }
  details.fine-tune {
    margin-top: 9px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg);
  }
  details.fine-tune summary {
    list-style: none;
    cursor: pointer;
    padding: 8px 11px;
    font-size: 11.5px;
    color: var(--ink-dim);
    display: flex;
    align-items: center;
    gap: 7px;
  }
  details.fine-tune summary::-webkit-details-marker {
    display: none;
  }
  details.fine-tune summary::before {
    content: "▸";
    color: var(--accent);
    font-size: 10px;
  }
  details.fine-tune[open] summary::before {
    content: "▾";
  }
  details.fine-tune summary span {
    color: var(--ink-3);
    font-size: 10px;
  }
  .fine-body {
    padding: 0 11px 11px;
  }
  .we-steps {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .we-step {
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--sunk);
    padding: 7px 8px;
  }
  .we-step > span {
    display: block;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-3);
    margin-bottom: 4px;
  }
  .we-step .row {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .we-step .num {
    flex: 1;
    text-align: center;
    font: 600 16px var(--font-mono);
    color: var(--ink);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 5px;
    padding: 1px 2px;
    font-variant-numeric: tabular-nums;
  }
  .we-step .colon {
    color: var(--ink-3);
    font: 600 14px var(--font-mono);
  }
  .we-step button {
    width: 27px;
    height: 27px;
    border-radius: 6px;
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink-dim);
    font-size: 14px;
    line-height: 1;
  }
  .we-step button:hover {
    color: var(--ink);
    border-color: var(--line-2);
  }
  .we-presets {
    display: flex;
    gap: 4px;
    margin-top: 7px;
  }
  .we-presets button {
    flex: 1;
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink-dim);
    border-radius: 5px;
    padding: 4px 0;
    font-size: 10.5px;
  }
  .we-presets button:hover {
    color: var(--ink);
    border-color: var(--line-2);
  }
  .we-actions {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  .hint {
    font-size: 10px;
    color: var(--ink-3);
  }
  @media (max-width: 980px) {
    .we-steps {
      grid-template-columns: 1fr;
    }
  }
</style>
