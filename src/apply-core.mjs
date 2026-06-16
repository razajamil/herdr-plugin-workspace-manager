// Shared logic for the event hook and the manual `apply` action: figure out
// which workspace/tab/pane to build into, then build + run the plan.

import { mkdirSync, existsSync, rmSync } from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildPlan } from "./plan.mjs";
import { executePlan } from "./runner.mjs";
import { runHerdrJson } from "./herdr.mjs";

function tryJson(text) {
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

// Pull whatever the worktree.created event gives us out of the environment.
// The real event wraps its records under `data` and carries `workspace`
// (with `active_tab_id` + nested `worktree`) and a top-level `worktree`; it
// does NOT include tab/root_pane records, so those are resolved live later.
//
// Crucially this reads ONLY the event payload -- never the ambient
// HERDR_PANE_ID / HERDR_TAB_ID / HERDR_WORKSPACE_ID. Those describe whichever
// pane happened to be focused when the event fired (e.g. your agent pane), and
// using them would build the layout into the wrong pane. The target pane is
// always resolved from the new worktree's workspace.
export function getEventPayload(env = process.env) {
  const ev = tryJson(env.HERDR_PLUGIN_EVENT_JSON) ?? {};
  const data = ev.data ?? ev;
  const workspace = data.workspace ?? {};
  const worktree = data.worktree ?? workspace.worktree ?? {};
  const wt = workspace.worktree ?? worktree;
  return {
    // workspace.focused carries only data.workspace_id; created events nest it.
    workspaceId: workspace.workspace_id ?? workspace.id ?? data.workspace_id ?? null,
    tabId: data.tab?.tab_id ?? workspace.active_tab_id ?? null,
    rootPaneId: data.root_pane?.pane_id ?? data.pane?.pane_id ?? null,
  };
}

// Look up a workspace's worktree facts + freshness by id. This is the source of
// truth (the workspace.focused payload only gives an id), and it's also more
// reliable than parsing per-event payload shapes for the created events.
export function getWorkspaceInfo(env = process.env, workspaceId) {
  let ws = null;
  try {
    ws = runHerdrJson(["workspace", "get", workspaceId], { env })?.workspace ?? null;
  } catch {
    ws = null;
  }
  if (!ws) {
    const list = runHerdrJson(["workspace", "list"], { env })?.workspaces ?? [];
    ws = list.find((w) => w.workspace_id === workspaceId) ?? null;
  }
  if (!ws) return null;
  const wt = ws.worktree ?? {};
  return {
    checkoutPath: wt.checkout_path ?? null,
    repoRoot: wt.repo_root ?? null,
    repoName: wt.repo_name ?? null,
    isLinkedWorktree: Boolean(wt.is_linked_worktree),
    tabCount: ws.tab_count ?? null,
    paneCount: ws.pane_count ?? null,
    activeTabId: ws.active_tab_id ?? null,
  };
}

// "Decided" cache (by workspace id) for the high-frequency workspace.focused
// event: once we've handled a workspace once, repeat focuses skip immediately
// without re-querying. Workspace ids are stable per logical workspace, so this
// is safe; the persistent `claim` (by checkout path) remains the real guard.
function decidedPath(env, workspaceId) {
  return path.join(stateDir(env), "decided", String(workspaceId).replace(/[^\w.-]/g, "_"));
}

export function isDecided(env, workspaceId) {
  return existsSync(decidedPath(env, workspaceId));
}

export function markDecided(env, workspaceId) {
  mkdirSync(path.join(stateDir(env), "decided"), { recursive: true });
  try {
    mkdirSync(decidedPath(env, workspaceId));
  } catch {
    /* already marked */
  }
}

// --- Idempotency + freshness guards --------------------------------------
// Both worktree.created and workspace.created fire for one CLI creation, and
// workspace.created can also fire on restore. These guards ensure a layout is
// applied exactly once per worktree, only when the workspace is brand new.

function stateDir(env) {
  return env.HERDR_PLUGIN_STATE_DIR || path.join(tmpdir(), "herdr-wsc-state");
}

function claimPath(env, checkoutPath) {
  const key = createHash("sha1").update(path.resolve(checkoutPath)).digest("hex");
  return path.join(stateDir(env), "applied", key);
}

// Atomically claim a worktree for application. Returns true if we won the claim
// (first to see it), false if it was already claimed. mkdir is atomic across
// processes, so concurrent worktree.created/workspace.created hooks can't both
// win. The claim persists, so restored worktrees are skipped after a restart.
export function claimApply(env, checkoutPath) {
  mkdirSync(path.join(stateDir(env), "applied"), { recursive: true });
  try {
    mkdirSync(claimPath(env, checkoutPath)); // non-recursive -> EEXIST if taken
    return true;
  } catch (err) {
    if (err.code === "EEXIST") return false;
    throw err;
  }
}

export function releaseApply(env, checkoutPath) {
  rmSync(claimPath(env, checkoutPath), { recursive: true, force: true });
}

export function alreadyApplied(env, checkoutPath) {
  return existsSync(claimPath(env, checkoutPath));
}

// A brand-new worktree workspace has exactly one tab and one pane. Anything
// else (a restored/already-arranged workspace) must be left alone.
export function isFreshWorkspace(env, workspaceId) {
  const tabs = runHerdrJson(["tab", "list", "--workspace", workspaceId], { env })?.tabs ?? [];
  if (tabs.length !== 1) return false;
  const panes = runHerdrJson(["pane", "list", "--workspace", workspaceId], { env })?.panes ?? [];
  return panes.filter((p) => p.tab_id === tabs[0].tab_id).length === 1;
}

function firstByNumber(items, key) {
  return [...items].sort((a, b) => (a.number ?? 0) - (b.number ?? 0))[0]?.[key] ?? null;
}

// Fill in any missing tab/pane/cwd for a workspace by querying the live server.
export function resolveTarget({ env = process.env, workspaceId, tabId, rootPaneId, cwd }) {
  if (!workspaceId) throw new Error("could not determine target workspace id");

  // herdr ids are workspace-prefixed ("w9:t1", "w9:p1"). Discard any tab/pane
  // id that doesn't belong to this workspace -- it's stale/ambient context and
  // must never be built into. Re-resolve from the workspace instead.
  const prefix = `${workspaceId}:`;
  let resolvedTab = tabId && tabId.startsWith(prefix) ? tabId : null;
  let resolvedPane = rootPaneId && rootPaneId.startsWith(prefix) ? rootPaneId : null;
  let resolvedCwd = cwd;

  if (!resolvedCwd || !resolvedTab) {
    const wsList = runHerdrJson(["workspace", "list"], { env });
    const ws = (wsList?.workspaces ?? []).find((w) => w.workspace_id === workspaceId);
    if (ws) {
      resolvedTab = resolvedTab ?? ws.active_tab_id;
      resolvedCwd = resolvedCwd ?? ws.worktree?.checkout_path ?? null;
    }
  }

  if (!resolvedTab) {
    const tabs = runHerdrJson(["tab", "list", "--workspace", workspaceId], { env });
    resolvedTab = firstByNumber(tabs?.tabs ?? [], "tab_id");
  }
  if (!resolvedTab) throw new Error(`no tab found for workspace ${workspaceId}`);

  if (!resolvedPane) {
    const panes = runHerdrJson(["pane", "list", "--workspace", workspaceId], { env });
    const inTab = (panes?.panes ?? []).filter((p) => p.tab_id === resolvedTab);
    resolvedPane = inTab[0]?.pane_id ?? null;
    if (!resolvedCwd) resolvedCwd = inTab[0]?.cwd ?? null;
  }
  if (!resolvedPane) throw new Error(`no root pane found for tab ${resolvedTab}`);

  return { workspaceId, rootTab: resolvedTab, rootPane: resolvedPane, cwd: resolvedCwd };
}

export async function applyLayout({ env = process.env, layout, target, logger }) {
  const plan = buildPlan(layout, { cwd: target.cwd });
  const summary = await executePlan(plan, target, { env, logger });
  return summary;
}
