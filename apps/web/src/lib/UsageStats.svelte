<script lang="ts">
  import type { UsageRange, UsageSummary } from "@breadboard/api-client";
  import { api, errMsg } from "./store.svelte";

  let {
    agent = null,
    issueKey = null,
    settingsEnabled = false,
    compact = false,
  }: {
    agent?: string | null;
    issueKey?: string | null;
    settingsEnabled?: boolean;
    compact?: boolean;
  } = $props();

  const RANGES: UsageRange[] = ["24h", "7d", "30d", "90d", "all"];
  let range = $state<UsageRange>("7d");
  let summary = $state<UsageSummary | null>(null);
  let loading = $state(false);
  let error = $state("");
  let saving = $state(false);
  let thresholdDraft = $state("5.00");

  const subject = $derived(agent ? `agent:${agent}` : issueKey ? `issue:${issueKey}` : "");

  $effect(() => {
    // Depend on the subject and range; load() reads the same values.
    void subject;
    void range;
    if (subject) void load();
  });

  async function load() {
    if (!subject) return;
    const key = `${subject}|${range}`;
    loading = true;
    error = "";
    try {
      const next = agent
        ? await api.getAgentUsage(agent, range)
        : await api.getIssueUsage(issueKey!, range);
      if (`${subject}|${range}` !== key) return;
      summary = next;
      thresholdDraft = next.spend_warn_usd.toFixed(2);
    } catch (e) {
      if (`${subject}|${range}` !== key) return;
      summary = null;
      error = errMsg(e);
    } finally {
      if (`${subject}|${range}` === key) loading = false;
    }
  }

  async function saveThreshold() {
    const value = Number(thresholdDraft);
    if (!Number.isFinite(value) || value < 0) {
      error = "Enter a non-negative amount";
      return;
    }
    saving = true;
    error = "";
    try {
      const saved = await api.setUsageSettings({ spend_warn_usd: value });
      if (summary) {
        summary = {
          ...summary,
          spend_warn_usd: saved.spend_warn_usd,
          over_threshold: summary.cost > saved.spend_warn_usd && saved.spend_warn_usd > 0,
        };
      }
    } catch (e) {
      error = errMsg(e);
    } finally {
      saving = false;
    }
  }

  function money(value: number): string {
    if (value >= 100) return `$${value.toFixed(0)}`;
    if (value >= 1) return `$${value.toFixed(2)}`;
    if (value >= 0.01) return `$${value.toFixed(3)}`;
    return value === 0 ? "$0" : `$${value.toFixed(4)}`;
  }

  /**
   * Spend with unpriced token rows kept distinct from reported cost. A source
   * row with tokens but a zero cost is not free, it is unpriced.
   */
  function spendLabel(value: { cost: number; unpriced_tokens: number }): string {
    if (value.unpriced_tokens > 0) {
      return value.cost > 0 ? `${money(value.cost)} + unpriced` : "unpriced";
    }
    return money(value.cost);
  }

  function tokens(value: number): string {
    if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
    if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
    if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
    return String(value);
  }

  function pct(value: number): string {
    return `${Math.round(value * 100)}%`;
  }
</script>

