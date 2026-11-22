import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Static regression check for BB-230: the Agents page hides stale cards by
// default, exposes a counted "Show stale" toggle, keeps live cards visible and
// first, and reports live/stale/abandoned/hidden in the header.
//
// The web UI has no unit runner (only live-server playwright checks), and a
// stale fixture would need a 120-second-old heartbeat that the REST API cannot
// fabricate, so this asserts the source-level contract instead.
//
// Run: node apps/web/tests/agents-stale.static.mjs (from apps/web)

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const agents = readFileSync(join(root, "src", "lib", "Agents.svelte"), "utf8");

assert.match(
  agents,
  /let showStale = \$state\(false\);/,
  "stale cards are hidden by default",
);
assert.ok(
  agents.includes('if (g.presence === "stale" && !showStale && g.name !== focus) continue;'),
  "the default view filters stale roster presence unless the toggle is on",
);
assert.ok(
  agents.includes('if (g.presence === "stale" && !showStale && g.name !== focus) return false;'),
  "stale cards cannot render while hidden by default",
);
assert.ok(
  agents.includes('const staleGroups = $derived(allGroups.filter((g) => g.presence === "stale"));'),
  "the toggle count derives from roster presence, not claim heartbeat",
);
assert.ok(
  agents.includes('{showStale ? "Showing stale" : "Show stale"} <span>{staleAgentCount}</span>'),
  "the toggle is labelled Show stale with its count",
);
assert.ok(
  agents.includes("aria-pressed={showStale}"),
  "the stale toggle exposes its pressed state",
);
assert.ok(
  agents.includes('<span>stale</span>') && agents.includes("<b>{staleAgentCount}</b>"),
  "the header reports a stale count",
);
assert.ok(
  agents.includes('<span>live</span>') &&
    agents.includes('<span>abandoned</span>') &&
    agents.includes('<span>hidden</span>'),
  "the header reports live, abandoned and hidden beside stale",
);
assert.ok(
  agents.includes('g.paused ? 2 : g.presence === "live" ? 0 : 1'),
  "live cards sort before stale cards, with paused last",
);
assert.ok(
  agents.includes("stale and hidden from the default view"),
  "the empty state explains that stale cards are hidden by default",
);

console.log("agents-stale static checks passed: 10 assertions over Agents.svelte");
