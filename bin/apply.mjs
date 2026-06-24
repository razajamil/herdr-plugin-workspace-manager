#!/usr/bin/env node
// Manual `apply` action. Applies a layout to the current (or a target)
// workspace. Layout selection order:
//   1. CLI argument           (node bin/apply.mjs <layoutId>)
//   2. HERDR_WSM_LAYOUT env
//   3. the workspace's default layout, matched by its checkout path
//
// Target selection (each overridable for testing):
//   workspace: HERDR_WSM_WORKSPACE | HERDR_WORKSPACE_ID | context
//   tab:       HERDR_WSM_TAB       | HERDR_TAB_ID       | workspace's first tab
//   pane:      HERDR_WSM_PANE      | HERDR_PANE_ID      | tab's first pane
//   cwd:       HERDR_WSM_CWD       | workspace checkout path

import { loadConfig, findLayout, matchWorkspaceLayout } from "../src/config.mjs";
import { resolveTarget, applyLayout, getWorktreeBranch } from "../src/apply-core.mjs";

const env = process.env;
const log = (msg) => process.stderr.write(`[workspace-manager] ${msg}\n`);

function contextJson() {
  try {
    return JSON.parse(env.HERDR_PLUGIN_CONTEXT_JSON ?? "null");
  } catch {
    return null;
  }
}

async function main() {
  const { path: configPath, config } = loadConfig(env);
  if (!configPath) throw new Error("no config file found");

  const ctx = contextJson() ?? {};
  const workspaceId =
    env.HERDR_WSM_WORKSPACE ||
    env.HERDR_WORKSPACE_ID ||
    ctx.workspace?.workspace_id ||
    ctx.workspace_id ||
    null;
  const tabId = env.HERDR_WSM_TAB || env.HERDR_TAB_ID || ctx.tab?.tab_id || null;
  const paneId = env.HERDR_WSM_PANE || env.HERDR_PANE_ID || ctx.pane?.pane_id || null;
  const cwd = env.HERDR_WSM_CWD || ctx.workspace?.worktree?.checkout_path || null;

  const target = resolveTarget({ env, workspaceId, tabId, rootPaneId: paneId, cwd });

  const explicitId = process.argv[2] || env.HERDR_WSM_LAYOUT || null;
  let layout;
  if (explicitId) {
    layout = findLayout(config, explicitId);
    if (!layout) {
      throw new Error(
        `layout "${explicitId}" not found (have: ${config.layouts.map((l) => l.id).join(", ") || "none"})`,
      );
    }
  } else {
    const wt = ctx.workspace?.worktree ?? {};
    const branch = config.workspaces.some((ws) => ws.layoutMatching.length)
      ? getWorktreeBranch(env, target.workspaceId, target.cwd)
      : null;
    const match = matchWorkspaceLayout(config, {
      checkoutPath: target.cwd,
      repoRoot: wt.repo_root ?? null,
      repoName: wt.repo_name ?? null,
      branch,
    });
    if (!match) {
      throw new Error(
        `no layout id given and no workspace default matches ${target.cwd}`,
      );
    }
    layout = match.layout;
  }

  log(`applying layout "${layout.id}" to workspace ${target.workspaceId}`);
  const summary = await applyLayout({ env, layout, target, logger: log });
  process.stdout.write(JSON.stringify(summary) + "\n");
}

main().catch((err) => {
  log(`error: ${err.message}`);
  process.exit(1);
});