{#if subject}
  <section class="usage" class:compact aria-label="Usage statistics">
    <header class="uhead">
      <h3>{agent ? "Spend and tokens" : "Issue spend"}</h3>
      <span class="grow"></span>
      <div class="range" role="group" aria-label="Time range">
        {#each RANGES as r (r)}
          <button class:on={range === r} onclick={() => (range = r)} title={r === "all" ? "All time" : `Last ${r}`}>
            {r}
          </button>
        {/each}
      </div>
    </header>

    {#if error}
      <p class="err">{error}</p>
    {/if}

    {#if summary}
      <div class="grid">
        <div class="cell" class:bad={summary.over_threshold}>
          <b title={summary.unpriced_tokens > 0 ? `${tokens(summary.unpriced_tokens)} tokens have no reported cost` : undefined}>
            {spendLabel(summary)}
          </b>
          <span>spend</span>
        </div>
        <div class="cell">
          <b>{tokens(summary.total_tokens)}</b>
          <span>tokens</span>
        </div>
        <div class="cell">
          <b>{summary.session_count}</b>
          <span>session{summary.session_count === 1 ? "" : "s"}</span>
        </div>
        <div class="cell">
          <b>{summary.models.length}</b>
          <span>model{summary.models.length === 1 ? "" : "s"}</span>
        </div>
      </div>

      {#if summary.over_threshold}
        <p class="warn" role="status">
          Over the {money(summary.spend_warn_usd)} spend warning threshold.
        </p>
      {/if}

      <p class="tokline">
        <span>{tokens(summary.input_tokens)} in</span>
        <span>{tokens(summary.output_tokens)} out</span>
        <span>{tokens(summary.reasoning_tokens)} reasoning</span>
        <span>{tokens(summary.cache_read_tokens)} cache read</span>
        <span>{tokens(summary.cache_write_tokens)} cache write</span>
      </p>

      {#if summary.unpriced_tokens > 0}
        <p class="unpriced" role="status">
          {tokens(summary.unpriced_tokens)} tokens have no reported cost. They are shown as
          unpriced, not $0, and are excluded from spend and the warning threshold.
        </p>
      {/if}

      {#if summary.models.length}
        <div class="models">
          {#each summary.models as m (m.model + (m.provider ?? ""))}
            <div class="model">
              <div class="mrow">
                <span class="mname" title={m.model}>{m.model}</span>
                <span class="share">{pct(m.share)}</span>
                <span class="mcost">{spendLabel(m)} · {tokens(m.total_tokens)} tok</span>
              </div>
              <div class="bar"><i style="width:{Math.max(2, m.share * 100)}%"></i></div>
            </div>
          {/each}
        </div>
      {/if}

      {#if !agent && summary.agents?.length}
        <p class="agents">
          {#each summary.agents as a (a.agent_name)}
            <span class="achip">
              {a.agent_name} {pct(a.share)}{a.cost <= 0 && a.unpriced_tokens > 0
                ? " · unpriced"
                : ""}
            </span>
          {/each}
        </p>
      {/if}

      {#if !compact && agent && summary.sessions.length}
        <div class="sessions">
          <span class="slabel">Sessions</span>
          {#each summary.sessions.slice(0, 6) as s (s.session_id)}
            <div class="srow">
              <code>{s.session_id.slice(0, 14)}</code>
              <span>{s.first_day}{s.last_day !== s.first_day ? ` → ${s.last_day}` : ""}</span>
              <span>{spendLabel(s)} · {tokens(s.total_tokens)} tok</span>
            </div>
          {/each}
        </div>
      {/if}

      {#if settingsEnabled}
        <div class="threshold">
          <label for="spend-warn">Warn over $</label>
          <input
            id="spend-warn"
            type="number"
            min="0"
            step="0.5"
            bind:value={thresholdDraft}
            onkeydown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void saveThreshold();
              }
            }}
          />
          <button class="btn ghost sm" disabled={saving} onclick={() => void saveThreshold()}>
            {saving ? "Saving" : "Save"}
          </button>
          <span class="hint">0 disables warnings; applies to any agent or issue in this range</span>
        </div>
      {/if}
    {:else if loading}
      <p class="muted">Loading usage…</p>
    {:else}
      <p class="muted">No usage recorded for this {agent ? "agent" : "issue"} in the selected range.</p>
    {/if}
  </section>
{/if}

<style>
  .usage {
    margin: 12px 0 0;
    padding: 12px 14px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: color-mix(in srgb, var(--ink) 2%, transparent);
  }
  .usage.compact {
    margin-top: 10px;
    padding: 10px 12px;
  }
  .uhead {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
  }
  .uhead h3 {
    margin: 0;
    font: 600 13px/1 var(--font-display);
  }
  .grow {
    flex: 1;
  }
  .range {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 7px;
    overflow: hidden;
  }
  .range button {
    padding: 4px 8px;
    border: 0;
    background: transparent;
    color: var(--ink-3);
    font: 500 10px/1 var(--font-mono);
    cursor: pointer;
  }
  .range button.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    color: var(--ink);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 6px;
  }
  .cell {
    padding: 7px 8px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: color-mix(in srgb, var(--ink) 3%, transparent);
    text-align: center;
  }
  .cell b {
    display: block;
    font: 650 15px/1 var(--font-display);
    font-variant-numeric: tabular-nums;
  }
  .cell span {
    display: block;
    margin-top: 4px;
    font: 500 9px/1 var(--font-mono);
    letter-spacing: 0.04em;
    color: var(--ink-3);
  }
  .cell.bad {
    border-color: color-mix(in srgb, var(--danger) 40%, transparent);
    background: color-mix(in srgb, var(--danger) 9%, transparent);
  }
  .cell.bad b {
    color: var(--danger);
  }
  .warn {
    margin: 8px 0 0;
    padding: 6px 8px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font: 500 11px/1.3 var(--font-mono);
  }
  .tokline {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    margin: 9px 0 0;
    color: var(--ink-3);
    font: 400 10px/1.4 var(--font-mono);
  }
  .unpriced {
    margin: 8px 0 0;
    padding: 6px 8px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--ink) 6%, transparent);
    color: var(--ink);
    font: 500 11px/1.3 var(--font-mono);
  }
  .models {
    display: flex;
    flex-direction: column;
    gap: 7px;
    margin-top: 10px;
  }
  .mrow {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font: 400 10.5px/1.3 var(--font-mono);
  }
  .mname {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink);
  }
  .share {
    margin-left: auto;
    color: var(--ink);
    font-weight: 600;
  }
  .mcost {
    color: var(--ink-3);
    white-space: nowrap;
  }
  .bar {
    height: 4px;
    margin-top: 3px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    border-radius: 99px;
    background: linear-gradient(90deg, var(--accent), color-mix(in srgb, var(--accent) 55%, var(--ok)));
  }
  .agents {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin: 9px 0 0;
  }
  .achip {
    padding: 3px 6px;
    border: 1px solid var(--line);
    border-radius: 99px;
    color: var(--ink-3);
    font: 500 9.5px/1 var(--font-mono);
  }
  .sessions {
    margin-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .slabel {
    font: 600 9.5px/1 var(--font-mono);
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .srow {
    display: flex;
    gap: 8px;
    align-items: baseline;
    color: var(--ink-3);
    font: 400 10px/1.3 var(--font-mono);
  }
  .srow code {
    color: var(--ink);
  }
  .srow span:nth-child(2) {
    margin-left: auto;
  }
  .threshold {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
    padding-top: 9px;
    border-top: 1px solid var(--line);
    color: var(--ink-3);
    font: 400 10.5px/1.3 var(--font-mono);
  }
  .threshold input {
    width: 70px;
    padding: 4px 6px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
  }
  .hint {
    color: var(--ink-3);
  }
  .muted {
    margin: 6px 0 0;
    color: var(--ink-3);
    font: 400 11px/1.4 var(--font-mono);
  }
  .err {
    margin: 6px 0 0;
    color: var(--danger);
    font: 400 11px/1.4 var(--font-mono);
  }
</style>
