// Logic for removing "gone" worktrees, shared by the `herdr-workspace-manager
// remove-gone` CLI and the `remove-gone` (preview) plugin action: find the
// *linked* worktrees of the current repo whose remote-tracking branch was
// deleted ("gone"), and remove them.
//
// "Gone" is git's own term: a branch whose configured upstream no longer exists
// after a prune. A branch that never had an upstream — never pushed, never
// tracked — is NOT gone, so those worktrees are left alone, exactly as required.
// The repo's main checkout and the workspace we're invoked from are never
// candidates either.

import { spawnSync } from "node:child_process";
import { runHerdrJson } from "./herdr.mjs";

// Run git in a given directory. Never throws on a non-zero exit — callers decide
// what a failure means (a missing remote shouldn't abort the whole sweep).
export function runGit(args, { cwd, env = process.env } = {}) {
  const res = spawnSync("git", args, {
    cwd,
    encoding: "utf8",
    // Fail fast instead of hanging on a credential prompt in this non-interactive
    // action context; we fall back to cached refs if a fetch can't authenticate.
    env: { ...env, GIT_TERMINAL_PROMPT: "0" },
    maxBuffer: 16 * 1024 * 1024,
  });
  return {
    status: res.status ?? (res.error ? -1 : 0),
    stdout: res.stdout ?? "",
    stderr: res.stderr ?? "",
    error: res.error ?? null,
  };
}

// Parse the output of
//   git for-each-ref --format='%(refname:short)\t%(upstream:track,nobracket)' refs/heads
// into the set of local branch names whose upstream is "gone". `upstream:track`
// is empty for a branch with no upstream OR one still in sync — neither appears
// here, which is precisely the "never pushed / still tracked" exclusion.
export function parseGoneBranches(text) {
  const gone = new Set();
  for (const line of String(text ?? "").split("\n")) {
    if (!line.trim()) continue;
    const tab = line.indexOf("\t");
    const branch = (tab === -1 ? line : line.slice(0, tab)).trim();
    const track = (tab === -1 ? "" : line.slice(tab + 1)).trim();
    if (branch && track === "gone") gone.add(branch);
  }
  return gone;
}

// From a herdr `worktree list` and the gone-branch set, pick removal candidates.
// Excludes: the repo's main checkout (is_linked_worktree:false), detached
// worktrees (no branch -> no upstream -> never "gone"), and branches whose
// upstream still exists. The invoking workspace is flagged (`isCurrent`) rather
// than dropped, so the preview can explain why it's left in place.
export function selectGoneWorktrees(worktrees, goneBranches, { currentWorkspaceId = null } = {}) {
  const candidates = [];
  for (const wt of worktrees ?? []) {
    if (!wt.is_linked_worktree) continue;
    if (wt.is_detached || !wt.branch) continue;
    if (!goneBranches.has(wt.branch)) continue;
    candidates.push({
      branch: wt.branch,
      path: wt.path ?? null,
      workspaceId: wt.open_workspace_id ?? null,
      isCurrent: Boolean(currentWorkspaceId && wt.open_workspace_id === currentWorkspaceId),
      dirty: false,
    });
  }
  return candidates;
}

// The workspace this action was invoked from. Mirrors bin/apply.mjs's resolution
// order; the env overrides exist for tests.
export function resolveWorkspaceId(env = process.env) {
  let ctx = null;
  try {
    ctx = JSON.parse(env.HERDR_PLUGIN_CONTEXT_JSON ?? "null");
  } catch {
    ctx = null;
  }
  return (
    env.HERDR_WSM_WORKSPACE ||
    env.HERDR_WORKSPACE_ID ||
    ctx?.workspace?.workspace_id ||
    ctx?.workspace_id ||
    null
  );
}

// All worktrees of the current repo, plus its `source` (repo_root/repo_name).
// Scoped to the invoking workspace's repo, so this is "the current repo".
export function listRepoWorktrees(env, workspaceId) {
  const args = ["worktree", "list", "--json"];
  if (workspaceId) args.push("--workspace", workspaceId);
  const result = runHerdrJson(args, { env });
  return { source: result?.source ?? null, worktrees: result?.worktrees ?? [] };
}

// Map of workspace_id -> its display name (label), so candidates can be shown by
// the name you see in herdr rather than just a branch. Best-effort.
export function workspaceLabels(env) {
  const map = new Map();
  let list = [];
  try {
    list = runHerdrJson(["workspace", "list"], { env })?.workspaces ?? [];
  } catch {
    list = [];
  }
  for (const w of list) {
    if (w.workspace_id) map.set(w.workspace_id, w.label ?? null);
  }
  return map;
}

// Prune stale remote-tracking refs across all remotes so a deleted upstream
// actually reads as "gone". Best-effort: on failure we keep going with whatever
// refs are already cached (and tell the caller).
export function fetchPrune(env, repoRoot, logger) {
  const r = runGit(["fetch", "--all", "--prune"], { cwd: repoRoot, env });
  if (r.status !== 0 && logger) {
    logger(
      `git fetch --prune failed; using cached refs (a still-present upstream may read as gone): ` +
        (r.stderr.trim() || r.error?.message || "unknown error"),
    );
  }
  return r.status === 0;
}

export function goneBranchSet(env, repoRoot) {
  const r = runGit(
    ["for-each-ref", "--format=%(refname:short)\t%(upstream:track,nobracket)", "refs/heads"],
    { cwd: repoRoot, env },
  );
  if (r.status !== 0) {
    throw new Error(`git for-each-ref failed: ${r.stderr.trim() || r.error?.message || "unknown error"}`);
  }
  return parseGoneBranches(r.stdout);
}

