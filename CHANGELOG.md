# Changelog

All notable changes to this project are documented here.

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
