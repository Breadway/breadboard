import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-248: a description change is one compact activity
// entry with an expand affordance, not the whole body dumped inline. Later
// comments stay reachable, and the full text is still available on demand.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/activity-truncate.browser.mjs
//
// Without a session cookie it uses /auth/dev-login. The fixture creates its
// own project, issue, description and comment.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;

const BODY = [
  "## Summary",
  "",
  "A description with several paragraphs, a checklist and a fenced block so the activity entry has something worth collapsing.",
  "",
  "- first checklist item",
  "- second checklist item",
  "",
  "```js",
  "const longLine = 'this line is deliberately long and must not widen the drawer when the body is expanded';",
  "```",
  "",
  "One more paragraph so the raw body is clearly longer than a compact entry.",
].join("\n");

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  if (session) await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  if (!session) {
    const login = await page.request.post(`${base}/auth/dev-login`, {
      data: { login: `activity-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const marker = `latest comment ${stamp}`;
  const fixture = await page.evaluate(
    async ({ stamp, body, marker }) => {
      const api = async (method, path, payload) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: payload === undefined ? undefined : JSON.stringify(payload),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const project = await api("POST", "/api/projects", { key: `A${stamp}`, name: `Activity ${stamp}` });
      const issue = await api("POST", "/api/issues", { title: `Activity ${stamp}`, project: project.key });
      await api("PATCH", `/api/issues/${issue.key}`, { body });
      await api("POST", `/api/issues/${issue.key}/comments`, { body: marker });
      return { key: issue.key };
    },
    { stamp, body: BODY, marker },
  );

  await page.goto(`${base}/#${fixture.key}`);
  await page.reload();
  await page.waitForSelector(".drawer .tl");
  await page.waitForFunction(() => document.querySelectorAll(".drawer .tl .ev").length >= 1);

  const entry = page.locator(".drawer .tl .ev").filter({ hasText: "updated the description" });
  await entry.scrollIntoViewIfNeeded();
  assert.equal(await entry.count(), 1, "the description change is one activity entry");
  assert.doesNotMatch(await entry.innerText(), /```/, "the collapsed entry hides the raw fence");
  assert.doesNotMatch(await entry.innerText(), /deliberately long/, "the collapsed entry hides the body text");

  const geometry = await entry.evaluate((el) => {
    const rect = el.getBoundingClientRect();
    return { height: rect.height, width: rect.width, bodyWidth: el.closest(".drawer")?.getBoundingClientRect().width };
  });
  assert.ok(geometry.height < 80, `the entry is compact (${geometry.height.toFixed(1)}px)`);
  assert.ok(
    geometry.width <= (geometry.bodyWidth ?? 390) + 1,
    "the compact entry does not widen the drawer",
  );

  // Later activity is reachable without scrolling past a wall of body text.
  const comment = page.locator(".drawer .tl .cm").filter({ hasText: marker });
  await comment.scrollIntoViewIfNeeded();
  assert.ok(await comment.isVisible(), "the later comment is reachable");
  const gap = await page.evaluate((marker) => {
    const entry = [...document.querySelectorAll(".drawer .tl .ev")].find((el) =>
      el.textContent.includes("updated the description"),
    );
    const comment = [...document.querySelectorAll(".drawer .tl .cm")].find((el) => el.textContent.includes(marker));
    return comment.getBoundingClientRect().top - entry.getBoundingClientRect().bottom;
  }, marker);
  assert.ok(gap < 120, `the later comment sits close behind the entry (${gap.toFixed(1)}px)`);

  // The body opens on demand, complete and inside the drawer.
  await entry.locator("summary").click();
  const pre = entry.locator("details pre");
  await pre.waitFor({ state: "visible" });
  assert.equal(await pre.innerText(), BODY, "expanding shows the full new description");
  const expanded = await pre.evaluate((el) => {
    const rect = el.getBoundingClientRect();
    return { scrollWidth: el.scrollWidth, clientWidth: el.clientWidth, right: rect.right, drawerRight: el.closest(".drawer").getBoundingClientRect().right };
  });
  assert.ok(
    expanded.scrollWidth <= expanded.clientWidth + 1 || expanded.right <= expanded.drawerRight + 1,
    "the expanded body stays inside the drawer",
  );

  // The page has no horizontal scroll with the body expanded at 390.
  const doc = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    viewport: window.innerWidth,
  }));
  assert.ok(doc.scroll <= doc.viewport + 1, `no horizontal page scroll (${doc.scroll} <= ${doc.viewport})`);

  console.log("activity-truncate: compact body entry with an expand affordance");
} finally {
  await browser.close();
}
