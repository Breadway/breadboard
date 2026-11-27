import assert from "node:assert/strict";
import { chromium, request } from "playwright";

// Browser regression for BB-247: review outcome controls only appear for the
// named reviewer. A third party sees a read-only row instead of Accept,
// Changes and Approve buttons that always fail with 403.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/review-outcome.browser.mjs
//
// Without a session cookie it uses /auth/dev-login. The fixture creates a
// second user to own the issue and the review requests.

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
      data: { login: `review-viewer-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);
  const me = await (await context.request.get(`${base}/api/me`)).json();
  assert.ok(me.login, "the signed-in viewer has a login");

  // A different human owns the issue and files the review requests.
  const stamp = Date.now().toString().slice(-6);
  const requester = await request.newContext({ baseURL: base });
  try {
    const login = await requester.post("/auth/dev-login", {
      data: { login: `review-requester-${stamp}`, name: "Review Requester" },
    });
    assert.ok(login.ok(), `requester dev login: ${login.status()}`);

    const fixture = await (async () => {
      const projectResponse = await requester.post("/api/projects", {
        data: { key: `R${stamp}`, name: `Review outcome ${stamp}` },
      });
      assert.ok(projectResponse.ok(), `project: ${projectResponse.status()} ${await projectResponse.text()}`);
      const project = await projectResponse.json();

      const issueResponse = await requester.post("/api/issues", {
        data: { title: `Review outcome ${stamp}`, project: project.key },
      });
      assert.ok(issueResponse.ok(), `issue: ${issueResponse.status()} ${await issueResponse.text()}`);
      const issue = await issueResponse.json();

      for (const [reviewer, prNumber] of [
        ["third-reviewer", 901],
        [me.login, 902],
      ]) {
        const reviewResponse = await requester.post(`/api/issues/${issue.key}/reviews`, {
          data: { reviewer, pr_number: prNumber },
        });
        assert.ok(
          reviewResponse.ok(),
          `review for ${reviewer}: ${reviewResponse.status()} ${await reviewResponse.text()}`,
        );
      }
      return { issue, project };
    })();

    await page.goto(`${base}/#${fixture.issue.key}`);
    // A full load re-fetches the issues the requester just created.
    await page.reload();
    await page.waitForSelector(".reviews .req");
    await page.waitForFunction(() => document.querySelectorAll(".reviews .req").length >= 2);

    const rowFor = (reviewer) =>
      page.locator(".reviews .req").filter({ has: page.locator(".who", { hasText: reviewer }) });

    const other = rowFor("third-reviewer");
    assert.equal(await other.count(), 1, "the third-reviewer request renders");
    assert.equal(
      await other.getByRole("button", { name: /^(Accept|Changes|Approve)$/ }).count(),
      0,
      "a third party sees no outcome buttons",
    );
    assert.match(await other.innerText(), /Read-only: only third-reviewer/, "the row explains who can act");

    const mine = rowFor(me.login);
    assert.equal(await mine.count(), 1, "the request naming the viewer renders");
    for (const label of ["Accept", "Changes", "Approve"]) {
      assert.equal(
        await mine.getByRole("button", { name: label, exact: true }).count(),
        1,
        `the named reviewer still sees ${label}`,
      );
    }

    // The visible control really works: no 403, the state moves.
    await mine.getByRole("button", { name: "Accept", exact: true }).click();
    await page.waitForSelector(".reviews .req .chip.state-accepted");
    assert.equal(await page.locator(".reviews .err").count(), 0, "the server accepted the outcome");
    assert.match(await rowFor(me.login).innerText(), /Accepted/, "the row reflects the new state");

    console.log("review-outcome: buttons only for the named reviewer, and they work");
  } finally {
    await requester.dispose();
  }
} finally {
  await browser.close();
}
