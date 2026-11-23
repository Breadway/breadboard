import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Static companion to agents-pinned.browser.mjs for BB-235: the Agents page
// must render pinned cards regardless of the stale/abandoned hide rules, show
// the pinner, expose a pin/unpin control, count pinned in the header, and sort
// pinned cards above unpinned ones with paused last inside each half.
//
// Run: node apps/web/tests/agents-pinned.static.mjs (from apps/web)

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const agents = readFileSync(join(root, "src", "lib", "Agents.svelte"), "utf8");
const store = readFileSync(join(root, "src", "lib", "store.svelte.ts"), "utf8");
const client = readFileSync(join(root, "..", "..", "packages", "api-client", "src", "index.ts"), "utf8");

assert.ok(agents.includes("pinned: boolean;"), "the group carries a pinned flag");
assert.ok(agents.includes("pinnedBy: string | null;"), "the group carries the pinner");
assert.ok(agents.includes("g.pinned = r.pinned;"), "roster pinned is copied into the group");
assert.ok(agents.includes("g.pinnedBy = r.pinned_by_login ?? null;"), "roster pinner is copied");
assert.ok(
  agents.includes("!staleVisible && !g.pinned && g.name !== focus"),
  "pinned cards bypass the stale and abandoned hide rules",
);
assert.ok(
  agents.includes("(g.pinned ? 0 : 3) + (g.paused ? 2 : g.presence === \"live\" ? 0 : 1)"),
  "pinned cards sort above unpinned, with paused last inside each half",
);
assert.ok(
  agents.includes('class="pinned-badge"'),
  "the card shows a pinned badge",
);
assert.ok(
  agents.includes("{g.pinned ? \"Unpin\" : \"Pin\"}"),
  "the card has a pin/unpin control",
);
assert.ok(
  agents.includes('<b>{pinnedCount}</b><span>pinned</span>'),
  "the header counts pinned agents",
);
assert.ok(
  agents.includes("const staleToggleCount = $derived(staleGroups.filter((g) => !g.pinned).length);"),
  "the stale toggle counts only the cards it can reveal",
);
assert.ok(
  agents.includes("Automatic roster cleanup never removes this row."),
  "the detail note explains that pin guards automatic cleanup only",
);
assert.ok(
  store.includes("async pinAgent(name: string)") && store.includes("async unpinAgent(name: string)"),
  "the store exposes pin and unpin",
);
assert.ok(
  client.includes("/pin`") && client.includes("/unpin`"),
  "the api client exposes both pin routes",
);

console.log("agents pinned static checks passed: 14 assertions");
