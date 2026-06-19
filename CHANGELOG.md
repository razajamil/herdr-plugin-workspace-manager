# Changelog

All notable changes to this project are documented here.

## [0.2.0] - 2026-06-19

- Clean up the current repo's linked worktrees whose remote branch was deleted
  ("gone"). Preview lists candidates by workspace name; applying removes them,
  skipping the invoking workspace and (unless forced) any worktree with
  uncommitted changes. Branches that never pushed/tracked a remote and the
  repo's main checkout are never touched. A `git fetch --prune` runs first
  (skip with `--no-fetch` / `HERDR_WSM_NO_FETCH`).
  - New `herdr-workspace-manager` CLI (in `bin/`): `remove-gone` lists the gone
    worktrees and prompts `[y/N]` before removing. Flags: `--dry-run` (list
    only, no prompt), `--confirm` (remove without prompting), `--force` (also
    remove dirty), `--no-fetch`, `--workspace ID`. Prints to your terminal.
  - Preview also exposed as the `remove-gone` plugin action for the TUI
    (headless, no prompt, removes nothing).

## [0.1.0] - 2026-06-16

Initial release.

- Declarative tab/pane layouts defined in YAML.
- Per-workspace default layouts, matched by repo (`repo_root`/`repo_name`) or
  worktree path prefix.
- Automatic layout application on new worktrees, for both creation paths:
  - `herdr worktree create` (CLI) — via `worktree.created` / `workspace.created`.
  - the herdr TUI "new worktree" command — via `workspace.focused`.
- Optional per-layout `setup` command with `blocking` support.
- Per-pane startup commands and split directions (`vertical`/`horizontal`).
- Idempotent + restart-safe (atomic claim) and fresh-workspace-only guards.
- `apply` and `validate` actions; configurable readiness/timeout env vars.
- Zero runtime dependencies (bundled minimal YAML parser); no build step.
