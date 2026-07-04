import { test } from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { homedir } from "node:os";
import { parseYaml } from "../src/yaml.mjs";
import {
  validateConfig,
  matchWorkspaceLayout,
  findLayout,
  expandHome,
  globToRegExp,
  ConfigError,
} from "../src/config.mjs";

const SAMPLE = [
  "layouts:",
  "  - id: web-app",
  "    setup:",
  "      command: mise run setup",
  "      blocking: true",
  "    tabs:",
  "      - title: main",
  "        panes:",
  "          - title: agent",
  "            command: opencode",
  "            setup: true",
  "          - title: editor",
  "            command: nvim",
  "            split: vertical",
  "      - title: dev-server",
  "        panes:",
  "          - title: server",
  "          - title: review",
  "            split: horizontal",
  "workspaces:",
  "  - path: ~/.herdr/worktrees/web-app",
  "    defaultLayout: web-app",
].join("\n");

test("normalizes the sample config and maps split aliases", () => {
  const config = validateConfig(parseYaml(SAMPLE));
  const layout = findLayout(config, "web-app");
  assert.ok(layout);
  assert.deepEqual(layout.setup, { command: "mise run setup", blocking: true });
  // vertical -> right, horizontal -> down
  assert.equal(layout.tabs[0].panes[1].split, "right");
  assert.equal(layout.tabs[1].panes[1].split, "down");
  // first panes have no split
  assert.equal(layout.tabs[0].panes[0].split, null);
  assert.equal(layout.tabs[0].panes[0].setup, true);
});

test("accepts literal right/down and validates ratio", () => {
  const config = validateConfig(
    parseYaml(
      [
        "layouts:",
        "  - id: x",
        "    tabs:",
        "      - title: t",
        "        panes:",
        "          - title: a",
        "          - title: b",
        "            split: down",
        "            ratio: 0.3",
      ].join("\n"),
    ),
  );
  assert.equal(config.layouts[0].tabs[0].panes[1].split, "down");
  assert.equal(config.layouts[0].tabs[0].panes[1].ratio, 0.3);
});

test("parses pane size in cells, percent, and fraction forms", () => {
  const config = validateConfig(
    parseYaml(
      [
        "layouts:",
        "  - id: x",
        "    tabs:",
        "      - title: t",
        "        panes:",
        "          - title: a",
        "          - title: cells",
        "            split: vertical",
        "            size: 40",
        "          - title: percent",
        "            split: vertical",
        '            size: "30%"',
        "          - title: fraction",
        "            split: vertical",
        "            size: 0.25",
      ].join("\n"),
    ),
  );
  const panes = config.layouts[0].tabs[0].panes;
  assert.equal(panes[0].size, null);
  assert.deepEqual(panes[1].size, { kind: "cells", value: 40 });
  assert.deepEqual(panes[2].size, { kind: "percent", value: 30 });
  assert.deepEqual(panes[3].size, { kind: "percent", value: 25 });
});

