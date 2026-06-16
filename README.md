# herdr-plugin-workspace-manager

A [herdr](https://herdr.dev) plugin for **declarative tab/pane layouts** with
**per-workspace defaults**. Set a default layout for a workspace and every new
worktree you create there is automatically arranged into that layout — tabs,
panes, splits, and per-pane startup commands — so you get the view you want
without rebuilding it by hand each time.

It can also run a one-off **setup command** (e.g. `mise run setup`) in a chosen
pane before the rest of the layout spawns.

## How it works

The plugin subscribes to **three** events, because herdr creates worktrees two
different ways and they emit different events to plugins:

| You create a worktree via… | herdr emits to plugins |
| --- | --- |
| `herdr worktree create` (CLI / socket API) | `worktree.created` **and** `workspace.created` |
| the **TUI** right-click "new worktree" (in-app) | **only** `workspace.focused` (it focuses the new worktree on creation) |

So the hook listens for `worktree.created`, `workspace.created`, **and**
`workspace.focused`. On any of them it:

1. Loads the YAML config and resolves the focused/created workspace id.
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
> hook spawns a tiny check each time (a "decided" cache makes repeat focuses a
> no-op). A side effect: focusing a *fresh, empty* worktree of a configured repo
> applies its layout (once) — which is exactly how the TUI flow works, since the
> TUI focuses the new worktree on creation. If you don't want the focus trigger,
> remove the `[[events]] on = "workspace.focused"` block from the manifest;
> `herdr worktree create` will still work via the other two events.

```
 herdr worktree create ──► herdr emits worktree.created ──► bin/event.mjs
        │ payload: { data: { workspace{ active_tab_id, worktree.checkout_path }, worktree } }
        ▼
   load config ─► match workspaces[] (repo or path) ─► default layout
        │                                                   │
        └── no match / no default ─► exit 0 (no-op)         ▼
                                          depth-first walk → herdr tab/pane CLI:
                                            tab 0  → reuse the worktree's root tab + pane
                                            tab N  → herdr tab create
                                            pane J → herdr pane split <prev pane>
                                            each pane → rename + run command
                                            setup pane → run setup (blocking? wait) then its command
```

Because it's just the herdr CLI under the hood, the result is a set of ordinary
panes — the plugin doesn't manage their lifecycle afterwards.

## Requirements

- herdr ≥ 0.7.0
- Node ≥ 18 on your `PATH` (used to run the hook — no other dependencies, no build step)

## Install

From GitHub / the herdr marketplace (recommended):

```sh
herdr plugin install razajamil/herdr-plugin-workspace-manager
herdr plugin list
herdr plugin config-dir herdr-plugin-workspace-manager   # then add config.yml here
```

Local development (live edits — `link` skips any build step):

```sh
git clone https://github.com/razajamil/herdr-plugin-workspace-manager
herdr plugin link ./herdr-plugin-workspace-manager
```

Pin a revision with `--ref`, or use `--yes` for a non-interactive install.

## Configuration

Config lives in the herdr-managed plugin config directory:

```sh
herdr plugin config-dir herdr-plugin-workspace-manager
# -> ~/.config/herdr/plugins/config/herdr-plugin-workspace-manager
```

Put a `config.yml` there. A fallback path is also accepted:
`~/.herdr/plugins/herdr-plugin-workspace-manager/config.yml`. See
[`config.example.yml`](./config.example.yml) for a fully commented template.

```yaml
layouts:
  - id: reckon-frontend
    setup:
      command: mise run setup   # optional one-off command
      blocking: true            # if true, no other panes spawn until it finishes
    tabs:
      - title: main
        panes:
          - title: agent
            command: opencode    # optional; run in the pane
            setup: true          # this pane runs `setup.command` first
          - title: editor
            command: nvim
            split: vertical      # split relative to the previous pane
      - title: dev-server
        panes:
          - title: server
          - title: review
            split: horizontal
workspaces:
  - repo: ~/dev/reckon-frontend     # matches any linked worktree of this repo
    defaultLayout: reckon-frontend
```

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
| `panes[].split` | pane | For panes after the first: `vertical`\|`horizontal`\|`right`\|`down`. |
| `panes[].ratio` | pane | Optional split ratio `(0, 1)`. |
| `workspaces[].repo` | workspace | **Recommended.** Repo root (`~` expanded) or bare repo name. Matches any *linked worktree* of that repo; the main checkout is never touched. |
| `workspaces[].path` | workspace | Alternative/legacy: prefix-match the worktree's checkout path. More brittle (herdr's worktrees dir is configurable). |
| `workspaces[].defaultLayout` | workspace | Layout id applied to matching new worktrees. |

