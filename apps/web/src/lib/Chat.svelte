<script lang="ts">
  import { onMount } from "svelte";
  import { api, errMsg, store } from "./store.svelte";
  import type { ChatMessage, ChatRoom, ChatUpload } from "@breadboard/api-client";
  import { agentColor } from "./ui/colors";

  let rooms = $state<ChatRoom[]>([]);
  let selectedId = $state<number | null>(null);
  let messages = $state<ChatMessage[]>([]);
  let draft = $state("");
  let sending = $state(false);
  let pending = $state<ChatUpload[]>([]);
  let uploading = $state(false);
  let fileEl: HTMLInputElement | undefined = $state();
  let newRoom = $state(false);
  let roomName = $state("");
  let roomDesc = $state("");
  let dmPicker = $state(false);
  let dmTarget = $state("");
  let scroller: HTMLDivElement | undefined = $state();

  const selected = $derived(rooms.find((r) => r.id === selectedId));
  const shared = $derived(
    rooms
      .filter((r) => r.kind === "room")
      .sort((a, b) => (a.name === "lobby" ? -1 : b.name === "lobby" ? 1 : (a.name ?? "").localeCompare(b.name ?? ""))),
  );
  const dms = $derived(
    rooms
      .filter((r) => r.kind === "dm")
      .sort((a, b) => (b.last_message?.created_at ?? "").localeCompare(a.last_message?.created_at ?? "")),
  );
  const knownAgents = $derived([...new Set(store.agents.map((a) => a.agent_name))].sort());

  function memberDisplay(member: string): string {
    const i = member.indexOf(":");
    return i === -1 ? member : member.slice(i + 1);
  }

  function roomLabel(room: ChatRoom): string {
    if (room.kind === "room") return room.name ?? "room";
    return room.members.map(memberDisplay).join(" ↔ ");
  }

  function roomTint(room: ChatRoom): string {
    const agent = room.members.find((m) => m.startsWith("agent:"));
    return agent ? agentColor(memberDisplay(agent)) : "var(--ink-3)";
  }

  async function loadRooms(selectFirst = false) {
    try {
      const next = await api.listChatRooms();
      rooms = next;
      if (selectFirst || selectedId == null || !next.some((r) => r.id === selectedId)) {
        selectedId = next.find((r) => r.name === "lobby")?.id ?? next[0]?.id ?? null;
      }
    } catch {
      /* keep the current list */
    }
  }

  async function loadMessages(id: number | null) {
    if (id == null) {
      messages = [];
      return;
    }
    try {
      const next = await api.listChatMessages(id, { limit: 300 });
      if (selectedId === id) messages = next;
    } catch {
      /* ignore */
    }
  }

  function pick(id: number) {
    selectedId = id;
    void loadMessages(id);
  }

  async function onFiles(e: Event) {
    const input = e.target as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = "";
    if (!files.length) return;
    uploading = true;
    try {
      for (const file of files) {
        const uploaded = await api.uploadChatFile(file);
        pending = [...pending, uploaded];
      }
    } catch (err) {
      store.flash(`Couldn't upload: ${errMsg(err)}`);
    } finally {
      uploading = false;
    }
  }

  function dropPending(id: number) {
    pending = pending.filter((p) => p.id !== id);
  }

  function humanSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }

  function isImage(mime: string): boolean {
    return /^image\/(png|jpeg|gif|webp)$/.test(mime);
  }

  async function send() {
    const body = draft.trim();
    if ((!body && !pending.length) || selectedId == null || sending) return;
    sending = true;
    try {
      await api.postChatMessage({
        room: String(selectedId),
        body,
        uploads: pending.map((p) => p.id),
      });
      draft = "";
      pending = [];
      await Promise.all([loadMessages(selectedId), loadRooms()]);
    } catch (e) {
      store.flash(`Couldn't send: ${errMsg(e)}`);
    } finally {
      sending = false;
    }
  }

  async function createRoom() {
    if (!roomName.trim()) return;
    try {
      const room = await api.createChatRoom(roomName.trim(), roomDesc.trim() || undefined);
      newRoom = false;
      roomName = "";
      roomDesc = "";
      await loadRooms();
      pick(room.id);
    } catch (e) {
      store.flash(`Couldn't create the room: ${errMsg(e)}`);
    }
  }

  async function openDm() {
    if (!dmTarget) return;
    try {
      const room = await api.openDm(dmTarget);
      dmPicker = false;
      dmTarget = "";
      await loadRooms();
      pick(room.id);
    } catch (e) {
      store.flash(`Couldn't open the DM: ${errMsg(e)}`);
    }
  }

  function when(ts: string): string {
    const d = new Date(ts);
    const today = new Date().toDateString() === d.toDateString();
    return today
      ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
      : d.toLocaleDateString(undefined, { month: "short", day: "numeric" }) +
          " " +
          d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  }

  onMount(() => {
    void loadRooms(true).then(() => loadMessages(selectedId));
    const t = setInterval(() => {
      void loadRooms();
      void loadMessages(selectedId);
    }, 5_000);
    return () => clearInterval(t);
  });

  // SSE nudge: a message landed somewhere.
  $effect(() => {
    void store.chatTick;
    void loadRooms();
    void loadMessages(selectedId);
  });

  // Keep the newest message in view.
  $effect(() => {
    void messages.length;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  });
