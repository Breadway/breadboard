import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Static regression check for BB-160: typed evidence is wired end to end.
// The web UI has no unit runner, so this asserts the source-level contract:
// the drawer mounts the Evidence component, the component renders images
// inline, benches as a table and text evidence in a viewer, and the server
// registers the routes plus the immutability trigger.
//
// Run: node apps/web/tests/artifacts.static.mjs (from the repo root)

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const repo = join(root, "..", "..");
const read = (p) => readFileSync(join(root, p), "utf8");

const drawer = read("src/lib/IssueDrawer.svelte");
assert.ok(drawer.includes('import Artifacts from "./Artifacts.svelte"'), "drawer imports the Evidence component");
assert.ok(drawer.includes("<Artifacts issueKey={issue.key} {comments} />"), "drawer mounts Evidence for the open issue");

const artifacts = read("src/lib/Artifacts.svelte");
assert.ok(artifacts.includes("<img src={api.artifactUrl(a.id)}"), "images preview inline");
assert.ok(artifacts.includes('class="bench"'), "benches render as a table");
assert.ok(artifacts.includes('class="viewer"'), "logs and diffs render in a viewer");
assert.ok(artifacts.includes("{a.uploader_agent}"), "each artifact shows who produced it");
assert.ok(artifacts.includes("{a.session_id.slice(0, 10)}"), "each artifact shows the session that produced it");
assert.ok(artifacts.includes("ARTIFACT_KINDS"), "kind options come from the shared list");
assert.ok(artifacts.includes("50 MB"), "upload UI states the CI log size limit");
assert.ok(artifacts.includes("expires {fmtDate(a.expires_at)}"), "expiry is visible on each artifact");

const http = readFileSync(join(repo, "crates/breadboard-server/src/http.rs"), "utf8");
assert.ok(http.includes('"/issues/{key}/artifacts"'), "issue artifact routes are registered");
assert.ok(http.includes('"/artifacts/{id}"'), "artifact download route is registered");

const migration = readFileSync(
  join(repo, "migrations/0021_issue_artifacts.sql"),
  "utf8",
);
assert.ok(migration.includes("CHECK (kind IN ('bench', 'screenshot', 'ci-log', 'diff', 'trace', 'other'))"), "migration constrains the artifact kinds");
assert.ok(migration.includes("RAISE(ABORT, 'issue_artifacts rows are immutable')"), "migration makes rows immutable");
assert.ok(migration.includes("expires_at"), "migration stores an expiry");

console.log("artifacts static checks passed across drawer, Evidence, routes and migration");
