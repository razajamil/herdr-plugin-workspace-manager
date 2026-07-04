import { test } from "node:test";
import assert from "node:assert/strict";
import { parseYaml } from "../src/yaml.mjs";
import { validateConfig, findLayout } from "../src/config.mjs";
import { buildPlan, splitRatioArg, clampRatio } from "../src/plan.mjs";

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
  "      - title: review",
  "        panes:",
  "          - title: agent",
  "            command: opencode",
  "          - title: editor",
  "            command: nvim",
  "            split: vertical",
].join("\n");

function plan(cwd = "/work") {
  const config = validateConfig(parseYaml(SAMPLE));
  return buildPlan(findLayout(config, "web-app"), { cwd });
}

test("produces the exact depth-first step sequence", () => {
  const { layoutId, steps } = plan("/work");
  assert.equal(layoutId, "web-app");
  assert.deepEqual(steps, [
    // tab 0 "main" reuses the worktree's root tab + root pane
    { kind: "reuse-tab", tab: "t0", title: "main" },
    { kind: "rename-pane", pane: "t0p0", title: "agent" },
    { kind: "run-setup", pane: "t0p0", command: "mise run setup", blocking: true },
    { kind: "run", pane: "t0p0", command: "opencode" },
    { kind: "split", pane: "t0p1", from: "t0p0", direction: "right", ratio: null, size: null, cwd: "/work" },
    { kind: "rename-pane", pane: "t0p1", title: "editor" },
    { kind: "run", pane: "t0p1", command: "nvim" },
    // tab 1 "dev-server"
    { kind: "create-tab", tab: "t1", pane: "t1p0", title: "dev-server", cwd: "/work" },
    { kind: "rename-pane", pane: "t1p0", title: "server" },
    { kind: "split", pane: "t1p1", from: "t1p0", direction: "down", ratio: null, size: null, cwd: "/work" },
    { kind: "rename-pane", pane: "t1p1", title: "review" },
    // tab 2 "review"
    { kind: "create-tab", tab: "t2", pane: "t2p0", title: "review", cwd: "/work" },
    { kind: "rename-pane", pane: "t2p0", title: "agent" },
    { kind: "run", pane: "t2p0", command: "opencode" },
    { kind: "split", pane: "t2p1", from: "t2p0", direction: "right", ratio: null, size: null, cwd: "/work" },
    { kind: "rename-pane", pane: "t2p1", title: "editor" },
    { kind: "run", pane: "t2p1", command: "nvim" },
  ]);
});

test("blocking setup runs before any later tab is spawned", () => {
  const { steps } = plan();
  const setupIdx = steps.findIndex((s) => s.kind === "run-setup");
  const firstCreateTab = steps.findIndex((s) => s.kind === "create-tab");
  assert.ok(setupIdx >= 0 && firstCreateTab >= 0);
  assert.ok(setupIdx < firstCreateTab, "setup must precede the first create-tab");
});

test("setup pane runs setup then its own command", () => {
  const { steps } = plan();
  const setupIdx = steps.findIndex((s) => s.kind === "run-setup");
  const runIdx = steps.findIndex((s) => s.kind === "run" && s.pane === "t0p0");
  assert.ok(setupIdx < runIdx, "setup command precedes the pane command on the setup pane");
});

test("first pane of each tab is never split; later panes split from the previous", () => {
  const { steps } = plan();
  const splits = steps.filter((s) => s.kind === "split");
  assert.deepEqual(
    splits.map((s) => [s.pane, s.from]),
    [
      ["t0p1", "t0p0"],
      ["t1p1", "t1p0"],
      ["t2p1", "t2p0"],
    ],
  );
});

test("single-tab single-pane layout only reuses the root (no splits/tabs)", () => {
  const config = validateConfig(
    parseYaml(
      [
        "layouts:",
        "  - id: solo",
        "    tabs:",
        "      - title: only",
        "        panes:",
        "          - title: shell",
        "            command: htop",
      ].join("\n"),
    ),
  );
  const { steps } = buildPlan(findLayout(config, "solo"), { cwd: null });
  assert.deepEqual(steps, [
    { kind: "reuse-tab", tab: "t0", title: "only" },
    { kind: "rename-pane", pane: "t0p0", title: "shell" },
    { kind: "run", pane: "t0p0", command: "htop" },
  ]);
});

test("split steps carry a normalized pane size", () => {
  const config = validateConfig(
    parseYaml(
      [
        "layouts:",
        "  - id: sized",
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
  const { steps } = buildPlan(findLayout(config, "sized"), { cwd: null });
  const splits = steps.filter((s) => s.kind === "split");
  assert.deepEqual(
    splits.map((s) => s.size),
    [
      { kind: "cells", value: 40 },
      { kind: "percent", value: 30 },
      { kind: "percent", value: 25 },
    ],
  );
});

test("clampRatio keeps ratios inside herdr's open (0,1) interval", () => {
  assert.equal(clampRatio(0.5), 0.5);
  assert.equal(clampRatio(-3), 0.01); // a cell size >= the whole pane
  assert.equal(clampRatio(5), 0.99);
  assert.equal(clampRatio(NaN), null);
});

test("splitRatioArg inverts a pane size into the from-pane's share", () => {
  // Nothing to size -> no --ratio.
  assert.equal(splitRatioArg({}), null);
  // Legacy ratio is the from-pane share already: passed through untouched.
  assert.equal(splitRatioArg({ ratio: 0.3 }), 0.3);
  // A 30%-wide new pane means the from pane keeps 70%.
  assert.equal(splitRatioArg({ size: { kind: "percent", value: 30 } }), 0.7);
  // A fixed 50-cell pane out of 200 -> new pane 25% -> from pane keeps 75%.
  assert.equal(splitRatioArg({ size: { kind: "cells", value: 50 } }, 200), 0.75);
  // A cell size >= the available extent clamps rather than going degenerate.
  assert.equal(splitRatioArg({ size: { kind: "cells", value: 300 } }, 200), 0.01);
  // A cell size with no known extent yields no --ratio (herdr's default split).
  assert.equal(splitRatioArg({ size: { kind: "cells", value: 40 } }, null), null);
});
