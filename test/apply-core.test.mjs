import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import path from "node:path";
import {
  getEventPayload,
  claimApply,
  alreadyApplied,
  releaseApply,
  reapOrphanClaims,
  isDecided,
  markDecided,
} from "../src/apply-core.mjs";

// The on-disk claim directory for a checkout path (mirrors apply-core's keying).
function claimDir(stateDir, checkout) {
  const key = createHash("sha1").update(path.resolve(checkout)).digest("hex");
  return path.join(stateDir, "applied", key);
}

test("getEventPayload reads the workspace.focused (id-only) shape", () => {
  const env = {
    HERDR_PLUGIN_EVENT_JSON: JSON.stringify({
      event: "workspace_focused",
      data: { type: "workspace_focused", workspace_id: "wY" },
    }),
  };
  assert.equal(getEventPayload(env).workspaceId, "wY");
});

test("getEventPayload reads the nested created shape", () => {
  const env = {
    HERDR_PLUGIN_EVENT_JSON: JSON.stringify({
      event: "worktree_created",
      data: { workspace: { workspace_id: "wZ", active_tab_id: "wZ:t1" } },
    }),
  };
  const p = getEventPayload(env);
  assert.equal(p.workspaceId, "wZ");
  assert.equal(p.tabId, "wZ:t1");
});

test("claimApply is atomic and idempotent per checkout path", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "wsc-claim-"));
  const env = { HERDR_PLUGIN_STATE_DIR: dir };
  const checkout = "/some/worktree/path";
  try {
    assert.equal(alreadyApplied(env, checkout), false);
    assert.equal(claimApply(env, checkout), true, "first claim wins");
    assert.equal(alreadyApplied(env, checkout), true);
    assert.equal(claimApply(env, checkout), false, "second claim loses");
    // a different path is independent
    assert.equal(claimApply(env, "/another/path"), true);
    // releasing allows a re-claim (used on transient apply failure)
    releaseApply(env, checkout);
    assert.equal(alreadyApplied(env, checkout), false);
    assert.equal(claimApply(env, checkout), true);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("claimApply keeps the claim for the same worktree (restore-safe)", () => {
  const state = mkdtempSync(path.join(tmpdir(), "wsc-restore-"));
  const wt = mkdtempSync(path.join(tmpdir(), "wsc-wt-"));
  const env = { HERDR_PLUGIN_STATE_DIR: state };
  try {
    assert.equal(claimApply(env, wt), true, "first claim wins");
    // Same directory, untouched: a restored worktree must still be skipped so we
    // don't clobber an already-arranged workspace.
    assert.equal(claimApply(env, wt), false, "unchanged worktree stays claimed");
  } finally {
    rmSync(state, { recursive: true, force: true });
    rmSync(wt, { recursive: true, force: true });
  }
});

test("claimApply re-claims a recreated worktree at the same path (stale claim)", () => {
  const state = mkdtempSync(path.join(tmpdir(), "wsc-stale-"));
  const wt = mkdtempSync(path.join(tmpdir(), "wsc-wt-"));
  const env = { HERDR_PLUGIN_STATE_DIR: state };
  try {
    assert.equal(claimApply(env, wt), true);
    assert.equal(claimApply(env, wt), false);

    // Simulate the worktree having been removed and recreated at the same path:
    // the recorded identity (here, the inode) no longer matches the live dir.
    writeFileSync(
      path.join(claimDir(state, wt), "meta.json"),
      JSON.stringify({ path: path.resolve(wt), ino: "0", birthtimeMs: 0 }),
    );
    assert.equal(
      claimApply(env, wt),
      true,
      "identity mismatch -> stale claim reset -> re-applies",
    );
    // ...and the refreshed claim now matches again.
    assert.equal(claimApply(env, wt), false, "fresh claim is honoured");
  } finally {
    rmSync(state, { recursive: true, force: true });
    rmSync(wt, { recursive: true, force: true });
  }
});

test("reapOrphanClaims drops claims whose worktree is gone, keeps live ones", () => {
  const state = mkdtempSync(path.join(tmpdir(), "wsc-reap-"));
  const live = mkdtempSync(path.join(tmpdir(), "wsc-live-"));
  const gone = mkdtempSync(path.join(tmpdir(), "wsc-gone-"));
  const env = { HERDR_PLUGIN_STATE_DIR: state };
  try {
    assert.equal(claimApply(env, live), true);
    assert.equal(claimApply(env, gone), true);
    rmSync(gone, { recursive: true, force: true }); // worktree removed out-of-band

    assert.equal(reapOrphanClaims(env), 1, "only the orphaned claim is reaped");
    assert.equal(alreadyApplied(env, gone), false, "orphan claim removed");
    assert.equal(alreadyApplied(env, live), true, "live claim kept");
    assert.equal(reapOrphanClaims(env), 0, "second sweep is a no-op");
  } finally {
    rmSync(state, { recursive: true, force: true });
    rmSync(live, { recursive: true, force: true });
  }
});

test("decided cache is per workspace id", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "wsc-decided-"));
  const env = { HERDR_PLUGIN_STATE_DIR: dir };
  try {
    assert.equal(isDecided(env, "w5"), false);
    markDecided(env, "w5");
    assert.equal(isDecided(env, "w5"), true);
    assert.equal(isDecided(env, "w6"), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
