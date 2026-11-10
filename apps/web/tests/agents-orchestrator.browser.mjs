import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for the manual orchestrator controls on the Agents page:
// drag a card onto another card to parent it, drag it to the top-level target
// to clear, refuse a cycle with a visible message, use the card menu instead of
// dragging, undo the optimistic move, and keep hidden state untouched.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7291 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-orchestrator.browser.mjs
//
// The fixture creates its own project and agents, so it is safe on a scratch
// database and does not depend on the production board.

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7291";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 1680, height: 1000 } });
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();
  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const names = {
    root: `orch-root-${stamp}`,
    rootSession: `ses_orch_root_${stamp}`,
    other: `orch-other-${stamp}`,
    otherSession: `ses_orch_other_${stamp}`,
    child: `drag-child-${stamp}`,
    childSession: `ses_drag_child_${stamp}`,
    grandchild: `drag-leaf-${stamp}`,
    grandchildSession: `ses_drag_leaf_${stamp}`,
    hidden: `hidden-child-${stamp}`,
    hiddenSession: `ses_hidden_child_${stamp}`,
  };

  const fixture = await page.evaluate(
    async ({ stamp, names }) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const project = await api("POST", "/api/projects", {
        key: `O${stamp}`,
        name: `Orchestrator ${stamp}`,
      });
      const issue = (title) => api("POST", "/api/issues", { title, project: project.key });
      const register = (agent, sessionId, parentSessionId) =>
        api("POST", "/api/agents/register", {
          agent_name: agent,
          session_id: sessionId,
          ...(parentSessionId ? { parent_session_id: parentSessionId } : {}),
          host: "test",
          pid: 42,
        });
      const claim = async (agent, sessionId) => {
        const created = await issue(`${agent} ${stamp}`);
        await api("POST", `/api/issues/${created.key}/claim`, {
          agent_name: agent,
          session_id: sessionId,
          message: `${agent} fixture`,
        });
      };

      await register(names.root, names.rootSession);
      await claim(names.root, names.rootSession);
      await register(names.other, names.otherSession);
      await claim(names.other, names.otherSession);
      await register(names.child, names.childSession, names.rootSession);
      await claim(names.child, names.childSession);
      await register(names.grandchild, names.grandchildSession, names.childSession);
      await claim(names.grandchild, names.grandchildSession);
      await register(names.hidden, names.hiddenSession, names.rootSession);
      await claim(names.hidden, names.hiddenSession);
      await api("POST", `/api/agents/${names.hidden}/hide`);

      return { project: project.key };
    },
    { stamp, names },
  );

  const rosterRow = async (name) => {
    const response = await context.request.get(`${base}/api/agents/roster`);
    assert.ok(response.ok(), "roster request succeeds");
    const roster = await response.json();
    const row = roster.find((entry) => entry.agent_name === name);
    assert.ok(row, `${name} is on the roster`);
    return row;
  };

  const card = (name) => page.locator(`[data-agent="${name}"]`);
  const toastText = () => page.locator(".toast").innerText();

  // Playwright's pointer-driven dragTo does not start a native HTML5 drag on a
  // draggable div in Chromium, so dispatch the browser DragEvents the page's
  // handlers listen for. This still runs the real Svelte handlers and a real
  // DataTransfer.
  const dragCard = async (sourceName, targetSelector) => {
    await page.evaluate(
      ({ sourceName, targetSelector }) => {
        const source = document.querySelector(`[data-agent="${sourceName}"]`);
        const target = document.querySelector(targetSelector);
        if (!source) throw new Error(`drag source ${sourceName} is not rendered`);
        if (!target) throw new Error(`drop target ${targetSelector} is not rendered`);
        const data = new DataTransfer();
        source.dispatchEvent(
          new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer: data }),
        );
        target.dispatchEvent(
          new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: data }),
        );
        target.dispatchEvent(
          new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: data }),
        );
        source.dispatchEvent(
          new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer: data }),
        );
      },
      { sourceName, targetSelector },
    );
    await page.waitForTimeout(450);
  };

  const openAgents = async () => {
    if (await page.locator(".agent-card").count()) return;
    await page.getByRole("button", { name: "Agents" }).click();
    await page.waitForSelector(".agent-card");
    await page.waitForTimeout(200);
  };

  for (const width of [1680, 1280]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.reload();
    await openAgents();

    // Reset to a known tree: child under root, hidden child under root.
    await page.evaluate(
      async ({ names }) => {
        const post = (agent, orchestrator) =>
          fetch(`/api/agents/${agent}/orchestrator`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(orchestrator ? { orchestrator } : {}),
          });
        await post(names.child, names.root);
        await post(names.hidden, names.root);
      },
      { names },
    );
    await page.getByRole("button", { name: "Refresh" }).click();
    await page.waitForTimeout(350);

    // Drag the child onto the second root.
    await dragCard(names.child, `[data-agent="${names.other}"]`);
    assert.match(await toastText(), new RegExp(`Moved ${names.child} under ${names.other}`, "i"));
    assert.equal(
      (await rosterRow(names.child)).parent_session_id,
      names.otherSession,
      `${width}: drag sets the parent session`,
    );
    assert.equal(
      await card(names.child).getAttribute("data-parent"),
      names.other,
      `${width}: the card re-renders under its new orchestrator`,
    );

    // Undo is one click and restores the previous parent.
    await page.getByRole("button", { name: "Undo" }).click();
    await page.waitForTimeout(400);
    assert.equal(
      (await rosterRow(names.child)).parent_session_id,
      names.rootSession,
      `${width}: undo restores the previous orchestrator`,
    );

    // The card menu is the non-drag path.
    await card(names.child).locator(".card-menu-btn").click();
    await page.locator(`#card-orchestrator-${names.child}`).selectOption(names.other);
    await card(names.child).getByRole("menuitem", { name: "Apply" }).click();
    await page.waitForTimeout(400);
    assert.equal(
      (await rosterRow(names.child)).parent_session_id,
      names.otherSession,
      `${width}: card menu sets the orchestrator`,
    );

    // Drop on the root target to clear it.
    await dragCard(names.child, '[data-drop-root="true"]');
    assert.ok(
      (await rosterRow(names.child)).parent_session_id == null,
      `${width}: root drop clears the orchestrator`,
    );
    assert.equal(await card(names.child).getAttribute("data-parent"), "", `${width}: card returns to the root`);

    // A cycle is refused with a message, not a silent no-op.
    await dragCard(names.child, `[data-agent="${names.grandchild}"]`);
    assert.match(await toastText(), /cycle/i, `${width}: a cycle drop explains the refusal`);
    assert.ok(
      (await rosterRow(names.child)).parent_session_id == null,
      `${width}: a refused cycle leaves the tree alone`,
    );

    if (width === 1680) {
      // A hidden card can be moved while revealed, and it stays hidden.
      await page.getByRole("button", { name: /Show hidden/i }).click();
      await page.waitForTimeout(300);
      await dragCard(names.hidden, `[data-agent="${names.other}"]`);
      const hidden = await rosterRow(names.hidden);
      assert.equal(hidden.parent_session_id, names.otherSession, `${width}: hidden card moved`);
      assert.equal(hidden.hidden, true, `${width}: moving a hidden card never unhides it`);
      await dragCard(names.hidden, '[data-drop-root="true"]');
      const cleared = await rosterRow(names.hidden);
      assert.ok(cleared.parent_session_id == null, `${width}: hidden card cleared`);
      assert.equal(cleared.hidden, true, `${width}: clearing never unhides it`);
    }
  }

  console.log("agents orchestrator browser regressions: passed", fixture.project);
} finally {
  await browser.close();
}
