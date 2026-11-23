import assert from "node:assert/strict";
import { chromium } from "playwright";

// Behavioural regression for BB-235: a pinned agent stays on the Agents page
// even when it is stale or the next maintenance pass would call it abandoned,
// shows who pinned it, and sorts above unpinned cards. The unpinned stale row
// is still hidden by default, so the pin is doing the work.
//
// Run against a scratch server (the same harness the other Agents browser
// tests use):
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-pinned.browser.mjs
//
// Only the three agent list endpoints are intercepted, with deterministic
// fixtures. Board, auth and chat stay real, so this does not touch or depend
// on production data.

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const staleAt = new Date(Date.now() - 30 * 60 * 1000).toISOString();
const freshAt = new Date().toISOString();

function rosterRow(name, lastSeen, status, overrides = {}) {
  return {
    agent_name: name,
    session_id: null,
    session_title: null,
    host: null,
    pid: null,
    note: null,
    first_seen: lastSeen,
    last_seen: lastSeen,
    live: status === "live",
    paused: false,
    status,
    abandoned: false,
    abandoned_reason: null,
    message: null,
    state: "implementing",
    claim_count: 0,
    exclusive_count: 0,
    intent_count: 0,
    exclusive_limit: 10,
    orchestrator_exclusive_count: 0,
    orchestrator_exclusive_limit: 100,
    user_id: 1,
    login: "tester",
    paused_by: null,
    paused_by_login: null,
    paused_at: null,
    pinned: false,
    pinned_by: null,
    pinned_by_login: null,
    pinned_at: null,
    hidden: false,
    hidden_by: null,
    hidden_by_login: null,
    hidden_at: null,
    parent_session_id: null,
    parent_agent_name: null,
    orchestrator: null,
    orchestrator_session_id: null,
    ...overrides,
  };
}

async function installFixture(page, roster) {
  await page.route("**/api/agents**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/agents/status") {
      await route.fulfill({
        json: {
          total: 0,
          live: 0,
          stale: 0,
          queued: 0,
          done: 0,
          active: 0,
          review: 0,
          verifying: 0,
          blocked: 0,
          remaining: 0,
          line: "No active agent claims",
          claims: [],
        },
      });
      return;
    }
    if (path === "/api/agents/roster") {
      await route.fulfill({ json: roster });
      return;
    }
    if (path === "/api/agents") {
      await route.fulfill({ json: [] });
      return;
    }
    await route.continue();
  });
}

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  await installFixture(page, [
    rosterRow("pinned-stale", staleAt, "stale", {
      abandoned: true,
      abandoned_reason: "no claims, chat or reviews; idle 30h",
      pinned: true,
      pinned_by: 1,
      pinned_by_login: "tester",
      pinned_at: freshAt,
    }),
    rosterRow("unpinned-stale", staleAt, "stale"),
    rosterRow("unpinned-abandoned", staleAt, "stale", {
      abandoned: true,
      abandoned_reason: "no claims, chat or reviews; idle 30h",
    }),
    rosterRow("fresh-agent", freshAt, "live"),
  ]);

  await page.goto(base);
  await page.getByRole("button", { name: "Agents" }).click();
  await page.waitForSelector(".agent-card");

  const cards = () =>
    page.$$eval(".agent-card", (els) => els.map((el) => el.getAttribute("data-agent")));
  const shown = await cards();
  assert.ok(shown.includes("pinned-stale"), "a pinned stale/abandoned card renders by default");
  assert.ok(!shown.includes("unpinned-stale"), "an unpinned stale card stays hidden by default");
  assert.ok(
    !shown.includes("unpinned-abandoned"),
    "an unpinned abandoned card stays hidden by default",
  );
  assert.ok(shown.includes("fresh-agent"), "fresh cards still render");
  assert.equal(shown[0], "pinned-stale", "pinned cards sort above unpinned live cards");

  const pinnedCard = page.locator('[data-agent="pinned-stale"]');
  assert.equal(await pinnedCard.getAttribute("data-pinned"), "true");
  assert.match(await pinnedCard.innerText(), /pinned/i, "the card shows a pinned badge");
  const badge = pinnedCard.locator(".pinned-badge");
  assert.match(await badge.getAttribute("title"), /tester/, "the badge names the pinner");
  assert.equal(await pinnedCard.locator(".pin-card").innerText(), "Unpin");

  const header = await page.locator(".stats").innerText();
  assert.match(header, /1\s+pinned/i, "the header counts pinned agents");
  // The toggle counts only the two unpinned rows it can reveal, not the
  // pinned card already on screen.
  assert.match(await page.locator(".stale-toggle").innerText(), /Show stale\s*2/i);

  assert.equal(
    await page.locator('[data-agent="unpinned-stale"]').count(),
    0,
    "the unpinned stale card is not rendered",
  );

  // Show stale still reveals the unpinned rows, including the abandoned one.
  await page.locator(".stale-toggle").click();
  await page.waitForSelector('[data-agent="unpinned-stale"]');
  assert.ok((await cards()).includes("unpinned-stale"));
  assert.ok((await cards()).includes("unpinned-abandoned"));
  assert.ok((await cards()).includes("pinned-stale"), "the pinned card stays visible");
  await page.locator(".stale-toggle").click();

  // Show abandoned is independent and still reveals only abandoned rows; the
  // pinned abandoned card remains visible either way.
  await page.locator(".abandoned-toggle").click();
  await page.waitForSelector('[data-agent="unpinned-abandoned"]');
  assert.ok(!(await cards()).includes("unpinned-stale"), "plain stale stays hidden");
  assert.ok((await cards()).includes("pinned-stale"));

  console.log("agents pinned browser regression: passed");
} finally {
  await browser.close();
}