Each `workspaces[]` entry needs `repo` and/or `path`. A `repo` match wins over a
`path` match. Match against the example repo root with `repo: ~/dev/reckon-frontend`.

**Split direction.** herdr splits are `right` or `down`. This plugin maps
`vertical → right` (panes side by side) and `horizontal → down` (panes stacked);
`right`/`down` are also accepted literally. The first pane of a tab is never
split; each later pane splits from the previous pane in that tab.

**Setup pane.** Only one pane per layout may set `setup: true`. The setup
command runs there first; with `blocking: true` the hook waits for it to finish
(via a printed sentinel + `herdr wait output`) before building anything else.
After setup, that pane still runs its own `command` if it has one. Put the setup
pane first so nothing spawns ahead of it.

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
| `HERDR_WSM_CONFIG` | — | Absolute path to a config file (overrides the default lookup; used by tests). |
| `HERDR_WSM_PANE_READY_MS` | `700` | Delay before sending the first command to a freshly spawned pane (its shell needs a moment, or early keystrokes are dropped). |
| `HERDR_WSM_SETUP_TIMEOUT_MS` | `600000` | Max wait for a blocking setup command to finish. |

## Testing

```sh
npm run test:unit        # YAML parser, config validation, planner (no herdr needed)
npm run test:integration # live: drives a real herdr workspace end-to-end
npm test                 # both
```

- **Unit tests** cover the YAML subset parser, config validation/normalization
  (incl. split aliasing and the single-setup-pane rule), and the pure planner
  (asserts the exact depth-first step sequence and blocking ordering).
- **The integration test** is the real-terminal check. It auto-skips when no
  herdr server is running; otherwise it creates a throwaway temp dir + a **real
  herdr workspace**, drives the **real event hook** with a payload shaped like
  herdr's actual `worktree.created` event, then asserts:
  - tab labels + pane counts via `herdr tab list` / `herdr pane list`;
  - that each pane command **actually executed** — every command writes a marker
    file, checked on disk (filesystem proof, not terminal scraping);
  - that the **blocking setup** completed before later panes were built;
  - that the setup pane produced terminal output (`herdr wait output`).
  It closes the workspace and removes the temp dir afterwards.

## Notes & limitations

- The first tab/pane of a layout reuses the worktree's existing root tab/pane;
  additional tabs/panes are created.
- Layouts are applied additively; the plugin does not tear panes down.
- Pure ESM, no runtime dependencies (includes a small YAML-subset parser), so it
  works immediately under `herdr plugin link` with no build step.

## Trust & security

A herdr plugin is ordinary code that runs on your machine with your environment
and can call the full herdr CLI. This plugin runs the commands you put in your
`config.yml` (e.g. `mise run setup`, `nvim`) in your worktrees. Review the
manifest and `config.yml` before use — you control exactly what runs.

## Contributing / publishing

This repo is a self-contained herdr plugin (a `herdr-plugin.toml` manifest plus
ESM scripts). To list it in the herdr marketplace, the GitHub repo carries the
`herdr-plugin` topic; anyone can then `herdr plugin install <owner>/<repo>`.
Unit tests run in CI (`.github/workflows/ci.yml`); the integration test requires
a local herdr server and is run manually.

## License

[MIT](./LICENSE) © Raza Jamil
