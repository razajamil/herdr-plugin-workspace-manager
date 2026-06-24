// Shared logic for the event hook and the manual `apply` action: figure out
// which workspace/tab/pane to build into, then build + run the plan.

import {
  mkdirSync,
  existsSync,
  rmSync,
  statSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
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

// The git branch of a workspace's worktree. herdr's `workspace get`/`list` omit
// the branch from the nested worktree record, so we read it from `worktree list`
// (scoped to the workspace's repo) and match on the open workspace id, falling
// back to the checkout path. Returns null for a detached HEAD or when it can't
// be resolved -- callers then fall back to the workspace's defaultLayout. This
// is only queried when the config actually uses branch-based layoutMatching.
export function getWorktreeBranch(env = process.env, workspaceId, checkoutPath = null) {
  let worktrees;
  try {
    const args = ["worktree", "list", "--json"];
    if (workspaceId) args.push("--workspace", workspaceId);
    worktrees = runHerdrJson(args, { env })?.worktrees ?? [];
  } catch {
    return null;
  }
  const wt =
    (workspaceId && worktrees.find((w) => w.open_workspace_id === workspaceId)) ||
    (checkoutPath &&
      worktrees.find((w) => w.path && path.resolve(w.path) === path.resolve(checkoutPath))) ||
    null;
  return wt?.branch || null; // "" (detached) -> null
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

function appliedDir(env) {
  return path.join(stateDir(env), "applied");
}

function claimPath(env, checkoutPath) {
  const key = createHash("sha1").update(path.resolve(checkoutPath)).digest("hex");
  return path.join(appliedDir(env), key);
}

function claimMetaPath(dir) {
  return path.join(dir, "meta.json");
}

// The worktree's filesystem identity. A delete+recreate at the same path gets a
// new inode (a brand-new directory) and, on filesystems that record it, a new
// birth time -- either of which marks the claim from the previous worktree as
// stale. Returns nulls if the path can't be stat'd (e.g. already gone), in
// which case callers treat the identity as unknowable.
function worktreeIdentity(checkoutPath) {
  try {
    const st = statSync(checkoutPath);
    return {
      ino: String(st.ino),
      birthtimeMs: Number.isFinite(st.birthtimeMs) && st.birthtimeMs > 0
        ? Math.floor(st.birthtimeMs)
        : null,
    };
  } catch {
    return { ino: null, birthtimeMs: null };
  }
}

function readClaimMeta(dir) {
  try {
    return JSON.parse(readFileSync(claimMetaPath(dir), "utf8"));
  } catch {
    return null;
  }
}

function writeClaimMeta(dir, checkoutPath) {
  try {
    writeFileSync(
      claimMetaPath(dir),
      JSON.stringify({ path: path.resolve(checkoutPath), ...worktreeIdentity(checkoutPath) }),
    );
  } catch {
    /* best effort -- a missing record just falls back to the mtime heuristic */
  }
}

// Is an existing claim stale -- i.e. left over from a *previous* worktree that
// lived at this same path and has since been removed and recreated? A worktree
// can be removed by this plugin, another plugin, or the user directly, and none
// of those reliably emit an event, so we decide from filesystem ground truth at
// claim time rather than trying to catch the removal.
function isStaleClaim(dir, checkoutPath) {
  const cur = worktreeIdentity(checkoutPath);
  if (cur.ino == null && cur.birthtimeMs == null) return false; // identity unknowable
  const meta = readClaimMeta(dir);
  // A different inode means the directory was recreated.
  if (meta?.ino != null && cur.ino != null && meta.ino !== cur.ino) return true;
  // The current worktree was born after the claim was established -> it's a
  // newer instance at the same path. Use the recorded birth time, or for legacy
  // claims (no record) fall back to the claim directory's own mtime.
  let claimAt = meta?.birthtimeMs ?? null;
  if (claimAt == null) {
    try {
      claimAt = statSync(dir).mtimeMs;
    } catch {
      claimAt = null;
    }
  }
  if (claimAt != null && cur.birthtimeMs != null && cur.birthtimeMs > claimAt) return true;
  return false;
}

// Atomically claim a worktree for application. Returns true if we won the claim
// (first to see it), false if a valid claim already exists. mkdir is atomic
// across processes, so concurrent worktree.created/workspace.created hooks can't
// both win. The claim persists across restarts so *restored* worktrees are
// skipped -- but a stale claim left by a removed-and-recreated worktree at the
// same path is detected and reset, so the layout is re-applied on recreate.
export function claimApply(env, checkoutPath) {
  const dir = claimPath(env, checkoutPath);
  mkdirSync(appliedDir(env), { recursive: true });
  try {
    mkdirSync(dir); // non-recursive -> EEXIST if taken
    writeClaimMeta(dir, checkoutPath);
    return true;
  } catch (err) {
    if (err.code !== "EEXIST") throw err;
    if (isStaleClaim(dir, checkoutPath)) {
      rmSync(dir, { recursive: true, force: true });
      mkdirSync(dir);
      writeClaimMeta(dir, checkoutPath);
      return true;
    }
    return false;
  }
}

export function releaseApply(env, checkoutPath) {
  rmSync(claimPath(env, checkoutPath), { recursive: true, force: true });
}

export function alreadyApplied(env, checkoutPath) {
  return existsSync(claimPath(env, checkoutPath));
}

// Opportunistic GC: drop claims whose worktree no longer exists on disk, so the
// `applied/` directory doesn't grow without bound and a future worktree at a
// reclaimed path starts clean. Pure filesystem, no herdr query -- cheap enough
// to run on each (non-hot-path) hook invocation. Legacy claims with no record
// can't be mapped back to a path, so they're left for isStaleClaim to handle on
// recreate. Returns the number of claims reaped.
export function reapOrphanClaims(env) {
  let entries;
  try {
    entries = readdirSync(appliedDir(env));
  } catch {
    return 0; // nothing claimed yet
  }
  let reaped = 0;
  for (const name of entries) {
    const claim = path.join(appliedDir(env), name);
    const meta = readClaimMeta(claim);
    if (!meta?.path) continue; // legacy or in-progress claim -- leave it
    if (!existsSync(meta.path)) {
      rmSync(claim, { recursive: true, force: true });
      reaped += 1;
    }
  }
  return reaped;
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
