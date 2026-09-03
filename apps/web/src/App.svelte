<script lang="ts">
  import { onMount } from "svelte";
  import { BreadboardClient, type BoardEvent, type Status } from "@breadboard/api-client";

  const client = new BreadboardClient();

  const columns: { status: Status; label: string }[] = [
    { status: "backlog", label: "Backlog" },
    { status: "todo", label: "Todo" },
    { status: "doing", label: "Doing" },
    { status: "review", label: "Review" },
    { status: "done", label: "Done" },
  ];

  let health = $state<"ok" | "bad" | "checking">("checking");
  let version = $state("");
  let lastEvent = $state<BoardEvent | null>(null);

  onMount(() => {
    client
      .health()
      .then((h) => {
        health = h.db ? "ok" : "bad";
        version = h.version;
      })
      .catch(() => (health = "bad"));

    return client.subscribe((event) => (lastEvent = event));
  });
</script>

<header>
  <strong>breadboard</strong>
  <span class="status">
    <span class="dot" class:ok={health === "ok"} class:bad={health === "bad"}></span>
    {health === "checking" ? "connecting…" : `server ${health}${version ? ` · v${version}` : ""}`}
  </span>
</header>

<div class="board">
  {#each columns as col (col.status)}
    <section class="column">
      <h2>{col.label}</h2>
      <p style="color: var(--ink-dim); font-size: 12px;">No issues yet.</p>
    </section>
  {/each}
</div>

{#if lastEvent}
  <p style="padding: 0 16px; color: var(--ink-dim); font-size: 12px;">
    last event: <code>{lastEvent.type}</code>
  </p>
{/if}
