# Changelog

All notable changes to this project are documented here.

## [Unreleased]

- `layouts[].setup.command` now also accepts a list of commands, run
  consecutively: each one only runs if the previous one succeeded, and the
  recorded exit status is the first failing command's. A single string keeps
  working unchanged.

## [0.6.0] - 2026-07-26

Rebuilt on the herdr APIs that have landed since this plugin was first written.
Existing configs keep working unchanged; everything new is additive.

### Layouts are built declaratively, one request per tab

- Each tab is now built with a single `layout.apply` request carrying the whole
  pane tree — splits, labels, cwd, env and commands together — instead of a
  round-trip per `pane split`, `pane rename` and `pane run`. The example config
  went from ~20 sequential herdr invocations to 4. `layout.apply` has no CLI
  wrapper, so the plugin talks to herdr's socket API directly for that one call
  (newline-delimited JSON over `HERDR_SOCKET_PATH`).
- A fixed `size:` in cells is resolved against the tab's cell area, queried once
  per apply, instead of re-measuring the previous pane after every split.
- **Pane commands are launched, not typed.** A pane's `command` is now the
  pane's own process rather than keystrokes sent to its shell, so it doesn't
  appear in scrollback and there's no race with a shell that isn't listening
  yet. It still runs inside your interactive login shell, so `mise`/`asdf`/`nvm`
  shims, aliases and `PATH` from your rc files apply exactly as before.
- New `persist: false` on a pane lets it close when its command exits; the
  default (`true`) keeps today's behaviour of returning to a prompt. The shell
  you're handed back to is interactive but not a *login* shell, so login-only
  profile side effects (starting an ssh-agent, printing a MOTD) don't fire a
  second time per pane; it inherits the environment the login shell exported.

### Agent panes

- New `agent:` field starts a recognized coding agent with `herdr agent start`,
  which returns only once herdr has **detected** the agent and marked it ready
  for input — so a layout knows the agent actually came up instead of assuming
  a typed `claude` worked. Companion fields: `agentName`, `agentArgs`, `prompt`,
  `agentTimeoutMs`, `promptTimeoutMs`.
- An agent that fails to start no longer takes the layout down with it: the
  plugin warns, raises a notification, and continues with the other panes.

### Setup is gated on a real signal

- The setup command's completion and exit status are now recorded by the setup
  script itself and read back from disk, replacing the sentinel that was printed
  into the terminal and matched with `pane wait-output`. That sentinel could be
  missed when it scrolled out of the matched rows or was echoed back by the
  shell before the command ran, and needed careful quoting to work across bash
  and zsh.
- A setup that exceeds `HERDR_WSM_SETUP_TIMEOUT_MS` now warns instead of failing
  the apply — the layout is already built by then.
- An agent on the setup pane always waits for setup to finish, blocking or not.

### Failures are visible

- A blocking setup puts a `setup` token on its pane's sidebar row (`running`,
  then `failed-<code>` / `timed-out`), and setup failures, agent-start failures
  and failed applies raise a herdr notification. Previously all of these only
  reached the plugin log.

### Environment variables

- New `env:` blocks at layout and pane level, passed to the pane process.
  Pane entries win over layout entries.

### Cheaper event handling

- New `[[startup]]` hook does the state maintenance (reaping claims for removed
  worktrees, clearing the focus cache) that used to run on every event
  invocation. It also means a worktree removed while herdr wasn't running is
  noticed before any event consults a stale claim.
- `workspace.focused` — 47 of the last 50 recorded invocations of this plugin —
  now exits after a single `stat`: the decided-cache check moved ahead of config
  loading, and the shim skips its `find`-based rebuild check for that event.
  Measured at ~11.4ms → ~8.4ms per event over 50 runs. Both figures are mostly
  `sh` and process spawn, so this is a modest constant saving, not a step
  change; the point is that the work is now bounded regardless of config size.
- New `HERDR_WSM_FOCUS_HOOK=0` disables the focus trigger outright, for herdr
  builds where TUI worktree creation does emit `worktree.created`. The README
  documents how to check.

### Removed

- `HERDR_WSM_PANE_READY_MS`. The fixed pre-typing delay it configured is gone;
  agents wait for an observed shell prompt (`pane process-info`) instead, and
  commands no longer need one at all.

## [0.5.1] - 2026-07-25

- Fix blocking setup (`setup.blocking: true`), which never worked. It shelled
  out to `herdr wait output`, a command no released herdr has ever had, so every
  blocking setup failed with `unknown command: wait` — and because the error
  propagated out of the plan executor, it took the whole layout apply down with
  it rather than just the wait. It now calls `herdr pane wait-output`, added in
  herdr 0.7.5, which returns the same `matched_line` the exit-code parsing
  already expected. **`min_herdr_version` is now 0.7.5.** The integration test
  drove the same non-existent command, so it could only pass on a machine with
  no herdr server; it now exercises the real one.

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
