<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api, errMsg, store } from "./store.svelte";
  import type { ChatMention, ChatMessage, ChatRoom, ChatUpload } from "@breadboard/api-client";
  import { agentColor } from "./ui/colors";

  let rooms = $state<ChatRoom[]>([]);
  let selectedId = $state<number | null>(null);
  let messages = $state<ChatMessage[]>([]);
  let draft = $state("");
  let sending = $state(false);
  let deleting = $state<number | null>(null);
  let compacting = $state(false);
  let deletingRoom = $state<number | null>(null);
  let mentions = $state<ChatMention[]>([]);
  let inputEl: HTMLInputElement | undefined = $state();
  let mentionState = $state<{ start: number; query: string } | null>(null);
  let mentionIndex = $state(0);
  let pending = $state<ChatUpload[]>([]);
  let uploading = $state(false);
  let fileEl: HTMLInputElement | undefined = $state();
  let newRoom = $state(false);
  let roomName = $state("");
  let roomDesc = $state("");
  let dmPicker = $state(false);
  let dmTarget = $state("");
  let scroller: HTMLDivElement | undefined = $state();
  let subs = $state<number[]>([]);
  let narrow = $state(false);
  let roomsOpen = $state(true);

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
  const mentionItems = $derived.by(() => {
    const current = mentionState;
    if (!current || !selected) return [];
    const list: ChatMention[] = [];
    if (selected.kind === "room") {
      list.push({ name: "everyone", slug: "everyone", kind: "room" });
    }
    list.push(...mentions);
    return list.filter((m) => m.slug.startsWith(current.query)).slice(0, 8);
  });

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

  async function deleteMessage(message: ChatMessage) {
    if (deleting != null) return;
    if (!confirm("Delete this message? It disappears for everyone.")) return;
    deleting = message.id;
    try {
      await api.deleteChatMessage(message.id);
      messages = messages.filter((m) => m.id !== message.id);
      void loadRooms();
      store.info("Message deleted");
    } catch (e) {
      store.flash(`Couldn't delete: ${errMsg(e)}`);
    } finally {
      deleting = null;
    }
  }

  async function compactRoom() {
    if (selectedId == null || compacting) return;
    if (!confirm("Compact this room? The model will summarize the visible messages and hide them.")) return;
    compacting = true;
    store.info("Compacting room, this can take a minute...");
    try {
      await api.compactChatRoom(selectedId);
      await loadMessages(selectedId);
      await loadRooms();
      store.info("Room compacted");
    } catch (e) {
      store.flash(`Couldn't compact: ${errMsg(e)}`);
    } finally {
      compacting = false;
    }
  }

  function mentionParts(body: string) {
    const out: Array<{ text: string; mention: boolean; everyone: boolean }> = [];
    const re = /@([A-Za-z0-9_-]+)/g;
    let last = 0;
    let match: RegExpExecArray | null;
    while ((match = re.exec(body))) {
      if (match.index > last) {
        out.push({ text: body.slice(last, match.index), mention: false, everyone: false });
      }
      out.push({
        text: match[0],
        mention: true,
        everyone: match[1].toLowerCase() === "everyone",
      });
      last = match.index + match[0].length;
    }
    if (last < body.length) {
      out.push({ text: body.slice(last), mention: false, everyone: false });
    }
    return out;
  }

  async function loadMentions() {
    try {
      mentions = await api.chatMentions();
    } catch {
      /* autocomplete is optional */
    }
  }

  function updateMention() {
    if (!inputEl) return;
    const caret = inputEl.selectionStart ?? draft.length;
    const before = draft.slice(0, caret);
    const at = before.lastIndexOf("@");
    if (at === -1) {
      mentionState = null;
      return;
    }
    const query = before.slice(at + 1);
    if (!/^[A-Za-z0-9_-]*$/.test(query)) {
      mentionState = null;
      return;
    }
    if (at > 0 && !/\s/.test(before[at - 1])) {
      mentionState = null;
      return;
    }
    mentionState = { start: at, query: query.toLowerCase() };
    mentionIndex = 0;
  }

  function chooseMention(item: ChatMention) {
    if (!mentionState) return;
    const caret = inputEl?.selectionStart ?? draft.length;
    const next =
      draft.slice(0, mentionState.start) + "@" + item.slug + " " + draft.slice(caret);
    const position = mentionState.start + item.slug.length + 2;
    draft = next;
    mentionState = null;
    void tick().then(() => {
      inputEl?.focus();
      inputEl?.setSelectionRange(position, position);
    });
  }

  function onComposerKey(event: KeyboardEvent) {
    if (!mentionState || !mentionItems.length) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      mentionIndex = (mentionIndex + 1) % mentionItems.length;
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      mentionIndex = (mentionIndex - 1 + mentionItems.length) % mentionItems.length;
    } else if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      const item = mentionItems[Math.min(mentionIndex, mentionItems.length - 1)];
      if (item) chooseMention(item);
    } else if (event.key === "Escape") {
      mentionState = null;
    }
  }

  async function deleteRoom() {
    if (!selected || deletingRoom != null) return;
    const target = selected;
    if (target.kind === "room" && target.name === "lobby") return;
    const label = target.kind === "dm" ? "this DM for both members" : `#${target.name} for everyone`;
    if (!confirm(`Delete ${label}? The messages are removed too.`)) return;
    deletingRoom = target.id;
    try {
      await api.deleteChatRoom(target.id);
      await loadRooms();
      store.info("Room deleted");
    } catch (e) {
      store.flash(`Couldn't delete room: ${errMsg(e)}`);
    } finally {
      deletingRoom = null;
    }
  }

  async function loadSubs() {
    try {
      subs = await api.getChatPushSubs();
    } catch {
      /* best-effort */
    }
  }

  async function togglePush(room: ChatRoom) {
    const enabled = !subs.includes(room.id);
    try {
      await api.setChatPushSub(String(room.id), enabled);
      await loadSubs();
      store.info(enabled ? `Push on for #${room.name}` : `Push off for #${room.name}`);
    } catch (e) {
      store.flash(`Couldn't change push: ${errMsg(e)}`);
    }
  }

  async function loadRooms(selectFirst = false) {
    try {
      const next = await api.listChatRooms();
      rooms = next;
      store.chatUnread = next.reduce((n, r) => n + r.unread, 0);
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
    if (narrow) roomsOpen = false;
    void loadMessages(id).then(() => loadRooms());
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
      mentionState = null;
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
    const mq = window.matchMedia("(max-width: 860px)");
    const apply = () => {
      narrow = mq.matches;
      if (!narrow) roomsOpen = true;
    };
    apply();
    mq.addEventListener("change", apply);
    void loadSubs();
    void loadRooms(true).then(() => loadMessages(selectedId));
    const t = setInterval(() => {
      void loadRooms();
      void loadMessages(selectedId);
    }, 5_000);
    return () => {
      clearInterval(t);
      mq.removeEventListener("change", apply);
    };
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

<div class="chat" class:narrow class:rooms-open={roomsOpen}>
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
        {#if r.unread > 0}<span class="unread" title="{r.unread} unread">{r.unread > 99 ? "99+" : r.unread}</span>{/if}
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
        {#if r.unread > 0}<span class="unread" title="{r.unread} unread">{r.unread > 99 ? "99+" : r.unread}</span>{/if}
        {#if r.last_message}<span class="rprev">{memberDisplay(r.last_message.author_member)}: {r.last_message.body}</span>{/if}
      </button>
    {/each}
    {#if !dms.length}<p class="none">No DMs yet</p>{/if}
  </aside>

  <section class="main">
    {#if selected}
      <header class="mhead">
        {#if narrow}
          <button class="back" aria-label="Back to rooms" onclick={() => (roomsOpen = true)}>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m10 3-5 5 5 5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
          </button>
        {/if}
        <div>
          <h2>{selected.kind === "room" ? `#${selected.name}` : roomLabel(selected)}</h2>
          {#if selected.description}<p class="desc">{selected.description}</p>{/if}
        </div>
        {#if selected.kind === "room"}
          <button class="compact" disabled={compacting} onclick={() => void compactRoom()}>
            {compacting ? "compacting..." : "compact"}
          </button>
          <button
            class="push"
            class:on={subs.includes(selected.id)}
            title={subs.includes(selected.id) ? "Push notifications on for this room" : "Turn on push for this room"}
            aria-pressed={subs.includes(selected.id)}
            onclick={() => void togglePush(selected)}
          >
            <svg viewBox="0 0 16 16" aria-hidden="true">
              <path d="M8 2.2a4 4 0 0 1 4 4v3l1.2 2H2.8L4 9.2v-3a4 4 0 0 1 4-4zM6.4 12.4a1.6 1.6 0 0 0 3.2 0" fill={subs.includes(selected.id) ? "currentColor" : "none"} stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" />
            </svg>
            {subs.includes(selected.id) ? "push on" : "push off"}
          </button>
        {/if}
        {#if !(selected.kind === "room" && selected.name === "lobby")}
          <button
            class="danger"
            disabled={deletingRoom === selected.id}
            onclick={() => void deleteRoom()}
          >
            {deletingRoom === selected.id ? "deleting..." : "delete"}
          </button>
        {/if}
        <span class="members" title="Members">
          {selected.kind === "dm" ? "DM" : `${shared.length ? "shared" : ""} room`}
        </span>
      </header>

      <div class="messages" bind:this={scroller}>
        {#each messages as m (m.id)}
          {#if m.message_kind === "summary"}
            <div class="summary">
              <div class="stitle">
                <span>Compacted summary</span>
                <span>{when(m.created_at)}</span>
                <button
                  class="msgdel"
                  title="Delete summary"
                  aria-label="Delete summary"
                  disabled={deleting === m.id}
                  onclick={() => void deleteMessage(m)}
                >delete</button>
              </div>
              <div class="sbody">{m.body}</div>
            </div>
          {:else}
            <div class="msg" class:me={m.author_member === `human:${store.me?.login?.toLowerCase()}`}>
              <span class="who" style="--tint:{m.author_kind === 'agent' ? agentColor(m.author_display) : 'var(--accent)'}">
                {m.author_display}
                {#if m.author_kind === "human"}<span class="tag">human</span>{/if}
              </span>
              <div class="content">
                {#if m.body}
                  <span class="body">
                    {#each mentionParts(m.body) as part}
                      {#if part.mention}
                        <span class="mention" class:everyone={part.everyone}>{part.text}</span>
                      {:else}{part.text}{/if}
                    {/each}
                  </span>
                {/if}
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
              <span class="at">
                {when(m.created_at)}
                <button
                  class="msgdel"
                  title="Delete message"
                  aria-label="Delete message"
                  disabled={deleting === m.id}
                  onclick={() => void deleteMessage(m)}
                >delete</button>
              </span>
            </div>
          {/if}
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
        <div class="fieldwrap">
          <input
            class="field"
            placeholder={selected.kind === "room" ? `Message #${selected.name}` : "Message the DM"}
            bind:value={draft}
            bind:this={inputEl}
            oninput={updateMention}
            onkeydown={onComposerKey}
            onblur={() => setTimeout(() => (mentionState = null), 150)}
            aria-label="Message"
            autocomplete="off"
          />
          {#if mentionState && mentionItems.length}
            <div class="mentions" role="listbox" aria-label="Mention suggestions">
              {#each mentionItems as item, i}
                <button
                  type="button"
                  class:on={i === mentionIndex}
                  onmousedown={(e) => { e.preventDefault(); chooseMention(item); }}
                >
                  <span class="mat">@{item.slug}</span>
                  <span class="mkind">{item.kind}</span>
                </button>
              {/each}
            </div>
          {/if}
        </div>
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
    flex: 1;
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
  .unread {
    margin-left: auto;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 99px;
    background: var(--accent);
    color: var(--on-accent);
    font: 600 9.5px/16px var(--font-mono);
    text-align: center;
    flex: none;
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
  .push {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid var(--line-2);
    background: none;
    color: var(--ink-3);
    border-radius: 6px;
    padding: 4px 8px;
    font: 500 10px/1 var(--font-mono);
  }
  .push:hover {
    color: var(--ink-dim);
  }
  .push.on {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: var(--accent-soft);
  }
  .push svg {
    width: 12px;
    height: 12px;
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

  .back {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border: 0;
    border-radius: 7px;
    background: none;
    color: var(--ink-dim);
    padding: 0;
  }
  .back:hover {
    background: var(--raise);
    color: var(--ink);
  }
  .back svg {
    width: 16px;
    height: 16px;
  }

  .compact {
    height: 24px;
    padding: 0 8px;
    border: 1px solid var(--line-2);
    border-radius: 6px;
    background: none;
    color: var(--ink-3);
    font: 500 10.5px/1 var(--font-mono);
  }
  .compact:hover:not(:disabled) {
    color: var(--accent);
    border-color: var(--accent);
  }
  .compact:disabled {
    opacity: 0.6;
  }
  .msgdel {
    margin-left: 6px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--ink-3);
    font: 400 9.5px/1 var(--font-mono);
    opacity: 0;
  }
  .msg:hover .msgdel,
  .summary:hover .msgdel {
    opacity: 1;
  }
  .msgdel:hover {
    color: var(--danger);
  }
  .summary {
    margin: 10px 8px;
    padding: 10px 12px;
    border: 1px dashed var(--line-2);
    border-radius: 8px;
    background: var(--sunk);
  }
  .stitle {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font: 600 9.5px/1.4 var(--font-mono);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .stitle span:nth-child(2) {
    margin-left: auto;
    letter-spacing: 0;
    text-transform: none;
  }
  .sbody {
    margin-top: 6px;
    white-space: pre-wrap;
    font-size: 12.5px;
    line-height: 1.55;
    color: var(--ink-2);
  }
  .fieldwrap {
    position: relative;
    flex: 1;
    display: flex;
    min-width: 0;
  }
  .fieldwrap .field {
    flex: 1;
  }
  .mentions {
    position: absolute;
    left: 0;
    right: 0;
    bottom: calc(100% + 6px);
    z-index: 20;
    padding: 4px;
    max-height: 220px;
    overflow-y: auto;
    background: var(--surface);
    border: 1px solid var(--line-2);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
  }
  .mentions button {
    display: flex;
    width: 100%;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 8px;
    border: 0;
    border-radius: 5px;
    background: none;
    color: var(--ink-2);
    font-size: 12px;
    text-align: left;
  }
  .mentions button.on,
  .mentions button:hover {
    background: var(--raise);
    color: var(--ink);
  }
  .mat {
    font-family: var(--font-mono);
    color: var(--accent);
  }
  .mkind {
    font-size: 9px;
    text-transform: uppercase;
    color: var(--ink-3);
  }
  .mention {
    padding: 0 3px;
    border-radius: 4px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    font-weight: 600;
  }
  .mention.everyone {
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 14%, transparent);
  }
  .danger {
    height: 24px;
    padding: 0 8px;
    border: 1px solid var(--line-2);
    border-radius: 6px;
    background: none;
    color: var(--ink-3);
    font: 500 10.5px/1 var(--font-mono);
  }
  .danger:hover:not(:disabled) {
    color: var(--danger);
    border-color: var(--danger);
  }
  .danger:disabled {
    opacity: 0.6;
  }
  @media (max-width: 860px) {
    .msgdel {
      opacity: 1;
    }
    .chat.narrow {
      grid-template-columns: minmax(0, 1fr);
    }
    .chat.narrow .rooms,
    .chat.narrow .main {
      grid-column: 1;
      grid-row: 1;
    }
    .chat.narrow:not(.rooms-open) .rooms {
      display: none;
    }
    .chat.narrow.rooms-open .main {
      display: none;
    }
    .rooms {
      border-right: 0;
      padding: 12px 14px calc(12px + env(safe-area-inset-bottom, 0px));
    }
    .room {
      padding: 11px 10px;
    }
    .rname {
      font-size: 14px;
    }
    .rprev {
      font-size: 11.5px;
    }
    .mhead {
      padding: 10px 12px;
    }
    .messages {
      padding: 12px 12px;
    }
    .msg {
      grid-template-columns: minmax(0, 1fr) auto;
      gap: 4px 8px;
      padding: 7px 6px;
    }
    .who {
      grid-column: 1 / -1;
    }
    .composer {
      padding: 8px 12px calc(10px + env(safe-area-inset-bottom, 0px));
    }
    .img img {
      max-width: min(260px, 70vw);
    }
  }
</style>
