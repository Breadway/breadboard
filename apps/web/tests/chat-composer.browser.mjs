import assert from "node:assert/strict";
import { chromium } from "playwright";

// Browser regression for BB-243: the composer is a textarea. Enter inserts a
// newline, Ctrl+Enter sends, and a pasted multi-line block (including a code
// fence) is preserved verbatim instead of being flattened to one line.
//
// Run against a scratch server:
//
//   BREADBOARD_TEST_URL=http://127.0.0.1:7292 \
//   BREADBOARD_TEST_SESSION=<bb_session cookie> \
//   node apps/web/tests/chat-composer.browser.mjs
//
// Without a session cookie it uses /auth/dev-login. The fixture creates its
// own agent and DM.

const base = (process.env.BREADBOARD_TEST_URL ?? "http://127.0.0.1:7292").replace(/\/$/, "");
const session = process.env.BREADBOARD_TEST_SESSION;
const BLOCK = "```js\nconst answer = 42;\nconsole.log(answer);\n```";

const browser = await chromium.launch({
  headless: true,
  args: ["--no-sandbox"],
  ...(process.env.BREADBOARD_CHROME_PATH ? { executablePath: process.env.BREADBOARD_CHROME_PATH } : {}),
});

try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    permissions: ["clipboard-read", "clipboard-write"],
  });
  if (session) await context.addCookies([{ name: "bb_session", value: session, url: base }]);
  const page = await context.newPage();

  if (!session) {
    const login = await page.request.post(`${base}/auth/dev-login`, {
      data: { login: `composer-${Date.now().toString().slice(-6)}` },
    });
    assert.ok(login.ok(), `/auth/dev-login: ${login.status()} ${await login.text()}`);
  }

  await page.goto(base);

  const stamp = Date.now().toString().slice(-6);
  const agent = `composer-agent-${stamp}`;
  const room = await page.evaluate(async ({ agent }) => {
    const api = async (method, path, body) => {
      const response = await fetch(path, {
        method,
        headers: { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
      return response.status === 204 ? undefined : response.json();
    };
    await api("POST", "/api/agents/register", {
      agent_name: agent,
      session_id: `ses_${agent}`,
      host: "test",
      pid: 12,
    });
    return api("POST", "/api/chat/dm", { with: agent });
  }, { agent });

  const countMessages = async () => {
    const response = await context.request.get(`${base}/api/chat/rooms/${room.id}/messages`);
    assert.ok(response.ok(), `message list: ${response.status()}`);
    return (await response.json()).length;
  };

  await page.goto(`${base}/chat?dm=${encodeURIComponent(agent)}`);
  const composer = page.locator(".composer textarea.field");
  await composer.waitFor({ state: "visible" });

  assert.equal(
    await composer.evaluate((el) => el.tagName),
    "TEXTAREA",
    "the composer is a textarea, not a single-line input",
  );

  const before = await countMessages();

  // Enter inserts a newline and does not submit.
  await composer.click();
  await page.keyboard.type("line one");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(150);
  assert.equal(await composer.inputValue(), "line one\n", "Enter inserts a newline");
  assert.equal(await countMessages(), before, "Enter does not send the message");
  assert.ok(
    await page.evaluate(() => document.activeElement?.classList.contains("field")),
    "focus stays in the composer after Enter",
  );

  // A pasted multi-line fence comes through intact.
  await page.evaluate(async (text) => {
    await navigator.clipboard.writeText(text);
  }, BLOCK);
  await page.keyboard.press("Control+V");
  const composed = await composer.inputValue();
  assert.ok(composed.endsWith(BLOCK), "a pasted fenced block keeps every newline");

  // Ctrl+Enter sends the whole body; the button remains the discoverable path.
  await page.keyboard.press("Control+Enter");
  await page.waitForFunction(
    () => document.querySelectorAll(".msg .codeblock").length > 0,
    undefined,
    { timeout: 5000 },
  );
  const after = await countMessages();
  assert.equal(after, before + 1, "Ctrl+Enter sends exactly one message");

  const messages = await (await context.request.get(`${base}/api/chat/rooms/${room.id}/messages`)).json();
  const sent = messages[messages.length - 1];
  assert.equal(sent.body, `line one\n${BLOCK}`, "the server receives the newlines and indentation verbatim");

  const code = await page.locator(".msg .codeblock pre code").last().innerText();
  assert.equal(code, "const answer = 42;\nconsole.log(answer);", "the fence renders as a code block");

  assert.equal(await composer.inputValue(), "", "the composer clears after sending");
  const height = await composer.evaluate((el) => el.getBoundingClientRect().height);
  assert.ok(height < 60, `the composer shrinks back after sending (${height}px)`);

  assert.match(
    await page.getByRole("button", { name: "Send" }).getAttribute("title"),
    /Ctrl\+Enter/,
    "the Send button names the shortcut",
  );

  console.log("chat-composer: newline on Enter, Ctrl+Enter send, paste preserved");
} finally {
  await browser.close();
}
