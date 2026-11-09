<script lang="ts">
  import {
    ARTIFACT_KINDS,
    type ArtifactKind,
    type Comment,
    type IssueArtifact,
  } from "@breadboard/api-client";
  import { ago, api, errMsg, store } from "./store.svelte";
  import { agentColor } from "./ui/colors";

  let {
    issueKey,
    comments,
  }: { issueKey: string; comments: Comment[] } = $props();

  let artifacts = $state<IssueArtifact[]>([]);
  let loading = $state(false);
  let uploading = $state(false);
  let uploadError = $state("");
  let fileEl: HTMLInputElement | undefined = $state();
  let file = $state<File | null>(null);
  let kind = $state<ArtifactKind>("screenshot");
  let commentId = $state<number | "">("");
  let reviewRef = $state("");

  type BenchRow = { name: string; value: string; unit: string };
  let expanded = $state<Record<number, boolean>>({});
  let textCache = $state<Record<number, string>>({});
  let benchCache = $state<Record<number, BenchRow[] | null | undefined>>({});
  let textError = $state<Record<number, string>>({});

  const LIMITS: Record<ArtifactKind, string> = {
    bench: "10 MB",
    screenshot: "10 MB",
    "ci-log": "50 MB",
    diff: "25 MB",
    trace: "50 MB",
    other: "10 MB",
  };

  async function load(key = issueKey) {
    if (!key) return;
    loading = true;
    try {
      const rows = await api.listIssueArtifacts(key);
      if (key === issueKey) artifacts = rows;
    } catch (e) {
      if (key === issueKey) store.flash(`Couldn't load evidence: ${errMsg(e)}`);
    } finally {
      if (key === issueKey) loading = false;
    }
  }

  $effect(() => {
    const key = issueKey;
    artifacts = [];
    expanded = {};
    textCache = {};
    benchCache = {};
    textError = {};
    file = null;
    commentId = "";
    reviewRef = "";
    uploadError = "";
    if (key) void load(key);
  });

  function inferKind(name: string): ArtifactKind | null {
    const ext = name.split(".").pop()?.toLowerCase() ?? "";
    if (["png", "jpg", "jpeg", "gif", "webp"].includes(ext)) return "screenshot";
    if (["diff", "patch"].includes(ext)) return "diff";
    if (["log", "txt"].includes(ext)) return "ci-log";
    if (["json"].includes(ext)) return "bench";
    return null;
  }

  function pick(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    file = input.files?.[0] ?? null;
    if (file) {
      const inferred = inferKind(file.name);
      if (inferred) kind = inferred;
    }
  }

  async function upload() {
    if (!file || uploading) return;
    uploading = true;
    uploadError = "";
    try {
      const saved = await api.uploadIssueArtifact(issueKey, file, {
        kind,
        comment_id: commentId === "" ? null : Number(commentId),
        review_ref: reviewRef.trim() || null,
      });
      artifacts = [saved, ...artifacts];
      file = null;
      if (fileEl) fileEl.value = "";
      commentId = "";
      reviewRef = "";
      store.info(`Attached ${saved.filename} to ${issueKey}`);
    } catch (e) {
      uploadError = errMsg(e);
    } finally {
      uploading = false;
    }
  }

  function fmtSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function fmtDate(iso: string): string {
    return new Date(iso).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }

  async function text(id: number): Promise<string> {
    if (textCache[id] != null) return textCache[id];
    try {
      const res = await fetch(api.artifactUrl(id), { credentials: "include" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const body = await res.text();
      const capped =
        body.length > 200_000
          ? `${body.slice(0, 200_000)}\n\n[showing the first 200,000 characters of ${body.length}]`
          : body;
      textCache = { ...textCache, [id]: capped };
      return capped;
    } catch (e) {
      textError = { ...textError, [id]: errMsg(e) };
      throw e;
    }
  }

  function parseBench(body: string): BenchRow[] | null {
    const normalize = (name: unknown, value: unknown, unit: unknown): BenchRow => ({
      name: String(name ?? "bench"),
      value: value == null ? "" : String(value),
      unit: unit == null ? "" : String(unit),
    });
    try {
      const parsed = JSON.parse(body);
      if (Array.isArray(parsed)) {
        const rows = parsed
          .map((row) => {
            if (row && typeof row === "object") {
              const r = row as Record<string, unknown>;
              const name = r.name ?? r.bench ?? r.metric ?? r.label;
              const value = r.value ?? r.mean ?? r.median ?? r.result ?? r.time;
              if (name == null || value == null) return null;
              return normalize(name, value, r.unit);
            }
            return null;
          })
          .filter((row): row is BenchRow => row !== null);
        return rows.length ? rows : null;
      }
      if (parsed && typeof parsed === "object") {
        const rows = Object.entries(parsed as Record<string, unknown>).map(([name, value]) => {
          if (value && typeof value === "object") {
            const v = value as Record<string, unknown>;
            return normalize(name, v.value ?? v.mean ?? v.median, v.unit);
          }
          return normalize(name, value, "");
        });
        return rows.length ? rows : null;
      }
    } catch {
      /* fall through to the line parser */
    }
    const rows = body
      .split("\n")
      .map((line) => line.match(/^([^:=]+)[:=]\s*([\d.,]+)\s*([A-Za-z%/µ]+)?/))
      .filter((m): m is RegExpMatchArray => m !== null)
      .map((m) => normalize(m[1].trim(), m[2], m[3]));
    return rows.length ? rows : null;
  }

  async function toggle(a: IssueArtifact) {
    const next = !expanded[a.id];
    expanded = { ...expanded, [a.id]: next };
    if (!next) return;
    if (a.kind === "bench" && benchCache[a.id] === undefined) {
      try {
        benchCache = { ...benchCache, [a.id]: parseBench(await text(a.id)) };
      } catch {
        benchCache = { ...benchCache, [a.id]: null };
      }
    } else if (a.kind !== "bench" && textCache[a.id] == null) {
      void text(a.id).catch(() => {});
    }
  }

  const previewable = (a: IssueArtifact) =>
    a.kind === "screenshot" || a.mime.startsWith("image/");
  const viewable = (a: IssueArtifact) =>
    a.kind === "bench" || a.kind === "ci-log" || a.kind === "diff" || a.kind === "trace";
</script>

<section class="sec evidence">
  <h3>
    Evidence
    {#if artifacts.length}<span class="count">{artifacts.length}</span>{/if}
    <span class="grow"></span>
    <button onclick={() => void load()} disabled={loading}>{loading ? "Loading…" : "Refresh"}</button>
  </h3>

  <div class="upload">
    <div class="up-row">
      <input
        bind:this={fileEl}
        type="file"
        aria-label="Artifact file"
        onchange={pick}
      />
      <select class="field kind" bind:value={kind} aria-label="Artifact kind">
        {#each ARTIFACT_KINDS as k (k)}
          <option value={k}>{k}</option>
        {/each}
      </select>
      <button class="btn primary" onclick={upload} disabled={!file || uploading}>
        {uploading ? "Uploading…" : "Attach"}
      </button>
    </div>
    <div class="up-row optional">
      <label>
        Link to comment
        <select class="field" bind:value={commentId} aria-label="Link to comment">
          <option value="">None</option>
          {#each comments as c (c.id)}
            <option value={c.id}>
              #{c.id} {c.body.replace(/\s+/g, " ").slice(0, 50)}
            </option>
          {/each}
        </select>
      </label>
      <label>
        Review link
        <input
          class="field"
          bind:value={reviewRef}
          placeholder="review key or URL (optional)"
          aria-label="Review reference"
        />
      </label>
    </div>
    <p class="hint">Immutable once attached · {LIMITS[kind]} max for {kind}</p>
    {#if uploadError}<p class="err">{uploadError}</p>{/if}
  </div>

  {#if !artifacts.length}
    <p class="dim small">{loading ? "Loading evidence…" : "No evidence attached yet."}</p>
  {:else}
    <div class="list">
      {#each artifacts as a (a.id)}
        <article class="art kind-{a.kind}">
          <header>
            <span class="badge">{a.kind}</span>
            <span class="name" title={a.filename}>{a.filename}</span>
            <span class="size">{fmtSize(a.size)}</span>
            <span class="grow"></span>
            {#if viewable(a)}
              <button class="tiny" onclick={() => void toggle(a)}>
                {expanded[a.id] ? "Hide" : "View"}
              </button>
            {/if}
            <a class="tiny" href={api.artifactUrl(a.id)} download={a.filename}>Download</a>
          </header>

          <div class="meta">
            <span
              class="who"
              style="--agent:{a.uploader_agent.startsWith('human:') ? '#d98a3d' : agentColor(a.uploader_agent)}"
              title={a.uploader_agent.startsWith("human:") ? "Uploaded from the web" : "Uploaded by this agent"}
            >
              {a.uploader_agent}
            </span>
            {#if a.session_id}
              <span class="sid mono" title={a.session_id}>{a.session_id.slice(0, 10)}</span>
            {/if}
            <span>{ago(a.created_at)}</span>
            <span title="Artifacts expire on this date">expires {fmtDate(a.expires_at)}</span>
            {#if a.comment_id}<span>comment #{a.comment_id}</span>{/if}
            {#if a.review_ref}<span class="rev" title={a.review_ref}>review {a.review_ref.slice(0, 40)}</span>{/if}
          </div>

          {#if previewable(a)}
            <a class="shot" href={api.artifactUrl(a.id)} target="_blank" rel="noreferrer">
              <img src={api.artifactUrl(a.id)} alt={a.filename} loading="lazy" />
            </a>
          {:else if a.kind === "bench"}
            {#if expanded[a.id]}
              {@const rows = benchCache[a.id]}
              {#if rows === undefined}
                <p class="dim small">Reading bench…</p>
              {:else if rows === null}
                <p class="dim small">Not a bench table. Expected JSON or <code>name: value unit</code> lines.</p>
                <pre class="viewer">{textCache[a.id] ?? textError[a.id] ?? "Reading…"}</pre>
              {:else}
                <table class="bench">
                  <thead><tr><th>Bench</th><th>Value</th><th>Unit</th></tr></thead>
                  <tbody>
                    {#each rows as row, i (i)}
                      <tr><td>{row.name}</td><td class="num">{row.value}</td><td>{row.unit}</td></tr>
                    {/each}
                  </tbody>
                </table>
              {/if}
            {:else}
              <p class="dim small">Bench table opens when you press View.</p>
            {/if}
          {:else if expanded[a.id]}
            {#if textError[a.id]}
              <p class="err">Couldn't load the file: {textError[a.id]}</p>
            {:else if textCache[a.id] == null}
              <p class="dim small">Reading…</p>
            {:else}
              <pre class="viewer">{textCache[a.id]}</pre>
            {/if}
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  .evidence {
    gap: 10px;
  }
  .count {
    font: 500 10px/1 var(--font-mono);
    color: var(--ink-3);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 2px 6px;
  }
  .grow {
    flex: 1;
  }
  .upload {
    border: 1px dashed var(--line-2);
    border-radius: 8px;
    padding: 9px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .up-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .up-row.optional {
    align-items: flex-end;
  }
  .up-row.optional label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 10px;
    color: var(--ink-3);
    min-width: 0;
    flex: 1;
  }
  .up-row input[type="file"] {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    color: var(--ink-dim);
  }
  .kind {
    width: 108px;
    flex: none;
  }
  .hint {
    margin: 0;
    font-size: 10px;
    color: var(--ink-3);
  }
  .err {
    margin: 0;
    font-size: 11px;
    color: var(--danger, #d86a6a);
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .art {
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg);
    overflow: hidden;
  }
  .art header {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 9px;
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }
  .badge {
    font: 600 9px/1 var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-dim);
    border: 1px solid var(--line-2);
    border-radius: 5px;
    padding: 3px 5px;
    flex: none;
  }
  .kind-screenshot .badge { color: #7db0e8; border-color: #3b5d80; }
  .kind-bench .badge { color: #d8b46a; border-color: #6f5a2f; }
  .kind-ci-log .badge { color: #9fc77a; border-color: #4c6438; }
  .kind-diff .badge { color: #c79ad8; border-color: #5f3f70; }
  .kind-trace .badge { color: #d88a7a; border-color: #70443b; }
  .name {
    font: 500 11px/1.3 var(--font-mono);
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .size {
    font-size: 10px;
    color: var(--ink-3);
    flex: none;
  }
  .tiny {
    font-size: 10px;
    color: var(--ink-dim);
    background: none;
    border: 0;
    padding: 2px 4px;
    text-decoration: none;
    flex: none;
  }
  .tiny:hover { color: var(--ink); }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 9px;
    padding: 6px 9px;
    font-size: 10px;
    color: var(--ink-3);
  }
  .who {
    color: var(--agent, var(--ink-dim));
    font-weight: 600;
  }
  .sid {
    font: 10px/1 var(--font-mono);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 2px 4px;
  }
  .rev {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 180px;
  }
  .shot {
    display: block;
    padding: 0 9px 9px;
  }
  .shot img {
    display: block;
    max-width: 100%;
    max-height: 340px;
    object-fit: contain;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface);
  }
  .viewer {
    margin: 0 9px 9px;
    max-height: 360px;
    overflow: auto;
    padding: 8px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface);
    font: 11px/1.45 var(--font-mono);
    color: var(--ink-dim);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .bench {
    width: calc(100% - 18px);
    margin: 0 9px 9px;
    border-collapse: collapse;
    font-size: 11px;
  }
  .bench th,
  .bench td {
    text-align: left;
    padding: 4px 6px;
    border-bottom: 1px solid var(--line);
  }
  .bench th {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-3);
  }
  .bench .num {
    font-family: var(--font-mono);
    color: var(--ink);
  }
  .small { font-size: 11px; }
  .dim { color: var(--ink-3); }
</style>
