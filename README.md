<div align="center">

# 🪟 herdr-plugin-workspace-manager

**Every new worktree, opened fully arranged.**

Define your tabs, panes, and startup commands once.
Every worktree you create opens straight into them —
and the merged ones clean up in one command.

[Install](#install) · [Quick start](#quick-start) ·
[Layouts by branch](#pick-a-layout-by-branch) ·
[Cleanup](#clean-up-merged-worktrees) · [Highlights](#highlights) · [Reference](#reference)

</div>

![A new worktree opening into its declarative tab/pane layout (2× speed)](docs/demo.gif)

herdr-plugin-workspace-manager is a [herdr](https://herdr.dev) plugin that arranges
every new worktree into a declarative layout — and cleans up the ones you're done with:

- **Declarative layouts.** Tabs, panes, splits, environment, and per-pane
  startup commands, defined once in YAML.
- **Applied automatically, per repo.** Point a repo at a layout and every new
  worktree — created from the CLI _or_ the herdr TUI — opens straight into it,
  fully arranged. No rebuilding your working view by hand each time.
- **Agents that are actually running.** Declare `agent: claude` and the plugin
  waits until herdr reports it ready for input — optionally handing it the task
  as a `prompt`.
- **Picked by branch.** Route `fix/*` branches to a trimmed layout and `docs/*`
  to another; the first matching rule wins.
- **Zero dependencies except Rust.** No Node, no npm — the plugin is a single
  native binary it compiles itself on first use, then runs with no runtime deps.
- **Cleanup after the merge.** `herdr-workspace-manager remove-gone` removes the
  worktrees whose upstream branch is gone, leaving the main checkout and
  anything in progress untouched.

## Install

Requires **herdr ≥ 0.7.5**, **Linux or macOS**, and a **Rust toolchain**
(`cargo`, [rustup.rs](https://rustup.rs)) on your `PATH`. The plugin compiles
itself on first use — a one-off `cargo build --release` — and runs as a single
native binary from then on, with no runtime dependencies at all.

```sh
herdr plugin install razajamil/herdr-plugin-workspace-manager
```

<details>
<summary>Local development / pinning / non-interactive</summary>

```sh
# live edits from a clone (the shim rebuilds automatically when src changes)
git clone https://github.com/razajamil/herdr-plugin-workspace-manager.git
herdr plugin link ./herdr-plugin-workspace-manager

# pin a release with --ref, or install non-interactively with --yes
herdr plugin install razajamil/herdr-plugin-workspace-manager --ref <tag> --yes
```

</details>

## Quick start

### 1. Drop a `config.yml` in the plugin's config directory

```sh
herdr plugin config-dir herdr-plugin-workspace-manager
# -> ~/.config/herdr/plugins/config/herdr-plugin-workspace-manager
```

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/schema.json
layouts:
  - id: web-app
    setup:
      command: make setup   # optional one-off, run before the rest of the layout
      blocking: true         # if true, no other panes spawn until it finishes
    tabs:
      - title: code
        panes:
          - title: agent
            agent: claude      # started with `herdr agent start`, waits until it's ready
            setup: true        # runs setup.command first, then starts the agent
          - title: editor
            command: nvim      # optional command to run in the pane
            split: vertical    # placed beside the agent pane
      - title: server
        panes:
          - title: dev
            command: make dev
            env:
              PORT: 3000       # environment for this pane's process
          - title: shell
            split: horizontal  # stacked below the dev server
      - title: git
        panes:
          - title: lazygit
            command: lazygit

workspaces:
  - repo: ~/code/web-app       # any linked worktree of this repo gets the layout
    defaultLayout: web-app
```

The first line is a modeline: schema-aware editors autocomplete and validate the
file as you type (see [Editor autocomplete](#editor-autocomplete)). A fully
annotated template lives in [`config.example.yml`](./config.example.yml).

### 2. Create a worktree

Create a worktree for `~/code/web-app` — from the TUI or `herdr worktree create` —
and it opens with the `code` / `server` / `git` tabs already laid out, every pane
running its command.

## Pick a layout by branch

One layout rarely fits every kind of branch. `layoutMatching` picks the layout
from the new worktree's **branch** name — full working view for features, a
trimmed one for quick fixes:

```yaml
layouts:
  # …the web-app layout from the quick start, plus a trimmed variant:
  - id: web-app-hotfix
    tabs:
      - title: code
        panes:
          - title: agent
            command: claude
          - title: editor
            command: nvim
            split: vertical

workspaces:
  - repo: ~/code/web-app
    defaultLayout: web-app          # used when no rule matches
    layoutMatching:                 # first match wins — ordering is yours
      - title: Hotfix branches
        worktreePattern: fix/rwr-*  # glob over the whole branch: * = any chars, ? = one
        layout: web-app-hotfix
```

A worktree on a `fix/rwr-*` branch opens the trimmed layout; anything else gets
the `defaultLayout`. Details in [Per-branch layouts](#per-branch-layouts).

## Clean up merged worktrees

After a PR merges and its branch is deleted upstream, the local worktree
lingers. The bundled **`herdr-workspace-manager`** CLI removes the current
repo's linked worktrees whose upstream branch is gone:

```sh
herdr-workspace-manager remove-gone            # list them, then "Remove N worktree(s)? [y/N]"
herdr-workspace-manager remove-gone --dry-run  # just print the list, remove nothing
```

Safe by default: only branches that **had an upstream that was then deleted**
are candidates, and the main checkout, the workspace you run it from, and
anything with uncommitted changes are always skipped (see
[the `remove-gone` CLI](#the-remove-gone-cli)).

Installing the plugin doesn't put the CLI on your `PATH` — one command does
(it symlinks into `~/.local/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/install.sh | sh
```

Layouts work without the CLI — it's only needed for cleanup.

## Highlights

- **No runtime dependencies.** A single native Rust binary (including a small
  YAML-subset parser); the shim entrypoint builds it on first use, so a
  `herdr plugin link`-ed clone works with nothing but a Rust toolchain.
- **One request per tab.** Each tab is built from a declarative tree in a single
  `layout.apply` call — structure, labels, cwd, env and commands together —
  instead of a round-trip per split, rename and command.
- **Real agents, not typed commands.** `agent: claude` starts a recognized agent
  with `herdr agent start`, which returns only once herdr has *detected* it and
  marked it ready for input. Add a `prompt:` to hand it the task.
- **CLI and TUI worktrees both covered.** herdr emits different events for the
  two creation paths; the plugin subscribes to all of them and dedupes with an
  atomic claim, so a layout is applied exactly once per worktree.
- **Fresh worktrees only.** A layout only builds into a fresh (1-tab/1-pane)
  workspace and never touches the repo's main checkout — refocusing or restoring
  existing worktrees is a no-op.
- **Pane sizing.** Size any pane along its split axis with fixed cells
  (`size: 40`), a percentage (`"30%"`), or a fraction (`0.3`); splits default to
  50/50. See [Pane sizing](#pane-sizing).
- **Editor autocomplete.** The repo ships a JSON Schema — one modeline gets you
  completion, hover docs, and validation in any YAML-language-server editor.
- **On demand too.** `apply` and `validate` plugin actions (plus a
  `prefix+shift+l` keybinding) apply a layout or check your config any time.
- **Failures are visible.** A slow or failing setup shows on the pane's sidebar
  row, and failures raise a herdr notification instead of only reaching the
  plugin log.
- **Just herdr's own APIs under the hood.** The result is ordinary tabs and
  panes — nothing proprietary to unwind.

---

# Reference

## Configuration

The plugin reads `config.yml` from the directory printed by
`herdr plugin config-dir herdr-plugin-workspace-manager`; a fallback path
`~/.herdr/plugins/herdr-plugin-workspace-manager/config.yml` also works, and
`HERDR_WSM_CONFIG` overrides the lookup entirely. A fully annotated template
lives in [`config.example.yml`](./config.example.yml).

| Field | Where | Meaning |
| --- | --- | --- |
| `layouts[].id` | layout | Unique id, referenced by `defaultLayout`. |
| `layouts[].setup.command` | layout | Optional command(s) run on the `setup: true` pane — a single string, or a list run consecutively. |
| `layouts[].setup.blocking` | layout | If `true`, no further tabs spawn until setup finishes. |
| `layouts[].env` | layout | Environment variables for every pane in the layout. |
| `tabs[].title` | tab | Tab label. The first tab replaces the worktree's existing tab. |
| `panes[].title` | pane | Pane label. |
| `panes[].command` | pane | Optional command to run in the pane, in your interactive login shell. Mutually exclusive with `agent`. |
| `panes[].persist` | pane | Default `true` — stay at a shell prompt after `command` exits. `false` lets the pane close with it. |
| `panes[].env` | pane | Environment variables for this pane, merged over `layouts[].env`. |
| `panes[].agent` | pane | Start a recognized coding agent here (`claude`, `codex`, `opencode`, …). See [Agent panes](#agent-panes). |
| `panes[].agentName` | pane | Stable alias for that agent (`[a-z][a-z0-9_-]`, unique among live agents). Defaults to one derived from the kind + workspace. |
| `panes[].agentArgs` | pane | Arguments passed through to the agent's own executable. |
| `panes[].prompt` | pane | Prompt submitted once the agent is ready. |
| `panes[].agentTimeoutMs` | pane | How long to wait for the agent to become ready (herdr allows >3000, ≤300000). |
| `panes[].promptTimeoutMs` | pane | Setting it makes the apply wait for the agent to settle after `prompt`. Omit to submit and move on. |
| `panes[].setup` | pane | Marks the single pane that runs `setup.command` (at most one per layout). |
| `panes[].split` | pane | For panes after the first: `vertical` \| `horizontal` \| `right` \| `down`. |
| `panes[].size` | pane | Optional size of **this** pane along the split axis: fixed cells (`40`), a fraction (`0.3`), or a percentage (`"30%"`). See [Pane sizing](#pane-sizing). |
| `panes[].ratio` | pane | Legacy split ratio `(0, 1)` — the fraction the **previous** pane keeps. Prefer `size` (mutually exclusive with it). |
| `workspaces[].repo` | workspace | **Recommended.** Repo root (`~` expanded) or bare repo name. Matches any *linked worktree* of that repo; the main checkout is never touched. |
| `workspaces[].path` | workspace | Alternative: prefix-match the worktree's checkout path. |
| `workspaces[].defaultLayout` | workspace | Layout id applied to a matching new worktree when no `layoutMatching` rule matches its branch. |
| `workspaces[].layoutMatching[]` | workspace | Optional ordered rules that pick a layout by the new worktree's **branch** name. First match wins. |
| `…layoutMatching[].worktreePattern` | rule | Glob matched against the whole branch name (`*` = any chars, `?` = one). e.g. `fix/rwr-*`. |
| `…layoutMatching[].layout` | rule | Layout id applied when this pattern matches. |
| `…layoutMatching[].title` | rule | Optional label (documentation only). |

Each `workspaces[]` entry needs `repo` and/or `path`; a `repo` match wins over a
`path` match.

### Per-branch layouts

Within a matched workspace, `layoutMatching` chooses a layout from the new
worktree's **branch** name. Rules are tried in the order you write them and the
first whose `worktreePattern` matches wins; if none match (or the worktree has
no branch, e.g. a detached HEAD) the `defaultLayout` applies. With neither a
matching rule nor a `defaultLayout`, nothing is applied. `worktreePattern` is a
glob over the entire branch name: `*` matches any run of characters (including
`/`) and `?` matches a single one, so `fix/rwr-*` matches `fix/rwr-142-login`
but not `hotfix/rwr-1`.

### Split direction

herdr splits are `right` or `down`. This plugin maps `vertical → right` (side
by side) and `horizontal → down` (stacked); `right`/`down` are also accepted.
The first pane of a tab is never split; each later pane splits from the
previous one.

### Pane sizing

By default each split is even (50/50). Give a pane a `size` to size **that**
pane along the split axis — columns for a `vertical`/`right` split, rows for a
`horizontal`/`down` split. `size` takes three forms:

| `size` value | Meaning |
| --- | --- |
| `40` (whole number) | **Fixed** — 40 cells (columns for a vertical split, rows for a horizontal one). |
| `"30%"` (string) | **Percentage** — 30% of the space being split. |
| `0.3` (0 < n < 1) | **Fraction** — the same as `"30%"`. |

```yaml
panes:
  - title: editor
  - title: sidebar
    split: vertical
    size: 40        # a 40-column sidebar
  - title: terminal
    split: horizontal
    size: "25%"     # bottom terminal takes 25% of the height
```

A percentage/fraction is applied directly. A **fixed** cell size is converted to
a ratio against the tab's cell area, which the plugin queries once per apply —
so it lands on ~N cells when the layout is built; if you later resize the
window, herdr rebalances the panes proportionally (the plugin doesn't manage
them afterwards). A fixed size larger than the available space is clamped so
both panes stay visible.

`size` refers to the pane you put it on. The older `ratio` field is the
opposite — it's herdr's raw ratio, the fraction the **previous** pane keeps —
so `ratio: 0.3` makes the previous pane 30% and *this* pane 70%. `ratio` still
works but a pane can't set both; prefer `size`.

### Commands and the shell

A pane's `command` runs inside **your own interactive login shell**, so
everything your `.zshrc` / `.bash_profile` sets up — `mise`/`asdf`/`nvm` shims,
aliases, `PATH` edits — applies exactly as it would if you'd typed the command
into the pane yourself. When the command exits you're left at a prompt; set
`persist: false` when a pane should close with its command instead:

```yaml
panes:
  - title: dev
    command: npm run dev      # exits -> you're back at a prompt
  - title: lazygit
    command: lazygit
    persist: false            # exits -> the pane closes
```

Unlike earlier versions, the command is not typed into the pane, so it doesn't
appear in scrollback and there's no keystroke race on a freshly created pane.
The prompt you're handed back to is interactive but not a *login* shell — it
inherits the environment the login shell already exported, so login-only profile
side effects (starting an ssh-agent, printing a MOTD) don't run twice per pane.

### Environment variables in a layout

`env` blocks attach environment variables to the pane's process. Put shared
values on the layout and per-pane overrides on the pane; pane entries win.
Values are stringified, so numbers and booleans need no quoting:

```yaml
layouts:
  - id: web-app
    env:
      NODE_ENV: development
    tabs:
      - title: server
        panes:
          - title: dev
            command: npm run dev
            env:
              PORT: 3000
```

### Agent panes

Instead of a `command`, a pane can declare an `agent`. The plugin starts it with
`herdr agent start`, which returns only once herdr has **detected** the agent in
that pane and marked it ready for interactive input — so a layout knows the
agent actually came up rather than assuming it did:

```yaml
panes:
  - title: agent
    agent: claude
    agentName: web-main                 # optional stable alias
    agentArgs:                          # optional, passed to the agent itself
      - --permission-mode
      - plan
    prompt: Read TASK.md and start.     # optional, submitted once it's ready
```

Supported kinds are the ones `herdr agent start --kind` accepts: `pi`, `claude`,
`codex`, `gemini`, `cursor`, `devin`, `agy`, `cline`, `omp`, `mastracode`,
`opencode`, `copilot`, `kimi`, `kiro`, `droid`, `amp`, `grok`, `hermes`, `kilo`,
`qodercli`, `maki`. An unknown kind is rejected by `validate`, not halfway
through building the layout.

`agentName` gives the agent a stable alias you can use afterwards
(`herdr agent prompt web-main "…"`). Names must be unique among **live** agents,
so reusing one across two worktrees open at once will fail; leave it out and the
plugin derives a unique name from the kind and workspace.

A `prompt` is submitted and left running. Add `promptTimeoutMs` when you want
the apply to wait for the agent to settle (idle or done) before finishing.

If an agent fails to start, the layout it was part of stays built — the plugin
warns, raises a notification, and carries on with the remaining panes.

### Setup pane

At most one pane per layout may set `setup: true`. The setup command runs there
first; with `blocking: true` the plugin waits for it to finish before building
any later tab. After setup, that pane still runs its own `command` (or starts
its `agent` — an agent on the setup pane always waits for setup, blocking or
not). Put the setup pane first so nothing spawns ahead of it.

`setup.command` also accepts a list, run consecutively:

```yaml
setup:
  command:
    - mise install
    - npm install
  blocking: true
```

Each entry only runs if the previous one succeeded, so the recorded exit
status is the first failing command's, not a later step run against a
broken state.

While a blocking setup runs, the pane's sidebar row carries a `setup` token
(`running`, then `failed-<code>` or `timed-out` if it doesn't succeed), and a
failure also raises a herdr notification. The exit status is captured by the
setup script itself rather than scraped from terminal output, so it can't be
missed because the marker scrolled away.

### Editor autocomplete

The repo ships a JSON Schema ([`schema.json`](./schema.json)). Editors backed
by the YAML Language Server — VS Code (Red Hat YAML extension), Neovim
(`yamlls`), Helix, etc. — give you completion, hover docs, and validation when
the file starts with this modeline (the bundled `config.example.yml` already
includes it):

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/schema.json
```

Or map it without editing the file, e.g. in VS Code `settings.json`:

```json
"yaml.schemas": {
  "https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/schema.json": "**/herdr-plugin-workspace-manager/config.yml"
}
```

## Actions & keybinding

```sh
# Apply a layout to the current workspace (or pass a layout id):
herdr plugin action invoke apply --plugin herdr-plugin-workspace-manager

# Validate the config and print the resolved layouts/workspaces:
herdr plugin action invoke validate --plugin herdr-plugin-workspace-manager
```

A keybinding (`prefix+shift+l` → apply) is declared in the manifest.

> **`apply` rebuilds the first tab.** The layout's first tab *replaces* the
> workspace's existing first tab (herdr creates the replacement, then closes the
> old one), so running `apply` by hand against a workspace with work in that tab
> will take its panes and their processes with it. On a fresh worktree — the
> automatic path — that tab is empty, so nothing is lost. Later tabs are
> appended, not replaced.

A `[[startup]]` hook also runs once per herdr server start; it does state
maintenance only and never builds a layout.

## The `remove-gone` CLI

`herdr-workspace-manager` is a CLI bundled with the plugin, needed only for
cleanup — layouts work without it. It prints straight to your terminal, lists
the gone worktrees by workspace name, then asks for confirmation before
removing them:

```sh
# List the gone worktrees, then prompt "Remove N worktree(s)? [y/N]":
herdr-workspace-manager remove-gone

# Just print the list; remove nothing, no prompt:
herdr-workspace-manager remove-gone --dry-run

# Skip the prompt (for scripts); add --force to also remove dirty worktrees:
herdr-workspace-manager remove-gone --confirm --force
```

Pass `--workspace <id>` to target a repo other than the current pane's.

**Semantics.** A branch is only ever a candidate when it **had an upstream that
was then deleted** ("gone", in git's terms). Worktrees on branches that never
pushed/tracked a remote are left alone, as is the repo's main checkout. Removal
additionally **skips** (and reports) the workspace you run it from and — unless
`--force` — any worktree with uncommitted changes, so nothing in-progress is
destroyed silently. A clean worktree's committed history survives removal (it
stays in the repo's object store/reflog). A `git fetch --prune` runs first so
deleted branches are detected accurately; pass `--no-fetch` (or set
`HERDR_WSM_NO_FETCH=1`) to use cached refs.

**Installing it.** Installing the plugin doesn't put the CLI on your `PATH`;
the bundled [`install.sh`](./install.sh) symlinks it into `~/.local/bin`:

```sh
# From a clone of this repo:
./install.sh

# Or, if you installed the plugin and have no clone, fetch and run it:
curl -fsSL https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/install.sh | sh
```

Link it elsewhere by passing a directory (`./install.sh ~/bin`); the installer
works whether the plugin is installed or linked, and warns if the target isn't
on your `PATH`. Then run `herdr-workspace-manager --help`.

> **Upgrading from a Node install (pre-0.5.0).** The CLI entrypoint moved from
> `bin/herdr-workspace-manager.mjs` to a `bin/herdr-workspace-manager` shim, so
> a symlink created by an older `install.sh` now dangles (you'll see "command
> not found"). Re-run `./install.sh` once to repoint it, then `hash -r` (or open
> a new shell) so your session forgets the stale path.

**As a plugin action.** The preview is also exposed as a plugin action for the
TUI action menu / keybindings. It runs headless (no prompt) and removes
nothing — `action invoke` streams output to the plugin log rather than your
terminal, so the CLI is the way to actually remove worktrees:

```sh
herdr plugin action invoke remove-gone --plugin herdr-plugin-workspace-manager  # preview only
```

## Environment variables

| Var | Default | Purpose |
| --- | --- | --- |
| `HERDR_WSM_CONFIG` | — | Absolute path to a config file (overrides the default lookup). |
| `HERDR_WSM_SETUP_TIMEOUT_MS` | `600000` | Max wait for a blocking setup command to finish. Exceeding it warns; it doesn't fail the layout. |
| `HERDR_WSM_APPLY_TIMEOUT_MS` | `60000` | Max wait for a single `layout.apply` request (one per tab). |
| `HERDR_WSM_AGENT_TIMEOUT_MS` | `60000` | Default wait for an agent to become ready; `agentTimeoutMs` overrides per pane. |
| `HERDR_WSM_SHELL_READY_MS` | `15000` | Max wait for a pane to be back at its shell prompt before starting an agent in it. |
| `HERDR_WSM_FOCUS_HOOK` | — | Set to `0` to disable the `workspace.focused` trigger entirely. See [below](#is-the-workspacefocused-hook-still-needed). |
| `HERDR_WSM_NO_FETCH` | — | If set, `remove-gone` skips the `git fetch --prune` and uses cached remote-tracking refs. |

`apply` normally targets the workspace you invoke it from. These override that,
which is useful for scripting it against a specific workspace:

| Var | Falls back to | Purpose |
| --- | --- | --- |
| `HERDR_WSM_WORKSPACE` | `HERDR_WORKSPACE_ID`, then the invocation context | Workspace to build into. |
| `HERDR_WSM_TAB` | `HERDR_TAB_ID`, then the workspace's first tab | Tab to replace with the layout's first tab. |
| `HERDR_WSM_PANE` | `HERDR_PANE_ID`, then that tab's first pane | Root pane, used to measure the tab area. |
| `HERDR_WSM_CWD` | the workspace's checkout path | Working directory for the layout's panes. |
| `HERDR_WSM_LAYOUT` | the workspace's default layout | Layout id to apply. An `apply <id>` argument wins over it. |

```sh
# Build the `web-app` layout into a specific workspace:
HERDR_WSM_WORKSPACE=w7 herdr-workspace-manager apply web-app
```

`HERDR_WSM_PANE_READY_MS` is gone. It existed to delay typing into a pane whose
shell might not be listening yet; commands are now launched as the pane's
process, and agents wait for an observed shell prompt instead of a fixed guess.

## How it works

The plugin subscribes to **three** events, because herdr creates worktrees two
different ways and they emit different events to plugins:

| You create a worktree via… | herdr emits to plugins |
| --- | --- |
| `herdr worktree create` (CLI / socket API) | `worktree.created` **and** `workspace.created` |
| the **TUI** right-click "new worktree" (in-app) | **only** `workspace.focused` (it focuses the new worktree on creation) |

So the hook listens for `worktree.created`, `workspace.created`, **and**
`workspace.focused`. On any of them it:

1. Loads the config and resolves the focused/created workspace id.
2. **Queries** that workspace for its worktree facts (the `workspace.focused`
   payload carries only an id).
3. Skips unless it's a **linked worktree** (never the repo's main checkout).
4. Matches against `workspaces[]` by **repo** (`repo_root`/`repo_name`) or path,
   then picks the layout: the first `layoutMatching` rule whose glob matches the
   worktree's **branch**, else the workspace's `defaultLayout`.
5. Only builds into a **fresh** (1-tab/1-pane) workspace.
6. **Dedupes** with an atomic claim (the events can fire together; also skips
   restored worktrees after a restart) — applied exactly once per worktree.
7. Builds each tab with **one** `layout.apply` request, then starts any agents.

```
 new worktree (CLI or TUI) ──► event hook (herdr-workspace-manager event)
        ▼
   load config ─► match workspaces[] (repo or path) ─► default layout
        │                                                   │
        └── no match / not fresh / already done ─► no-op     ▼
                                      per tab: one layout.apply request
                                        tab 0  → replaces the worktree's root tab
                                        tab N  → appended to the workspace
                                        tree   → splits, labels, cwd, env, commands
                                        setup pane → runs setup first (blocking? wait)
                                      then, per agent pane:
                                        herdr agent start ─► optional prompt
```

A `[[startup]]` hook runs once after the server restores its session: it drops
claims whose worktree no longer exists and clears the per-session focus cache.

Because it's herdr's own layout and agent APIs under the hood, the result is a
set of ordinary panes — the plugin doesn't manage their lifecycle afterwards.

### Is the `workspace.focused` hook still needed?

`workspace.focused` fires on **every** workspace switch, making it by far the
most frequent reason this plugin runs. It's subscribed to only because herdr's
TUI worktree creation has historically not delivered `worktree.created` to
plugins. Its early-exit path is therefore kept to a single `stat` — no config
parse, no herdr query, and the shim skips its rebuild check for this event.

If your herdr build *does* deliver `worktree.created` for TUI-created worktrees,
you can drop the trigger entirely. To check:

```sh
# Create a worktree from the TUI (right-click a space -> new worktree), then:
herdr plugin log list --plugin herdr-plugin-workspace-manager
```

If the log shows a `worktree.created` or `workspace.created` entry for that
creation, the focus hook is redundant — set `HERDR_WSM_FOCUS_HOOK=0` in herdr's
environment, or remove the `[[events]] on = "workspace.focused"` block from the
manifest.

Keeping it on is harmless. One side effect worth knowing: focusing a *fresh,
empty* worktree of a configured repo applies its layout (once) — which is
exactly what the TUI flow relies on.

## Trust & security

A herdr plugin is ordinary code that runs on your machine with your environment
and can call the full herdr CLI. This plugin runs the commands you put in your
`config.yml` (e.g. `make setup`, `nvim`) in your worktrees. Review the manifest
and `config.yml` before use — you control exactly what runs.

## Notes & limitations

- The first tab of a layout **replaces** the worktree's existing root tab (herdr
  creates the replacement first, then closes the old one); later tabs are
  appended. On a brand-new worktree that root tab is empty, so nothing is lost —
  but running `apply` by hand against a workspace with work in its first tab
  will replace that tab and the processes in it.
- Layouts are applied additively beyond the first tab; the plugin does not tear
  panes down.
- Requires herdr **0.7.5+** and Unix (Linux/macOS). `layout.apply` has no CLI
  wrapper, so the plugin speaks herdr's socket API directly for that one call;
  on Windows the same API lives behind a named pipe, which this plugin doesn't
  implement.
- A single Rust binary with no runtime dependencies (includes a small
  YAML-subset parser). The `bin/herdr-workspace-manager` shim compiles it on
  first use, so it works under `herdr plugin link` with just a Rust toolchain.
- Plugin installs and links are **global to your user** as of herdr 0.7.5 (they
  used to be per-session). If you linked this plugin inside a named session on
  0.7.3, link it again once.

## Development

```sh
cargo test                                    # unit tests + integration test
cargo test --bin herdr-workspace-manager      # unit tests only (no herdr needed)
cargo test --test integration                 # live: creates a real herdr worktree and drives the hook end-to-end

# Opt in to the live agent check (starts a real agent in a throwaway workspace):
HERDR_WSM_ITEST_AGENT=claude cargo test --test integration
```

The integration test auto-skips when no herdr server is running; otherwise it
creates a throwaway git repo + a real linked worktree, drives the real
`workspace.focused` hook, and asserts the tab/pane structure, pane labels, env
injection, that each pane command actually ran (via marker files),
blocking-setup ordering, and idempotency — then cleans everything up. The agent
test is opt-in because it needs the agent's own binary installed.

This repo is a self-contained herdr plugin (a `herdr-plugin.toml` manifest plus
a Rust crate). To list it in the herdr marketplace, the GitHub repo carries the
`herdr-plugin` topic; anyone can then `herdr plugin install <owner>/<repo>`. Unit
tests run in CI (`.github/workflows/ci.yml`); the integration test auto-skips
there and needs a local herdr server to run for real.

## Credits

The declarative layout config is inspired by [workmux](https://github.com/raine/workmux).

## License

[MIT](./LICENSE) © Raza Jamil
