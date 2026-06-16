// Pure layout planner.
//
// Turns a normalized layout into an ordered list of symbolic steps that mirror
// how a user would build the layout by hand (a depth-first walk of tabs then
// panes). The steps reference panes/tabs by SYMBOLIC handles; the runner maps
// those to real herdr ids as it executes (because a pane's real id is only
// known after `tab create` / `pane split` returns).
//
// Handles:
//   tab  ti  -> "t0", "t1", ...      ("t0" is the worktree's existing root tab)
//   pane pj  -> "t<ti>p<pj>"         ("t0p0" is the worktree's existing root pane)
//
// Step kinds:
//   { kind: "reuse-tab",  tab,            title }                 rename existing root tab
//   { kind: "create-tab", tab, pane,      title, cwd }            herdr tab create
//   { kind: "split",      pane, from, direction, ratio, cwd }     herdr pane split
//   { kind: "rename-pane", pane,          title }                 herdr pane rename
//   { kind: "run-setup",  pane, command,  blocking }              run setup cmd (+ wait)
//   { kind: "run",        pane,           command }               herdr pane run
//
// Because the walk is depth-first and a blocking "run-setup" step pauses the
// runner before any later step, putting "setup: true" on the first pane gives
// the documented guarantee: no other tabs/panes spawn until setup finishes.

export const ROOT_TAB = "t0";
export const ROOT_PANE = "t0p0";

export function tabHandle(ti) {
  return `t${ti}`;
}

export function paneHandle(ti, pj) {
  return `t${ti}p${pj}`;
}

export function buildPlan(layout, { cwd = null } = {}) {
  const steps = [];

  layout.tabs.forEach((tab, ti) => {
    const tHandle = tabHandle(ti);

    if (ti === 0) {
      steps.push({ kind: "reuse-tab", tab: tHandle, title: tab.title });
    } else {
      steps.push({
        kind: "create-tab",
        tab: tHandle,
        pane: paneHandle(ti, 0),
        title: tab.title,
        cwd,
      });
    }

    tab.panes.forEach((pane, pj) => {
      const pHandle = paneHandle(ti, pj);

      if (pj > 0) {
        steps.push({
          kind: "split",
          pane: pHandle,
          from: paneHandle(ti, pj - 1),
          direction: pane.split ?? "right",
          ratio: pane.ratio,
          cwd,
        });
      }

      if (pane.title) {
        steps.push({ kind: "rename-pane", pane: pHandle, title: pane.title });
      }

      // The single setup pane runs the layout-level setup command first
      // (optionally blocking), then its own command (if any).
      if (pane.setup && layout.setup) {
        steps.push({
          kind: "run-setup",
          pane: pHandle,
          command: layout.setup.command,
          blocking: layout.setup.blocking,
        });
      }
      if (pane.command) {
        steps.push({ kind: "run", pane: pHandle, command: pane.command });
      }
    });
  });

  return { layoutId: layout.id, steps };
}
