import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  getEventPayload,
  claimApply,
  alreadyApplied,
  releaseApply,
  isDecided,
  markDecided,
} from "../src/apply-core.mjs";

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
