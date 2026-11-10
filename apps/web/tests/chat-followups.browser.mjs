import assert from "node:assert/strict";
import { chromium } from "playwright";

// Regression check for the BB-194 chat findings, all three in one scratch
// fixture:
//
//   BB-204  the DM section sits above the shared rooms, so all five DMs are
//           reachable without scrolling past twelve rooms, at 1680, 1280 and
//           390. Nothing is hidden: every room still renders.
//   BB-205  opening a room clears its unread badge immediately, and the shared
//           total follows, without waiting for the 30s poll.
//   BB-206  a fenced code block renders as a code element with monospace text
//           and no literal backticks or language tag in the prose.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7290 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/chat-followups.browser.mjs
//
// The fixture creates its own rooms, agents and DMs, so it is safe on a
// scratch database. Run it on an empty scratch database: it asserts exact
// counts for the rooms it seeded.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7290").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;
assert.ok(session, "BREADBOARD_TEST_SESSION must be a local test session cookie");

const ROOMS = 12;
const DMS = 5;

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
  const fence = [
    "Here is the patch:",
    "",
    "```rust",
    "fn main() {",
    '    println!("hi");',
    "}",
    "```",
    "",
    "Run it.",
  ].join("\n");

  const fixture = await page.evaluate(
    async ({ stamp, rooms, dms, fence }) => {
      const api = async (method, path, body) => {
        const response = await fetch(path, {
          method,
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.status === 204 ? undefined : response.json();
      };
      // Shared rooms: keep them read (the human wrote the last message) so
      // every unread count below comes from a DM.
      for (let i = 0; i < rooms; i++) {
        const room = await api("POST", "/api/chat/rooms", {
          name: `fixture-${stamp}-${i}`,
          description: `fixture room ${i}`,
        });
        await api("POST", "/api/chat/messages", {
          room: String(room.id),
          body: `last note in ${room.name}`,
        });
      }
      // DMs with one unread message each. The first one carries the fenced
      // code block, so opening it covers BB-205 and BB-206 together.
      const dmRooms = [];
      for (let i = 0; i < dms; i++) {
        const agent = `chat-fixture-${stamp}-${i}`;
        await api("POST", "/api/agents/register", {
          agent_name: agent,
          session_id: `ses_${agent}`,
          host: "test",
          pid: 90 + i,
        });
        const room = await api("POST", "/api/chat/dm", { with: agent });
        await api("POST", "/api/chat/messages", {
          agent_name: agent,
          room: String(room.id),
          body: i === 0 ? fence : `unread note ${i} from ${agent}`,
        });
        dmRooms.push({ agent, room: room.id });
      }
      return { dmRooms };
    },
    { stamp, rooms: ROOMS, dms: DMS, fence },
  );

  const seededRooms = await (await context.request.get(`${base}/api/chat/rooms`)).json();
  const seededDms = seededRooms.filter((r) => r.kind === "dm" && r.members.some((m) => m.includes(stamp)));
  const seededShared = seededRooms.filter((r) => r.kind === "room");
  assert.equal(seededDms.length, DMS, "the fixture created five DMs");
  assert.ok(
    seededShared.filter((r) => (r.name ?? "").includes(stamp)).length === ROOMS,
    "the fixture created twelve rooms",
  );
  assert.equal(
    seededDms.reduce((n, r) => n + r.unread, 0),
    DMS,
    "each fixture DM starts unread",
  );
  const expectedShared = seededShared.length;

  // --- BB-204: DMs first and fully reachable at every width -----------------
  await page.goto(`${base}/chat`);
  await page.waitForSelector(".rooms .room");

  const dmOrder = await page.$$eval(".rooms > .room", (rows) =>
    rows.map((row) => row.classList.contains("dm")),
  );
  assert.equal(dmOrder.length, expectedShared + DMS, "every room and DM still renders");
  assert.ok(
    dmOrder.slice(0, DMS).every(Boolean),
    "the DM rows come before the shared rooms",
  );
  assert.ok(
    dmOrder.slice(DMS).every((isDm) => !isDm),
    "the shared rooms follow the DM section",
  );

  for (const [width, height] of [
    [1680, 1000],
    [1280, 900],
    [390, 844],
  ]) {
    await page.setViewportSize({ width, height });
    await page.waitForTimeout(150);
    const measure = await page.evaluate(() => {
      const pane = document.querySelector(".rooms");
      pane.scrollTop = 0;
      const bounds = pane.getBoundingClientRect();
      const rows = [...document.querySelectorAll(".rooms > .room.dm")];
      return {
        visible: rows.filter((row) => {
          const r = row.getBoundingClientRect();
          return r.top >= bounds.top - 0.5 && r.bottom <= bounds.bottom + 0.5;
        }).length,
        dms: rows.length,
        rooms: [...document.querySelectorAll(".rooms > .room:not(.dm)")].length,
      };
    });
    assert.equal(measure.dms, DMS, `${width}: all ${DMS} DMs render`);
    assert.equal(
      measure.rooms,
      expectedShared,
      `${width}: all ${expectedShared} shared rooms render, none hidden`,
    );
    assert.equal(
      measure.visible,
      DMS,
      `${width}: every DM row is inside the room pane without scrolling`,
    );
  }

  // --- BB-205: opening a room clears its badge immediately ------------------
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.waitForTimeout(150);
  const navBadge = page.locator(".side nav button.nav", { hasText: "Chat" }).locator(".badge");
  const navBefore = (await navBadge.count()) ? await navBadge.first().innerText() : "0";
  assert.equal(navBefore, String(DMS), "the sidebar total counts every unread DM");

  const firstDm = page.locator(".rooms > .room.dm").first();
  await firstDm.click();
  // The badge must clear on the click, not 30 seconds later. Give the pick a
  // single animation frame, then read the DOM before any poll can run.
  await page.waitForTimeout(100);
  const opened = page.locator(".rooms > .room.dm.on");
  assert.equal(await opened.count(), 1, "the clicked DM is selected");
  assert.equal(
    await opened.locator(".unread").count(),
    0,
    "the opened room's unread badge clears immediately",
  );
  assert.equal(
    (await navBadge.count()) ? await navBadge.first().innerText() : "0",
    String(DMS - 1),
    "the shared total drops by one immediately",
  );
  // A room-list refresh taken before the server's background read receipt
  // landed must not put the badge back (the BB-205 ordering case).
  await page.waitForTimeout(1500);
  assert.equal(
    await page.locator(".rooms > .room.dm.on .unread").count(),
    0,
    "the badge stays clear after a room-list refresh",
  );
  assert.equal(
    (await navBadge.count()) ? await navBadge.first().innerText() : "0",
    String(DMS - 1),
    "the shared total stays correct after a refresh",
  );
  const serverRooms = await (await context.request.get(`${base}/api/chat/rooms`)).json();
  const openedRoom = serverRooms.find((r) => r.id === fixture.dmRooms[0].room);
  assert.equal(openedRoom.unread, 0, "the server receipt advanced for the opened room");

  // --- BB-206: fenced code renders as code ---------------------------------
  const block = page.locator(".messages .codeblock");
  await block.first().waitFor({ state: "visible" });
  assert.equal(await block.count(), 1, "the fenced block rendered once");
  assert.equal(
    (await block.locator(".lang").innerText()).trim().toLowerCase(),
    "rust",
    "the language tag is kept as a subtle label",
  );
  const codeText = await block.locator("pre code").innerText();
  assert.ok(codeText.includes("fn main()"), "the code preserves its text");
  assert.ok(codeText.includes('println!("hi");'), "the code preserves its quotes");
  assert.ok(!codeText.includes("```"), "the fence markers are gone from the code");
  const bodyText = await page.locator(".msg .body").first().innerText();
  assert.ok(!bodyText.includes("```"), "no literal fence markers in the message body");
  assert.ok(!bodyText.includes("```rust"), "the language tag is not shown in the prose");
  assert.match(bodyText, /Here is the patch:/, "prose before the fence still renders");
  assert.match(bodyText, /Run it\./, "prose after the fence still renders");
  const codeStyle = await block.locator("pre code").evaluate((el) => {
    const style = getComputedStyle(el);
    return { family: style.fontFamily, whiteSpace: style.whiteSpace };
  });
  assert.match(codeStyle.family, /mono/i, "the code uses a monospace family");
  assert.equal(codeStyle.whiteSpace, "pre", "the code keeps its whitespace and does not wrap");
  const preRadius = await block.evaluate((el) => getComputedStyle(el).borderRadius);
  assert.ok(preRadius && preRadius !== "0px", "the code block is a styled panel");

  console.log(`chat follow-up browser regressions: passed (${fixture.dmRooms.length} DMs)`);
} finally {
  await browser.close();
}
