import assert from "node:assert/strict";
import { chromium } from "playwright";

// Manual regression check for the orchestrator links on the Agents page.
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7290 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-links.browser.mjs
//
// The fixture creates its own project, tall orchestrator and subagents, so it
// does not depend on the production board.

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7290";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 1000 } });
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();
  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const names = {
    orch: `big-orch-${stamp}`,
    orchSession: `ses_orch_${stamp}`,
    child: `little-sub-${stamp}`,
    childSession: `ses_little_${stamp}`,
    busy: `busy-sub-${stamp}`,
    busySession: `ses_busy_${stamp}`,
    orphan: `orphan-sub-${stamp}`,
    orphanSession: `ses_orphan_${stamp}`,
    missingParent: `ses_missing_${stamp}`,
    hidden: `hidden-sub-${stamp}`,
    hiddenSession: `ses_hidden_${stamp}`,
    paused: `paused-sub-${stamp}`,
    pausedSession: `ses_paused_${stamp}`,
  };

  const fixture = await page.evaluate(
    async ({ stamp, names }) => {
      const headers = { "content-type": "application/json" };
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers,
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const project = await api("POST", "/api/projects", { key: `O${stamp}`, name: `Orchestrators ${stamp}` });
      const issue = (title) => api("POST", "/api/issues", { title, project: project.key });
      const claim = async (agent, sessionId, count) => {
        for (let i = 0; i < count; i++) {
          const created = await issue(`${agent} ${stamp} ${i + 1}`);
          await api("POST", `/api/issues/${created.key}/claim`, {
            agent_name: agent,
            session_id: sessionId,
            message: `${agent} fixture`,
          });
        }
      };

      await api("POST", "/api/agents/register", {
        agent_name: names.orch,
        session_id: names.orchSession,
        host: "test",
        pid: 10,
      });
      await claim(names.orch, names.orchSession, 95);

      for (const [agent, sid] of [
        [names.child, names.childSession],
        [names.busy, names.busySession],
        [names.hidden, names.hiddenSession],
        [names.paused, names.pausedSession],
      ]) {
        await api("POST", "/api/agents/register", {
          agent_name: agent,
          session_id: sid,
          parent_session_id: names.orchSession,
          host: "test",
          pid: 11,
        });
      }
      await claim(names.busy, names.busySession, 2);
      await api("POST", "/api/agents/register", {
        agent_name: names.orphan,
        session_id: names.orphanSession,
        parent_session_id: names.missingParent,
        host: "test",
        pid: 12,
      });
      await api("POST", `/api/agents/${names.hidden}/hide`);
      await api("POST", `/api/agents/${names.paused}/pause`);

      for (const neighbor of [`crumb-${stamp}`, `zesty-${stamp}`]) {
        await api("POST", "/api/agents/register", { agent_name: neighbor, session_id: `ses_${neighbor}` });
      }
      return { project: project.key };
    },
    { stamp, names },
  );

  const cardInfo = () =>
    page.$$eval(".agent-card", (els) =>
      els.map((el) => {
        const r = el.getBoundingClientRect();
        return {
          name: el.getAttribute("data-agent"),
          hidden: el.getAttribute("data-hidden") === "true",
          claims: Number(el.getAttribute("data-claims") || 0),
          x: Math.round(r.x),
          y: Math.round(r.y),
          width: Math.round(r.width),
          height: Math.round(r.height),
          tiles: el.querySelectorAll(".tile").length,
          tilesInCard: [...el.querySelectorAll(".tile")].every((t) => {
            const tr = t.getBoundingClientRect();
            return tr.x >= r.x - 1 && tr.x + tr.width <= r.x + r.width + 1;
          }),
          under: el.querySelector(".rel-chip.under")?.textContent?.trim() ?? null,
          unresolved: el.querySelector(".rel-chip.unresolved")?.textContent?.trim() ?? null,
          subs: el.querySelector(".subagent-badge")?.textContent?.trim() ?? null,
          unhide: !!el.querySelector(".unhide-card"),
        };
      }),
    );

  const refreshLive = async () => {
    for (const [agent, sid, parent] of [
      [names.orch, names.orchSession, undefined],
      [names.child, names.childSession, names.orchSession],
      [names.busy, names.busySession, names.orchSession],
      [names.orphan, names.orphanSession, names.missingParent],
      [names.hidden, names.hiddenSession, names.orchSession],
    ]) {
      const response = await context.request.post(`${base}/api/agents/register`, {
        data: { agent_name: agent, session_id: sid, parent_session_id: parent },
      });
      assert.ok(response.ok(), `refresh ${agent}: ${response.status()}`);
    }
  };

  for (const width of [1680, 1280, 640]) {
    await refreshLive();
    await page.setViewportSize({ width, height: 1000 });
    await page.reload();
    await page.getByRole("button", { name: "Agents" }).click();
    await page.waitForSelector(".agent-card");
    await page.waitForTimeout(200);

    const before = await cardInfo();
    const byName = Object.fromEntries(before.map((c) => [c.name, c]));
    const tall = byName[names.orch];
    assert.ok(tall, `${width}: tall orchestrator card is present`);
    assert.equal(tall.tiles, 95, `${width}: orchestrator keeps its 95 claim tiles`);
    assert.ok(tall.tilesInCard, `${width}: claim tiles stay inside the card`);
    assert.equal(byName[names.child].claims, 0, `${width}: claimless child renders`);
    assert.equal(byName[names.child].under, `under ${names.orch}`, `${width}: child links to orchestrator`);
    assert.equal(byName[names.busy].under, `under ${names.orch}`, `${width}: busy child links`);
    assert.equal(byName[names.orphan].unresolved, `under ${names.missingParent}`, `${width}: unresolved parent keeps raw id`);
    assert.equal(byName[names.hidden], undefined, `${width}: hidden child is excluded by default`);
    assert.equal(byName[names.paused].under, `under ${names.orch}`, `${width}: paused child keeps the link`);
    assert.equal(tall.subs, "3 live", `${width}: orchestrator counts live subagents`);

    const columns = new Set(before.map((c) => c.x)).size;
    assert.ok(columns >= 2, `${width}: masonry uses ${columns} columns`);
    const besideTall = before.filter(
      (c) => c.name !== names.orch && c.x > tall.x + 20 && c.y < tall.y + tall.height * 0.6,
    );
    assert.ok(besideTall.length > 0, `${width}: cards tile beside the tall orchestrator`);

    // The chip focuses the orchestrator and its direct subagents.
    await page.locator(`[data-agent="${names.child}"] .rel-chip.under`).click();
    await page.waitForTimeout(200);
    const focusedText = await page.locator(".wrap").innerText();
    const focused = await cardInfo();
    assert.match(focusedText, new RegExp(`Focused on\\s+${names.orch}`, "i"), `${width}: focus banner appears`);
    assert.ok(focused.some((c) => c.name === names.orch), `${width}: focused orchestrator remains visible`);
    assert.ok(!focused.some((c) => c.name.startsWith("crumb-")), `${width}: focus hides non-linked agents`);
    assert.ok(!focused.some((c) => c.name === names.hidden), `${width}: focus still respects hidden`);
    await page.getByRole("button", { name: "Clear" }).click();
    await page.waitForTimeout(150);

    // Revealing hidden keeps the relationship and offers Unhide.
    await page.getByRole("button", { name: /Show hidden/i }).click();
    await page.waitForTimeout(200);
    const revealed = await cardInfo();
    const hidden = revealed.find((c) => c.name === names.hidden);
    assert.ok(hidden?.hidden, `${width}: hidden child is revealed`);
    assert.equal(hidden?.under, `under ${names.orch}`, `${width}: hidden child keeps the link`);
    assert.ok(hidden?.unhide, `${width}: hidden child has Unhide`);
    assert.ok(revealed.some((c) => c.name === names.orch), `${width}: hiding the child leaves the orchestrator`);

    // Order and chip are stable through a refresh and a heartbeat.
    await page.getByRole("button", { name: /Showing hidden/i }).click();
    await page.waitForTimeout(100);
    const order1 = (await cardInfo()).map((c) => c.name);
    const chip1 = (await cardInfo()).find((c) => c.name === names.child)?.under;
    await page.evaluate(
      async ({ agent, sessionId, parent }) => {
        await fetch("/api/agents/register", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ agent_name: agent, session_id: sessionId, parent_session_id: parent, note: "heartbeat" }),
        });
      },
      { agent: names.child, sessionId: names.childSession, parent: names.orchSession },
    );
    await page.getByRole("button", { name: "Refresh" }).click();
    await page.waitForTimeout(400);
    const order2 = (await cardInfo()).map((c) => c.name);
    const chip2 = (await cardInfo()).find((c) => c.name === names.child)?.under;
    assert.deepEqual(order2, order1, `${width}: card order is stable across refresh and heartbeat`);
    assert.equal(chip2, chip1, `${width}: orchestrator chip is stable`);
  }
  console.log("agents orchestrator-link browser regressions: passed", fixture.project);
} finally {
  await browser.close();
}
