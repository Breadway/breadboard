import assert from "node:assert/strict";
import { chromium } from "playwright";

// Manual regression check for the Agents hierarchy. It builds the shape the
// live roster actually has: one root orchestrator with two dozen direct
// children (one of them a tall card), a second independent root orchestrator,
// a real nested orchestrator with grandchildren, a hidden orchestrator that
// still has visible children, unlinked entries, paused children and a killed
// agent that must disappear from the roster entirely.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7290 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-hierarchy.browser.mjs
//
// The fixture creates its own project and agents, so it is safe on a scratch
// database and does not depend on the production board.

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
    root: `sourdough-scribe-${stamp}`,
    rootSession: `ses_sourdough_${stamp}`,
    study: `study-steward-${stamp}`,
    studySession: `ses_study_${stamp}`,
    studyVoice: `study-voice-${stamp}`,
    crumbScholar: `crumb-scholar-${stamp}`,
    studyPwa: `study-pwa-${stamp}`,
    childOrch: `batch-lead-${stamp}`,
    childOrchSession: `ses_batch_lead_${stamp}`,
    grandchildOrch: `batch-lead-deep-${stamp}`,
    grandchildOrchSession: `ses_batch_deep_${stamp}`,
    greatGrandchild: `batch-leaf-${stamp}`,
    tall: `tall-sub-${stamp}`,
    hiddenChild: `hidden-sub-${stamp}`,
    pausedChild: `paused-sub-${stamp}`,
    removedChild: `removed-sub-${stamp}`,
    hiddenOrch: `hidden-orch-${stamp}`,
    hiddenOrchSession: `ses_hidden_orch_${stamp}`,
    hiddenOrchChild: `hidden-orch-child-${stamp}`,
    missingParent: `ses_missing_${stamp}`,
    unlinkedA: `unlinked-a-${stamp}`,
    unlinkedB: `unlinked-b-${stamp}`,
    independent: `lone-agent-${stamp}`,
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
      const project = await api("POST", "/api/projects", { key: `H${stamp}`, name: `Hierarchy ${stamp}` });
      const issue = (title) => api("POST", "/api/issues", { title, project: project.key });
      const register = (agent, sessionId, parentSessionId) =>
        api("POST", "/api/agents/register", {
          agent_name: agent,
          session_id: sessionId,
          ...(parentSessionId ? { parent_session_id: parentSessionId } : {}),
          host: "test",
          pid: 42,
        });
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

      // Root 1: sourdough scribe, two dozen direct children. Two of them are
      // nested orchestrators so the three-level cap is exercised for real.
      await register(names.root, names.rootSession);
      await claim(names.root, names.rootSession, 1);
      const plain = [];
      for (let i = 0; i < 20; i++) {
        const agent = `sourdough-child-${stamp}-${i + 1}`;
        await register(agent, `ses_sourdough_child_${stamp}_${i + 1}`, names.rootSession);
        await claim(agent, `ses_sourdough_child_${stamp}_${i + 1}`, 1);
        plain.push(agent);
      }
      await register(names.tall, `ses_tall_${stamp}`, names.rootSession);
      await claim(names.tall, `ses_tall_${stamp}`, 95);
      await register(names.hiddenChild, `ses_hidden_child_${stamp}`, names.rootSession);
      await claim(names.hiddenChild, `ses_hidden_child_${stamp}`, 1);
      await api("POST", `/api/agents/${names.hiddenChild}/hide`);
      await register(names.pausedChild, `ses_paused_child_${stamp}`, names.rootSession);
      await claim(names.pausedChild, `ses_paused_child_${stamp}`, 1);
      await api("POST", `/api/agents/${names.pausedChild}/pause`);

      // Kill removes the agent: no roster row, no card, no placeholder.
      await register(names.removedChild, `ses_removed_child_${stamp}`, names.rootSession);
      await claim(names.removedChild, `ses_removed_child_${stamp}`, 1);
      await api("POST", `/api/agents/${names.removedChild}/kill`);

      await register(names.childOrch, names.childOrchSession, names.rootSession);
      await claim(names.childOrch, names.childOrchSession, 1);
      await register(names.grandchildOrch, names.grandchildOrchSession, names.childOrchSession);
      await claim(names.grandchildOrch, names.grandchildOrchSession, 1);
      await register(names.greatGrandchild, `ses_great_${stamp}`, names.grandchildOrchSession);
      await claim(names.greatGrandchild, `ses_great_${stamp}`, 1);
      await register(`batch-leaf-a-${stamp}`, `ses_batch_a_${stamp}`, names.childOrchSession);
      await register(`batch-leaf-b-${stamp}`, `ses_batch_b_${stamp}`, names.childOrchSession);

      // Root 2: study steward reports no parent, so it is a top-level section.
      await register(names.study, names.studySession);
      await claim(names.study, names.studySession, 1);
      for (const [agent, sid] of [
        [names.studyVoice, `ses_study_voice_${stamp}`],
        [names.crumbScholar, `ses_crumb_scholar_${stamp}`],
        [names.studyPwa, `ses_study_pwa_${stamp}`],
      ]) {
        await register(agent, sid, names.studySession);
        await claim(agent, sid, 1);
      }

      // Hidden orchestrator with visible children: a placeholder header must
      // keep the relationship readable.
      await register(names.hiddenOrch, names.hiddenOrchSession);
      await register(names.hiddenOrchChild, `ses_hidden_orch_child_${stamp}`, names.hiddenOrchSession);
      await claim(names.hiddenOrchChild, `ses_hidden_orch_child_${stamp}`, 1);
      await register(`hidden-orch-child-b-${stamp}`, `ses_hidden_orch_b_${stamp}`, names.hiddenOrchSession);
      await claim(`hidden-orch-child-b-${stamp}`, `ses_hidden_orch_b_${stamp}`, 1);
      await api("POST", `/api/agents/${names.hiddenOrch}/hide`);

      // Unlinked entries: the parent session has no matching roster row.
      for (const agent of [names.unlinkedA, names.unlinkedB]) {
        await register(agent, `ses_${agent}`, names.missingParent);
        await claim(agent, `ses_${agent}`, 1);
      }

      await register(names.independent, `ses_${names.independent}`);
      await claim(names.independent, `ses_${names.independent}`, 1);

      return { project: project.key, plain };
    },
    { stamp, names },
  );

  const sectionInfo = () =>
    page.$$eval("[data-section]", (sections) =>
      sections.map((section) => {
        const r = section.getBoundingClientRect();
        return {
          key: section.getAttribute("data-section"),
          kind: section.getAttribute("data-kind"),
          orchestrator: section.getAttribute("data-orchestrator"),
          text: section.querySelector(".group-head")?.textContent?.trim() ?? "",
          x: Math.round(r.x),
          y: Math.round(r.y),
        };
      }),
    );

  const cardInfo = () =>
    page.$$eval(".agent-card", (els) =>
      els.map((el) => {
        const r = el.getBoundingClientRect();
        return {
          name: el.getAttribute("data-agent"),
          role: el.getAttribute("data-role"),
          parent: el.getAttribute("data-parent"),
          direct: Number(el.getAttribute("data-direct-subagents") || 0),
          hidden: el.getAttribute("data-hidden") === "true",
          paused: el.classList.contains("paused"),
          pausedBadge: el.querySelector(".paused-badge")?.textContent?.trim() ?? null,
          x: Math.round(r.x),
          y: Math.round(r.y),
          width: Math.round(r.width),
          height: Math.round(r.height),
          tiles: el.querySelectorAll(".tile").length,
          section: el.closest("[data-section]")?.getAttribute("data-section") ?? null,
          tilesInCard: [...el.querySelectorAll(".tile")].every((t) => {
            const tr = t.getBoundingClientRect();
            return tr.x >= r.x - 1 && tr.x + tr.width <= r.x + r.width + 1;
          }),
          relation: el.querySelector(".rel-link")?.textContent?.trim() ?? null,
          rawParent: el.querySelector(".rel-raw")?.textContent?.trim() ?? null,
          relChips: el.querySelectorAll(".rel-chip").length,
          subs: el.querySelector(".subagent-badge")?.textContent?.trim() ?? null,
        };
      }),
    );

  const statValue = (label) =>
    page.$$eval(
      ".stat",
      (els, wanted) => {
        const el = els.find((e) => e.querySelector("span")?.textContent?.trim() === wanted);
        return el ? Number(el.querySelector("b")?.textContent?.trim()) : null;
      },
      label,
    );

  for (const width of [1680, 1280, 640]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.reload();
    await page.getByRole("button", { name: "Agents" }).click();
    await page.waitForSelector(".agent-card");
    await page.waitForTimeout(250);

    const sections = await sectionInfo();
    const cards = await cardInfo();
    const byName = Object.fromEntries(cards.map((c) => [c.name, c]));
    const rootSection = sections.find((s) => s.orchestrator === names.root);
    const studySection = sections.find((s) => s.orchestrator === names.study);
    const hiddenSection = sections.find((s) => s.orchestrator === names.hiddenOrch);
    const unlinkedSection = sections.find(
      (s) => s.kind === "unlinked" && s.text.includes(names.missingParent),
    );

    assert.ok(rootSection, `${width}: sourdough scribe has its own orchestrator section`);
    assert.equal(rootSection.kind, "orchestrator", `${width}: sourdough section is an orchestrator section`);
    assert.match(rootSection.text, /24 subagents/i, `${width}: section header names 24 direct subagents`);
    assert.ok(studySection, `${width}: study steward has its own section`);
    assert.equal(
      byName[names.study].parent,
      "",
      `${width}: a null parent_session_id keeps study steward at the root`,
    );
    assert.equal(byName[names.study].role, "orchestrator", `${width}: study steward reads as an orchestrator`);
    assert.notEqual(
      byName[names.study].section,
      byName[names.root].section,
      `${width}: study steward is not nested under sourdough scribe`,
    );

    assert.equal(byName[names.root].role, "orchestrator", `${width}: root card carries the orchestrator role`);
    assert.equal(byName[names.root].direct, 24, `${width}: root card counts 24 direct subagents`);
    assert.match(byName[names.root].subs ?? "", /^\d+ live$/, `${width}: root card shows a live subagent count`);
    assert.equal(byName[names.tall].role, "subagent", `${width}: tall child is tagged as a subagent`);
    assert.equal(byName[names.tall].parent, names.root, `${width}: tall child names its orchestrator`);
    assert.equal(byName[names.tall].tiles, 95, `${width}: tall card keeps its 95 claim tiles`);
    assert.ok(byName[names.tall].tilesInCard, `${width}: tall card tiles stay inside the card`);
    assert.ok(byName[names.tall].height > 300, `${width}: the tall card stays visibly tall`);

    assert.equal(byName[names.childOrch].role, "orchestrator", `${width}: a nested orchestrator reads as one`);
    assert.equal(byName[names.childOrch].parent, names.root, `${width}: nested orchestrator links to its parent`);
    assert.equal(
      byName[names.childOrch].section,
      byName[names.root].section,
      `${width}: nested orchestrator lives inside the root section`,
    );
    assert.ok(
      cards.some((c) => c.parent === names.childOrch),
      `${width}: grandchildren render under the nested orchestrator`,
    );

    assert.equal(byName[names.grandchildOrch]?.role, "orchestrator", `${width}: third-level orchestrator renders`);
    assert.equal(byName[names.greatGrandchild], undefined, `${width}: fourth level is capped`);
    const overflow = await page.locator(`[data-overflow-for="${names.grandchildOrch}"]`).innerText();
    assert.match(overflow, new RegExp(`1 more under\\s+${names.grandchildOrch}`, "i"), `${width}: cap says N more under X`);

    assert.equal(byName[names.hiddenChild], undefined, `${width}: hidden child is excluded by default`);
    assert.equal(byName[names.pausedChild]?.paused, true, `${width}: paused child renders as paused`);
    assert.equal(byName[names.pausedChild]?.pausedBadge, "paused", `${width}: paused child has a paused badge`);
    assert.equal(byName[names.removedChild], undefined, `${width}: a killed agent is absent from the cards`);
    const roster = await context.request.get(`${base}/api/agents/roster`);
    assert.ok(roster.ok(), `${width}: roster request succeeds`);
    const rosterNames = (await roster.json()).map((entry) => entry.agent_name);
    assert.ok(!rosterNames.includes(names.removedChild), `${width}: a killed agent is absent from /api/agents/roster`);
    assert.ok(hiddenSection, `${width}: hidden orchestrator keeps a section header`);
    assert.match(hiddenSection.text, new RegExp(names.hiddenOrch, "i"), `${width}: placeholder names the hidden orchestrator`);
    assert.match(hiddenSection.text, /hidden/i, `${width}: placeholder calls it hidden`);
    assert.ok(byName[names.hiddenOrchChild], `${width}: hidden orchestrator's child stays visible`);
    assert.equal(byName[names.hiddenOrch], undefined, `${width}: placeholder replaces the hidden card`);

    assert.ok(unlinkedSection, `${width}: unlinked agents have their own section`);
    assert.match(unlinkedSection.text, new RegExp(names.missingParent), `${width}: unlinked header carries the raw session id`);
    assert.equal(byName[names.unlinkedA].parent, "", `${width}: unlinked card has no resolved orchestrator`);
    assert.match(byName[names.unlinkedA].rawParent ?? "", new RegExp(names.missingParent), `${width}: unlinked card shows the full raw id`);
    assert.equal(byName[names.unlinkedA].relChips, 0, `${width}: no truncated relationship chip remains`);

    for (const card of cards) {
      assert.equal(card.relChips, 0, `${width}: ${card.name} keeps no legacy rel-chip`);
      if (card.parent) {
        assert.equal(card.relation, "orchestrator", `${width}: ${card.name} has a jump-to-orchestrator link`);
      }
    }

    assert.ok((await statValue("orchestrators")) >= 5, `${width}: header counts orchestrators`);
    assert.ok((await statValue("subagents")) >= 33, `${width}: header counts subagents`);
    assert.ok((await statValue("unlinked")) >= 2, `${width}: header counts unlinked agents`);
    assert.ok((await statValue("live sessions")) >= 2, `${width}: header keeps the live-session count`);
    assert.ok((await statValue("claim holders")) >= 2, `${width}: header keeps the claim-holder count`);

    const rootChildren = cards.filter((c) => c.section === rootSection.key);
    const columns = new Set(rootChildren.map((c) => c.x)).size;
    assert.ok(columns >= (width === 640 ? 1 : 2), `${width}: subagents use ${columns} columns inside the section`);

    if (width === 1680) {
      // Focus jumps to the whole tree and leaves the other root out.
      await page.locator(`[data-agent="${names.tall}"] .rel-link`).click();
      await page.waitForTimeout(250);
      const focusText = await page.locator(".wrap").innerText();
      assert.match(focusText, new RegExp(`Focused on\\s+${names.root}`, "i"), `${width}: jump focuses the orchestrator`);
      const focused = await cardInfo();
      assert.ok(focused.some((c) => c.name === names.root), `${width}: focused root stays visible`);
      assert.ok(!focused.some((c) => c.name === names.study), `${width}: jump hides unrelated root sections`);
      await page.getByRole("button", { name: "Clear", exact: true }).click();
      await page.waitForTimeout(150);

      // Revealing hidden restores the hidden orchestrator card and child.
      await page.getByRole("button", { name: /Show hidden/i }).click();
      await page.waitForTimeout(250);
      const revealed = await cardInfo();
      const revealedHidden = revealed.find((c) => c.name === names.hiddenOrch);
      assert.equal(revealedHidden?.hidden, true, `${width}: hidden orchestrator card is revealed`);
      assert.equal(revealedHidden?.role, "orchestrator", `${width}: revealed orchestrator keeps its role`);
      assert.ok(revealed.some((c) => c.name === names.hiddenChild), `${width}: hidden child is revealed`);
      await page.getByRole("button", { name: /Showing hidden/i }).click();
      await page.waitForTimeout(150);
    }
  }

  // Stability: a heartbeat and a roster refresh must not reorder sections or
  // cards. Check at 1280 with the fixture's biggest tree.
  await page.setViewportSize({ width: 1280, height: 1000 });
  await page.reload();
  await page.getByRole("button", { name: "Agents" }).click();
  await page.waitForSelector(".agent-card");
  await page.waitForTimeout(250);
  const beforeSections = (await sectionInfo()).map((s) => s.key);
  const beforeCards = (await cardInfo()).map((c) => c.name);
  await page.evaluate(
    async ({ agent, sessionId, parent }) => {
      await fetch("/api/agents/register", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          agent_name: agent,
          session_id: sessionId,
          parent_session_id: parent,
          note: "heartbeat",
        }),
      });
    },
    { agent: names.tall, sessionId: `ses_tall_${stamp}`, parent: names.rootSession },
  );
  await page.getByRole("button", { name: "Refresh" }).click();
  await page.waitForTimeout(450);
  const afterSections = (await sectionInfo()).map((s) => s.key);
  const afterCards = (await cardInfo()).map((c) => c.name);
  assert.deepEqual(afterSections, beforeSections, "section order is stable across refresh and heartbeat");
  assert.deepEqual(afterCards, beforeCards, "card order is stable across refresh and heartbeat");

  console.log("agents hierarchy browser regressions: passed", fixture.project);
} finally {
  await browser.close();
}
