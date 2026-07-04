#!/usr/bin/env node
// `validate` action: parse + validate the config and print a human-readable
// summary of the resolved layouts and workspace mappings.

import { loadConfig, configCandidates } from "../src/config.mjs";

const env = process.env;

function describeSize(size) {
  if (!size) return null;
  if (size.kind === "cells") return `size:${size.value}c`;
  return `size:${size.value}%`;
}

function describePane(pane) {
  const bits = [pane.title ?? "(untitled)"];
  if (pane.split) bits.push(`split:${pane.split}`);
  const size = describeSize(pane.size);
  if (size) bits.push(size);
  else if (pane.ratio != null) bits.push(`ratio:${pane.ratio}`);
  if (pane.setup) bits.push("setup");
  if (pane.command) bits.push(`$ ${pane.command}`);
  return bits.join("  ");
}

try {
  const { path: configPath, config } = loadConfig(env);
  if (!configPath) {
    process.stdout.write("No config file found. Looked in:\n");
    for (const c of configCandidates(env)) process.stdout.write(`  - ${c}\n`);
    process.exit(0);
  }

  process.stdout.write(`Config: ${configPath}\n\n`);

  process.stdout.write(`Layouts (${config.layouts.length}):\n`);
  for (const layout of config.layouts) {
    process.stdout.write(`  ${layout.id}\n`);
    if (layout.setup) {
      process.stdout.write(
        `    setup: ${layout.setup.command}${layout.setup.blocking ? " (blocking)" : ""}\n`,
      );
    }
    layout.tabs.forEach((tab, ti) => {
      process.stdout.write(`    tab ${ti}: ${tab.title ?? "(untitled)"}\n`);
      tab.panes.forEach((pane, pj) => {
        process.stdout.write(`      pane ${pj}: ${describePane(pane)}\n`);
      });
    });
  }

  process.stdout.write(`\nWorkspaces (${config.workspaces.length}):\n`);
  for (const ws of config.workspaces) {
    const target = [ws.repo && `repo:${ws.repo}`, ws.path && `path:${ws.path}`]
      .filter(Boolean)
      .join("  ");
    process.stdout.write(`  ${target} -> ${ws.defaultLayout ?? "(none)"}\n`);
    for (const rule of ws.layoutMatching) {
      const title = rule.title ? ` (${rule.title})` : "";
      process.stdout.write(`      branch ~ ${rule.worktreePattern} -> ${rule.layout}${title}\n`);
    }
  }
  process.stdout.write("\nConfig is valid.\n");
} catch (err) {
  process.stderr.write(`Config error: ${err.message}\n`);
  process.exit(1);
}
