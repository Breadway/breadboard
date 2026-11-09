// End-to-end tests for record_commit workdir handling.
//
// These spawn the built MCP server over stdio and point it at a local mock API
// server. A positive case proves a real repository fills tree_sha and patch_id;
// a negative case proves an invalid workdir refuses and never sends the record.

import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const GIT_ENV = {
  ...process.env,
  GIT_AUTHOR_NAME: "BB-163 test",
  GIT_AUTHOR_EMAIL: "bb163@example.com",
  GIT_COMMITTER_NAME: "BB-163 test",
  GIT_COMMITTER_EMAIL: "bb163@example.com",
};

const SERVER = fileURLToPath(new URL("../dist/index.js", import.meta.url));

function git(cwd, args) {
  return execFileSync("git", args, { cwd, encoding: "utf8", env: GIT_ENV }).trim();
}

function realRepo() {
  const dir = mkdtempSync(join(tmpdir(), "bb163-repo-"));
  git(dir, ["init", "-q"]);
  writeFileSync(join(dir, "file.txt"), "hello\n");
  git(dir, ["add", "file.txt"]);
  git(dir, ["commit", "-q", "-m", "first"]);
  return dir;
}

function sendJson(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
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
      if (req.method === "POST" && req.url === "/api/commits") {
        return sendJson(res, 201, {
          id: 1,
          repo: parsed.repo,
          sha: parsed.sha,
          issue_id: 1,
          issue_key: parsed.issue,
          agent_name: parsed.agent_name,
          session_id: parsed.session_id,
          orchestrator: null,
          orchestrator_session_id: null,
          model: null,
          provider: null,
          model_source: "unknown",
          patch_id: parsed.patch_id ?? null,
          tree_sha: parsed.tree_sha ?? null,
          note: parsed.note ?? null,
          created_at: "2026-11-10T00:00:00Z",
        });
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
  const configDir = mkdtempSync(join(tmpdir(), "bb163-mcp-"));
  const env = {
    ...Object.fromEntries(
      Object.entries(process.env).filter((entry) => entry[1] !== undefined),
    ),
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
  const client = new Client({ name: "bb163-test", version: "1.0.0" });
  try {
    await client.connect(transport);
    await run({ client, api });
  } finally {
    await client.close().catch(() => {});
    await new Promise((resolve) => api.server.close(resolve));
  }
}

test("a real repository fills tree_sha and patch_id", async () => {
  const repo = realRepo();
  const sha = git(repo, ["rev-parse", "HEAD"]);
  const tree = git(repo, ["rev-parse", "HEAD^{tree}"]);

  await withClient(async ({ client, api }) => {
    const result = await client.callTool({
      name: "record_commit",
      arguments: {
        repo: "Breadway/breadboard",
        sha,
        issue: "BB-163",
        workdir: repo,
        note: "positive workdir case",
      },
    });
    assert.notEqual(result.isError, true, JSON.stringify(result.content));

    const calls = api.requests.filter(
      (request) => request.method === "POST" && request.url === "/api/commits",
    );
    assert.equal(calls.length, 1);
    assert.equal(calls[0].body.sha, sha);
    assert.equal(calls[0].body.tree_sha, tree);
    assert.match(calls[0].body.patch_id, /^[0-9a-f]{40}$/);
  });
});

test("a non-repo workdir fails explicitly and records nothing", async () => {
  const dir = mkdtempSync(join(tmpdir(), "bb163-notrepo-"));
  const sha = "a".repeat(40);

  await withClient(async ({ client, api }) => {
    const result = await client.callTool({
      name: "record_commit",
      arguments: {
        repo: "Breadway/breadboard",
        sha,
        issue: "BB-163",
        workdir: dir,
      },
    });
    assert.equal(result.isError, true, "a non-repo workdir must be an MCP error");
    const text = result.content.map((item) => item.text ?? "").join("\n");
    assert.match(text, /not a git work tree/i);
    assert.ok(text.includes(dir), `error should name ${dir}: ${text}`);

    const calls = api.requests.filter(
      (request) => request.method === "POST" && request.url === "/api/commits",
    );
    assert.equal(calls.length, 0, "nothing may be recorded");
  });
});

test("a commit that does not resolve fails explicitly", async () => {
  const repo = realRepo();
  const missing = "b".repeat(40);

  await withClient(async ({ client, api }) => {
    const result = await client.callTool({
      name: "record_commit",
      arguments: {
        repo: "Breadway/breadboard",
        sha: missing,
        issue: "BB-163",
        workdir: repo,
      },
    });
    assert.equal(result.isError, true, "an unresolved commit must be an MCP error");
    const text = result.content.map((item) => item.text ?? "").join("\n");
    assert.match(text, /does not resolve/i);
    assert.ok(text.includes(repo), `error should name ${repo}: ${text}`);
    assert.equal(
      api.requests.filter(
        (request) => request.method === "POST" && request.url === "/api/commits",
      ).length,
      0,
    );
  });
});
