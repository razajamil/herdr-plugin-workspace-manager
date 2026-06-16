import { ROOT_TAB, ROOT_PANE } from "./plan.mjs";
import { runHerdrJson, paneIdOf, tabIdOf } from "./herdr.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function intEnv(env, key, fallback) {
  const v = env[key];
  if (v == null || v === "") return fallback;
  const n = Number.parseInt(v, 10);
  return Number.isFinite(n) ? n : fallback;
}

// Build a setup command wrapped with a completion sentinel printed after it.
// The marker (HERDR_WSM_DONE_<token>) is assembled by printf's `%s`, so the
// full literal never appears in the *echoed* command line -- only in the
// command's actual output. That matters because `herdr wait output` also sees
// the echoed input, and would otherwise match immediately (before the command
// finished). Using printf %s instead of shell quote tricks keeps this correct
// across bash and zsh (including zsh's RC_QUOTES, where 'a''b' is a literal
// quote, not concatenation). The token is base36 [0-9a-z], safe to single-quote.
function wrapSetup(command, token) {
  return `( ${command} ) ; printf 'HERDR_WSM_DONE_%s %s\\n' '${token}' "$?"`;
}

// Execute a symbolic plan against the live herdr server.
//
// target: { workspaceId, rootTab, rootPane, cwd }
//   rootTab/rootPane are the worktree's existing first tab + pane (from the
//   worktree.created payload or queried live).
export async function executePlan(plan, target, { env = process.env, logger } = {}) {
  const log = logger ?? (() => {});
  const readyMs = intEnv(env, "HERDR_WSM_PANE_READY_MS", 700);
  const setupTimeoutMs = intEnv(env, "HERDR_WSM_SETUP_TIMEOUT_MS", 600_000);

  const handles = new Map();
  handles.set(ROOT_TAB, target.rootTab);
  handles.set(ROOT_PANE, target.rootPane);

  const readied = new Set();
  const applied = []; // { handle, paneId, tabId?, title?, command? } for the summary

  const resolvePane = (handle) => {
    const id = handles.get(handle);
    if (!id) throw new Error(`unresolved pane handle "${handle}"`);
    return id;
  };

  // Wait for a freshly-spawned pane's shell to be ready before typing into it.
  const ensureReady = async (paneId) => {
    if (readied.has(paneId)) return;
    readied.add(paneId);
    if (readyMs > 0) await sleep(readyMs);
  };

  const cwdArgs = target.cwd ? ["--cwd", target.cwd] : [];

  for (const step of plan.steps) {
    switch (step.kind) {
      case "reuse-tab": {
        const tabId = resolveTab(handles, step.tab);
        if (step.title) {
          runHerdrJson(["tab", "rename", tabId, step.title], { env });
        }
        log(`reuse tab ${tabId}${step.title ? ` as "${step.title}"` : ""}`);
        break;
      }
      case "create-tab": {
        const args = ["tab", "create", "--workspace", target.workspaceId, "--no-focus"];
        if (step.title) args.push("--label", step.title);
        args.push(...(step.cwd ? ["--cwd", step.cwd] : cwdArgs));
        const result = runHerdrJson(args, { env });
        const tabId = tabIdOf(result);
        const paneId = paneIdOf(result);
        if (!tabId || !paneId) {
          throw new Error(`tab create did not return ids: ${JSON.stringify(result)}`);
        }
        handles.set(step.tab, tabId);
        handles.set(step.pane, paneId);
        log(`create tab ${tabId} (pane ${paneId})${step.title ? ` "${step.title}"` : ""}`);
        break;
      }
      case "split": {
        const fromId = resolvePane(step.from);
        const args = ["pane", "split", fromId, "--direction", step.direction, "--no-focus"];
        if (step.ratio != null) args.push("--ratio", String(step.ratio));
        args.push(...(step.cwd ? ["--cwd", step.cwd] : cwdArgs));
        const result = runHerdrJson(args, { env });
        const paneId = paneIdOf(result);
        if (!paneId) {
          throw new Error(`pane split did not return a pane id: ${JSON.stringify(result)}`);
        }
        handles.set(step.pane, paneId);
        log(`split ${fromId} ${step.direction} -> ${paneId}`);
        break;
      }
      case "rename-pane": {
        const paneId = resolvePane(step.pane);
        runHerdrJson(["pane", "rename", paneId, step.title], { env });
        log(`rename pane ${paneId} -> "${step.title}"`);
        break;
      }
      case "run-setup": {
        const paneId = resolvePane(step.pane);
        await ensureReady(paneId);
        const token = `${process.pid.toString(36)}${Date.now().toString(36)}`;
        const marker = `HERDR_WSM_DONE_${token}`;
        runHerdrJson(["pane", "run", paneId, wrapSetup(step.command, token)], { env });
        log(`run setup in ${paneId}: ${step.command}${step.blocking ? " (blocking)" : ""}`);
        if (step.blocking) {
          const waitRes = runHerdrJson(
            ["wait", "output", paneId, "--match", marker, "--timeout", String(setupTimeoutMs)],
            { env },
          );
          const line = waitRes?.matched_line ?? "";
          const code = Number.parseInt(line.trim().split(/\s+/).pop(), 10);
          if (Number.isFinite(code) && code !== 0) {
            log(`WARNING: setup command exited ${code} in ${paneId}`);
          } else {
            log(`setup finished in ${paneId}`);
          }
        }
        break;
      }
      case "run": {
        const paneId = resolvePane(step.pane);
        await ensureReady(paneId);
        runHerdrJson(["pane", "run", paneId, step.command], { env });
        log(`run in ${paneId}: ${step.command}`);
        break;
      }
      default:
        throw new Error(`unknown step kind "${step.kind}"`);
    }
  }

  // Build a handle -> id summary for callers/tests.
  for (const [handle, id] of handles.entries()) {
    if (handle.includes("p")) applied.push({ handle, paneId: id });
  }
  return { layoutId: plan.layoutId, panes: applied, handles: Object.fromEntries(handles) };
}

function resolveTab(handles, handle) {
  const id = handles.get(handle);
  if (!id) throw new Error(`unresolved tab handle "${handle}"`);
  return id;
}
