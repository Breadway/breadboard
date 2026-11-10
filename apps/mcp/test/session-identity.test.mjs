// One MCP process, several OpenCode sessions (BB-196, BB-215).
//
// OpenCode runs a single local breadboard MCP server for the whole app and
// sends the calling conversation's id in each request as
// `_meta["ai.opencode/sessionID"]`. These tests drive one server process with
// two different ids and assert that registration, claims, chat and commit
// attribution all follow the caller rather than a value captured at startup.

import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const SERVER = fileURLToPath(new URL("../dist/index.js", import.meta.url));

function sendJson(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

function sessionOf(body) {
  return typeof body?.session_id === "string" ? body.session_id : undefined;
}

async function startMockApi() {
  const requests = [];
  const names = new Map(); // session id -> agent name
  const api = { requests, commitFailure: null };
  let nextName = 1;

  function register(body) {
    const session = sessionOf(body);
    const explicit = typeof body?.agent_name === "string" ? body.agent_name.trim() : "";
    if (explicit) {
      const held = session ? names.get(session) : undefined;
      if (held && held !== explicit) {
        return {
          status: 409,
          body: {
            error: `session ${session} is already registered as '${held}'; an explicit different name '${explicit}' is refused`,
            agent_name: explicit,
            existing_name: held,
            session_id: session,
            rename_required: true,
          },
        };
      }
      if (session) names.set(session, explicit);
      return {
        status: 200,
        body: {
          agent_name: explicit,
          alternatives: [],
          live: true,
          paused: false,
          session_id: session ?? null,
        },
      };
    }
    const existing = session ? names.get(session) : undefined;
    const assigned = existing ?? `agent-${nextName++}`;
    if (session) names.set(session, assigned);
    return {
      status: 200,
      body: {
        agent_name: assigned,
        alternatives: [],
        live: true,
        paused: false,
        session_id: session ?? null,
      },
    };
  }

  function claim(key, body) {
    return {
      id: 1,
      issue_id: 1,
      key,
      issue_title: "session identity",
      issue_status: "doing",
      claimed_by: 1,
      claimed_by_login: "tester",
      agent_name: body.agent_name,
      message: null,
      mode: body.mode ?? "exclusive",
      session_id: body.session_id ?? null,
      state: body.state ?? "implementing",
      note: null,
      branch: null,
      worktree: null,
      repo: null,
      pr_url: null,
      pr_number: null,
      head_sha: null,
      board_status: "doing",
      claimed_at: "2026-11-10T00:00:00Z",
      updated_at: "2026-11-10T00:00:00Z",
      last_heartbeat: "2026-11-10T00:00:00Z",
    };
  }

  const server = createServer((req, res) => {
    let raw = "";
    req.on("data", (chunk) => (raw += chunk));
    req.on("end", () => {
      const body = raw ? JSON.parse(raw) : null;
      requests.push({ method: req.method, url: req.url, body });

      if (req.method === "POST" && req.url === "/api/agents/register") {
        const result = register(body);
        return sendJson(res, result.status, result.body);
      }
      if (req.method === "POST" && /^\/api\/agents\/[^/]+\/rename$/.test(req.url)) {
        const old = decodeURIComponent(req.url.split("/")[3]);
        const next = body.new_name;
        for (const [session, name] of names.entries()) {
          if (name === old) names.set(session, next);
        }
        return sendJson(res, 200, { agent_name: next });
      }
      if (req.method === "PUT" && req.url === "/api/agents/message") {
        return sendJson(res, 200, { agent_name: body.agent_name, message: body.message });
      }
      if (req.method === "POST" && /^\/api\/issues\/[^/]+\/claim$/.test(req.url)) {
        const key = decodeURIComponent(req.url.split("/")[3]);
        return sendJson(res, 201, claim(key, body));
      }
      if (req.method === "PUT" && req.url.startsWith("/api/issues/")) {
        const key = decodeURIComponent(req.url.split("/")[3]);
        return sendJson(res, 200, claim(key, body));
      }
      if (req.method === "GET" && /^\/api\/issues\/[^/]+$/.test(req.url)) {
        const key = decodeURIComponent(req.url.split("/")[3]);
        return sendJson(res, 200, { key, last_handoff: null });
      }
      if (req.method === "POST" && req.url === "/api/chat/messages") {
        return sendJson(res, 201, {
          id: 1,
          room_id: 1,
          author_display: body.agent_name,
          author_kind: "agent",
          body: body.body,
          created_at: "2026-11-10T00:00:00Z",
        });
      }
      if (req.method === "POST" && req.url === "/api/chat/dm") {
        return sendJson(res, 201, { id: 7, name: null, kind: "dm" });
      }
      if (req.method === "POST" && req.url === "/api/commits") {
        if (api.commitFailure) {
          return sendJson(res, api.commitFailure.status, api.commitFailure.body);
        }
        return sendJson(res, 201, {
          id: 1,
          repo: body.repo,
          sha: body.sha,
          issue_id: 1,
          issue_key: body.issue,
          agent_name: body.agent_name,
          session_id: body.session_id ?? null,
          orchestrator: null,
          orchestrator_session_id: null,
          model: body.model ?? null,
          provider: body.provider ?? null,
          model_source: body.model ? "reported" : "unknown",
          patch_id: body.patch_id ?? null,
          tree_sha: body.tree_sha ?? null,
          note: body.note ?? null,
          created_at: "2026-11-10T00:00:00Z",
        });
      }
      return sendJson(res, 404, { error: "not found" });
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return { server, api, base: `http://127.0.0.1:${address.port}` };
}

async function withClient(extraEnv, run) {
  const mock = await startMockApi();
  const api = mock.api;
  const configDir = mkdtempSync(join(tmpdir(), "bb196-mcp-"));
  const env = {
    ...Object.fromEntries(
      Object.entries(process.env).filter((entry) => entry[1] !== undefined),
    ),
    BREADBOARD_SERVER: mock.base,
    BREADBOARD_TOKEN: "test-token",
    BREADBOARD_CHAT_POLLER: "0",
    XDG_CONFIG_HOME: configDir,
    ...extraEnv,
  };
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER],
    env,
    stderr: "ignore",
  });
  const client = new Client({ name: "bb196-test", version: "1.0.0" });
  try {
    await client.connect(transport);
    await run({ client, api });
  } finally {
    await client.close().catch(() => {});
    await new Promise((resolve) => mock.server.close(resolve));
  }
}

const call = (client, name, args, session) =>
  client.callTool({
    name,
    arguments: args,
    ...(session === undefined ? {} : { _meta: { "ai.opencode/sessionID": session } }),
  });

function bodyText(result) {
  return result.content.map((item) => item.text ?? "").join("\n");
}

function posts(api, url) {
  return api.requests.filter((request) => request.method === "POST" && request.url === url);
}

test("two sessions behind one MCP process keep separate names and attribution", async () => {
  const a = "ses_alpha";
  const b = "ses_beta";
  const shaA = "a".repeat(40);
  const shasB = "b".repeat(40);

  await withClient({}, async ({ client, api }) => {
    const claimedA = await call(client, "claim_issue", { key: "BB-1" }, a);
    const claimedB = await call(client, "claim_issue", { key: "BB-2" }, b);
    assert.notEqual(claimedA.isError, true, bodyText(claimedA));
    assert.notEqual(claimedB.isError, true, bodyText(claimedB));

    const claims = posts(api, "/api/issues/BB-1/claim").concat(
      posts(api, "/api/issues/BB-2/claim"),
    );
    assert.equal(claims.length, 2);
    assert.equal(claims[0].body.agent_name, "agent-1");
    assert.equal(claims[0].body.session_id, a);
    assert.equal(claims[1].body.agent_name, "agent-2");
    assert.equal(claims[1].body.session_id, b);

    await call(client, "update_claim", { key: "BB-1" }, a);
    await call(client, "update_claim", { key: "BB-2" }, b);
    const heartbeats = api.requests.filter(
      (request) => request.method === "PUT" && request.url.startsWith("/api/issues/"),
    );
    assert.deepEqual(
      heartbeats.map((request) => request.body.session_id),
      [a, b],
      "a heartbeat must name the session that sent it",
    );

    await call(client, "chat_send", { body: "from alpha" }, a);
    await call(client, "chat_send", { body: "from beta" }, b);
    const chats = posts(api, "/api/chat/messages");
    assert.equal(chats.length, 2);
    assert.deepEqual(
      chats.map((request) => [request.body.agent_name, request.body.session_id]),
      [
        ["agent-1", a],
        ["agent-2", b],
      ],
    );

    const commitA = await call(
      client,
      "record_commit",
      { repo: "Breadway/breadboard", sha: shaA, issue: "BB-1" },
      a,
    );
    const commitB = await call(
      client,
      "record_commit",
      { repo: "Breadway/breadboard", sha: shasB, issue: "BB-2" },
      b,
    );
    assert.notEqual(commitA.isError, true, bodyText(commitA));
    assert.notEqual(commitB.isError, true, bodyText(commitB));
    const commits = posts(api, "/api/commits");
    assert.deepEqual(
      commits.map((request) => [request.body.agent_name, request.body.session_id]),
      [
        ["agent-1", a],
        ["agent-2", b],
      ],
    );

    // Naming session B must not rename session A or move A's attribution.
    const named = await call(client, "name_agent", { agent_name: "gamma" }, b);
    assert.notEqual(named.isError, true, bodyText(named));

    await call(
      client,
      "record_commit",
      { repo: "Breadway/breadboard", sha: "c".repeat(40), issue: "BB-1" },
      a,
    );
    await call(
      client,
      "record_commit",
      { repo: "Breadway/breadboard", sha: "d".repeat(40), issue: "BB-2" },
      b,
    );
    const afterRename = posts(api, "/api/commits").slice(2);
    assert.deepEqual(
      afterRename.map((request) => [request.body.agent_name, request.body.session_id]),
      [
        ["agent-1", a],
        ["gamma", b],
      ],
    );

    // Every registration carried the session that made the call.
    for (const request of posts(api, "/api/agents/register")) {
      assert.ok(["ses_alpha", "ses_beta"].includes(request.body.session_id));
    }
  });
});

test("a startup session id is never sent on registration or a write", async () => {
  await withClient({ OPENCODE_SESSION_ID: "ses_stale" }, async ({ client, api }) => {
    const result = await call(client, "record_commit", {
      repo: "Breadway/breadboard",
      sha: "e".repeat(40),
      issue: "BB-136",
    });
    assert.notEqual(result.isError, true, bodyText(result));

    const registrations = posts(api, "/api/agents/register");
    assert.equal(registrations.length, 1);
    assert.equal(
      registrations[0].body.session_id,
      undefined,
      "a shared process must not register a startup session id",
    );
    const commits = posts(api, "/api/commits");
    assert.equal(commits.length, 1);
    assert.equal(
      commits[0].body.session_id,
      undefined,
      "a shared process must not send a startup session id on a write",
    );
  });
});

test("an explicit BREADBOARD_SESSION_ID still belongs to a dedicated process", async () => {
  await withClient({ BREADBOARD_SESSION_ID: "ses_dedicated" }, async ({ client, api }) => {
    const result = await call(client, "record_commit", {
      repo: "Breadway/breadboard",
      sha: "f".repeat(40),
      issue: "BB-136",
    });
    assert.notEqual(result.isError, true, bodyText(result));
    assert.equal(posts(api, "/api/agents/register")[0].body.session_id, "ses_dedicated");
    assert.equal(posts(api, "/api/commits")[0].body.session_id, "ses_dedicated");
  });
});

test("identity resolution failures are explicit and are not retried", async () => {
  await withClient({}, async ({ client, api }) => {
    api.commitFailure = {
      status: 409,
      body: {
        error:
          "session ses_alpha is already registered as 'agent-1'; an explicit different name 'agent-2' is refused",
        rename_required: true,
      },
    };
    const refused = await call(
      client,
      "record_commit",
      { repo: "Breadway/breadboard", sha: "1".repeat(40), issue: "BB-196" },
      "ses_alpha",
    );
    assert.equal(refused.isError, true);
    assert.match(bodyText(refused), /already registered as 'agent-1'/);
    assert.doesNotMatch(bodyText(refused), /internal error/i);
    assert.equal(posts(api, "/api/commits").length, 1);

    api.commitFailure = {
      status: 503,
      body: { error: "database is busy; retry the request" },
    };
    const busy = await call(
      client,
      "record_commit",
      { repo: "Breadway/breadboard", sha: "2".repeat(40), issue: "BB-196" },
      "ses_alpha",
    );
    assert.equal(busy.isError, true);
    assert.match(bodyText(busy), /database is busy/);
    assert.doesNotMatch(bodyText(busy), /internal error/i);
    assert.equal(posts(api, "/api/commits").length, 2);
    // A failed resolution records nothing in the mock either: both attempts
    // were rejected before a row was created.
    assert.equal(api.commitFailure.status, 503);
  });
});
