import assert from "node:assert/strict";
import { chromium } from "playwright";

// Approvals page regressions: every decision path, the changes-requested hidden
// state, revision return, markdown/mermaid, both lightboxes, the calendar drag
// and the window editor. Runs against a real server with a real session cookie.
//
// Run: BREADBOARD_TEST_SESSION=<session> node tests/approvals.browser.mjs

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7290";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  // A console error or a page error fails the run: the page must not rely on a
  // swallowed exception to pass.
  const pageErrors = [];
  page.on("pageerror", (e) => pageErrors.push(e.message));
  page.on("console", (msg) => {
    if (msg.type() === "error" && !msg.text().includes("favicon")) pageErrors.push(msg.text());
  });

  await page.goto(base);

  const stamp = Date.now();
  const fx = await page.evaluate(
    async ({ stamp, PNG }) => {
      const headers = { "content-type": "application/json" };
      const call = async (method, path, body) => {
        const r = await fetch(path, { method, headers, body: body ? JSON.stringify(body) : undefined });
        const text = await r.text();
        if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${text}`);
        return text ? JSON.parse(text) : null;
      };

      const project = await call("POST", "/api/projects", { key: `AP${stamp % 10000}`, name: "Approvals test" });
      const issue = await call("POST", "/api/issues", { title: "approval host", project: project.key });
      const agent = `test scout ${stamp}`;
      const sessionId = `ses_approval_${stamp}`;
      await call("POST", "/api/agents/register", { agent_name: agent, session_id: sessionId, host: "hestia" });

      const shot = await call("POST", `/api/issues/${issue.key}/artifacts/base64`, {
        filename: "before.png",
        kind: "screenshot",
        data_base64: PNG,
        mime: "image/png",
        agent_name: agent,
        session_id: sessionId,
      });

      const baseAsk = {
        agent_name: agent,
        session_id: sessionId,
        issue: issue.key,
        environment: "hestia",
      };

      const approval = await call("POST", "/api/approvals", {
        ...baseAsk,
        kind: "approval",
        urgency: "high",
        title: "Rotate the runner token",
        summary: "A short summary for the inbox.",
        body_md:
          "### Why this needs you\n\nThe token is in the **old image** and `runner:v1.4.2`.\n\n```mermaid\nflowchart LR\n  A[Drain] --> B[Rotate]\n  B --> C{Smoke}\n```\n\n> The runner is idle right now.",
        checklist: ["Approve the two minute pause.", "I will not touch the database."],
        risk_md: "Low: the runner is idle.",
        rollback_md: "Restore the previous unit file.",
        deadline: "today 18:00",
        attachments: [{ artifact_id: shot.id, caption: "runner before" }],
      });

      const info = await call("POST", "/api/approvals", {
        ...baseAsk,
        kind: "info",
        urgency: "normal",
        title: "Which token is authoritative?",
        summary: "Two candidate tokens exist.",
        body_md: "The mirror file or the secret store?",
      });

      const decline = await call("POST", "/api/approvals", {
        ...baseAsk,
        kind: "approval",
        title: "Delete the old backup bucket",
        summary: "This one will be declined.",
        body_md: "Please approve the delete.",
      });

      const changes = await call("POST", "/api/approvals", {
        ...baseAsk,
        kind: "approval",
        title: "Add a webhook receiver",
        summary: "Sent back until it uses HMAC.",
        body_md: "### Proposal\n\nA small receiver.",
      });
      await call("POST", `/api/approvals/${changes.id}/decision`, {
        decision: "changes",
        note_md: "Use HMAC and a timestamp window, then resubmit.",
      });

      const week = new Date();
      week.setHours(0, 0, 0, 0);
      const monday = new Date(week);
      monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
      const at = (day, hour, minutes = 0) => {
        const d = new Date(monday);
        d.setDate(monday.getDate() + day);
        d.setHours(hour, minutes, 0, 0);
        return d.toISOString();
      };
      const windowBody = (day, hour, title) => ({
        ...baseAsk,
        kind: "maintenance",
        urgency: "high",
        title,
        summary: `Proposed window on day ${day}.`,
        body_md: "### What changes\n\nA short upgrade.",
        checklist: ["Approve the window."],
        risk_md: "Nextcloud is down for the window.",
        rollback_md: "Restore the snapshot.",
        window: {
          title,
          starts_at: at(day, hour),
          ends_at: at(day, hour + 2),
          timezone: "Australia/Perth",
          services: ["nextcloud", "mysql"],
          reason: "Move to 8.4 before the freeze.",
          risks: "Nextcloud is unavailable for the window.",
          rollback: "Restore the 01:45 snapshot.",
          runbook_md: "1. Announce the window\n2. Snapshot\n3. Upgrade\n4. Smoke test",
        },
      });
      const windowAsk = await call("POST", "/api/approvals", windowBody(2, 10, "Nextcloud MySQL 8.4"));
      const dragAsk = await call("POST", "/api/approvals", windowBody(3, 13, "Forgejo secret rotation"));

      return {
        projectKey: project.key,
        issueKey: issue.key,
        agent,
        sessionId,
        ids: {
          approval: approval.id,
          info: info.id,
          decline: decline.id,
          changes: changes.id,
          window: windowAsk.id,
          drag: dragAsk.id,
        },
        windows: { approved: windowAsk.window.id, dragged: dragAsk.window.id },
        windowStartsAt: windowAsk.window.starts_at,
      };
    },
    { stamp, PNG },
  );

  // --- navigation and inbox ------------------------------------------------
  await page.getByRole("button", { name: /^Approvals/ }).first().click();
  await page.locator("[data-approval-page]").waitFor();
  await page.locator(`[data-approval-card="${fx.ids.approval}"]`).waitFor();
  assert.equal(await page.locator("[data-approval-card]").count() >= 3, true);
  await page.locator(`[data-approval-card="${fx.ids.changes}"]`).count().then((n) => assert.equal(n, 0, "a changes-requested ask is hidden from Needs you"));

  // Search narrows the list server-side.
  const search = page.getByLabel("Search approvals");
  await search.fill("Rotate the runner");
  await page.waitForFunction(
    (id) => document.querySelectorAll(`[data-approval-card="${id}"]`).length === 1,
    fx.ids.approval,
  );
  await search.fill("");
  await page.waitForFunction(
    (id) => document.querySelectorAll(`[data-approval-card="${id}"]`).length === 1,
    fx.ids.info,
  );

  // --- detail, markdown, mermaid, screenshot lightbox ----------------------
  await page.locator(`[data-approval-card="${fx.ids.approval}"]`).click();
  await page.locator("[data-approval-status]").waitFor();
  assert.equal(await page.locator("[data-approval-status]").textContent(), "needs you");
  assert.equal(await page.locator("[data-approval-body] h3").first().textContent(), "Why this needs you");
  assert.equal(await page.locator("[data-approval-body] strong").first().textContent(), "old image");
  assert.equal(await page.locator("[data-approval-body] code").first().textContent(), "runner:v1.4.2");
  assert.equal(await page.locator("[data-approval-checklist] li").count(), 2);
  assert.equal(await page.locator('[data-panel="risk"]').count(), 1);
  assert.equal(await page.locator('[data-panel="rollback"]').count(), 1);

  await page.locator("[data-mermaid-card] svg").waitFor({ timeout: 30_000 });
  await page.locator("[data-mermaid-card]").click();
  await page.locator('[data-lightbox="mermaid"]').waitFor();
  await page.locator('[data-lightbox="mermaid"]').click();
  await page.locator('[data-lightbox="mermaid"]').waitFor({ state: "detached" });

  await page.locator("[data-screenshot]").click();
  await page.locator('[data-lightbox="screenshot"]').waitFor();
  const lightboxSrc = await page.locator('[data-lightbox="screenshot"] img').getAttribute("src");
  assert.match(lightboxSrc, /\/api\/artifacts\/\d+$/);
  await page.locator('[data-lightbox="screenshot"]').click();
  await page.locator('[data-lightbox="screenshot"]').waitFor({ state: "detached" });

  // --- approve -------------------------------------------------------------
  await page.locator("[data-decision-note]").fill("Go ahead, keep it short.");
  await page.locator('.composer [data-decision="approve"]').click();
  await page.waitForFunction(() => document.querySelector("[data-approval-status]")?.textContent === "approved");
  await page.locator('[data-resolution="approved"]').waitFor();
  assert.equal(await page.locator(".composer").count(), 0, "a decided ask has no composer");
  const approvedThread = await page.locator("[data-approval-thread] .bubble").last().textContent();
  assert.match(approvedThread, /Go ahead, keep it short/);

  // The ask leaves Needs you and appears under Answered.
  await page.locator('[data-filter="answered"]').click();
  await page.locator(`[data-approval-card="${fx.ids.approval}"]`).waitFor();
  await page.locator('[data-filter="needs"]').click();
  await page.waitForFunction(
    (id) => document.querySelectorAll(`[data-approval-card="${id}"]`).length === 0,
    fx.ids.approval,
  );

  // --- decline -------------------------------------------------------------
  await page.locator(`[data-approval-card="${fx.ids.decline}"]`).click();
  await page.locator('[data-decision-note]').fill("Not this month.");
  await page.locator('.composer [data-decision="decline"]').click();
  await page.waitForFunction(() => document.querySelector("[data-approval-status]")?.textContent === "declined");
  await page.locator('[data-resolution="declined"]').waitFor();

  // --- send info closes ------------------------------------------------
  await page.locator(`[data-approval-card="${fx.ids.info}"]`).click();
  await page.locator('[data-decision-note]').fill("The mirror file is authoritative.");
  await page.locator('.composer [data-decision="info"]').click();
  await page.waitForFunction(() => document.querySelector("[data-approval-status]")?.textContent === "answered");
  await page.locator('[data-resolution="answered"]').waitFor();
  assert.equal(await page.locator(".composer").count(), 0);

  // --- changes requested, hidden state and revision return ---------------
  await page.locator('[data-filter="changes"]').click();
  await page.locator(`[data-approval-card="${fx.ids.changes}"]`).click();
  await page.waitForFunction(() => document.querySelector("[data-approval-status]")?.textContent === "changes requested");
  await page.locator('[data-resolution="changes"]').waitFor();
  assert.match(await page.locator('[data-resolution="changes"]').textContent(), /Hidden from your inbox/);

  await page.evaluate(
    async ({ id, agent, sessionId }) => {
      const r = await fetch(`/api/approvals/${id}/revisions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          agent_name: agent,
          session_id: sessionId,
          summary: "Reworked to HMAC with a timestamp window.",
          body_md: "### Rework\n\nNow signed and replayed safely.",
        }),
      });
      if (!r.ok) throw new Error(`revision: ${r.status} ${await r.text()}`);
    },
    { id: fx.ids.changes, agent: fx.agent, sessionId: fx.sessionId },
  );
  await page.locator('[data-filter="needs"]').click();
  await page.locator(`[data-approval-card="${fx.ids.changes}"]`).waitFor();
  await page.locator(`[data-approval-card="${fx.ids.changes}"]`).click();
  await page.locator('[data-approval-status]').waitFor();
  await page.waitForFunction(() => document.querySelector("[data-approval-status]")?.textContent === "needs you");
  assert.match(await page.locator(".detail .ask-meta").first().textContent(), /revision 2/);
  assert.match(await page.locator("[data-approval-thread]").textContent(), /revision 2/);
  assert.match(await page.locator("[data-approval-body]").textContent(), /Now signed/);

  // Request changes from the UI as well: the same ask hides again.
  await page.locator('[data-decision-note]').fill("One more pass, please.");
  await page.locator('.composer [data-decision="changes"]').click();
  await page.waitForFunction(
    () => document.querySelector("[data-approval-status]")?.textContent === "changes requested",
  );
  await page.locator('[data-resolution="changes"]').waitFor();

  // --- maintenance ask editor ---------------------------------------------
  await page.locator(`[data-approval-card="${fx.ids.window}"]`).click();
  await page.locator("[data-window-editor]").waitFor();
  await page.locator("[data-window-editor] summary").click();
  const beforeHour = await page.locator("[data-we-start-hour]").textContent();
  await page.locator('[data-we-step="start-hour-plus"]').click();
  await page.waitForFunction(
    (before) => document.querySelector("[data-we-start-hour]")?.textContent !== before,
    beforeHour,
  );
  const afterHour = await page.locator("[data-we-start-hour]").textContent();
  assert.notEqual(afterHour, beforeHour, "the hour stepper moves the start");

  await page.locator('[data-we-preset="6"]').click();
  await page.waitForFunction(() => document.querySelector("[data-we-duration]")?.textContent === "6h");

  // Drag the block one hour later: the bar position and label follow.
  const bar = await page.locator("[data-we-bar]").boundingBox();
  assert.ok(bar, "window bar is visible");
  const labelBefore = await page.locator("[data-we-bar-label]").textContent();
  await page.mouse.move(bar.x + bar.width / 2, bar.y + bar.height / 2);
  await page.mouse.down();
  await page.mouse.move(bar.x + bar.width / 2 + 60, bar.y + bar.height / 2, { steps: 8 });
  await page.mouse.up();
  await page.waitForFunction(
    (before) => document.querySelector("[data-we-bar-label]")?.textContent !== before,
    labelBefore,
  );

  // The right edge resizes the end independently of the start.
  const resizedBar = await page.locator("[data-we-bar]").boundingBox();
  const durationBeforeResize = await page.locator("[data-we-duration]").textContent();
  const startLabelBeforeResize = await page.locator("[data-we-start-hour]").textContent();
  const startMinuteBeforeResize = await page.locator("[data-we-start-minute]").textContent();
  await page.mouse.move(resizedBar.x + resizedBar.width - 4, resizedBar.y + resizedBar.height / 2);
  await page.mouse.down();
  await page.mouse.move(resizedBar.x + resizedBar.width + 40, resizedBar.y + resizedBar.height / 2, { steps: 6 });
  await page.mouse.up();
  await page.waitForFunction(
    (before) => document.querySelector("[data-we-duration]")?.textContent !== before,
    durationBeforeResize,
  );
  assert.equal(await page.locator("[data-we-start-hour]").textContent(), startLabelBeforeResize, "resizing the right edge keeps the start");
  assert.equal(await page.locator("[data-we-start-minute]").textContent(), startMinuteBeforeResize);

  // Day chips move the proposal to another date and stay in sync with the bar.
  const dayBefore = await page.locator("[data-we-range-label]").textContent();
  await page.locator('[data-we-day="5"]').click();
  await page.waitForFunction(
    (before) => document.querySelector("[data-we-range-label]")?.textContent !== before,
    dayBefore,
  );

  // Keyboard moves the range too.
  const keyBefore = await page.locator("[data-we-bar-label]").textContent();
  await page.locator("[data-we-bar]").focus();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction(
    (before) => document.querySelector("[data-we-bar-label]")?.textContent !== before,
    keyBefore,
  );

  await page.locator('[data-window-action="approve"]').click();
  await page.waitForFunction(() => document.querySelector("[data-approval-status]")?.textContent === "approved");
  await page.locator("[data-window-summary]").waitFor();
  assert.match(await page.locator("[data-window-summary]").textContent(), /approved/);

  // --- maintenance calendar ------------------------------------------------
  await page.locator('[data-view="calendar"]').click();
  await page.locator("[data-cal-win]").first().waitFor({ timeout: 15_000 });
  assert.equal(await page.locator(`[data-cal-win="${fx.windows.approved}"]`).count(), 1, "the approved window renders");

  // Open the popup: services, reason, risk, rollback and the runbook steps.
  await page.locator(`[data-cal-win="${fx.windows.dragged}"]`).click();
  await page.locator("[data-cal-popup]").waitFor();
  assert.match(await page.locator("[data-cal-popup]").textContent(), /nextcloud|mysql/);
  assert.match(await page.locator("[data-cal-popup]").textContent(), /Restore the 01:45 snapshot/);
  assert.equal(await page.locator("[data-cal-popup] .runbook .step").count(), 4);
  await page.locator("[data-cal-close]").click();

  // Drag the proposed window a day later, then send the revised slot.
  const block = await page.locator(`[data-cal-win="${fx.windows.dragged}"]`).boundingBox();
  const column = await page.locator('[data-cal-day="3"]').boundingBox();
  assert.ok(block && column);
  await page.mouse.move(block.x + block.width / 2, block.y + block.height / 2);
  await page.mouse.down();
  await page.mouse.move(block.x + block.width / 2 + column.width, block.y + block.height / 2, { steps: 10 });
  await page.mouse.up();
  await page.locator("[data-cal-popup]").waitFor();
  await page.locator('[data-cal-action="changes"]').click();
  await page.locator("[data-cal-popup]").waitFor({ state: "detached" });
  await page.waitForFunction(
    (id) => document.querySelector(`[data-cal-win="${id}"]`)?.className.includes("proposed"),
    fx.windows.dragged,
  );

  // The revised slot is now a changes request on the ask.
  await page.evaluate(
    async (id) => {
      const r = await fetch(`/api/approvals/${id}`);
      if (!r.ok) throw new Error(`get ask: ${r.status}`);
      const ask = await r.json();
      if (ask.status !== "changes") throw new Error(`expected changes, got ${ask.status}`);
    },
    fx.ids.drag,
  );

  assert.deepEqual(pageErrors, [], `page errors: ${pageErrors.join(" | ")}`);
  console.log("approvals browser regressions: passed");
} finally {
  await browser.close();
}
