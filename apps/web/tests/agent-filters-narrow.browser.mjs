import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-245: at 390px the activity kind chips wrap inside
// the agent dialog instead of widening the modal body and hiding the last
// filters behind a horizontal scroll.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agent-filters-narrow.browser.mjs
//
// Without a session cookie it uses /auth/dev-login. The fixture registers its
// own agent.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;
const WIDTH = 390;

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: WIDTH, height: 844 } });
  if (session) await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  if (!session) {
    const login = await page.request.post(`${base}/auth/dev-login`, {
      data: { login: `filters-narrow-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const agent = `narrow-agent-${stamp}`;
  await page.evaluate(async ({ agent }) => {
    const response = await fetch("/api/agents/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ agent_name: agent, session_id: `ses_${agent}`, host: "test", pid: 31 }),
    });
    if (!response.ok) throw new Error(`register: ${response.status} ${await response.text()}`);
  }, { agent });

  await page.getByRole("button", { name: "Agents", exact: true }).click();
  await page.locator(`.agent-card[data-agent="${agent}"]`).waitFor({ state: "visible" });
  await page.locator(`.agent-card[data-agent="${agent}"]`).click();
  await page.waitForSelector(".modal .kinds .chip");
  await page.waitForTimeout(200);

  const layout = await page.evaluate(() => {
    const body = document.querySelector(".modal .mbody");
    const kinds = document.querySelector(".modal .kinds");
    const bodyRect = body.getBoundingClientRect();
    const chips = [...document.querySelectorAll(".modal .kinds .chip")].map((chip) => {
      const rect = chip.getBoundingClientRect();
      return { text: chip.textContent.trim(), left: rect.left, right: rect.right, top: rect.top };
    });
    return {
      bodyClient: body.clientWidth,
      bodyScroll: body.scrollWidth,
      bodyLeft: bodyRect.left,
      bodyRight: bodyRect.right,
      kindsClient: kinds.clientWidth,
      kindsScroll: kinds.scrollWidth,
      chips,
      rows: new Set(chips.map((c) => Math.round(c.top))).size,
      docScroll: document.documentElement.scrollWidth,
      viewport: window.innerWidth,
    };
  });

  assert.ok(layout.chips.length >= 10, `all kind filters render (${layout.chips.length})`);
  const labels = layout.chips.map((c) => c.text);
  for (const name of ["removed", "PR opened", "PR merged", "chat", "handoff"]) {
    assert.ok(labels.includes(name), `the ${name} filter is rendered`);
  }

  assert.ok(
    layout.bodyScroll <= layout.bodyClient + 1,
    `the dialog body does not scroll horizontally (${layout.bodyScroll} <= ${layout.bodyClient})`,
  );
  assert.ok(
    layout.kindsScroll <= layout.kindsClient + 1,
    `the chip row fits its column (${layout.kindsScroll} <= ${layout.kindsClient})`,
  );
  assert.ok(layout.rows >= 2, `the chips wrap onto multiple rows (${layout.rows})`);

  for (const chip of layout.chips) {
    assert.ok(
      chip.right <= layout.bodyRight + 1,
      `${chip.text} stays inside the dialog body (${chip.right.toFixed(1)} <= ${layout.bodyRight.toFixed(1)})`,
    );
    assert.ok(
      chip.left >= layout.bodyLeft - 1,
      `${chip.text} does not hang past the left edge (${chip.left.toFixed(1)} >= ${layout.bodyLeft.toFixed(1)})`,
    );
  }

  assert.ok(
    layout.docScroll <= layout.viewport + 1,
    `the page itself does not gain a horizontal scroll (${layout.docScroll} <= ${layout.viewport})`,
  );

  // The last filter is clickable, not just present off screen.
  const handoff = page.locator(".modal .kinds .chip", { hasText: "handoff" });
  await handoff.scrollIntoViewIfNeeded();
  const hit = await handoff.evaluate((el) => {
    const rect = el.getBoundingClientRect();
    const target = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    return { top: rect.top, bottom: rect.bottom, hit: !!target && (target === el || el.contains(target)) };
  });
  assert.ok(hit.top >= 0 && hit.bottom <= 844, "the last filter sits inside the viewport");
  assert.ok(hit.hit, "the last filter is hit-testable");
  await handoff.click();
  assert.ok(await handoff.evaluate((el) => el.classList.contains("on")), "the last filter can be selected");

  console.log("agent-filters-narrow: chips wrap inside the 390px dialog");
} finally {
  await browser.close();
}
