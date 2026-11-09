import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Static regression check for BB-149: no silently truncated issue titles.
// The web UI has no unit runner (only live-server playwright checks), so this
// asserts the source-level contract instead: every view that renders an issue
// title either wraps it fully or clamps it with a full-text tooltip.
//
// Run: node apps/web/tests/title-wrap.static.mjs (from the repo root)

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");
const cliRoot = join(root, "..", "..", "crates", "breadboard-cli", "src");
const readCli = (p) => readFileSync(join(cliRoot, p), "utf8");

const card = read("src/lib/Card.svelte");
assert.ok(card.includes("<div class=\"title\" title={issue.title}>"), "Card title exposes full text via title attribute");
assert.ok(card.includes("-webkit-line-clamp: 3;"), "Card title clamps at 3 lines instead of growing unbounded");

const backlog = read("src/lib/Backlog.svelte");
assert.ok(backlog.includes("<span class=\"t\" title={issue.title}>"), "Backlog row exposes full title via tooltip");

const palette = read("src/lib/CommandPalette.svelte");
assert.ok(palette.includes("title={i?.title"), "Command palette exposes full issue title via tooltip");

const agents = read("src/lib/Agents.svelte");
assert.ok(agents.includes("title={c.issue_title}"), "Agents claim rows expose full issue title via tooltip");

const filters = read("src/lib/Filters.svelte");
assert.ok(filters.includes("title={e.title}"), "Epic filter exposes full epic title via tooltip");

const drawer = read("src/lib/IssueDrawer.svelte");
assert.ok(drawer.includes("class=\"rel-main\" title={t ?"), "Issue drawer relation rows expose key plus full title via tooltip");

const roadmap = read("src/lib/Roadmap.svelte");
assert.ok(roadmap.includes("title=\"{i.key}: {i.title}\""), "Roadmap mini cards keep full key plus title tooltip");

const tui = readCli("tui.rs");
assert.ok(tui.includes("Wrap { trim: false }"), "TUI detail popup wraps the full title");
assert.ok(tui.includes("its own line so long titles are not cut"), "TUI board renders titles on their own line");

console.log("title-wrap static checks passed: 8 assertions across Card, Backlog, CommandPalette, Agents, Filters, IssueDrawer, Roadmap, TUI");
