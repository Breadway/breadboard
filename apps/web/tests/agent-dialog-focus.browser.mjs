import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-244: the agent detail dialog is a real modal.
// Tab and Shift+Tab stay inside it, and closing (Escape or the close button)
// returns focus to the agent card that opened it.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agent-dialog-focus.browser.mjs
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
      data: { login: `dialog-focus-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const agent = `focus-agent-${stamp}`;
  await page.evaluate(async ({ agent }) => {
    const response = await fetch("/api/agents/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ agent_name: agent, session_id: `ses_${agent}`, host: "test", pid: 21 }),
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

  const where = () =>
    page.evaluate(() => {
      const active = document.activeElement;
      return {
        tag: active?.tagName ?? null,
        inModal: !!active?.closest?.(".modal"),
        agent: active?.getAttribute?.("data-agent") ?? null,
        label: active?.getAttribute?.("aria-label") ?? null,
      };
    });

  const openByClick = async () => {
    await card.click();
    await page.waitForSelector(".modal", { state: "visible" });
    await page.waitForTimeout(120);
  };

  await openByClick();
  let at = await where();
  assert.ok(at.inModal, `focus starts inside the dialog (got ${at.tag})`);

  // Shift+Tab from the dialog container must wrap to the last control inside.
  await page.keyboard.press("Shift+Tab");
  at = await where();
  assert.ok(at.inModal, `Shift+Tab stays inside the dialog (got ${at.tag}.${at.label ?? at.agent ?? ""})`);

  // A full walk of the tab order never leaves the dialog.
  for (let i = 0; i < 90; i++) {
    await page.keyboard.press("Tab");
    at = await where();
    assert.ok(at.inModal, `Tab ${i + 1} stays inside the dialog (landed on ${at.tag})`);
  }

  await page.keyboard.press("Escape");
  await page.waitForSelector(".modal", { state: "detached" });
  await page.waitForTimeout(120);
  at = await where();
  assert.equal(at.agent, agent, "Escape restores focus to the opening agent card");
  assert.equal(at.tag, "DIV", "the restored target is the card element");

  // The close button takes the same path.
  await page.keyboard.press("Enter");
  await page.waitForSelector(".modal", { state: "visible" });
  await page.waitForTimeout(120);
  at = await where();
  assert.ok(at.inModal, "keyboard open moves focus into the dialog");
  await page.locator(".modal .close").click();
  await page.waitForSelector(".modal", { state: "detached" });
  await page.waitForTimeout(120);
  at = await where();
  assert.equal(at.agent, agent, "the close button restores focus to the opening agent card");

  console.log("agent-dialog-focus: trap holds and focus returns to the card");
} finally {
  await browser.close();
}
