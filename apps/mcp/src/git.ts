// Git metadata for commit attribution.
//
// A `record_commit` caller may pass a local workdir so breadboard can fill the
// tree SHA and patch id automatically. That must never be best-effort: silently
// recording null rewrite metadata defeats the feature. Any failure here throws
// with the path and the reason, and the tool records nothing.

import { spawnSync } from "node:child_process";

export interface CommitMetadata {
  sha: string;
  tree_sha: string;
  patch_id: string;
}

interface GitResult {
  ok: boolean;
  stdout: string;
  reason: string;
}

function runGit(args: string[], input?: string): GitResult {
  const result = spawnSync("git", args, {
    input,
    encoding: "utf8",
    timeout: 10_000,
    maxBuffer: 32 * 1024 * 1024,
  });
  const stdout = typeof result.stdout === "string" ? result.stdout.trim() : "";
  const stderr = typeof result.stderr === "string" ? result.stderr.trim() : "";
  const reason =
    result.error?.message ||
    stderr ||
    (result.status === 0 ? "" : `git exited with status ${result.status}`);
  return { ok: !result.error && result.status === 0, stdout, reason };
}

function firstToken(value: string): string | undefined {
  return value.split(/\s+/).find((part) => part.length > 0);
}

/**
 * Resolve a commit in a real work tree and return the full SHA plus its tree
 * and stable patch id. Throws instead of falling back to the caller's raw SHA.
 */
export function resolveCommitMetadata(workdir: string, sha: string): CommitMetadata {
  const path = workdir.trim();
  if (!path) throw new Error("workdir is empty");

  const inside = runGit(["-C", path, "rev-parse", "--is-inside-work-tree"]);
  if (!inside.ok || inside.stdout !== "true") {
    throw new Error(
      `workdir is not a git work tree: ${path} (${inside.reason || "not a git repository"})`,
    );
  }

  const commit = runGit(["-C", path, "rev-parse", "--verify", `${sha}^{commit}`]);
  const fullSha = firstToken(commit.stdout);
  if (!commit.ok || !fullSha) {
    throw new Error(
      `commit ${sha} does not resolve in ${path} (${commit.reason || "unknown revision"})`,
    );
  }

  const tree = runGit(["-C", path, "rev-parse", `${fullSha}^{tree}`]);
  const treeSha = firstToken(tree.stdout);
  if (!tree.ok || !treeSha) {
    throw new Error(
      `could not read the tree of commit ${sha} in ${path} (${tree.reason || "no tree"})`,
    );
  }

  // `git show` is empty for a merge commit, so patch-id has no stable value
  // there. Refuse rather than record a partial attribution; the caller can
  // omit workdir and pass tree_sha explicitly when that is intended.
  const patch = runGit(["-C", path, "show", "--no-color", "--pretty=format:", fullSha]);
  if (!patch.ok) {
    throw new Error(
      `could not read commit ${sha} in ${path} (${patch.reason || "git show failed"})`,
    );
  }
  const patchIdResult = runGit(["-C", path, "patch-id", "--stable"], patch.stdout);
  const patchId = firstToken(patchIdResult.stdout);
  if (!patchIdResult.ok || !patchId) {
    throw new Error(
      `could not compute patch id for commit ${sha} in ${path} (${patchIdResult.reason || "no patch id"})`,
    );
  }

  return { sha: fullSha, tree_sha: treeSha, patch_id: patchId };
}
