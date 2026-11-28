// Regression tests for propose_maintenance_window with an ask id.
//
// The tool accepts an ASK id, not a window id. It must read the ask, refuse an
// ask without a window and an ask owned by another agent, and patch the ask's
// own window id. The mock API records every request so the URL is asserted, not
// inferred from a successful response.

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

const WINDOW = {
  id: 9,
  ask_id: 31,
  project_id: 1,
  project_key: "BB",
  agent_name: "test agent",
  title: "Nextcloud MySQL 8.4",
  starts_at: "2026-11-29T02:00:00Z",
  ends_at: "2026-11-29T04:00:00Z",
  timezone: "Australia/Perth",
  services: ["nextcloud"],
  reason: "Upgrade",
  risks: "Downtime",
  rollback: "Snapshot",
  runbook_md: "1. Announce",
  status: "proposed",
  decided_by: null,
  decided_at: null,
  created_at: "2026-11-28T00:00:00Z",
  updated_at: "2026-11-28T00:00:00Z",
};

function ask(id, agent, window) {
  return {
    id,
    project_id: 1,
    project_key: "BB",
    project_name: "Breadboard",
    issue_id: null,
    issue_key: null,
    issue_title: null,
    issue_status: null,
    agent_name: agent,
    session_id: "ses_test",
    kind: "maintenance",
    urgency: "normal",
    title: "Maintenance ask",
    summary: "",
    body_md: "",
    environment: null,
    status: "pending",
    revision: 1,
    checklist: [],
    risk_md: null,
    rollback_md: null,
    expires_at: null,
    deadline: null,
    unread: false,
    decided_by: null,
    decided_by_login: null,
    decided_at: null,
    decision_note_md: null,
    created_at: "2026-11-28T00:00:00Z",
    updated_at: "2026-11-28T00:00:00Z",
    events: [],
    attachments: [],
    window,
  };
}

async function startMockApi() {
  const requests = [];
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const parsed = body ? JSON.parse(body) : null;
      requests.push({ method: req.method, url: req.url, body: parsed });
      if (req.url === "/api/agents/register") {
        return sendJson(res, 200, {
          agent_name: "test agent",
          alternatives: [],
          live: true,
          paused: false,
          session_id: "ses_test",
        });
      }
      if (req.method === "GET" && req.url.startsWith("/api/approvals/31")) {
        return sendJson(res, 200, ask(31, "test agent", WINDOW));
      }
      if (req.method === "GET" && req.url.startsWith("/api/approvals/32")) {
        return sendJson(res, 200, ask(32, "test agent", null));
      }
      if (req.method === "GET" && req.url.startsWith("/api/approvals/33")) {
        return sendJson(res, 200, ask(33, "other agent", { ...WINDOW, id: 12, ask_id: 33, agent_name: "other agent" }));
      }
      if (req.method === "PATCH" && /^\/api\/maintenance\/windows\/\d+$/.test(req.url)) {
        const id = Number(req.url.split("/").pop());
        return sendJson(res, 200, { ...WINDOW, id, ...parsed });
      }
      return sendJson(res, 404, { error: "not found" });
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return { server, requests, base: `http://127.0.0.1:${address.port}` };
}

async function withClient(run) {
  const api = await startMockApi();
  const configDir = mkdtempSync(join(tmpdir(), "approval-mcp-"));
  const env = {
    ...Object.fromEntries(Object.entries(process.env).filter((entry) => entry[1] !== undefined)),
    BREADBOARD_SERVER: api.base,
    BREADBOARD_TOKEN: "test-token",
    BREADBOARD_AGENT: "test agent",
    OPENCODE_SESSION_ID: "ses_test",
    XDG_CONFIG_HOME: configDir,
  };
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER],
    env,
    stderr: "ignore",
  });
  const client = new Client({ name: "approval-mcp-test", version: "1.0.0" });
  try {
    await client.connect(transport);
    await run({ client, api });
  } finally {
    await client.close().catch(() => {});
    await new Promise((resolve) => api.server.close(resolve));
  }
}

const call = (client, args) =>
  client.callTool({
    name: "propose_maintenance_window",
    arguments: {
      title: "Moved window",
      starts_at: "2026-11-29T03:00:00Z",
      ends_at: "2026-11-29T05:00:00Z",
      ...args,
    },
  });

const text = (result) => result.content.map((item) => item.text ?? "").join("\n");

test("an ask id patches that ask's window id, never the ask id", async () => {
  await withClient(async ({ client, api }) => {
    const result = await call(client, { ask_id: 31 });
    assert.notEqual(result.isError, true, text(result));

    const patches = api.requests.filter((request) => request.method === "PATCH");
    assert.equal(patches.length, 1);
    assert.equal(patches[0].url, "/api/maintenance/windows/9", "asks 31 owns window 9");
    assert.equal(patches[0].body.title, "Moved window");
    assert.equal(patches[0].body.agent_name, "test agent");

    const reads = api.requests.filter(
      (request) => request.method === "GET" && request.url.startsWith("/api/approvals/31"),
    );
    assert.equal(reads.length, 1, "the ask is read to resolve its window");
    assert.match(reads[0].url, /agent=test(?:%20|\+)agent/);
  });
});

test("an ask without a window is refused and nothing is patched", async () => {
  await withClient(async ({ client, api }) => {
    const result = await call(client, { ask_id: 32 });
    assert.equal(result.isError, true);
    assert.match(text(result), /no proposed maintenance window/i);
    assert.equal(api.requests.filter((request) => request.method === "PATCH").length, 0);
  });
});

test("another agent's ask is refused and nothing is patched", async () => {
  await withClient(async ({ client, api }) => {
    const result = await call(client, { ask_id: 33 });
    assert.equal(result.isError, true);
    assert.match(text(result), /belongs to other agent/i);
    assert.equal(api.requests.filter((request) => request.method === "PATCH").length, 0);
  });
});
