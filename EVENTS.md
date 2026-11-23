# board: bread event integration

breadboard is a self-hosted issue tracker: it works exactly the same with or
without `breadd` running. When breadd *is* present and the
`notifications.bread_events` setting is enabled, the server mirrors each
`BoardEvent` onto the shared bread automation fabric by shelling out to
`bread-emit`. See the parent `bread` repo's `Documentation.md`, specifically
its "Namespaces" and "Integrating a bread\* app" sections, for the general
convention this follows.

App id: **`board`**. Transport: `bread-emit`, spawned fire-and-forget for
each event. The same `BoardEvent` values also drive the server's SSE stream;
the bread names are the `bread.board.*` mapping in
`crates/breadboard-core/src/event.rs`.

## Events published (`bread.board.*`)

| Event | Data | When |
|-------|------|------|
| `bread.board.card.created` | `{ "id": <integer>, "key": "<issue key>" }` | A card is created. |
| `bread.board.card.updated` | `{ "id": <integer>, "key": "<issue key>" }` | A card's fields change. |
| `bread.board.card.moved` | `{ "id": <integer>, "key": "<issue key>", "from": "<status>", "to": "<status>" }` | A card changes status column. |
| `bread.board.card.assigned` | `{ "id": <integer>, "key": "<issue key>", "assignee_id": <integer or null> }` | A card is assigned or unassigned. |
| `bread.board.card.deleted` | `{ "id": <integer>, "key": "<issue key>" }` | A card is deleted. |
| `bread.board.cycle.started` | `{ "id": <integer>, "name": "<name>", "issues": <integer> }` | A cycle starts. `issues` is the number of issues included. |
| `bread.board.cycle.ended` | `{ "id": <integer>, "name": "<name>" }` | A cycle ends. |
| `bread.board.sync.drift` | `{ "issue_id": <integer>, "key": "<issue key>", "field": "<field>", "breadboard_value": "<value>", "forgejo_value": "<value>", "winner": "breadboard" \| "forgejo", "at": "<timestamp>" }` | A reconcile pass finds a field where Forgejo and breadboard disagree. When Forgejo wins, the value is also applied. |
| `bread.board.notified` | `{ "user_id": <integer> }` | A user receives a new in-app notification. Clients filter by their own id. |
| `bread.board.agent.claimed` | `{ "id": <integer>, "key": "<issue key>", "agent": "<name>", "mode": "<mode>", "session_id": <string or null>, "state": "<state>" }` | An agent claims an issue exclusively. |
| `bread.board.agent.updated` | `{ "id": <integer>, "key": "<issue key>", "agent": "<name>", "mode": "<mode>", "session_id": <string or null>, "state": "<state>", "claim": <AgentClaim object> }` | A claim heartbeat or field update. `claim` is the full claim so clients can patch one entry instead of refetching. |
| `bread.board.agent.released` | `{ "id": <integer>, "key": "<issue key>" }` | A claim is released and the card returns to its normal colour. |
| `bread.board.agent.removed` | `{ "agent": "<name>", "removed_by": <integer or null>, "claims_released": <integer> }` | An agent is removed, its claims released, and its roster, profile, hidden flag and chat state deleted. |
| `bread.board.agent.paused` | `{ "agent": "<name>", "paused_by": <integer or null> }` | An agent is paused: claims are kept, but new claims are refused and chat pushes no longer wake it. |
| `bread.board.agent.resumed` | `{ "agent": "<name>", "resumed_by": <integer or null> }` | A paused agent is resumed. |
| `bread.board.agent.hidden` | `{ "agent": "<name>", "hidden_by": <integer or null> }` | An agent is hidden from the default Agents view. Claims and chat identity are untouched. |
| `bread.board.agent.unhidden` | `{ "agent": "<name>", "unhidden_by": <integer or null> }` | A hidden agent is revealed again. |
| `bread.board.agent.orchestrator` | `{ "agent": "<name>", "previous": <string or null>, "orchestrator": <string or null>, "changed_by": <integer or null> }` | A human sets or clears an agent's orchestrator. Both sides are carried so a client can re-render the hierarchy. |
| `bread.board.chat.message` | `{ "room_id": <integer>, "id": <integer> }` | A chat message is posted to a room or DM. |
| `bread.board.chat.room.deleted` | `{ "id": <integer> }` | A shared room or DM is deleted; clients refetch the room list. |
| `bread.board.review.updated` | `{ "id": <integer>, "issue_id": <integer>, "key": "<issue key>", "reviewer": "<name>", "state": "<state>", "escalated": bool }` | A review request is created, changes state, or escalates. |

## Commands honored (`bread.command.board.*`)

None. breadboard does not subscribe to the command bus; it is a one-way event
publisher.

## Not implemented

- No `bread.command.board.*` verb exists. Board actions stay on the REST API,
  web UI, CLI, and TUI; there is no remote-triggered command surface on the
  bus.
- Not every REST mutation has a matching bus event. Only the `BoardEvent`
  variants listed above are mirrored.
- No event reports authentication, login, server health, SSE reconnects, or
  raw sync-log rows. Those stay in the server's own logs and API.
- Payloads name ids and keys only. They do not carry issue bodies, comments,
  chat message text, tokens, or webhook secrets.

## Fail-safe behavior

- If `bread-emit` is not on `PATH`, or breadd is not running, the spawned
  helper exits without affecting the request path. breadboard still serves
  REST and SSE normally.
- The helper is spawned and never waited on, so a missing or slow bus cannot
  add latency to an API request.
- If `notifications.bread_events` is disabled, no helper is spawned at all.
  A headless deployment that has no bread bus simply leaves it off.
- A failed publish is not retried. SSE remains the authoritative live stream
  for connected clients; bread is a best-effort mirror.
