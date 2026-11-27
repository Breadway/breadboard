import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-242: the issue drawer title wraps to its full
// height at desktop and narrow widths. The title textarea sits in a flex
// column whose default flex-shrink used to collapse it back to one row.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/issue-drawer-title.browser.mjs
//
// Without a session cookie it uses /auth/dev-login, so it also works with
// BREADBOARD_DEV_LOGIN=1. The fixture creates its own project and issue.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;
const TITLE =
  "A very long issue title that should wrap cleanly across several lines in the drawer header and never clip to a single row";

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
      data: { login: `drawer-title-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const fixture = await page.evaluate(
    async ({ stamp, title }) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const project = await api("POST", "/api/projects", { key: `T${stamp}`, name: `Drawer title ${stamp}` });
      const issue = await api("POST", "/api/issues", { title, project: project.key });
      return { key: issue.key };
    },
    { stamp, title: TITLE },
  );

  const measureTitle = () =>
    page.evaluate(() => {
      const el = document.querySelector(".drawer textarea.title");
      if (!el) return null;
      const style = getComputedStyle(el);
      const rect = el.getBoundingClientRect();
      return {
        value: el.value,
        clientHeight: el.clientHeight,
        scrollHeight: el.scrollHeight,
        renderedHeight: rect.height,
        flexShrink: style.flexShrink,
        lineHeight: parseFloat(style.lineHeight),
      };
    });

  for (const [width, height] of [
    [1440, 900],
    [390, 844],
  ]) {
    await page.setViewportSize({ width, height });
    // A cold load measures the title at the width it is actually rendered at.
    await page.goto(`${base}/#${fixture.key}`);
    await page.reload();
    await page.waitForSelector(".drawer textarea.title");
    await page.waitForFunction(() => {
      const el = document.querySelector(".drawer textarea.title");
      return !!el && el.clientHeight > 0;
    });
    // The panel slides in; the fitted height lands a frame after mount.
    await page.waitForTimeout(350);

    const m = await measureTitle();
    assert.ok(m, `${width}: the drawer title renders`);
    assert.equal(m.value, TITLE, `${width}: the full title is in the field`);
    assert.equal(m.flexShrink, "0", `${width}: the title does not shrink as a flex item`);
    assert.ok(
      m.scrollHeight <= m.clientHeight + 1,
      `${width}: the title height fits its content (client ${m.clientHeight}, scroll ${m.scrollHeight})`,
    );
    assert.ok(
      m.renderedHeight >= m.scrollHeight - 1,
      `${width}: the rendered title is the full height (${m.renderedHeight} >= ${m.scrollHeight})`,
    );
    const rows = Math.round(m.scrollHeight / m.lineHeight);
    assert.ok(rows >= 2, `${width}: the long title wraps to multiple rows (${rows})`);
  }

  console.log("issue-drawer-title: full wrapped title at 1440 and 390");
} finally {
  await browser.close();
}