</script>

<div class="chat">
  <aside class="rooms" aria-label="Chat rooms">
    <header>
      <span class="sh">Rooms</span>
      <button class="mini" title="New room" aria-label="New room" onclick={() => (newRoom = !newRoom)}>+</button>
    </header>

    {#if newRoom}
      <form class="newroom" onsubmit={(e) => { e.preventDefault(); void createRoom(); }}>
        <input class="field" placeholder="room-name" bind:value={roomName} aria-label="Room name" />
        <input class="field" placeholder="what is it for? (optional)" bind:value={roomDesc} aria-label="Room description" />
        <button class="btn sm primary" type="submit">Create</button>
      </form>
    {/if}

    {#each shared as r (r.id)}
      <button class="room" class:on={r.id === selectedId} onclick={() => pick(r.id)} style="--tint:{roomTint(r)}">
        <span class="hash">#</span>
        <span class="rname">{r.name}</span>
        {#if r.last_message}<span class="rprev">{memberDisplay(r.last_message.author_member)}: {r.last_message.body}</span>{/if}
      </button>
    {/each}

    <header class="dmhead">
      <span class="sh">DMs</span>
      <button class="mini" title="New DM" aria-label="New DM" onclick={() => (dmPicker = !dmPicker)}>+</button>
    </header>

    {#if dmPicker}
      <form class="newroom" onsubmit={(e) => { e.preventDefault(); void openDm(); }}>
        <select class="field" bind:value={dmTarget} aria-label="Agent to DM">
          <option value="">pick an agent…</option>
          {#each knownAgents as a (a)}<option value={a}>{a}</option>{/each}
        </select>
        <button class="btn sm primary" type="submit" disabled={!dmTarget}>Open</button>
      </form>
    {/if}

    {#each dms as r (r.id)}
      <button class="room dm" class:on={r.id === selectedId} onclick={() => pick(r.id)} style="--tint:{roomTint(r)}">
        <span class="dmDot"></span>
        <span class="rname">{roomLabel(r)}</span>
        {#if r.last_message}<span class="rprev">{memberDisplay(r.last_message.author_member)}: {r.last_message.body}</span>{/if}
      </button>
    {/each}
    {#if !dms.length}<p class="none">No DMs yet</p>{/if}
  </aside>

  <section class="main">
    {#if selected}
      <header class="mhead">
        <div>
          <h2>{selected.kind === "room" ? `#${selected.name}` : roomLabel(selected)}</h2>
          {#if selected.description}<p class="desc">{selected.description}</p>{/if}
        </div>
        <span class="members" title="Members">
          {selected.kind === "dm" ? "DM" : `${shared.length ? "shared" : ""} room`}
        </span>
      </header>

      <div class="messages" bind:this={scroller}>
        {#each messages as m (m.id)}
          <div class="msg" class:me={m.author_member === `human:${store.me?.login?.toLowerCase()}`}>
            <span class="who" style="--tint:{m.author_kind === 'agent' ? agentColor(m.author_display) : 'var(--accent)'}">
              {m.author_display}
              {#if m.author_kind === "human"}<span class="tag">human</span>{/if}
            </span>
            <div class="content">
              {#if m.body}<span class="body">{m.body}</span>{/if}
              {#if m.attachments.length}
                <div class="atts">
                  {#each m.attachments as a (a.id)}
                    {#if isImage(a.mime)}
                      <a class="img" href={`/api/chat/uploads/${a.id}`} target="_blank" rel="noreferrer" title={a.filename}>
                        <img src={`/api/chat/uploads/${a.id}`} alt={a.filename} loading="lazy" />
                      </a>
                    {:else}
                      <a class="file" href={`/api/chat/uploads/${a.id}`} target="_blank" rel="noreferrer" title={a.filename}>
                        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 1.8h5.2L13 5.6v8.6H4z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /><path d="M9 1.8v4h4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
                        <span class="fname">{a.filename}</span>
                        <span class="fsize">{humanSize(a.size)}</span>
                      </a>
                    {/if}
                  {/each}
                </div>
              {/if}
            </div>
            <span class="at">{when(m.created_at)}</span>
          </div>
        {:else}
          <p class="empty">
            {selected.kind === "room" ? "Say something to the room." : "Start the DM."}
          </p>
        {/each}
      </div>

      {#if pending.length || uploading}
        <div class="pending">
          {#each pending as p (p.id)}
            <span class="pchip">
              {#if isImage(p.mime)}
                <img src={`/api/chat/uploads/${p.id}`} alt="" />
              {:else}
                <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 1.8h5.2L13 5.6v8.6H4z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /><path d="M9 1.8v4h4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
              {/if}
              <span class="pname">{p.filename}</span>
              <button type="button" aria-label="Remove {p.filename}" onclick={() => dropPending(p.id)}>×</button>
            </span>
          {/each}
          {#if uploading}<span class="pupload">uploading…</span>{/if}
        </div>
      {/if}
      <form class="composer" onsubmit={(e) => { e.preventDefault(); void send(); }}>
        <input
          class="fileinput"
          type="file"
          multiple
          bind:this={fileEl}
          onchange={onFiles}
          aria-label="Attach files"
        />
        <button
          class="btn ghost attach"
          type="button"
          title="Attach files (max 10 MB each)"
          aria-label="Attach files"
          onclick={() => fileEl?.click()}
        >+</button>
        <input
          class="field"
          placeholder={selected.kind === "room" ? `Message #${selected.name}` : "Message the DM"}
          bind:value={draft}
          aria-label="Message"
          autocomplete="off"
        />
        <button class="btn primary" type="submit" disabled={sending || (!draft.trim() && !pending.length)}>Send</button>
      </form>
    {:else}
      <p class="empty">Pick a room.</p>
    {/if}
  </section>
</div>

<style>
  .chat {
    height: 100%;
    display: grid;
    grid-template-columns: 236px minmax(0, 1fr);
    min-height: 0;
  }

  .rooms {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px;
    border-right: 1px solid var(--line);
    background: var(--sunk);
    overflow-y: auto;
    min-height: 0;
  }
  .rooms header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px 6px;
  }
  .dmhead {
    margin-top: 8px;
  }
  .sh {
    font: 500 10px/1 var(--font-mono);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink-3);
    flex: 1;
  }
  .mini {
    width: 20px;
    height: 20px;
    border: 0;
    border-radius: 5px;
    background: none;
    color: var(--ink-3);
    font-size: 15px;
    line-height: 1;
  }
  .mini:hover {
    background: var(--raise);
    color: var(--ink);
  }
  .newroom {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 6px;
    margin-bottom: 4px;
    border: 1px dashed var(--line-2);
    border-radius: 8px;
  }
  .newroom .field {
    width: 100%;
    font-size: 12px;
    padding: 5px 7px;
  }
  .room {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    padding: 7px 8px;
    border: 0;
    border-radius: 7px;
    background: none;
    text-align: left;
    min-width: 0;
  }
  .room:hover {
    background: color-mix(in srgb, var(--ink) 4.5%, transparent);
  }
  .room.on {
    background: var(--raise);
    box-shadow: inset 2px 0 0 var(--tint, var(--accent));
  }
  .rname {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12.5px;
    font-weight: 550;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hash {
    color: var(--ink-3);
    font: 500 11px/1 var(--font-mono);
  }
  .dmDot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--tint, var(--ink-3));
    display: none;
  }
  .room.dm .rname {
    padding-left: 2px;
  }
  .rprev {
    font: 400 10.5px/1.3 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .none {
    margin: 4px 8px;
    font-size: 11.5px;
    color: var(--ink-3);
  }

  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .mhead {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
  }
  .mhead h2 {
    margin: 0;
    font: 650 16px/1.2 var(--font-display);
  }
  .desc {
    margin: 3px 0 0;
    font-size: 12px;
    color: var(--ink-3);
  }
  .members {
    margin-left: auto;
    font: 400 10px/1 var(--font-mono);
    color: var(--ink-3);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .messages {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .msg {
    display: grid;
    grid-template-columns: 130px minmax(0, 1fr) auto;
    gap: 10px;
    align-items: baseline;
    padding: 5px 8px;
    border-radius: 7px;
  }
  .msg:hover {
    background: color-mix(in srgb, var(--ink) 3%, transparent);
  }
  .who {
    font: 600 11px/1.4 var(--font-mono);
    color: var(--tint, var(--ink-dim));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tag {
    margin-left: 5px;
    font: 500 8.5px/1 var(--font-mono);
    color: var(--ink-3);
    border: 1px solid var(--line-2);
    border-radius: 3px;
    padding: 1px 3px;
    text-transform: uppercase;
  }
  .content {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .atts {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
  }
  .img {
    display: block;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid var(--line);
    line-height: 0;
  }
  .img img {
    max-width: 260px;
    max-height: 200px;
    object-fit: cover;
    display: block;
  }
  .file {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 320px;
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--raise);
    color: var(--ink-dim);
    text-decoration: none;
    font: 400 11px/1.3 var(--font-mono);
  }
  .file:hover {
    border-color: var(--line-2);
    color: var(--ink);
  }
  .file svg {
    width: 13px;
    height: 13px;
    flex: none;
  }
  .fname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fsize {
    color: var(--ink-3);
    flex: none;
  }
  .fileinput {
    display: none;
  }
  .attach {
    width: 30px;
    padding: 0;
    font-size: 16px;
    flex: none;
  }
  .pending {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 8px 16px 0;
  }
  .pchip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 220px;
    padding: 3px 6px 3px 4px;
    border: 1px solid var(--line-2);
    border-radius: 6px;
    background: var(--raise);
    font: 400 10.5px/1.2 var(--font-mono);
    color: var(--ink-dim);
  }
  .pchip img {
    width: 18px;
    height: 18px;
    border-radius: 4px;
    object-fit: cover;
  }
  .pchip svg {
    width: 12px;
    height: 12px;
  }
  .pname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pchip button {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 13px;
    line-height: 1;
    padding: 0;
  }
  .pchip button:hover {
    color: var(--danger);
  }
  .pupload {
    font: 400 10.5px/1 var(--font-mono);
    color: var(--ink-3);
  }
  .body {
    font-size: 13px;
    line-height: 1.5;
    color: var(--ink);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .at {
    font: 400 9.5px/1 var(--font-mono);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .empty {
    margin: auto;
    color: var(--ink-3);
    font-size: 12.5px;
  }
  .composer {
    display: flex;
    gap: 8px;
    padding: 10px 16px 14px;
    border-top: 1px solid var(--line);
  }
  .composer .field {
    flex: 1;
  }
  .btn.sm {
    height: 26px;
    padding: 0 9px;
    font-size: 11.5px;
  }

  @media (max-width: 860px) {
    .chat {
      grid-template-columns: 150px minmax(0, 1fr);
    }
    .msg {
      grid-template-columns: minmax(0, 1fr) auto;
    }
    .who {
      grid-column: 1 / -1;
    }
  }
</style>
