#!/usr/bin/env node
// `remove-gone` action (preview). Lists the linked worktrees of the current repo
// whose remote branch was deleted ("gone"). Removes NOTHING — plugin actions run
// headless and can't prompt, so removal lives in the CLI:
// `herdr-workspace-manager remove-gone` (prompts; pass --confirm to skip it).
//
// Worktrees whose branch never had a remote (never pushed/tracked) never appear
// here. Set HERDR_WSM_NO_FETCH=1 to skip the network fetch and use cached refs.

import { collectGoneWorktrees, formatPreview, repoDisplayName } from "../src/remove-gone.mjs";

const env = process.env;
const log = (msg) => process.stderr.write(`[workspace-manager] ${msg}\n`);

async function main() {
  const { repo, candidates } = collectGoneWorktrees({
    env,
    fetch: !env.HERDR_WSM_NO_FETCH,
    logger: log,
  });
  process.stdout.write(formatPreview(repoDisplayName(repo), candidates));
  if (candidates.length) {
    process.stdout.write(
      `\nPreview only — nothing was removed. ` +
        `Run \`herdr-workspace-manager remove-gone\` to remove the eligible ones.\n`,
    );
  }
}

main().catch((err) => {
  log(`error: ${err.message}`);
  process.exit(1);
});
