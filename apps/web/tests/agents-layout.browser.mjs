import assert from "node:assert/strict";
import { chromium } from "playwright";

// Manual regression check for the Agents roster: server-side hiding plus the
// masonry layout around a tall card. Run against a local server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7290 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/agents-layout.browser.mjs
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
  const fixture = await page.evaluate(
    async ({ stamp }) => {
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
      const project = await api("POST", "/api/projects", { key: `L${stamp}`, name: `Layout ${stamp}` });
      const issue = async (title) => api("POST", "/api/issues", { title, project: project.key });
      const claim = async (agent, count) => {
        for (let i = 0; i < count; i++) {
          const created = await issue(`${agent} ${stamp} ${i + 1}`);
          await api("POST", `/api/issues/${created.key}/claim`, {
            agent_name: agent,
            session_id: `ses_${agent}_${stamp}`,
            message: `${agent} fixture`,
          });
        }
      };

      await claim(`rye-ranger`, 95);
      for (const name of [
        `crumb-marshal-${stamp}`,
        `zesty-pretzel-${stamp}`,
        `sourdough-sentinel-${stamp}`,
        `toasty-ciabatta-${stamp}`,
      ]) {
        await claim(name, 1);
      }
      for (const name of [`idle-a-${stamp}`, `idle-b-${stamp}`, `idle-c-${stamp}`]) {
        await api("POST", "/api/agents/register", {
          agent_name: name,
          session_id: `ses_${name}`,
          host: "test",
          pid: 42,
        });
      }

      const hiddenClaim = `hidden-claim-${stamp}`;
      await claim(hiddenClaim, 1);
      await api("POST", `/api/agents/${hiddenClaim}/hide`);
      const hiddenIdle = `hidden-idle-${stamp}`;
      await api("POST", "/api/agents/register", { agent_name: hiddenIdle, session_id: `ses_${hiddenIdle}` });
      await api("POST", `/api/agents/${hiddenIdle}/hide`);
      const killed = `killed-${stamp}`;
      await api("POST", "/api/agents/register", { agent_name: killed, session_id: `ses_${killed}` });
      await api("POST", `/api/agents/${killed}/kill`);
      await api("POST", `/api/agents/${killed}/hide`);

      return { tall: "rye-ranger", hiddenClaim, hiddenIdle, killed, project: project.key };
    },
    { stamp },
  );

  const cardInfo = () =>
    page.$$eval(".agent-card", (els) =>
      els.map((el) => {
        const r = el.getBoundingClientRect();
        return {
          name: el.getAttribute("data-agent"),
          hidden: el.getAttribute("data-hidden") === "true",
          x: Math.round(r.x),
          y: Math.round(r.y),
          width: Math.round(r.width),
          height: Math.round(r.height),
          tiles: el.querySelectorAll(".tile").length,
          tilesInCard: [...el.querySelectorAll(".tile")].every((t) => {
            const tr = t.getBoundingClientRect();
            return tr.x >= r.x - 1 && tr.x + tr.width <= r.x + r.width + 1;
          }),
          unhide: !!el.querySelector(".unhide-card"),
        };
      }),
    );

  for (const width of [1680, 1280, 640]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.reload();
    await page.getByRole("button", { name: "Agents" }).click();
    await page.waitForSelector(".agent-card");
    await page.waitForTimeout(200);

    const shown = await cardInfo();
    assert.equal(shown.filter((c) => c.hidden).length, 0, `${width}: hidden agents are excluded by default`);
    const tall = shown.find((c) => c.name === fixture.tall);
    assert.ok(tall, `${width}: tall card is present`);
    assert.ok(tall.tiles >= 95, `${width}: tall card keeps its ${tall.tiles} claim tiles`);
    assert.ok(tall.tilesInCard, `${width}: claim tiles stay inside the card`);

    const columns = new Set(shown.map((c) => c.x)).size;
    assert.ok(columns >= 2, `${width}: masonry uses ${columns} columns`);
    const besideTall = shown.filter(
      (c) => c.name !== fixture.tall && c.x > tall.x + 20 && c.y < tall.y + tall.height * 0.6,
    );
    assert.ok(besideTall.length > 0, `${width}: cards tile beside the tall card`);

    const text = await page.locator(".wrap").innerText();
    assert.match(text, /shown/i, `${width}: header names the shown count`);
    assert.match(text, /hidden/i, `${width}: header names the hidden count`);
    assert.match(text, /still hold/i, `${width}: hidden claims are called out`);

    await page.getByRole("button", { name: /Show hidden/i }).click();
    await page.waitForTimeout(200);
    const revealed = await cardInfo();
    for (const name of [fixture.hiddenClaim, fixture.hiddenIdle, fixture.killed]) {
      const card = revealed.find((c) => c.name === name);
      assert.ok(card?.hidden, `${width}: ${name} is revealed`);
      assert.ok(card?.unhide, `${width}: ${name} has an Unhide control`);
    }
    const claimHidden = revealed.find((c) => c.name === fixture.hiddenClaim);
    assert.equal(claimHidden.tiles, 1, `${width}: a hidden agent keeps its claim tile`);

    if (width === 1280) {
      await page.getByRole("button", { name: /Showing hidden/i }).click();
      await page.waitForTimeout(100);
      const before = (await cardInfo()).map((c) => c.name);
      await page.getByRole("button", { name: "Refresh" }).click();
      await page.waitForTimeout(400);
      const after = (await cardInfo()).map((c) => c.name);
      assert.deepEqual(after, before, "card order is stable across a roster refresh");
    }
  }
  console.log("agents roster/layout browser regressions: passed");
} finally {
  await browser.close();
}
