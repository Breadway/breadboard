import assert from "node:assert/strict";
import { chromium } from "playwright";

// Regression check for the BB-194 web findings fixed together:
//
//   BB-199  the release handoff dialog is a real modal: focus moves in, Tab is
//           trapped, Escape closes, focus returns to the Release button.
//   BB-200  /#KEY opens that issue on a cold load, a reload and a runtime
//           hashchange.
//   BB-203  the mobile bottom nav fits all seven tabs at 390 so Settings is
//           reachable.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7290 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/web-a11y.browser.mjs
//
// When BREADBOARD_TEST_SESSION is absent the script uses /auth/dev-login, so
// it also works on a scratch server started with BREADBOARD_DEV_LOGIN=1. The
// fixture creates its own project, issues and agent claim, so it is safe on a
// scratch database.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7290").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

async function waitForDrawer(page, key) {
  await page.waitForSelector("aside.drawer", { timeout: 8000 });
  await page.waitForFunction(
    (k) => document.querySelector("aside.drawer .key")?.textContent?.trim() === k,
    key,
    { timeout: 8000 },
  );
}

try {
  const context = await browser.newContext({ viewport: { width: 1680, height: 1000 } });
  if (session) await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  if (!session) {
    const login = await page.request.post(`${base}/auth/dev-login`, {
      data: { login: `a11y-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);
  assert.ok(
    await page.locator("aside.side").isVisible(),
    "the app rendered a signed-in shell",
  );

  const stamp = Date.now().toString().slice(-6);
  const fixture = await page.evaluate(
    async ({ stamp }) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const project = await api("POST", "/api/projects", { key: `W${stamp}`, name: `Web a11y ${stamp}` });
      const first = await api("POST", "/api/issues", {
        title: `First deep link ${stamp}`,
        project: project.key,
      });
      const second = await api("POST", "/api/issues", {
        title: `Second deep link ${stamp}`,
        project: project.key,
      });
      const agent = `a11y-agent-${stamp}`;
      // A branch makes this a claim that held work, so releasing it requires a
      // handoff note and the dialog is the only way through.
      await api("POST", `/api/issues/${first.key}/claim`, {
        agent_name: agent,
        session_id: `ses_${agent}`,
        message: "web a11y fixture",
        branch: `fix/a11y-fixture-${stamp}`,
      });
      return { project, first, second, agent };
    },
    { stamp },
  );

  // Reload so the agent roster and the claim rows are part of the initial load.
  await page.reload();
  await page.locator("aside.side").waitFor();

  // --- BB-199: release handoff is a modal ---------------------------------
  await page.locator("aside.side button.nav", { hasText: "Agents" }).click();
  const card = page.locator(".agent-card[data-agent]").filter({ hasText: fixture.agent });
  await card.waitFor({ timeout: 8000 });
  await card.click();

  const panel = page.locator('.modal[role="dialog"]');
  await panel.waitFor();
  const release = panel.getByRole("button", { name: "Release", exact: true });
  await release.click();

  const dialog = page.locator('.dialog[role="dialog"]');
  await dialog.waitFor();

  await page.waitForFunction(
    () => document.activeElement?.tagName === "TEXTAREA" &&
      document.activeElement?.getAttribute("aria-label") === "Handoff note",
    undefined,
    { timeout: 3000 },
  );
  assert.equal(
    await page.evaluate(() => document.activeElement?.getAttribute("aria-label")),
    "Handoff note",
    "focus moves to the handoff note on open",
  );

  // Tab must stay inside the dialog, in both directions, and wrap.
  const inside = async () =>
    page.evaluate(() => {
      const active = document.activeElement;
      return !!active && !!active.closest('.dialog[role="dialog"]');
    });
  for (let i = 0; i < 5; i++) {
    await page.keyboard.press("Tab");
    assert.ok(await inside(), `Tab ${i + 1} stayed inside the release dialog`);
  }
  for (let i = 0; i < 5; i++) {
    await page.keyboard.press("Shift+Tab");
    assert.ok(await inside(), `Shift+Tab ${i + 1} stayed inside the release dialog`);
  }

  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden", timeout: 3000 });
  await page.waitForFunction(
    () => document.activeElement?.textContent?.trim() === "Release",
    undefined,
    { timeout: 3000 },
  );
  assert.equal(
    await page.evaluate(() => document.activeElement?.textContent?.trim()),
    "Release",
    "focus returns to the Release button on close",
  );
  assert.ok(await panel.isVisible(), "Escape closed only the dialog, not the agent panel");

  // --- BB-200: hash deep link ---------------------------------------------
  await page.goto(`${base}/#${fixture.first.key}`);
  await waitForDrawer(page, fixture.first.key);

  // A reload must reopen the same issue (the hash is kept, not consumed).
  await page.reload();
  await waitForDrawer(page, fixture.first.key);

  // A runtime hashchange while the drawer is open switches issues.
  await page.evaluate((key) => {
    location.hash = key;
  }, fixture.second.key);
  await waitForDrawer(page, fixture.second.key);

  // --- BB-203: mobile bottom nav ------------------------------------------
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(base);
  await page.locator("nav.tabbar").waitFor();
  assert.ok(await page.locator("nav.tabbar").isVisible(), "the mobile tab bar is visible");

  const nav = await page.locator("nav.tabbar").evaluate((el) => ({
    scrollWidth: el.scrollWidth,
    clientWidth: el.clientWidth,
  }));
  assert.ok(
    nav.scrollWidth <= nav.clientWidth + 1,
    `all seven tabs fit at 390 (scrollWidth ${nav.scrollWidth} <= clientWidth ${nav.clientWidth})`,
  );

  const settingsTab = page.locator("nav.tabbar button", { hasText: "Settings" });
  assert.equal(await settingsTab.count(), 1, "Settings is one of the bottom tabs");
  const box = await settingsTab.boundingBox();
  assert.ok(box, "Settings has a box");
  assert.ok(
    box.x >= 0 && box.x + box.width <= 390,
    `Settings is fully on screen at 390 (x ${box.x}, width ${box.width})`,
  );
  assert.ok(box.width >= 40, `Settings keeps a usable tap target (${box.width}px wide)`);

  await settingsTab.click();
  await page.getByRole("heading", { name: /Cycles/ }).waitFor({ timeout: 5000 });
  assert.ok(
    await page.locator("nav.tabbar button.on", { hasText: "Settings" }).isVisible(),
    "Settings is the active tab after tapping it",
  );

  console.log("web-a11y browser check passed");
} finally {
  await browser.close();
}
