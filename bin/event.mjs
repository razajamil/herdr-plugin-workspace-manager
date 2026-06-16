#!/usr/bin/env node
// Event hook for worktree.created, workspace.created AND workspace.focused.
//
// Why three events: the `herdr worktree create` CLI emits worktree.created (and
// workspace.created), but the herdr **UI** "new worktree" command creates the
// worktree in-app and emits NEITHER to plugins — the only thing it dispatches is
// workspace.focused (it focuses the new worktree right after creating it). So we
// listen for all three and resolve the worktree facts by querying the workspace.
//
// Guards (a layout is applied exactly once, only to a brand-new linked worktree):
//   - linked-worktree only (never the repo's main checkout)
//   - repo/path must match the config
//   - fresh 1-tab/1-pane workspace only (don't clobber arranged/restored ones)
//   - atomic claim by checkout path (dedupe across the multiple events + restarts)
//   - "decided" cache by workspace id so repeat focuses don't re-query

import { loadConfig, matchWorkspaceLayout } from "../src/config.mjs";
import {
  getEventPayload,
  getWorkspaceInfo,
  resolveTarget,
  applyLayout,
  claimApply,
  releaseApply,
  isDecided,
  markDecided,
} from "../src/apply-core.mjs";

const env = process.env;
const log = (msg) => process.stderr.write(`[workspace-manager] ${msg}\n`);

async function main() {
  const { path: configPath, config } = loadConfig(env);
  if (!configPath) return;

  const isFocus = (env.HERDR_PLUGIN_EVENT || "").includes("focus");
  const p = getEventPayload(env);
  if (!p.workspaceId) return;

  // Fast path: workspace.focused fires constantly; once a workspace has been
  // handled, skip without querying anything.
  if (isFocus && isDecided(env, p.workspaceId)) return;

  const done = (msg) => {
    if (msg) log(msg);
    if (isFocus) markDecided(env, p.workspaceId);
  };

  const info = getWorkspaceInfo(env, p.workspaceId);
  if (!info || !info.checkoutPath) return done(); // not a worktree workspace
  if (!info.isLinkedWorktree) return done(); // the repo's main checkout — never touch

  const match = matchWorkspaceLayout(config, {
    checkoutPath: info.checkoutPath,
    repoRoot: info.repoRoot,
    repoName: info.repoName,
  });
  if (!match) return done(`no workspace/default layout matches ${info.checkoutPath}; skipping`);

  if (info.tabCount !== 1 || info.paneCount !== 1) {
    return done(`workspace ${p.workspaceId} is not a fresh 1-tab/1-pane workspace; skipping`);
  }

  if (!claimApply(env, info.checkoutPath)) {
    return done(`layout already applied for ${info.checkoutPath}; skipping`);
  }

  log(`applying layout "${match.layout.id}" to ${info.checkoutPath}`);
  try {
    const target = resolveTarget({
      env,
      workspaceId: p.workspaceId,
      tabId: p.tabId ?? info.activeTabId,
      rootPaneId: p.rootPaneId,
      cwd: info.checkoutPath,
    });
    const summary = await applyLayout({ env, layout: match.layout, target, logger: log });
    if (isFocus) markDecided(env, p.workspaceId);
    process.stdout.write(JSON.stringify(summary) + "\n");
  } catch (err) {
    releaseApply(env, info.checkoutPath); // allow a retry on transient failure
    throw err;
  }
}

main().catch((err) => {
  log(`error: ${err.message}`);
  process.exit(1);
});
