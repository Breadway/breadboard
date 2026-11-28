# Approvals page

Top-level `Approvals` view (`store.view === "approvals"`). Agents open asks
through MCP or `POST /api/approvals`; the page is the operator's side.

## Layout

- Page head: title, one-line behaviour note, and a segmented control for
  `Inbox` and `Maintenance`. The inbox badge counts `pending` asks, the same
  number as the sidebar badge.
- Inbox: a 372px list and the detail pane. The list holds the mockup filter
  chips (`Needs you`, `All`, `Approvals`, `Maintenance`, `Info`,
  `Changes requested`, `Answered`), a debounced search, and project, agent,
  urgency and age selects. Filtering is server-side except age, which trims the
  page client-side.
- Detail: header (agent, age, tags, revision), status line, top decision
  buttons, meta strip, resolution banner, sanitized markdown, screenshots,
  mermaid, the window editor, checklist, risk and rollback, the conversation
  and the decision composer. A decided ask keeps its record but loses the
  composer and quick actions.
- Under 980px the list and detail swap; a back button returns to the list.

## Decisions and revisions

Approve, decline, request changes and send info post to
`/api/approvals/{id}/decision`. `Request changes` hides the ask from Needs you
and leaves it in `Changes requested`; the detail keeps the hidden-state banner.
The owning agent resubmits the same ask as a new revision, which returns it to
Needs you with the full thread and a `revision N` marker. `Send info` closes the
ask as `answered`. All four rules live on the server; the page only reflects
them.

## Markdown and mermaid

Bodies render through `lib/markdown.ts` (marked + DOMPurify, no raw HTML).
Fenced ` ```mermaid ` blocks are lifted out of the markdown and rendered by
`Mermaid.svelte`, which imports `mermaid` lazily so the inbox does not pay for
it unless a diagram exists. `securityLevel: "strict"` keeps labels sanitized
and disables diagram click handlers. A rendered diagram is a card; clicking it
opens a lightbox. Screenshots are `<img>` loads of the authenticated
`/api/artifacts/{id}` route and open in their own lightbox.

## Window editor (`ApprovalWindowEditor.svelte`)

A focused 12 hour timeline whose range follows the proposed window (one hour
before the start, clamped to the day). The block drags as a whole, its left and
right edges resize it, and every change snaps to 15 minutes. Day chips move the
proposal to another day in the same week. `Fine-tune times` exposes hour and
minute steppers plus 1h to 6h duration presets; all of them read and write the
same start/end pair, so the bar, labels, steppers and chips stay in sync.
Arrow keys move the block by 15 minutes (1 hour with Shift). The two action
buttons send the edited slot with the decision: `Approve as proposed` approves
it, `Send revised window` sends the ask back with the new slot.

## Maintenance calendar (`MaintenanceCalendar.svelte`)

A Monday to Sunday week, 24 hours per day at 38px per hour. Approved and
proposed windows render with their status, agent and range; a freeze covers the
whole day with a hatched overlay. An outline marks a window that overlaps
another window or a freeze. Conflicts never block: they are visual only, as the
operator asked. Proposed windows drag (and their top and bottom edges resize)
to a new day or slot. On drop the window popup opens with services, reason,
risk, rollback, the full runbook and status; `Approve this window` decides the
ask, and `Send revised slot` sends a changes request carrying the dragged
window. Approved windows are not draggable.

## Tests

`apps/web/tests/approvals.browser.mjs` runs against a scratch server with a
real session cookie and covers all four decision paths, the changes-requested
hidden state, revision return, sanitized markdown, mermaid, both lightboxes,
the editor (drag, edge resize, day chip, steppers, presets, keyboard) and the
calendar (drag and popup). Run it with:

```
BREADBOARD_TEST_URL=http://127.0.0.1:7290 BREADBOARD_TEST_SESSION=<cookie> \
  node apps/web/tests/approvals.browser.mjs
```