// Uncommitted changes (modified tracked files or untracked files). A "gone"
// branch's committed work lives on in the repo regardless of removal; only an
// unclean working tree risks data loss, so it's what we guard on.
export function isDirty(env, worktreePath) {
  if (!worktreePath) return false;
  const r = runGit(["status", "--porcelain"], { cwd: worktreePath, env });
  if (r.status !== 0) return false; // can't tell -> don't block on a guess
  return r.stdout.trim().length > 0;
}

// Gather the removal candidates for the current repo. Pure-ish orchestration:
// query herdr, (optionally) fetch+prune, diff against the gone set, flag dirty.
export function collectGoneWorktrees({ env = process.env, fetch = true, logger } = {}) {
  const workspaceId = resolveWorkspaceId(env);
  const { source, worktrees } = listRepoWorktrees(env, workspaceId);
  const repoRoot = source?.repo_root ?? null;
  if (!repoRoot) {
    throw new Error("could not determine the current repo (the workspace has no git worktree)");
  }

  if (fetch) fetchPrune(env, repoRoot, logger);
  const gone = goneBranchSet(env, repoRoot);
  const candidates = selectGoneWorktrees(worktrees, gone, { currentWorkspaceId: workspaceId });
  const labels = workspaceLabels(env);
  for (const c of candidates) {
    c.dirty = isDirty(env, c.path);
    // The workspace name as shown in herdr; fall back to the branch if the
    // worktree has no open workspace (or it's unnamed).
    c.label = (c.workspaceId && labels.get(c.workspaceId)) || c.branch;
  }

  return { repo: source, workspaceId, candidates };
}

// Remove one worktree by its open workspace id. Throws (via runHerdrJson) on
// failure so the caller can report it per-worktree.
export function removeWorktree(env, workspaceId, { force = false } = {}) {
  const args = ["worktree", "remove", "--workspace", workspaceId];
  if (force) args.push("--force");
  args.push("--json");
  return runHerdrJson(args, { env });
}

// --- shared rendering / execution (used by the CLI and the plugin actions) ---

export function repoDisplayName(repo) {
  return repo?.repo_name ?? repo?.repo_root ?? "the current repo";
}

// Why a candidate would be skipped during removal, or null if it's removable.
// Centralizes the safety policy so the preview, the prompt count, and the actual
// removal all agree on what's eligible.
export function removalSkipReason(c, { force = false } = {}) {
  if (c.isCurrent) return "current workspace — switch away, then re-run";
  if (!c.workspaceId) return "no open workspace — run `git worktree remove`";
  if (c.dirty && !force) return "uncommitted changes — re-run with --force to remove anyway";
  return null;
}

// The subset of candidates that would actually be removed under `force`.
export function removableCandidates(candidates, { force = false } = {}) {
  return (candidates ?? []).filter((c) => removalSkipReason(c, { force }) === null);
}

// Human-readable list of removal candidates, each led by its workspace name.
// An empty list renders the "nothing to do" line. `force` tunes the dirty note.
export function formatPreview(repoName, candidates, { force = false } = {}) {
  if (!candidates || candidates.length === 0) {
    return `No worktrees with a deleted remote branch in ${repoName}.\n`;
  }
  let out = `Workspaces in ${repoName} whose remote branch is gone (${candidates.length}):\n\n`;
  for (const c of candidates) {
    const flags = [];
    if (c.isCurrent) flags.push("CURRENT workspace — switch away first; will be skipped");
    if (!c.workspaceId) flags.push("no open workspace — remove with `git worktree remove`");
    if (c.dirty) {
      flags.push(
        force
          ? "uncommitted changes — will be force-removed"
          : "uncommitted changes — will be skipped unless --force",
      );
    }
    const branch = c.label === c.branch ? "" : `  (branch ${c.branch})`;
    const tag = flags.length ? `\n    ⚠ ${flags.join("; ")}` : "";
    out += `  • ${c.label}${branch}\n    ${c.path ?? "(unknown path)"}${tag}\n`;
  }
  return out;
}

// Remove the eligible candidates, returning { removed, skipped }. Skips (never
// destroys silently) the invoking workspace, worktrees with no open workspace,
// and — unless `force` — worktrees with uncommitted changes.
export function applyRemovals({ env = process.env, candidates, force = false, logger } = {}) {
  const removed = [];
  const skipped = [];
  for (const c of candidates ?? []) {
    const reason = removalSkipReason(c, { force });
    if (reason) {
      skipped.push({ ...c, reason });
      continue;
    }
    try {
      removeWorktree(env, c.workspaceId, { force: c.dirty });
      removed.push(c);
      if (logger) logger(`removed ${c.label} (${c.path})`);
    } catch (err) {
      skipped.push({ ...c, reason: err.message });
    }
  }
  return { removed, skipped };
}

export function formatApplyResult(repoName, removed, skipped) {
  let out = `Removed ${removed.length} gone worktree(s) in ${repoName}:\n`;
  for (const c of removed) out += `  ✓ ${c.label}  ${c.path ?? ""}\n`;
  if (skipped && skipped.length) {
    out += `\nSkipped ${skipped.length}:\n`;
    for (const c of skipped) out += `  • ${c.label} — ${c.reason}\n`;
  }
  return out;
}
