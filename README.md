# herdr-plugin-workspace-manager

A [herdr](https://herdr.dev) plugin that arranges every new worktree into a
declarative layout — and cleans up the ones you're done with.

- **Declarative tab/pane layouts.** Define tabs, panes, splits, and per-pane
  startup commands once in YAML.
- **Applied automatically, per repo.** Point a repo at a layout and every new
  worktree — created from the CLI *or* the herdr TUI — opens straight into it,
  fully arranged. No rebuilding your working view by hand each time.
- **One-off setup command.** Run e.g. `npm install` in a chosen pane before the
  rest of the layout spawns — optionally blocking until it finishes.
- **Clean up merged worktrees in one command.** `herdr-workspace-manager
  remove-gone` removes the current repo's linked worktrees whose upstream branch
  is gone (e.g. after a PR merged and its branch was deleted), leaving the main
  checkout and anything dirty or in-progress untouched.

## Demo

https://github.com/user-attachments/assets/2b222886-b256-4187-a8ae-1a560dd08eef

A new worktree opens straight into its declarative layout — the `agent` / `review` /
`git` / `dev-server` tabs, each with its editor and terminal panes already running.

## Quick start

Requires **herdr ≥ 0.7.0** and **Node ≥ 18** on your `PATH` (used to run the
hook). No other dependencies and no build step.

Install from GitHub:

```sh
herdr plugin install razajamil/herdr-plugin-workspace-manager
```

Then find the config directory and drop a `config.yml` in it (next section):

```sh
herdr plugin config-dir herdr-plugin-workspace-manager
# -> ~/.config/herdr/plugins/config/herdr-plugin-workspace-manager
```

Developing locally? `git clone` the repo and `herdr plugin link ./herdr-plugin-workspace-manager`
for live edits (`link` skips any build step). Pin a release with `--ref`, or use
`--yes` for a non-interactive install.

### Optional: the `remove-gone` CLI

`herdr-workspace-manager` is a CLI bundled with the plugin, needed only to
[remove merged worktrees](#removing-worktrees-whose-remote-branch-is-gone) —
layouts work without it. Installing the plugin doesn't put it on your `PATH`, so
run the bundled [`install.sh`](./install.sh), which symlinks it into `~/.local/bin`:

```sh
# From a clone of this repo:
./install.sh

# Or, if you installed the plugin and have no clone, fetch and run it:
curl -fsSL https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/install.sh | sh
```

Link it elsewhere by passing a directory (`./install.sh ~/bin`); the installer
works whether the plugin is installed or linked, and warns if the target isn't on
your `PATH`. Then run `herdr-workspace-manager --help`; pass `--workspace <id>` to
target a repo other than the current pane's.

## Configure a layout

Create `config.yml` in the config directory above (a fallback path
`~/.herdr/plugins/herdr-plugin-workspace-manager/config.yml` also works). A fully
annotated template lives in [`config.example.yml`](./config.example.yml).

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/schema.json
layouts:
  - id: web-app
    setup:
      command: npm install   # optional one-off, run before the rest of the layout
      blocking: true         # if true, no other panes spawn until it finishes
    tabs:
      - title: code
        panes:
          - title: agent
            command: claude    # optional command to run in the pane
            setup: true        # runs setup.command first, then `claude`
          - title: editor
            command: nvim
            split: vertical    # placed beside the agent pane
      - title: server
        panes:
          - title: dev
            command: npm run dev
          - title: shell
            split: horizontal  # stacked below the dev server
      - title: review
        panes:
          - title: agent
            command: opencode   # a different agent than the code tab
          - title: editor
            command: nvim
            split: vertical
      - title: git
        panes:
          - title: lazygit
            command: lazygit

workspaces:
  - repo: ~/code/web-app       # any linked worktree of this repo gets the layout
    defaultLayout: web-app
```

Now create a worktree for `~/code/web-app` (via the TUI or `herdr worktree create`)
and it opens with the `code` / `server` / `git` tabs already laid out and running.

### Schema

| Field | Where | Meaning |
| --- | --- | --- |
| `layouts[].id` | layout | Unique id, referenced by `defaultLayout`. |
| `layouts[].setup.command` | layout | Optional command run on the `setup: true` pane. |
| `layouts[].setup.blocking` | layout | If `true`, no further tabs/panes spawn until setup finishes. |
| `tabs[].title` | tab | Tab label. The first tab reuses the worktree's existing tab. |
| `panes[].title` | pane | Pane label. |
| `panes[].command` | pane | Optional command to run in the pane. |
| `panes[].setup` | pane | Marks the single pane that runs `setup.command` (at most one per layout). |
| `panes[].split` | pane | For panes after the first: `vertical` \| `horizontal` \| `right` \| `down`. |
| `panes[].ratio` | pane | Optional split ratio `(0, 1)`. |
| `workspaces[].repo` | workspace | **Recommended.** Repo root (`~` expanded) or bare repo name. Matches any *linked worktree* of that repo; the main checkout is never touched. |
| `workspaces[].path` | workspace | Alternative: prefix-match the worktree's checkout path. |
| `workspaces[].defaultLayout` | workspace | Layout id applied to matching new worktrees. |

Each `workspaces[]` entry needs `repo` and/or `path`; a `repo` match wins over a
`path` match.

**Split direction.** herdr splits are `right` or `down`. This plugin maps
`vertical → right` (side by side) and `horizontal → down` (stacked); `right`/`down`
are also accepted. The first pane of a tab is never split; each later pane splits
from the previous one.

**Setup pane.** At most one pane per layout may set `setup: true`. The setup
command runs there first; with `blocking: true` the hook waits for it to finish
before building anything else. After setup, that pane still runs its own
`command`. Put the setup pane first so nothing spawns ahead of it.

### Apply and validate

```sh
# Apply a layout to the current workspace (or pass a layout id):
herdr plugin action invoke apply --plugin herdr-plugin-workspace-manager

# Validate the config and print the resolved layouts/workspaces:
herdr plugin action invoke validate --plugin herdr-plugin-workspace-manager
```

A keybinding (`prefix+shift+l` → apply) is declared in the manifest.

### Removing worktrees whose remote branch is gone

After a PR merges and its branch is deleted upstream, the local worktree lingers.
The plugin can clean up the **current repo's** linked worktrees whose remote
branch was deleted ("gone", in git's terms).

The most convenient interface is the bundled CLI, **`herdr-workspace-manager`**
(see [Quick start](#optional-the-remove-gone-cli) for putting it on your `PATH`).
It prints straight to your terminal, lists the gone worktrees by workspace name,
then asks for confirmation before removing them:

```sh
# List the gone worktrees, then prompt "Remove N worktree(s)? [y/N]":
herdr-workspace-manager remove-gone

# Just print the list; remove nothing, no prompt:
herdr-workspace-manager remove-gone --dry-run

# Skip the prompt (for scripts); add --force to also remove dirty worktrees:
herdr-workspace-manager remove-gone --confirm --force
```

The preview is also exposed as a plugin **action** for the TUI action menu /
keybindings. It runs headless (no prompt) and removes nothing — `action invoke`
streams output to the plugin log rather than your terminal, so the CLI is the way
to actually remove worktrees:

```sh
herdr plugin action invoke remove-gone --plugin herdr-plugin-workspace-manager  # preview only
```

A branch is only ever a candidate when it **had an upstream that was then
deleted**. Worktrees on branches that never pushed/tracked a remote are left
alone, as is the repo's main checkout. Removal additionally **skips** (and
reports) the workspace you run it from and — unless `--force` — any worktree with
uncommitted changes, so nothing in-progress is destroyed silently. A clean
worktree's committed history survives removal (it stays in the repo's object
store/reflog). A `git fetch --prune` runs first so deleted branches are detected
accurately; pass `--no-fetch` (or set `HERDR_WSM_NO_FETCH=1`) to use cached refs.

### Editor autocomplete

The repo ships a JSON Schema ([`schema.json`](./schema.json)). Editors backed by
the YAML Language Server — VS Code (Red Hat YAML extension), Neovim (`yamlls`),
Helix, etc. — give you completion, hover docs, and validation when the file
starts with this modeline (the bundled `config.example.yml` already includes it):

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/schema.json
```

Or map it without editing the file, e.g. in VS Code `settings.json`:

```json
"yaml.schemas": {
  "https://raw.githubusercontent.com/razajamil/herdr-plugin-workspace-manager/main/schema.json": "**/herdr-plugin-workspace-manager/config.yml"
}
```

## Tuning (env vars)

| Var | Default | Purpose |
| --- | --- | --- |
| `HERDR_WSM_CONFIG` | — | Absolute path to a config file (overrides the default lookup). |
| `HERDR_WSM_PANE_READY_MS` | `700` | Delay before sending the first command to a freshly spawned pane (its shell needs a moment, or early keystrokes are dropped). |
| `HERDR_WSM_SETUP_TIMEOUT_MS` | `600000` | Max wait for a blocking setup command to finish. |
| `HERDR_WSM_NO_FETCH` | — | If set, `remove-gone` skips the `git fetch --prune` and uses cached remote-tracking refs. |

## Trust & security

A herdr plugin is ordinary code that runs on your machine with your environment
and can call the full herdr CLI. This plugin runs the commands you put in your
`config.yml` (e.g. `npm install`, `nvim`) in your worktrees. Review the manifest
and `config.yml` before use — you control exactly what runs.

## Testing

```sh
npm run test:unit        # YAML parser, config validation, planner, guards (no herdr needed)
npm run test:integration # live: creates a real herdr worktree and drives the hook end-to-end
npm test                 # both
```

The integration test auto-skips when no herdr server is running; otherwise it
creates a throwaway git repo + a real linked worktree, drives the real
`workspace.focused` hook, and asserts the tab/pane structure, that each pane
command actually ran (via marker files), blocking-setup ordering, and
idempotency — then cleans everything up.

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
4. Matches against `workspaces[]` by **repo** (`repo_root`/`repo_name`) or path.
5. Only builds into a **fresh** (1-tab/1-pane) workspace.
6. **Dedupes** with an atomic claim (the events can fire together; also skips
   restored worktrees after a restart) — applied exactly once per worktree.
7. Walks the matched `defaultLayout` **depth-first**, driving the herdr CLI to
   build it — exactly as if you'd done it by hand.

> **Note on `workspace.focused`.** It fires on every workspace switch, so the
> hook runs a tiny check each time (a "decided" cache makes repeat focuses a
> no-op). A side effect: focusing a *fresh, empty* worktree of a configured repo
> applies its layout (once) — which is exactly how the TUI flow works, since the
> TUI focuses the new worktree on creation. To turn off the focus trigger, remove
> the `[[events]] on = "workspace.focused"` block from the manifest; `herdr
> worktree create` still works via the other two events.

```
 new worktree (CLI or TUI) ──► event hook (bin/event.mjs)
        ▼
   load config ─► match workspaces[] (repo or path) ─► default layout
        │                                                   │
        └── no match / not fresh / already done ─► no-op     ▼
                                          depth-first walk → herdr tab/pane CLI:
                                            tab 0  → reuse the worktree's root tab + pane
                                            tab N  → herdr tab create
                                            pane J → herdr pane split <prev pane>
                                            each pane → rename + run command
                                            setup pane → run setup (blocking? wait) then its command
```

Because it's just the herdr CLI under the hood, the result is a set of ordinary
panes — the plugin doesn't manage their lifecycle afterwards.

## Notes & limitations

- The first tab/pane of a layout reuses the worktree's existing root tab/pane;
  additional tabs/panes are created.
- Layouts are applied additively; the plugin does not tear panes down.
- Pure ESM, no runtime dependencies (includes a small YAML-subset parser), so it
  works immediately under `herdr plugin link` with no build step.

## Credits

The declarative layout config is inspired by [workmux](https://github.com/raine/workmux).

## Contributing / publishing

This repo is a self-contained herdr plugin (a `herdr-plugin.toml` manifest plus
ESM scripts). To list it in the herdr marketplace, the GitHub repo carries the
`herdr-plugin` topic; anyone can then `herdr plugin install <owner>/<repo>`. Unit
tests run in CI (`.github/workflows/ci.yml`); the integration test needs a local
herdr server and is run manually.

## License

[MIT](./LICENSE) © Raza Jamil
