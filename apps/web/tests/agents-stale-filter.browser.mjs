import assert from "node:assert/strict";
import { chromium } from "playwright";

// Behavioural regression for BB-230: selecting the Stale claims segment must
// show stale-claim cards even though stale cards are hidden by default. Before
// the fix the hide-stale default filtered the cards before the segment filter
// ran, so the segment rendered nothing and the empty state claimed every claim
// was fresh.
//
// Run against a scratch server (the same harness the other Agents browser
// tests use):
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-stale-filter.browser.mjs
//
// Only the three agent list endpoints are intercepted, with a deterministic
// 30-minute-old claim. Board, auth and chat stay real, so this does not touch
// or depend on production data.

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const staleAt = new Date(Date.now() - 30 * 60 * 1000).toISOString();
const freshAt = new Date().toISOString();

function claim(agent, heartbeat) {
  return {
    id: 9001,
    issue_id: 9001,
    key: "STALE-1",
    issue_title: "A stale claim fixture",
    issue_status: "doing",
    claimed_by: 1,
    claimed_by_login: "tester",
    agent_name: agent,
    message: null,
    mode: "exclusive",
    session_id: null,
    state: "implementing",
    note: null,
    branch: null,
    worktree: null,
    repo: "Breadway/breadboard",
    pr_url: null,
    pr_number: null,
    head_sha: null,
    board_status: "doing",
    claimed_at: heartbeat,
    updated_at: heartbeat,
    last_heartbeat: heartbeat,
  };
}

function rosterRow(agent, lastSeen, status) {
  return {
    agent_name: agent,
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
    claim_count: 1,
    exclusive_count: 1,
    intent_count: 0,
    exclusive_limit: 10,
    orchestrator_exclusive_count: 1,
    orchestrator_exclusive_limit: 100,
    user_id: 1,
    login: "tester",
    paused_by: null,
    paused_by_login: null,
    paused_at: null,
    hidden: false,
    hidden_by: null,
    hidden_by_login: null,
    hidden_at: null,
    parent_session_id: null,
    parent_agent_name: null,
    orchestrator: null,
    orchestrator_session_id: null,
  };
}

async function installFixture(page, { claims, roster, line }) {
  await page.route("**/api/agents**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/agents/status") {
      await route.fulfill({
        json: {
          total: claims.length,
          live: 0,
          stale: claims.length,
          queued: 0,
          done: 0,
          active: claims.length,
          review: 0,
          verifying: 0,
          blocked: 0,
          remaining: claims.length,
          line,
          claims,
        },
      });
      return;
    }
    if (path === "/api/agents/roster") {
      await route.fulfill({ json: roster });
      return;
    }
    if (path === "/api/agents") {
      await route.fulfill({ json: claims });
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

  // A 30-minute-old claim on a stale roster row, plus a fresh one so the
  // default view has something visible to compare against.
  await installFixture(page, {
    claims: [
      claim("stale-claim-agent", staleAt),
      claim("fresh-claim-agent", freshAt),
    ],
    roster: [
      rosterRow("stale-claim-agent", staleAt, "stale"),
      rosterRow("fresh-claim-agent", freshAt, "live"),
    ],
    line: "2 active",
  });

  await page.goto(base);
  await page.getByRole("button", { name: "Agents" }).click();
  await page.waitForSelector(".agent-card");

  const cards = () => page.$$eval(".agent-card", (els) => els.map((el) => el.getAttribute("data-agent")));
  assert.ok(!(await cards()).includes("stale-claim-agent"), "default view hides the stale card");
  assert.ok((await cards()).includes("fresh-claim-agent"), "default view keeps the live card");

  await page.getByRole("button", { name: /Stale claims/i }).click();
  await page.waitForSelector('[data-agent="stale-claim-agent"]');
  assert.ok((await cards()).includes("stale-claim-agent"), "Stale claims segment shows the stale card");
  assert.equal(await page.locator(".empty").count(), 0, "no empty state while a stale card is shown");

  // Now an empty fixture: the empty state may appear, and its wording must be
  // about claim staleness, not agent heartbeat freshness.
  await page.unroute("**/api/agents**");
  await installFixture(page, { claims: [], roster: [], line: "No active agent claims" });
  await page.getByRole("button", { name: "Refresh" }).click();
  await page.waitForSelector(".empty");
  await page.getByRole("button", { name: /^All/ }).click();
  await page.getByRole("button", { name: /Stale claims/i }).click();
  await page.waitForSelector(".empty");
  const empty = await page.locator(".empty p").first().innerText();
  assert.match(empty, /No stale claims/i, "empty state names stale claims");
  assert.match(empty, /every claim has a heartbeat/i, "empty state talks about claim heartbeats");
  assert.doesNotMatch(empty, /heartbeated in the last 5 minutes/i, "old misleading wording is gone");
  assert.equal(await page.locator(".agent-card").count(), 0, "no cards when there is truly nothing");

  console.log("agents stale-filter browser regression: passed");
} finally {
  await browser.close();
}
