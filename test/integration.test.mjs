// Live integration test: applies the plugin to a REAL herdr linked worktree
// with REAL panes, driving the real event hook the way the herdr UI does
// (a workspace.focused event carrying only a workspace id), and verifies the
// layout + that the pane commands actually ran.
//
// Fully isolated and self-cleaning:
//   - creates a throwaway git repo + a real `herdr worktree create`
//   - drives bin/event.mjs with a synthetic workspace.focused payload + a temp
//     config/state dir, so the hook must query the workspace for its facts
//   - asserts tab/pane structure, command execution (marker FILES), blocking
//     setup ordering, idempotency (a second event is a no-op)
//   - removes the worktree, closes the source workspace, deletes temp dirs
//
// Skips automatically when no herdr server is running.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync, execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, existsSync, statSync, rmSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const EVENT_SCRIPT = path.join(HERE, "..", "bin", "event.mjs");
const HERDR = process.env.HERDR_BIN_PATH || "herdr";

function herdr(args) {
  const res = spawnSync(HERDR, args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  if (res.status !== 0 && !res.stdout) {
    throw new Error(`herdr ${args.join(" ")} failed: ${res.stderr || res.error?.message}`);
  }
  return res.stdout.trim() ? JSON.parse(res.stdout.trim()).result : null;
}

function serverUp() {
  try {
    const res = spawnSync(HERDR, ["workspace", "list"], { encoding: "utf8" });
    return res.status === 0 && /"workspaces"/.test(res.stdout ?? "");
  } catch {
    return false;
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForFiles(files, deadlineMs) {
  const start = Date.now();
  for (;;) {
    const missing = files.filter((f) => !existsSync(f));
    if (missing.length === 0) return;
    if (Date.now() - start > deadlineMs) {
      throw new Error(`timed out waiting for marker files: ${missing.join(", ")}`);
    }
    await sleep(200);
  }
}

test(
  "applies a layout to a real worktree via a workspace.focused event",
  { skip: serverUp() ? false : "no herdr server running", timeout: 120_000 },
  async () => {
    const tmpRoot = realpathSync(mkdtempSync(path.join(tmpdir(), "wsc-itest-")));
    const repoName = `wsc-it-${path.basename(tmpRoot)}`;
    const repo = path.join(tmpRoot, repoName);
    const markers = path.join(tmpRoot, "markers");
    const stateDir = path.join(tmpRoot, "state");
    const configPath = path.join(tmpRoot, "config.yml");
    mkdirSync(repo);
    mkdirSync(markers);

    // A real git repo so herdr can create a linked worktree from it.
    const git = (...a) => execFileSync("git", ["-C", repo, ...a], { stdio: "ignore" });
    execFileSync("git", ["init", "-q", repo], { stdio: "ignore" });
    git("-c", "user.email=t@t.co", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init");

    const m = (name) => path.join(markers, name);
    const token = `TOK${Date.now().toString(36)}`;
    // Match by repo root; structure mirrors a real layout but with cheap marker
    // commands. setup sleeps 1s so the blocking-ordering check is observable.
    //
    // The `itest` layout is selected by a layoutMatching rule on the branch
    // (`itest`), NOT by defaultLayout (a 1-tab decoy). This proves the hook
    // resolves the worktree's branch live -- if it couldn't, it would fall back
    // to the decoy and the tab/pane assertions below would fail.
    writeFileSync(
      configPath,
      `
layouts:
  - id: itest
    setup:
      command: sleep 1; echo done > ${m("setup.done")}
      blocking: true
    tabs:
      - title: alpha
        panes:
          - title: a0
            setup: true
            command: echo a0 > ${m("a0.cmd")}; printf 'A0OUT_%s\\n' '${token}'
          - title: a1
            split: vertical
            command: echo a1 > ${m("a1.cmd")}
      - title: beta
        panes:
          - title: b0
            command: echo b0 > ${m("b0.cmd")}
          - title: b1
            split: horizontal
            command: echo b1 > ${m("b1.cmd")}
  - id: itest-decoy
    tabs:
      - title: decoy
        panes:
          - title: only
workspaces:
  - repo: ${repo}
    defaultLayout: itest-decoy
    layoutMatching:
      - title: branch match
        worktreePattern: itest
        layout: itest
`,
    );

    let workspaceId = null;
    let sourceWorkspaceId = null;
    let worktreeParentDir = null;
    try {
      // 1. Create a real linked worktree (with --no-focus, so the real installed
      //    plugin doesn't act — and it wouldn't match this temp repo anyway).
      const created = herdr([
        "worktree",
        "create",
        "--cwd",
        repo,
        "--branch",
        "itest",
        "--no-focus",
        "--json",
      ]);
      workspaceId = created.worktree.open_workspace_id;
      worktreeParentDir = path.dirname(created.worktree.path); // ~/.herdr/worktrees/<repo>
      // herdr also opens the source repo as a workspace; find it for cleanup.
      const all = herdr(["workspace", "list"]).workspaces;
      sourceWorkspaceId =
        all.find((w) => w.worktree?.checkout_path === repo && w.workspace_id !== workspaceId)
          ?.workspace_id ?? null;

      // 2. Drive the hook the way the UI does: a workspace.focused event that
      //    carries only the workspace id. The hook must query the workspace.
      const childEnv = { ...process.env };
      delete childEnv.HERDR_PANE_ID;
      delete childEnv.HERDR_TAB_ID;
      delete childEnv.HERDR_WORKSPACE_ID;
      const runEvent = (eventName) =>
        spawnSync("node", [EVENT_SCRIPT], {
          encoding: "utf8",
          maxBuffer: 16 * 1024 * 1024,
          env: {
            ...childEnv,
            HERDR_WSM_CONFIG: configPath,
            HERDR_PLUGIN_STATE_DIR: stateDir,
            HERDR_PLUGIN_EVENT: eventName,
            HERDR_PLUGIN_EVENT_JSON: JSON.stringify({
              event: eventName.replace(".", "_"),
              data: { type: eventName.replace(".", "_"), workspace_id: workspaceId },
            }),
            HERDR_WSM_SETUP_TIMEOUT_MS: "20000",
          },
        });

      const run = runEvent("workspace.focused");
      assert.equal(run.status, 0, `event hook failed:\n${run.stderr}`);
      const summary = JSON.parse(run.stdout.trim().split("\n").pop());
      // The branch-matched layout won over the decoy defaultLayout -> the hook
      // resolved the worktree's branch (`itest`) from the live server.
      assert.equal(summary.layoutId, "itest");

      // 3. Structure: two tabs (alpha, beta), 2 panes each.
      const tabs = herdr(["tab", "list", "--workspace", workspaceId]).tabs;
      assert.deepEqual(tabs.map((t) => t.label).sort(), ["alpha", "beta"]);
      const panes = herdr(["pane", "list", "--workspace", workspaceId]).panes;
      assert.equal(panes.length, 4, "expected 4 panes total");
      for (const tab of tabs) {
        assert.equal(
          panes.filter((p) => p.tab_id === tab.tab_id).length,
          2,
          `tab ${tab.label} should have 2 panes`,
        );
      }

      // 4. Idempotency: a second event (e.g. the workspace.created the CLI also
      //    fires) must be a no-op via the claim, not a doubled layout.
      const dup = runEvent("workspace.created");
      assert.equal(dup.status, 0, `dedupe run failed:\n${dup.stderr}`);
      // No re-apply: the claim (race) or the freshness guard (sequential) skips it.
      assert.match(dup.stderr, /already applied|not a fresh/, "duplicate event should be a no-op");
      assert.equal(
        herdr(["pane", "list", "--workspace", workspaceId]).panes.length,
        4,
        "duplicate event must not add panes",
      );

      // 5. Command execution: each pane command wrote its marker file.
      const files = ["setup.done", "a0.cmd", "a1.cmd", "b0.cmd", "b1.cmd"].map(m);
      await waitForFiles(files, 20_000);

      // 6. Blocking setup finished (1s sleep) before the later panes were built.
      assert.ok(
        statSync(m("b1.cmd")).mtimeMs >= statSync(m("setup.done")).mtimeMs,
        "blocking setup should complete before later panes",
      );

      // 7. Terminal-level proof: the setup pane's command produced output
      //    (A0OUT_<token> via printf %s — present in output, not the echoed input).
      const w = spawnSync(
        HERDR,
        ["wait", "output", summary.handles.t0p0, "--match", `A0OUT_${token}`, "--timeout", "10000"],
        { encoding: "utf8" },
      );
      assert.match(w.stdout ?? "", /output_matched|matched_line/, `setup pane should print A0OUT_${token}`);
    } finally {
      if (workspaceId) {
        try {
          herdr(["worktree", "remove", "--workspace", workspaceId, "--force"]);
        } catch {
          /* ignore */
        }
      }
      if (sourceWorkspaceId) {
        try {
          herdr(["workspace", "close", sourceWorkspaceId]);
        } catch {
          /* ignore */
        }
      }
      rmSync(tmpRoot, { recursive: true, force: true });
      // Remove the now-empty ~/.herdr/worktrees/<repo> parent dir herdr created.
      if (worktreeParentDir) rmSync(worktreeParentDir, { recursive: true, force: true });
    }
  },
);
