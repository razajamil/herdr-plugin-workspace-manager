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
