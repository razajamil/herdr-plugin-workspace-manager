import { test } from "node:test";
import assert from "node:assert/strict";
import { parseYaml, YamlError } from "../src/yaml.mjs";

test("parses scalars: strings, bools, null, numbers", () => {
  const doc = parseYaml(
    [
      "a: hello",
      "b: true",
      "c: false",
      "d: null",
      "e: ~",
      "f: 42",
      "g: 0.5",
      "h: -3",
    ].join("\n"),
  );
  assert.deepEqual(doc, {
    a: "hello",
    b: true,
    c: false,
    d: null,
    e: null,
    f: 42,
    g: 0.5,
    h: -3,
  });
});

test("quoted strings keep their literal value", () => {
  const doc = parseYaml(
    ['s: "true"', "t: 'mise run setup'", 'u: "a: b # c"', "v: 'it''s ok'"].join("\n"),
  );
  assert.equal(doc.s, "true"); // quoted -> string, not boolean
  assert.equal(doc.t, "mise run setup");
  assert.equal(doc.u, "a: b # c"); // colon and # inside quotes are literal
  assert.equal(doc.v, "it's ok"); // '' -> '
});

test("strips comments but not inside quotes", () => {
  const doc = parseYaml(
    ["# full line comment", "a: 1 # trailing", 'b: "x # y" # real comment'].join("\n"),
  );
  assert.deepEqual(doc, { a: 1, b: "x # y" });
});

test("nested mappings via indentation", () => {
  const doc = parseYaml(["setup:", "  command: mise run setup", "  blocking: true"].join("\n"));
  assert.deepEqual(doc, { setup: { command: "mise run setup", blocking: true } });
});

test("sequence of scalars", () => {
  const doc = parseYaml(["items:", "  - one", "  - two", "  - 3"].join("\n"));
  assert.deepEqual(doc, { items: ["one", "two", 3] });
});

test("sequence of mappings with continuation lines", () => {
  const doc = parseYaml(
    [
      "tabs:",
      "  - title: main",
      "    panes:",
      "      - title: agent",
      "        command: opencode",
      "        setup: true",
      "      - title: editor",
      "        split: vertical",
      "  - title: dev",
      "    panes:",
      "      - title: server",
    ].join("\n"),
  );
  assert.deepEqual(doc, {
    tabs: [
      {
        title: "main",
        panes: [
          { title: "agent", command: "opencode", setup: true },
          { title: "editor", split: "vertical" },
        ],
      },
      { title: "dev", panes: [{ title: "server" }] },
    ],
  });
});

test("parses the full example shape", () => {
  const doc = parseYaml(
    [
      "layouts:",
      "  - id: reckon-frontend",
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
      "workspaces:",
      "  - path: ~/.herdr/worktrees/reckon-frontend",
      "    defaultLayout: reckon-frontend",
    ].join("\n"),
  );
  assert.equal(doc.layouts.length, 1);
  assert.equal(doc.layouts[0].id, "reckon-frontend");
  assert.equal(doc.layouts[0].setup.blocking, true);
  assert.equal(doc.layouts[0].tabs[0].panes[0].setup, true);
  assert.equal(doc.layouts[0].tabs[0].panes[1].split, "vertical");
  assert.deepEqual(doc.workspaces, [
    { path: "~/.herdr/worktrees/reckon-frontend", defaultLayout: "reckon-frontend" },
  ]);
});

test("empty / comment-only document is null", () => {
  assert.equal(parseYaml(""), null);
  assert.equal(parseYaml("# just a comment\n\n"), null);
});

test("rejects tab indentation", () => {
  assert.throws(() => parseYaml("a:\n\t- x"), YamlError);
});
