import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-246: when /api/agents* fails the Agents page says
// so, offers Retry, keeps the last known roster, and drops the green live
// indicator instead of presenting a healthy empty fleet.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-error.browser.mjs
//
// Without a session cookie it uses /auth/dev-login. The fixture registers its
// own agent.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  if (session) await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  if (!session) {
    const login = await page.request.post(`${base}/auth/dev-login`, {
      data: { login: `agents-error-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const agent = `error-agent-${stamp}`;
  await page.evaluate(async ({ agent }) => {
    const response = await fetch("/api/agents/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ agent_name: agent, session_id: `ses_${agent}`, host: "test", pid: 41 }),
    });
    if (!response.ok) throw new Error(`register: ${response.status} ${await response.text()}`);
  }, { agent });

  const openAgents = async () => {
    if (await page.locator(".agent-card").count()) return;
    const side = page.locator("aside.side nav button.nav", { hasText: "Agents" });
    if (await side.isVisible().catch(() => false)) await side.click();
    else await page.locator(".tabbar button", { hasText: "Agents" }).click();
    await page.waitForSelector(".agent-card, .empty");
  };

  await openAgents();
  const card = page.locator(`.agent-card[data-agent="${agent}"]`);
  await card.waitFor({ state: "visible" });
  assert.equal(await page.locator("[data-roster-error]").count(), 0, "a healthy load shows no error");
  assert.equal(await page.locator(".plive").innerText(), "live", "a healthy load keeps the live indicator");

  // Every roster endpoint fails.
  await page.route("**/api/agents**", (route) => route.fulfill({ status: 500, body: "boom" }));
  await page.getByRole("button", { name: "Refresh" }).click();

  const error = page.locator("[data-roster-error]");
  await error.waitFor({ state: "visible" });
  assert.match(await error.innerText(), /couldn't load the agent roster/i, "the failure is named");
  assert.match(await error.innerText(), /boom|500|failed|internal server error/i, "the server reason is shown");

  const live = page.locator(".plive");
  assert.equal(await live.innerText(), "roster offline", "the green live dot is replaced");
  assert.ok(await live.evaluate((el) => el.classList.contains("off")), "the indicator is switched off");
  const colour = await live.evaluate((el) => getComputedStyle(el).color);
  const ok = await page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue("--ok").trim(),
  );
  assert.notEqual(colour, ok, "the indicator is not painted with the ok colour");

  assert.ok(await card.isVisible(), "the last known roster stays on screen");
  const empty = await page.locator(".empty").count();
  if (empty) {
    assert.doesNotMatch(
      await page.locator(".empty").innerText(),
      /No agent is working an issue right now/,
      "a failure is not described as an empty fleet",
    );
  }

  // Retry recovers without a reload.
  await page.unroute("**/api/agents**");
  await error.getByRole("button", { name: "Retry" }).click();
  await page.waitForSelector("[data-roster-error]", { state: "detached" });
  assert.equal(await page.locator(".plive").innerText(), "live", "live returns after a successful retry");
  assert.ok(await card.isVisible(), "the roster is back");

  console.log("agents-error: failure state, no green dot, retry recovers");
} finally {
  await browser.close();
}
