# herdr-plugin-workspace-manager

A [herdr](https://herdr.dev) plugin for **declarative tab/pane layouts** with
**per-workspace defaults**. Define a layout once, point a repo at it, and every
new worktree you create — from the CLI *or* the herdr TUI — is automatically
arranged into that layout: tabs, panes, splits, and per-pane startup commands.
No more rebuilding your working view by hand each time.

It can also run a one-off **setup command** (e.g. `npm install`) in a chosen
pane before the rest of the layout spawns.

## Install

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

## Configuration

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

## Actions

```sh
# Apply a layout to the current workspace (or pass a layout id):
herdr plugin action invoke apply --plugin herdr-plugin-workspace-manager

# Validate the config and print the resolved layouts/workspaces:
herdr plugin action invoke validate --plugin herdr-plugin-workspace-manager
```

A keybinding (`prefix+shift+l` → apply) is declared in the manifest.

## Tuning (env vars)

| Var | Default | Purpose |
| --- | --- | --- |
| `HERDR_WSM_CONFIG` | — | Absolute path to a config file (overrides the default lookup). |
| `HERDR_WSM_PANE_READY_MS` | `700` | Delay before sending the first command to a freshly spawned pane (its shell needs a moment, or early keystrokes are dropped). |
| `HERDR_WSM_SETUP_TIMEOUT_MS` | `600000` | Max wait for a blocking setup command to finish. |

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

## Contributing / publishing

This repo is a self-contained herdr plugin (a `herdr-plugin.toml` manifest plus
ESM scripts). To list it in the herdr marketplace, the GitHub repo carries the
`herdr-plugin` topic; anyone can then `herdr plugin install <owner>/<repo>`. Unit
tests run in CI (`.github/workflows/ci.yml`); the integration test needs a local
herdr server and is run manually.

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

## License

[MIT](./LICENSE) © Raza Jamil
