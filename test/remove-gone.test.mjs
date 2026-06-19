import { test } from "node:test";
import assert from "node:assert/strict";
import {
  parseGoneBranches,
  selectGoneWorktrees,
  resolveWorkspaceId,
  repoDisplayName,
  formatPreview,
  formatApplyResult,
  removalSkipReason,
  removableCandidates,
} from "../src/remove-gone.mjs";

test("parseGoneBranches keeps only branches whose upstream is gone", () => {
  // Tab-separated: <branch>\t<upstream:track,nobracket>.
  const text = [
    "main\t", // upstream exists, in sync -> keep (not gone)
    "feature-a\tgone", // deleted upstream -> gone
    "feature-b\tahead 2", // ahead of an existing upstream -> not gone
    "local-only\t", // never pushed / no upstream -> not gone
    "feature-c\tgone", // gone
    "", // blank line ignored
  ].join("\n");
  const gone = parseGoneBranches(text);
  assert.deepEqual([...gone].sort(), ["feature-a", "feature-c"]);
});

test("parseGoneBranches tolerates empty / whitespace input", () => {
  assert.equal(parseGoneBranches("").size, 0);
  assert.equal(parseGoneBranches("   \n\n").size, 0);
  assert.equal(parseGoneBranches(null).size, 0);
});

const WORKTREES = [
  // main checkout — never a candidate even if (impossibly) "gone".
  { branch: "main", is_linked_worktree: false, path: "/repo", open_workspace_id: "w1" },
  // linked worktree, branch gone -> candidate.
  {
    branch: "feature-a",
    is_linked_worktree: true,
    path: "/wt/feature-a",
    open_workspace_id: "w2",
  },
  // linked, branch NOT gone -> excluded.
  {
    branch: "feature-keep",
    is_linked_worktree: true,
    path: "/wt/feature-keep",
    open_workspace_id: "w3",
  },
  // linked, detached -> excluded (no branch).
  {
    branch: "",
    is_detached: true,
    is_linked_worktree: true,
    path: "/wt/detached",
    open_workspace_id: "w4",
  },
  // linked, gone, but no open workspace -> still a candidate (workspaceId null).
  {
    branch: "feature-orphan",
    is_linked_worktree: true,
    path: "/wt/orphan",
    open_workspace_id: null,
  },
];

test("selectGoneWorktrees picks linked, gone, branch-bearing worktrees", () => {
  const gone = new Set(["feature-a", "feature-orphan", "main"]);
  const picked = selectGoneWorktrees(WORKTREES, gone);
  assert.deepEqual(
    picked.map((c) => c.branch).sort(),
    ["feature-a", "feature-orphan"],
  );
  const orphan = picked.find((c) => c.branch === "feature-orphan");
  assert.equal(orphan.workspaceId, null);
});

test("selectGoneWorktrees flags the invoking workspace", () => {
  const gone = new Set(["feature-a"]);
  const picked = selectGoneWorktrees(WORKTREES, gone, { currentWorkspaceId: "w2" });
  assert.equal(picked.length, 1);
  assert.equal(picked[0].isCurrent, true);
});

test("selectGoneWorktrees handles empty / missing inputs", () => {
  assert.deepEqual(selectGoneWorktrees(undefined, new Set()), []);
  assert.deepEqual(selectGoneWorktrees(WORKTREES, new Set()), []);
});

test("repoDisplayName prefers name, then root, then a fallback", () => {
  assert.equal(repoDisplayName({ repo_name: "r", repo_root: "/x" }), "r");
  assert.equal(repoDisplayName({ repo_root: "/x" }), "/x");
  assert.equal(repoDisplayName(null), "the current repo");
});

test("formatPreview lists candidates by workspace name and flags risks", () => {
  const out = formatPreview("myrepo", [
    { label: "feat-a", branch: "feat-a", path: "/wt/a", workspaceId: "w2", isCurrent: false, dirty: false },
    { label: "nice-name", branch: "feature/x", path: "/wt/x", workspaceId: "w3", isCurrent: false, dirty: true },
  ]);
  assert.match(out, /gone \(2\)/);
  assert.match(out, /• feat-a\n {4}\/wt\/a/);
  // Branch shown in parens only when it differs from the workspace name.
  assert.match(out, /• nice-name {2}\(branch feature\/x\)/);
  assert.doesNotMatch(out, /feat-a {2}\(branch/);
  assert.match(out, /uncommitted changes/);
});

test("formatPreview renders the empty case", () => {
  assert.equal(
    formatPreview("myrepo", []),
    "No worktrees with a deleted remote branch in myrepo.\n",
  );
});

test("formatApplyResult summarizes removed and skipped", () => {
  const out = formatApplyResult(
    "myrepo",
    [{ label: "a", path: "/wt/a" }],
    [{ label: "b", reason: "uncommitted changes" }],
  );
  assert.match(out, /Removed 1 gone worktree\(s\) in myrepo/);
  assert.match(out, /✓ a {2}\/wt\/a/);
  assert.match(out, /Skipped 1/);
  assert.match(out, /• b — uncommitted changes/);
});

test("removalSkipReason encodes the safety policy", () => {
  const clean = { workspaceId: "w2", isCurrent: false, dirty: false };
  assert.equal(removalSkipReason(clean), null);
  assert.match(removalSkipReason({ ...clean, isCurrent: true }), /current workspace/);
  assert.match(removalSkipReason({ ...clean, workspaceId: null }), /no open workspace/);
  assert.match(removalSkipReason({ ...clean, dirty: true }), /uncommitted changes/);
  // --force makes a dirty worktree removable.
  assert.equal(removalSkipReason({ ...clean, dirty: true }, { force: true }), null);
});

test("removableCandidates filters to what would actually be removed", () => {
  const candidates = [
    { label: "a", workspaceId: "w2", isCurrent: false, dirty: false }, // removable
    { label: "b", workspaceId: "w3", isCurrent: true, dirty: false }, // current -> skip
    { label: "c", workspaceId: "w4", isCurrent: false, dirty: true }, // dirty -> skip (no force)
  ];
  assert.deepEqual(removableCandidates(candidates).map((c) => c.label), ["a"]);
  assert.deepEqual(
    removableCandidates(candidates, { force: true }).map((c) => c.label),
    ["a", "c"],
  );
  assert.deepEqual(removableCandidates([]), []);
});

test("formatPreview reflects --force in the dirty note", () => {
  const dirty = [{ label: "x", branch: "x", path: "/wt/x", workspaceId: "w3", dirty: true }];
  assert.match(formatPreview("r", dirty), /will be skipped unless --force/);
  assert.match(formatPreview("r", dirty, { force: true }), /will be force-removed/);
});

test("formatApplyResult omits the skipped section when none skipped", () => {
  const out = formatApplyResult("myrepo", [{ label: "a", path: "/wt/a" }], []);
  assert.doesNotMatch(out, /Skipped/);
});

test("resolveWorkspaceId prefers env overrides then context json", () => {
  assert.equal(resolveWorkspaceId({ HERDR_WSM_WORKSPACE: "wA" }), "wA");
  assert.equal(resolveWorkspaceId({ HERDR_WORKSPACE_ID: "wB" }), "wB");
  assert.equal(
    resolveWorkspaceId({ HERDR_PLUGIN_CONTEXT_JSON: JSON.stringify({ workspace: { workspace_id: "wC" } }) }),
    "wC",
  );
  assert.equal(resolveWorkspaceId({ HERDR_PLUGIN_CONTEXT_JSON: "not json" }), null);
  assert.equal(resolveWorkspaceId({}), null);
});
