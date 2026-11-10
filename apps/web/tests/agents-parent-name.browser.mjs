import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-219: the hierarchy edge is organisational, not a
// session property. A sessionless card drags under a sessionful orchestrator,
// a sessionful card drags under a sessionless orchestrator, and a mixed-graph
// cycle is refused with a visible message rather than a silent no-op.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7291 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-parent-name.browser.mjs
//
// The fixture creates its own agents, so it is safe on a scratch database and
// does not depend on the production board. With no BREADBOARD_TEST_SESSION it
// uses /auth/dev-login, so it also works with BREADBOARD_DEV_LOGIN=1.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7291").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({ viewport: { width: 1680, height: 1000 } });
  if (session) await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  if (!session) {
    const login = await page.request.post(`${base}/auth/dev-login`, {
      data: { login: `parent-name-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const names = {
    sessionOrch: `pn-orch-${stamp}`,
    sessionOrchSession: `ses_pn_orch_${stamp}`,
    sessionlessChild: `pn-sessionless-${stamp}`,
    nameOnlyOrch: `pn-name-orch-${stamp}`,
    sessionChild: `pn-session-child-${stamp}`,
    sessionChildSession: `ses_pn_child_${stamp}`,
    mixedRoot: `pn-mixed-root-${stamp}`,
    mixedMid: `pn-mixed-mid-${stamp}`,
    mixedMidSession: `ses_pn_mixed_mid_${stamp}`,
    mixedLeaf: `pn-mixed-leaf-${stamp}`,
    mixedLeafSession: `ses_pn_mixed_leaf_${stamp}`,
  };

  await page.evaluate(
    async (names) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      const register = (agent_name, session_id) =>
        api("POST", "/api/agents/register", {
          agent_name,
          ...(session_id ? { session_id } : {}),
          host: "test",
          pid: 42,
        });

      await register(names.sessionOrch, names.sessionOrchSession);
      await register(names.sessionlessChild);
      await register(names.nameOnlyOrch);
      await register(names.sessionChild, names.sessionChildSession);

      // root by name -> mid by name -> leaf by session id. A cycle attempted
      // from root onto leaf has to walk both representations.
      await register(names.mixedRoot);
      await register(names.mixedMid, names.mixedMidSession);
      await api("POST", `/api/agents/${names.mixedMid}/orchestrator`, {
        orchestrator: names.mixedRoot,
      });
      await register(names.mixedLeaf, names.mixedLeafSession);
      await api("POST", `/api/agents/${names.mixedLeaf}/orchestrator`, {
        orchestrator: names.mixedMidSession,
      });
    },
    names,
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
  const waitCard = (name) =>
    page.waitForSelector(`[data-agent="${name}"]`, { state: "visible", timeout: 5000 });
  const waitToast = (text) =>
    page.waitForFunction(
      (expected) =>
        (document.querySelector(".toast")?.textContent ?? "")
          .toLowerCase()
          .includes(expected.toLowerCase()),
      text,
      { timeout: 3000 },
    );

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

  const hoverCard = async (sourceName, targetSelector) => {
    return page.evaluate(
      async ({ sourceName, targetSelector }) => {
        const source = document.querySelector(`[data-agent="${sourceName}"]`);
        const target = document.querySelector(targetSelector);
        if (!source || !target) throw new Error("drag hover elements are not rendered");
        const data = new DataTransfer();
        source.dispatchEvent(
          new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer: data }),
        );
        target.dispatchEvent(
          new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: data }),
        );
        await new Promise((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(resolve)),
        );
        return document.querySelector(".drag-reason")?.textContent ?? "";
      },
      { sourceName, targetSelector },
    );
  };

  if (!(await page.locator(".agent-card").count())) {
    await page.getByRole("button", { name: "Agents" }).click();
    await page.waitForSelector(".agent-card");
  }
  await waitCard(names.sessionlessChild);
  await waitCard(names.sessionOrch);
  await page.waitForTimeout(250);

  // Sessionless child under a sessionful orchestrator.
  await waitCard(names.sessionlessChild);
  await waitCard(names.sessionOrch);
  await dragCard(names.sessionlessChild, `[data-agent="${names.sessionOrch}"]`);
  await waitToast(`Moved ${names.sessionlessChild} under ${names.sessionOrch}`);
  let row = await rosterRow(names.sessionlessChild);
  assert.equal(row.parent_agent_name, names.sessionOrch, "the name edge is stored");
  assert.equal(row.parent_session_id, names.sessionOrchSession, "the session is kept as fallback");
  assert.equal(row.orchestrator, names.sessionOrch);
  assert.equal(row.orchestrator_session_id, names.sessionOrchSession);
  assert.equal(
    await card(names.sessionlessChild).getAttribute("data-parent"),
    names.sessionOrch,
    "the sessionless card re-renders under its orchestrator",
  );

  // Sessionful child under a sessionless orchestrator.
  await waitCard(names.sessionChild);
  await waitCard(names.nameOnlyOrch);
  await dragCard(names.sessionChild, `[data-agent="${names.nameOnlyOrch}"]`);
  await waitToast(`Moved ${names.sessionChild} under ${names.nameOnlyOrch}`);
  row = await rosterRow(names.sessionChild);
  assert.equal(row.parent_agent_name, names.nameOnlyOrch);
  assert.equal(row.parent_session_id ?? null, null, "no session id is invented for the target");
  assert.equal(row.orchestrator, names.nameOnlyOrch);
  assert.equal(row.orchestrator_session_id ?? null, null);
  assert.equal(
    await card(names.sessionChild).getAttribute("data-parent"),
    names.nameOnlyOrch,
  );

  // A mixed-graph cycle is refused with a visible message, and the hover
  // before the drop already names the reason.
  await waitCard(names.mixedRoot);
  await waitCard(names.mixedLeaf);
  const reason = await hoverCard(names.mixedRoot, `[data-agent="${names.mixedLeaf}"]`);
  assert.match(reason, /cycle/i, "the hover surfaces the cycle reason");
  await page.evaluate(() => {
    const source = document.querySelector('[data-agent^="pn-mixed-root-"]');
    source?.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true }));
  });
  await dragCard(names.mixedRoot, `[data-agent="${names.mixedLeaf}"]`);
  await waitToast("cycle");
  row = await rosterRow(names.mixedRoot);
  assert.equal(row.parent_agent_name ?? null, null, "a refused cycle leaves the tree alone");
  assert.equal(row.parent_session_id ?? null, null);

  console.log("agents parent-name browser regressions: passed");
} finally {
  await browser.close();
}
