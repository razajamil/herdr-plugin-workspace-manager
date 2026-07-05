# Changelog

All notable changes to this project are documented here.

## [0.5.0] - 2026-07-06

- **Rewritten in Rust — the Node.js runtime dependency is gone.** The plugin is
  now a single native binary that a small `sh` shim compiles on first use
  (one-off `cargo build --release`) and re-uses from then on, so the only
  requirement is a Rust toolchain at install time and nothing at runtime.
  Behavior, config format, CLI flags, output, and on-disk state (claim keys,
  `meta.json`) are unchanged — existing worktrees are not re-applied after
  upgrading. `install.sh` now builds the binary up front and no longer needs
  Node either. The full test suite (YAML parser, config validation, planner,
  guards, remove-gone, live integration) was ported to `cargo test`.

## [0.4.0] - 2026-07-04

- Size panes with a new `size` field. A pane may set `size` to a fixed cell
  count (`40`), a percentage (`"30%"`), or a fraction (`0.3`) to size **that**
  pane along the split axis — columns for a vertical split, rows for a horizontal
  one. Percentages/fractions apply directly; a fixed cell size is converted to a
  ratio from the pane's live size at creation (and clamped so both panes stay
  visible). The legacy `ratio` field still works but is the inverse (the fraction
  the previous pane keeps); a pane can't set both, and `size` is preferred.

## [0.3.0] - 2026-06-25

- Pick a workspace's layout by **branch** name. A `workspaces[]` entry may now
  carry `layoutMatching` — an ordered list of `{ title?, worktreePattern, layout }`
  rules. When a new worktree is created, the first rule whose `worktreePattern`
  (a glob: `*` = any characters, `?` = one) matches the worktree's branch wins;
  if none match (or the worktree has no branch) the existing `defaultLayout`
  applies, and with neither, nothing is applied as before. Ordering is yours —
  list the most specific patterns first.

## [0.2.2] - 2026-06-23

- Add an [`install.sh`](./install.sh) helper that puts the
  `herdr-workspace-manager` CLI on your `PATH` in one command — it resolves the
  plugin location via herdr, so it works whether the plugin is installed or
  linked.
- Reorganize the README: a feature-list intro, a "Quick start" section, and a
  single "Configure a layout" section covering layouts, `apply`/`validate`, and
  worktree cleanup.

## [0.2.1] - 2026-06-22

- Re-apply a workspace's layout when its worktree is removed and then recreated
  at the same path. The per-worktree "applied" claim is now validated against
  the live directory's identity (inode + birth time), so a stale claim left by a
  previous worktree — removed by this plugin, another plugin, or you directly —
  no longer suppresses the layout on the new worktree. Restored worktrees are
  still skipped (no clobber), and claims for worktrees that no longer exist are
  reaped opportunistically so the state directory doesn't grow without bound.

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