test("rejects setting both ratio and size on a pane", () => {
  const text = [
    "layouts:",
    "  - id: x",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
    "          - title: b",
    "            split: vertical",
    "            ratio: 0.5",
    "            size: 40",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

test("rejects an out-of-range percent and a fractional cell count", () => {
  const sized = (v) =>
    [
      "layouts:",
      "  - id: x",
      "    tabs:",
      "      - title: t",
      "        panes:",
      "          - title: a",
      "          - title: b",
      "            split: vertical",
      `            size: ${v}`,
    ].join("\n");
  assert.throws(() => validateConfig(parseYaml(sized('"150%"'))), ConfigError);
  assert.throws(() => validateConfig(parseYaml(sized('"0%"'))), ConfigError);
  assert.throws(() => validateConfig(parseYaml(sized("40.5"))), ConfigError); // fixed cells must be whole
  assert.throws(() => validateConfig(parseYaml(sized("0"))), ConfigError);
  assert.throws(() => validateConfig(parseYaml(sized('"wide"'))), ConfigError);
});

test("rejects two setup panes", () => {
  const text = [
    "layouts:",
    "  - id: x",
    "    setup:",
    "      command: echo hi",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
    "            setup: true",
    "          - title: b",
    "            setup: true",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

test("rejects setup command with no setup pane", () => {
  const text = [
    "layouts:",
    "  - id: x",
    "    setup:",
    "      command: echo hi",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

test("rejects duplicate layout ids", () => {
  const dup = [
    "layouts:",
    "  - id: dup",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
    "  - id: dup",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(dup)), ConfigError);
});

test("rejects a layout with no tabs", () => {
  const text = ["layouts:", "  - id: empty"].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

test("rejects workspace referencing unknown layout", () => {
  const text = [
    "layouts:",
    "  - id: known",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
    "workspaces:",
    "  - path: ~/x",
    "    defaultLayout: nope",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

test("expandHome expands ~", () => {
  assert.equal(expandHome("~/foo"), path.join(homedir(), "foo"));
  assert.equal(expandHome("/abs"), "/abs");
});

test("matchWorkspaceLayout matches paths under the workspace root", () => {
  const config = validateConfig(parseYaml(SAMPLE));
  const root = path.join(homedir(), ".herdr/worktrees/web-app");
  const match = matchWorkspaceLayout(config, path.join(root, "my-branch"));
  assert.ok(match);
  assert.equal(match.layout.id, "web-app");
  // exact path also matches
  assert.ok(matchWorkspaceLayout(config, root));
  // unrelated path does not
  assert.equal(matchWorkspaceLayout(config, "/tmp/other"), null);
});

test("matchWorkspaceLayout prefers the most specific (longest) path", () => {
  const cfg = validateConfig(
    parseYaml(
      [
        "layouts:",
        "  - id: parent",
        "    tabs:",
        "      - title: t",
        "        panes:",
        "          - title: a",
        "  - id: child",
        "    tabs:",
        "      - title: t",
        "        panes:",
        "          - title: a",
        "workspaces:",
        "  - path: /repos",
        "    defaultLayout: parent",
        "  - path: /repos/special",
        "    defaultLayout: child",
      ].join("\n"),
    ),
  );
  const match = matchWorkspaceLayout(cfg, "/repos/special/branch");
  assert.equal(match.layout.id, "child");
});

const REPO_CFG = [
  "layouts:",
  "  - id: rf",
  "    tabs:",
  "      - title: t",
  "        panes:",
  "          - title: a",
  "workspaces:",
  "  - repo: ~/dev/web-app",
  "    defaultLayout: rf",
].join("\n");

test("matches a worktree by repo_root", () => {
  const cfg = validateConfig(parseYaml(REPO_CFG));
  const match = matchWorkspaceLayout(cfg, {
    checkoutPath: path.join(homedir(), ".herdr/worktrees/web-app/some-branch"),
    repoRoot: path.join(homedir(), "dev/web-app"),
    repoName: "web-app",
  });
  assert.ok(match);
  assert.equal(match.layout.id, "rf");
});

test("matches a worktree by bare repo name", () => {
  const cfg = validateConfig(
    parseYaml(REPO_CFG.replace("~/dev/web-app", "web-app")),
  );
  const match = matchWorkspaceLayout(cfg, {
    checkoutPath: "/anywhere/else",
    repoRoot: "/some/other/path/web-app",
    repoName: "web-app",
  });
  assert.equal(match.layout.id, "rf");
});

test("a non-matching repo returns null", () => {
  const cfg = validateConfig(parseYaml(REPO_CFG));
  const match = matchWorkspaceLayout(cfg, {
    checkoutPath: "/x",
    repoRoot: "/Users/x/dev/other-repo",
    repoName: "other-repo",
  });
  assert.equal(match, null);
});

test("repo match wins over a path match", () => {
  const cfg = validateConfig(
    parseYaml(
      [
        "layouts:",
        "  - id: byrepo",
        "    tabs:",
        "      - title: t",
        "        panes:",
        "          - title: a",
        "  - id: bypath",
        "    tabs:",
        "      - title: t",
        "        panes:",
        "          - title: a",
        "workspaces:",
        "  - path: /wt/web-app",
        "    defaultLayout: bypath",
        "  - repo: /dev/web-app",
        "    defaultLayout: byrepo",
      ].join("\n"),
    ),
  );
  const match = matchWorkspaceLayout(cfg, {
    checkoutPath: "/wt/web-app/branch",
    repoRoot: "/dev/web-app",
    repoName: "web-app",
  });
  assert.equal(match.layout.id, "byrepo");
});

test("rejects a workspace with neither repo nor path", () => {
  const text = [
    "layouts:",
    "  - id: x",
    "    tabs:",
    "      - title: t",
    "        panes:",
    "          - title: a",
    "workspaces:",
    "  - defaultLayout: x",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

// --- layoutMatching: branch-pattern -> layout -----------------------------

// A minimal valid layout block in block YAML (the parser has no flow support).
const layoutY = (id) =>
  [`  - id: ${id}`, "    tabs:", "      - panes:", "          - title: a"].join("\n");

test("globToRegExp matches the whole branch, * spans any chars, ? one", () => {
  const re = globToRegExp("fix/rwr-*");
  assert.ok(re.test("fix/rwr-142-login"));
  assert.ok(re.test("fix/rwr-"));
  assert.ok(!re.test("hotfix/rwr-1"), "not a prefix match");
  assert.ok(!re.test("fix/rwr"), "full-string anchored");
  // regex metacharacters in the glob are matched literally
  assert.ok(globToRegExp("a.b+c").test("a.b+c"));
  assert.ok(!globToRegExp("a.b").test("axb"));
  // ? matches exactly one character
  assert.ok(globToRegExp("v?").test("v2"));
  assert.ok(!globToRegExp("v?").test("v12"));
});

// repo `rf` with three layouts: default, fix, docs.
const MATCH_CFG = [
  "layouts:",
  layoutY("rf"),
  layoutY("rf-fix"),
  layoutY("rf-docs"),
  "workspaces:",
  "  - repo: ~/dev/web-app",
  "    defaultLayout: rf",
  "    layoutMatching:",
  "      - title: Fix",
  "        worktreePattern: fix/rwr-*",
  "        layout: rf-fix",
  "      - title: Docs",
  "        worktreePattern: docs/*",
  "        layout: rf-docs",
].join("\n");

function matchBranch(cfgText, branch) {
  const cfg = validateConfig(parseYaml(cfgText));
  return matchWorkspaceLayout(cfg, {
    checkoutPath: path.join(homedir(), ".herdr/worktrees/web-app/wt"),
    repoRoot: path.join(homedir(), "dev/web-app"),
    repoName: "web-app",
    branch,
  });
}

test("layoutMatching applies the first pattern that matches the branch", () => {
  assert.equal(matchBranch(MATCH_CFG, "fix/rwr-9-login").layout.id, "rf-fix");
  assert.equal(matchBranch(MATCH_CFG, "docs/architecture").layout.id, "rf-docs");
});

test("layoutMatching falls back to defaultLayout when nothing matches", () => {
  assert.equal(matchBranch(MATCH_CFG, "main").layout.id, "rf");
});

test("a worktree with no branch uses defaultLayout", () => {
  assert.equal(matchBranch(MATCH_CFG, null).layout.id, "rf");
});

test("layoutMatching honors user order (first match wins)", () => {
  // Both rules match `feat/x`; the first one listed must win.
  const cfg = [
    "layouts:",
    layoutY("first"),
    layoutY("second"),
    "workspaces:",
    "  - repo: /dev/r",
    "    layoutMatching:",
    "      - worktreePattern: feat/*",
    "        layout: first",
    "      - worktreePattern: feat/x",
    "        layout: second",
  ].join("\n");
  const match = matchWorkspaceLayout(validateConfig(parseYaml(cfg)), {
    repoRoot: "/dev/r",
    repoName: "r",
    branch: "feat/x",
  });
  assert.equal(match.layout.id, "first");
});

test("a workspace with only layoutMatching yields null when nothing matches", () => {
  const cfg = [
    "layouts:",
    layoutY("only"),
    "workspaces:",
    "  - repo: /dev/r",
    "    layoutMatching:",
    "      - worktreePattern: release/*",
    "        layout: only",
  ].join("\n");
  const config = validateConfig(parseYaml(cfg));
  // branch matches -> applies
  assert.equal(
    matchWorkspaceLayout(config, { repoRoot: "/dev/r", repoName: "r", branch: "release/1" }).layout
      .id,
    "only",
  );
  // branch doesn't match and there's no defaultLayout -> nothing applies
  assert.equal(
    matchWorkspaceLayout(config, { repoRoot: "/dev/r", repoName: "r", branch: "main" }),
    null,
  );
});

test("a more specific workspace that yields no layout defers to a less specific default", () => {
  // path /wt/r/special (more specific) only has layoutMatching and won't match
  // `main`, so the broader /wt/r default applies -- mirrors pre-existing
  // skip-the-workspace-without-a-layout behavior.
  const cfg = [
    "layouts:",
    layoutY("broad"),
    layoutY("narrow"),
    "workspaces:",
    "  - path: /wt/r",
    "    defaultLayout: broad",
    "  - path: /wt/r/special",
    "    layoutMatching:",
    "      - worktreePattern: feat/*",
    "        layout: narrow",
  ].join("\n");
  const config = validateConfig(parseYaml(cfg));
  // under the more specific path, but on `main` -> narrow doesn't match -> broad
  assert.equal(
    matchWorkspaceLayout(config, { checkoutPath: "/wt/r/special/x", branch: "main" }).layout.id,
    "broad",
  );
  // on a feat branch -> the more specific workspace's rule wins
  assert.equal(
    matchWorkspaceLayout(config, { checkoutPath: "/wt/r/special/x", branch: "feat/y" }).layout.id,
    "narrow",
  );
});

test("rejects layoutMatching referencing an unknown layout", () => {
  const text = [
    "layouts:",
    layoutY("known"),
    "workspaces:",
    "  - repo: /dev/r",
    "    layoutMatching:",
    "      - worktreePattern: feat/*",
    "        layout: nope",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});

test("rejects a layoutMatching rule missing worktreePattern", () => {
  const text = [
    "layouts:",
    layoutY("known"),
    "workspaces:",
    "  - repo: /dev/r",
    "    layoutMatching:",
    "      - layout: known",
  ].join("\n");
  assert.throws(() => validateConfig(parseYaml(text)), ConfigError);
});
