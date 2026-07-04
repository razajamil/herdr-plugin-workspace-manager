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
//   { kind: "split", pane, from, direction, ratio, size, cwd }    herdr pane split
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
          size: pane.size,
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

// Clamp a split ratio into herdr's usable open interval (0, 1). A fixed cell
// size that meets or exceeds the available space would otherwise produce a
// degenerate 0-width pane; clamping keeps both panes visible.
export function clampRatio(r) {
  if (!Number.isFinite(r)) return null;
  return Math.min(0.99, Math.max(0.01, r));
}

// Resolve the `--ratio` to pass to `herdr pane split`. herdr's ratio is the
// fraction the PREVIOUS (from) pane keeps; the newly created pane gets the rest.
// A pane's `size` sizes the NEW pane, so it's inverted here:
//   percent p -> new pane is p%      -> from pane keeps (1 - p/100)
//   cells   w -> new pane is w cells -> from pane keeps (1 - w/extent)
// `extent` is the from pane's current size (columns/rows) along the split axis,
// needed only for a cell size; the runner queries it live. Legacy `ratio`
// (already the from-pane share) is passed through unchanged. Returns a number in
// (0, 1), or null when nothing sizes the split (or a cell size can't be
// converted because the extent is unknown).
export function splitRatioArg({ ratio = null, size = null } = {}, extent = null) {
  if (ratio != null) return ratio;
  if (!size) return null;
  if (size.kind === "percent") return clampRatio(1 - size.value / 100);
  if (size.kind === "cells") {
    if (!Number.isFinite(extent) || extent <= 0) return null;
    return clampRatio(1 - size.value / extent);
  }
  return null;
}
