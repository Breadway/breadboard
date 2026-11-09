import assert from "node:assert/strict";
import { chromium } from "playwright";

// Regression check for messaging an agent from its expanded card and the
// stable /chat?dm=<agent> deep link. The fixture registers a claimless agent,
// creates its DM and leaves one unread message, so the card has something real
// to report before the human opens the room.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7290 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/chat-dm.browser.mjs
//
// The fixture creates its own agent and DM, so it is safe on a scratch
// database.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7290").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 1000 },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();
  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const agent = `courier-${stamp}`;
  const firstMessage = `hello from ${agent} ${stamp}`;
  const reply = `reply to ${agent} ${stamp}`;

  const fixture = await page.evaluate(
    async ({ agent, firstMessage }) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      // Registered on the roster, no claim: the common idle-agent case.
      await api("POST", "/api/agents/register", {
        agent_name: agent,
        session_id: `ses_${agent}`,
        host: "test",
        pid: 77,
      });
      const room = await api("POST", "/api/chat/dm", { with: agent });
      await api("POST", "/api/chat/messages", {
        agent_name: agent,
        room: String(room.id),
        body: firstMessage,
      });
      return { room };
    },
    { agent, firstMessage },
  );

  // A claimless but registered agent still gets a roster card.
  const claims = await (await context.request.get(`${base}/api/agents`)).json();
  assert.ok(
    !claims.some((c) => c.agent_name.toLowerCase() === agent),
    "the fixture agent holds no claims",
  );
  const roster = await (await context.request.get(`${base}/api/agents/roster`)).json();
  assert.ok(
    roster.some((r) => r.agent_name.toLowerCase() === agent),
    "the fixture agent is registered on the roster",
  );

  await page.getByRole("button", { name: "Agents" }).click();
  await page.waitForSelector(`[data-agent="${agent}"]`);
  await page.locator(`[data-agent="${agent}"]`).click();

  const modal = page.locator(".modal");
  await modal.waitFor({ state: "visible" });
  await modal.getByRole("button", { name: "Message", exact: true }).waitFor({ state: "visible" });

  const href = await modal.locator(".dm-link").getAttribute("href");
  assert.equal(href, `/chat?dm=${encodeURIComponent(agent)}`, "the card carries the stable DM link");
  const unread = modal.locator(".chat-row");
  assert.equal(
    await unread.getAttribute("data-dm-unread"),
    "1",
    "the card shows the DM's unread count",
  );
  assert.match(await unread.innerText(), /1 unread/, "the unread count is readable on the card");

  // The link is copyable in one click and resolves to an absolute URL.
  await modal.getByRole("button", { name: "Copy link" }).click();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  assert.equal(copied, `${base}${href}`, "Copy link writes the absolute deep link");

  // Message opens or creates the DM, lands on that room and leaves a copyable
  // URL in the address bar.
  await modal.getByRole("button", { name: "Message", exact: true }).click();
  await page.waitForSelector(".chat");
  await page.waitForFunction(
    (name) => document.querySelector(".mhead h2")?.textContent?.includes(name),
    agent,
  );
  assert.match(await page.locator(".mhead h2").innerText(), new RegExp(agent));
  assert.ok(
    !(await page.locator(".mhead h2").innerText()).includes("lobby"),
    "Message lands on the DM, not the lobby",
  );
  const opened = new URL(page.url());
  assert.equal(opened.pathname, "/chat", "the DM URL uses the chat path");
  assert.equal(opened.searchParams.get("dm"), agent, "the DM URL names the agent");
  assert.ok(
    await page.locator(".messages .body", { hasText: firstMessage }).first().isVisible(),
    "the existing DM message is visible after Message",
  );

  // Sending from the human side works even though the agent has no claims.
  await page.locator('input[aria-label="Message"]').fill(reply);
  await page.getByRole("button", { name: "Send" }).click();
  await page.locator(".messages .body", { hasText: reply }).first().waitFor({ state: "visible" });

  // A cold load of the copied link reopens the same room. It must not fall
  // back to the lobby while the room list resolves.
  await page.goto(`${base}${href}`);
  await page.waitForSelector(".chat");
  await page.waitForFunction(
    (name) => document.querySelector(".mhead h2")?.textContent?.includes(name),
    agent,
  );
  const cold = new URL(page.url());
  assert.equal(cold.pathname, "/chat", "a cold deep link keeps the chat path");
  assert.equal(cold.searchParams.get("dm"), agent, "a cold deep link keeps the agent");
  assert.match(await page.locator(".mhead h2").innerText(), new RegExp(agent));
  assert.ok(
    !(await page.locator(".mhead h2").innerText()).includes("lobby"),
    "a cold deep link lands on the DM, not the lobby",
  );
  const selectedRoom = await page.locator(".rooms .room.dm.on .rname").innerText();
  assert.match(selectedRoom, new RegExp(agent), "the DM is the selected room in the sidebar");
  assert.ok(await page.locator(".messages .body", { hasText: firstMessage }).first().isVisible(), "the DM history survives a cold load");
  assert.ok(await page.locator(".messages .body", { hasText: reply }).first().isVisible(), "the human reply survives a cold load");

  // A deep link for a registered agent with no DM yet creates the room. The
  // room-list fetch that started before the create must not steal the pick.
  const freshAgent = `courier-new-${stamp}`;
  await page.evaluate(
    async ({ agent }) => {
      const response = await fetch("/api/agents/register", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ agent_name: agent, session_id: `ses_${agent}`, host: "test" }),
      });
      if (!response.ok) throw new Error(`register: ${response.status}`);
    },
    { agent: freshAgent },
  );
  await page.goto(`${base}/chat?dm=${encodeURIComponent(freshAgent)}`);
  await page.waitForSelector(".chat");
  await page.waitForFunction(
    (name) => document.querySelector(".mhead h2")?.textContent?.includes(name),
    freshAgent,
  );
  const freshUrl = new URL(page.url());
  assert.equal(freshUrl.pathname, "/chat", "a new-DM deep link keeps the chat path");
  assert.equal(freshUrl.searchParams.get("dm"), freshAgent, "a new-DM deep link names the agent");
  assert.ok(
    !(await page.locator(".mhead h2").innerText()).includes("lobby"),
    "a new-DM deep link creates the room instead of falling back to the lobby",
  );
  await page.locator(".messages").getByText("Start the DM.").waitFor({ state: "visible" });
  await page.locator('input[aria-label="Message"]').fill(`first note to ${freshAgent}`);
  await page.getByRole("button", { name: "Send" }).click();
  await page.locator(".messages .body", { hasText: `first note to ${freshAgent}` }).first().waitFor({ state: "visible" });
  const rooms = await (await context.request.get(`${base}/api/chat/rooms`)).json();
  assert.ok(
    rooms.some(
      (r) =>
        r.kind === "dm" &&
        r.members.some((m) => m.toLowerCase() === `agent:${freshAgent}`),
    ),
    "the cold deep link created the DM",
  );

  // Race regression: hold the first room-list snapshot (taken before the DM
  // exists), let openDm and pick finish, then release it. The stale response
  // must not replace the applied list or move the header back to #lobby.
  const raceAgent = `courier-race-${stamp}`;
  await page.evaluate(
    async ({ agent }) => {
      const response = await fetch("/api/agents/register", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ agent_name: agent, session_id: `ses_${agent}`, host: "test" }),
      });
      if (!response.ok) throw new Error(`register: ${response.status}`);
    },
    { agent: raceAgent },
  );

  let heldRoute;
  let heldSnapshot;
  let roomsCalls = 0;
  let releaseFirst;
  const firstRequestHeld = new Promise((resolve) => (releaseFirst = resolve));
  await page.route("**/api/chat/rooms", async (route) => {
    if (roomsCalls++ === 0) {
      // Fetch now, before the DM exists, then keep the response parked.
      const response = await route.fetch();
      heldSnapshot = {
        status: response.status(),
        headers: response.headers(),
        body: await response.body(),
      };
      heldRoute = route;
      releaseFirst();
      return;
    }
    await route.continue();
  });
  // Do not create the DM until the stale snapshot has been captured.
  await page.route("**/api/chat/dm", async (route) => {
    await firstRequestHeld;
    await route.continue();
  });

  await page.goto(`${base}/chat?dm=${encodeURIComponent(raceAgent)}`);
  await page.waitForSelector(".chat");
  await page.waitForFunction(
    (name) => document.querySelector(".mhead h2")?.textContent?.includes(name),
    raceAgent,
  );

  const snapshotRooms = JSON.parse(heldSnapshot.body.toString());
  assert.ok(
    !snapshotRooms.some(
      (r) =>
        r.kind === "dm" &&
        r.members.some((m) => m.toLowerCase() === `agent:${raceAgent}`),
    ),
    "the held room list predates the DM",
  );

  // Release the stale snapshot after pick has completed.
  await heldRoute.fulfill({
    status: heldSnapshot.status,
    headers: heldSnapshot.headers,
    body: heldSnapshot.body,
  });
  await page.waitForTimeout(500);

  const racedHeader = await page.locator(".mhead h2").innerText();
  assert.match(racedHeader, new RegExp(raceAgent), "a stale room list must not change the DM header");
  assert.ok(!racedHeader.includes("lobby"), "a stale room list must not show the lobby");
  const racedUrl = new URL(page.url());
  assert.equal(racedUrl.pathname, "/chat", "the URL still uses the chat path");
  assert.equal(racedUrl.searchParams.get("dm"), raceAgent, "the URL still names the agent");
  const racedSelected = await page.locator(".rooms .room.dm.on .rname").innerText();
  assert.match(racedSelected, new RegExp(raceAgent), "the DM stays selected after the stale response");

  await page.unroute("**/api/chat/rooms");
  await page.unroute("**/api/chat/dm");

  console.log("agent DM deep-link browser regressions: passed", fixture.room.id);
} finally {
  await browser.close();
}
