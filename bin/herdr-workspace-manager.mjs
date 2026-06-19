#!/usr/bin/env node
// herdr-workspace-manager — command-line interface for the Workspace Manager
// plugin. Unlike herdr's `plugin action invoke` (which streams output to the
// plugin log), this prints straight to your terminal and accepts flags.
//
// Usage:
//   herdr-workspace-manager remove-gone [--apply] [--force] [--no-fetch] [--workspace ID]

import { createInterface } from "node:readline";
import {
  collectGoneWorktrees,
  applyRemovals,
  removableCandidates,
  formatPreview,
  formatApplyResult,
  repoDisplayName,
} from "../src/remove-gone.mjs";

const env = process.env;
const log = (msg) => process.stderr.write(`[workspace-manager] ${msg}\n`);

const USAGE = `herdr-workspace-manager — Workspace Manager plugin CLI

Usage:
  herdr-workspace-manager <command> [options]

Commands:
  remove-gone   Remove the current repo's worktrees whose remote branch was
                deleted ("gone"), after a y/n confirmation. Worktrees that never
                pushed/tracked a remote, the repo's main checkout, and the
                workspace you run it from are never removed.

Options (remove-gone):
  --dry-run            Only print the list; remove nothing and don't prompt.
  --confirm            Remove without the interactive confirmation prompt.
  --force              Also remove worktrees with uncommitted changes.
  --no-fetch           Skip the pre-run \`git fetch --prune\` (use cached refs).
  --workspace <id>     Target a specific workspace's repo (default: current pane's).
  -h, --help           Show this help.
`;

function parseRemoveGoneFlags(argv) {
  const flags = { dryRun: false, confirm: false, force: false, noFetch: false, workspace: null, help: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--dry-run") flags.dryRun = true;
    else if (a === "--confirm") flags.confirm = true;
    else if (a === "--force") flags.force = true;
    else if (a === "--no-fetch") flags.noFetch = true;
    else if (a === "--workspace") flags.workspace = argv[++i] ?? null;
    else if (a === "-h" || a === "--help") flags.help = true;
    else throw new Error(`unknown option "${a}" for remove-gone (see --help)`);
  }
  return flags;
}

// Ask a y/n question on the terminal. Reads from stdin so a piped `yes` works
// too; an empty answer or closed stdin (no input) counts as "no".
function confirm(question) {
  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout });
    let answered = false;
    rl.question(question, (answer) => {
      answered = true;
      rl.close();
      resolve(/^y(es)?$/i.test(answer.trim()));
    });
    rl.on("close", () => {
      if (!answered) resolve(false);
    });
  });
}

async function removeGone(argv) {
  const flags = parseRemoveGoneFlags(argv);
  if (flags.help) {
    process.stdout.write(USAGE);
    return;
  }
  const cliEnv = flags.workspace ? { ...env, HERDR_WSM_WORKSPACE: flags.workspace } : env;
  const { repo, candidates } = collectGoneWorktrees({
    env: cliEnv,
    fetch: !(flags.noFetch || env.HERDR_WSM_NO_FETCH),
    logger: log,
  });
  const repoName = repoDisplayName(repo);

  process.stdout.write(formatPreview(repoName, candidates, { force: flags.force }));
  if (candidates.length === 0) return;

  if (flags.dryRun) {
    process.stdout.write(`\nDry run — nothing was removed.\n`);
    return;
  }

  const removable = removableCandidates(candidates, { force: flags.force });
  if (removable.length === 0) {
    process.stdout.write(`\nNothing eligible to remove (see the notes above).\n`);
    return;
  }

  if (!flags.confirm) {
    const ok = await confirm(`\nRemove ${removable.length} worktree(s)? [y/N] `);
    if (!ok) {
      process.stdout.write(`Aborted — nothing was removed.\n`);
      return;
    }
  }

  const { removed, skipped } = applyRemovals({ env: cliEnv, candidates, force: flags.force, logger: log });
  process.stdout.write("\n" + formatApplyResult(repoName, removed, skipped));
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  switch (cmd) {
    case "remove-gone":
      return removeGone(rest);
    case undefined:
    case "help":
    case "-h":
    case "--help":
      process.stdout.write(USAGE);
      return;
    default:
      process.stderr.write(`unknown command "${cmd}"\n\n${USAGE}`);
      process.exit(2);
  }
}

main().catch((err) => {
  log(`error: ${err.message}`);
  process.exit(1);
});
