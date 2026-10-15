import assert from "node:assert/strict";
import { chromium } from "playwright";

const base = process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7290";
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});
try {
  const context = await browser.newContext();
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();
  await page.goto(base);

  const stamp = Date.now();
  const fixture = await page.evaluate(async (stamp) => {
    const headers = { "content-type": "application/json" };
    const createProject = async (key, name) => {
      const response = await fetch("/api/projects", {
        method: "POST",
        headers,
        body: JSON.stringify({ key, name }),
      });
      if (!response.ok) throw new Error(`create ${key}: ${response.status} ${await response.text()}`);
      return response.json();
    };
    const alpha = await createProject(`A${stamp}`, "Alpha");
    const beta = await createProject(`B${stamp}`, "Beta");
    const cycleName = `Browser ${stamp}`;
    const created = await fetch("/api/cycles", {
      method: "POST",
      headers,
      body: JSON.stringify({
        name: cycleName,
        starts_on: "2026-10-01",
        ends_on: "2026-10-31",
        project_ids: [alpha.id],
      }),
    });
    if (!created.ok) throw new Error(`create cycle: ${created.status} ${await created.text()}`);
    const cycle = await created.json();
    return { alpha, beta, cycleName, cycleId: cycle.id };
  }, stamp);

  const persistedIds = () =>
    page.evaluate(async (cycleId) => {
      const r = await fetch("/api/cycles");
      if (!r.ok) throw new Error(`list cycles: ${r.status}`);
      const cycles = await r.json();
      const found = cycles.find((c) => c.id === cycleId);
      if (!found) throw new Error("fixture cycle missing from GET /api/cycles");
      return found.project_ids;
    }, fixture.cycleId);

  const goSettings = async () => {
    await page.getByRole("button", { name: "Settings" }).click();
    await page.locator("tbody tr", { hasText: fixture.cycleName }).waitFor();
  };
  const row = () => page.locator("tbody tr", { hasText: fixture.cycleName });
  const selector = () => row().locator("select[multiple]");
  const selected = async () => selector().locator("option:checked").allTextContents();
  const waitPatch = () =>
    page.waitForResponse(
      (r) => r.url().includes("/api/cycles/") && r.request().method() === "PATCH",
    );

  await goSettings();

  // 1. Initial persisted selection renders instead of an empty control.
  assert.deepEqual(await selected(), [fixture.alpha.key], "saved defaults render on first load");
  assert.deepEqual(await persistedIds(), [fixture.alpha.id]);

  // 2. Additive edit keeps the existing default.
  await Promise.all([waitPatch(), selector().selectOption([String(fixture.alpha.id), String(fixture.beta.id)])]);
  await page.waitForFunction(
    ([name, want]) => {
      const rows = [...document.querySelectorAll("tbody tr")];
      const target = rows.find((tr) => tr.textContent.includes(name));
      if (!target) return false;
      const checked = [...target.querySelectorAll("option:checked")].map((o) => o.textContent);
      return checked.length === want;
    },
    [fixture.cycleName, 2],
  );
  assert.deepEqual(await selected(), [fixture.alpha.key, fixture.beta.key], "editing is additive");
  assert.deepEqual(await persistedIds(), [fixture.alpha.id, fixture.beta.id]);

  // 3. Saved selections survive reload.
  await page.reload();
  await goSettings();
  assert.deepEqual(await selected(), [fixture.alpha.key, fixture.beta.key], "saved selections survive reload");

  // 4. A rejected PATCH rolls back to the persisted selection.
  await page.route("**/api/cycles/*", (route) =>
    route.request().method() === "PATCH"
      ? route.fulfill({ status: 400, contentType: "application/json", body: '{"error":"rejected"}' })
      : route.continue(),
  );
  await selector().selectOption([String(fixture.beta.id)]);
  await page.waitForResponse((r) => r.url().includes("/api/cycles") && r.request().method() === "GET");
  await page.waitForFunction(
    ([name, want]) => {
      const rows = [...document.querySelectorAll("tbody tr")];
      const target = rows.find((tr) => tr.textContent.includes(name));
      if (!target) return false;
      return target.querySelectorAll("option:checked").length === want;
    },
    [fixture.cycleName, 2],
  );
  assert.deepEqual(await selected(), [fixture.alpha.key, fixture.beta.key], "rejected writes reload persisted selection");
  assert.deepEqual(await persistedIds(), [fixture.alpha.id, fixture.beta.id]);

  // 5. Defaults can be cleared.
  await page.unrouteAll();
  await Promise.all([waitPatch(), selector().selectOption([])]);
  await page.waitForFunction(
    ([name]) => {
      const rows = [...document.querySelectorAll("tbody tr")];
      const target = rows.find((tr) => tr.textContent.includes(name));
      if (!target) return false;
      return target.querySelectorAll("option:checked").length === 0;
    },
    [fixture.cycleName],
  );
  assert.deepEqual(await selected(), [], "defaults can be cleared");
  assert.deepEqual(await persistedIds(), []);
  console.log("cycle defaults browser regressions: passed");
} finally {
  await browser.close();
}
