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
// Because the walk is depth-first and a blocking RunSetup step pauses the
// runner before any later step, putting "setup: true" on the first pane gives
// the documented guarantee: no other tabs/panes spawn until setup finishes.

use crate::config::{Direction, Layout, Size};

pub const ROOT_TAB: &str = "t0";
pub const ROOT_PANE: &str = "t0p0";

pub fn tab_handle(ti: usize) -> String {
    format!("t{}", ti)
}

pub fn pane_handle(ti: usize, pj: usize) -> String {
    format!("t{}p{}", ti, pj)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Rename the worktree's existing root tab.
    ReuseTab { tab: String, title: Option<String> },
    /// herdr tab create
    CreateTab { tab: String, pane: String, title: Option<String>, cwd: Option<String> },
    /// herdr pane split
    Split {
        pane: String,
        from: String,
        direction: Direction,
        ratio: Option<f64>,
        size: Option<Size>,
        cwd: Option<String>,
    },
    /// herdr pane rename
    RenamePane { pane: String, title: String },
    /// Run the setup command (+ wait when blocking).
    RunSetup { pane: String, command: String, blocking: bool },
    /// herdr pane run
    Run { pane: String, command: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub layout_id: String,
    pub steps: Vec<Step>,
}

pub fn build_plan(layout: &Layout, cwd: Option<&str>) -> Plan {
    let mut steps = Vec::new();

    for (ti, tab) in layout.tabs.iter().enumerate() {
        let t_handle = tab_handle(ti);

        if ti == 0 {
            steps.push(Step::ReuseTab { tab: t_handle, title: tab.title.clone() });
        } else {
            steps.push(Step::CreateTab {
                tab: t_handle,
                pane: pane_handle(ti, 0),
                title: tab.title.clone(),
                cwd: cwd.map(String::from),
            });
        }

        for (pj, pane) in tab.panes.iter().enumerate() {
            let p_handle = pane_handle(ti, pj);

            if pj > 0 {
                steps.push(Step::Split {
                    pane: p_handle.clone(),
                    from: pane_handle(ti, pj - 1),
                    direction: pane.split.unwrap_or(Direction::Right),
                    ratio: pane.ratio,
                    size: pane.size.clone(),
                    cwd: cwd.map(String::from),
                });
            }

            if let Some(title) = &pane.title {
                steps.push(Step::RenamePane { pane: p_handle.clone(), title: title.clone() });
            }

            // The single setup pane runs the layout-level setup command first
            // (optionally blocking), then its own command (if any).
            if pane.setup {
                if let Some(setup) = &layout.setup {
                    steps.push(Step::RunSetup {
                        pane: p_handle.clone(),
                        command: setup.command.clone(),
                        blocking: setup.blocking,
                    });
                }
            }
            if let Some(command) = &pane.command {
                steps.push(Step::Run { pane: p_handle.clone(), command: command.clone() });
            }
        }
    }

    Plan { layout_id: layout.id.clone(), steps }
}

// Clamp a split ratio into herdr's usable open interval (0, 1). A fixed cell
// size that meets or exceeds the available space would otherwise produce a
// degenerate 0-width pane; clamping keeps both panes visible.
pub fn clamp_ratio(r: f64) -> Option<f64> {
    if !r.is_finite() {
        return None;
    }
    Some(r.clamp(0.01, 0.99))
}

// Resolve the `--ratio` to pass to `herdr pane split`. herdr's ratio is the
// fraction the PREVIOUS (from) pane keeps; the newly created pane gets the rest.
// A pane's `size` sizes the NEW pane, so it's inverted here:
//   percent p -> new pane is p%      -> from pane keeps (1 - p/100)
//   cells   w -> new pane is w cells -> from pane keeps (1 - w/extent)
// `extent` is the from pane's current size (columns/rows) along the split axis,
// needed only for a cell size; the runner queries it live. Legacy `ratio`
// (already the from-pane share) is passed through unchanged. Returns a number in
// (0, 1), or None when nothing sizes the split (or a cell size can't be
// converted because the extent is unknown).
pub fn split_ratio_arg(ratio: Option<f64>, size: Option<&Size>, extent: Option<f64>) -> Option<f64> {
    if let Some(r) = ratio {
        return Some(r);
    }
    match size {
        None => None,
        Some(Size::Percent(p)) => clamp_ratio(1.0 - p / 100.0),
        Some(Size::Cells(w)) => {
            let extent = extent.filter(|e| e.is_finite() && *e > 0.0)?;
            clamp_ratio(1.0 - *w as f64 / extent)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{find_layout, validate_config, Config};
    use crate::yaml::parse_yaml;

    fn sample_config() -> Config {
        let text = [
            "layouts:",
            "  - id: web-app",
            "    setup:",
            "      command: mise run setup",
            "      blocking: true",
            "    tabs:",
            "      - title: main",
            "        panes:",
            "          - title: agent",
            "            command: opencode",
            "            setup: true",
            "          - title: editor",
            "            command: nvim",
            "            split: vertical",
            "      - title: dev-server",
            "        panes:",
            "          - title: server",
            "          - title: review",
            "            split: horizontal",
            "      - title: review",
            "        panes:",
            "          - title: agent",
            "            command: opencode",
            "          - title: editor",
            "            command: nvim",
            "            split: vertical",
        ]
        .join("\n");
        validate_config(&parse_yaml(&text).unwrap()).unwrap()
    }

    fn plan(cwd: Option<&str>) -> Plan {
        let config = sample_config();
        build_plan(find_layout(&config, "web-app").unwrap(), cwd)
    }

    fn s(x: &str) -> String {
        x.to_string()
    }

    #[test]
    fn produces_the_exact_depth_first_step_sequence() {
        let Plan { layout_id, steps } = plan(Some("/work"));
        assert_eq!(layout_id, "web-app");
        let cwd = Some(s("/work"));
        assert_eq!(
            steps,
            vec![
                // tab 0 "main" reuses the worktree's root tab + root pane
                Step::ReuseTab { tab: s("t0"), title: Some(s("main")) },
                Step::RenamePane { pane: s("t0p0"), title: s("agent") },
                Step::RunSetup { pane: s("t0p0"), command: s("mise run setup"), blocking: true },
                Step::Run { pane: s("t0p0"), command: s("opencode") },
                Step::Split {
                    pane: s("t0p1"),
                    from: s("t0p0"),
                    direction: Direction::Right,
                    ratio: None,
                    size: None,
                    cwd: cwd.clone(),
                },
                Step::RenamePane { pane: s("t0p1"), title: s("editor") },
                Step::Run { pane: s("t0p1"), command: s("nvim") },
                // tab 1 "dev-server"
                Step::CreateTab {
                    tab: s("t1"),
                    pane: s("t1p0"),
                    title: Some(s("dev-server")),
                    cwd: cwd.clone(),
                },
                Step::RenamePane { pane: s("t1p0"), title: s("server") },
                Step::Split {
                    pane: s("t1p1"),
                    from: s("t1p0"),
                    direction: Direction::Down,
                    ratio: None,
                    size: None,
                    cwd: cwd.clone(),
                },
                Step::RenamePane { pane: s("t1p1"), title: s("review") },
                // tab 2 "review"
                Step::CreateTab {
                    tab: s("t2"),
                    pane: s("t2p0"),
                    title: Some(s("review")),
                    cwd: cwd.clone(),
                },
                Step::RenamePane { pane: s("t2p0"), title: s("agent") },
                Step::Run { pane: s("t2p0"), command: s("opencode") },
                Step::Split {
                    pane: s("t2p1"),
                    from: s("t2p0"),
                    direction: Direction::Right,
                    ratio: None,
                    size: None,
                    cwd: cwd.clone(),
                },
                Step::RenamePane { pane: s("t2p1"), title: s("editor") },
                Step::Run { pane: s("t2p1"), command: s("nvim") },
            ]
        );
    }

    #[test]
    fn blocking_setup_runs_before_any_later_tab_is_spawned() {
        let Plan { steps, .. } = plan(None);
        let setup_idx = steps.iter().position(|st| matches!(st, Step::RunSetup { .. }));
        let first_create_tab = steps.iter().position(|st| matches!(st, Step::CreateTab { .. }));
        assert!(setup_idx.is_some() && first_create_tab.is_some());
        assert!(setup_idx < first_create_tab, "setup must precede the first create-tab");
    }

    #[test]
    fn setup_pane_runs_setup_then_its_own_command() {
        let Plan { steps, .. } = plan(None);
        let setup_idx = steps.iter().position(|st| matches!(st, Step::RunSetup { .. })).unwrap();
        let run_idx = steps
            .iter()
            .position(|st| matches!(st, Step::Run { pane, .. } if pane == "t0p0"))
            .unwrap();
        assert!(setup_idx < run_idx, "setup command precedes the pane command on the setup pane");
    }

    #[test]
    fn first_pane_of_each_tab_is_never_split_later_panes_split_from_previous() {
        let Plan { steps, .. } = plan(None);
        let splits: Vec<(&str, &str)> = steps
            .iter()
            .filter_map(|st| match st {
                Step::Split { pane, from, .. } => Some((pane.as_str(), from.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(splits, vec![("t0p1", "t0p0"), ("t1p1", "t1p0"), ("t2p1", "t2p0")]);
    }

    #[test]
    fn single_tab_single_pane_layout_only_reuses_the_root() {
        let text = [
            "layouts:",
            "  - id: solo",
            "    tabs:",
            "      - title: only",
            "        panes:",
            "          - title: shell",
            "            command: htop",
        ]
        .join("\n");
        let config = validate_config(&parse_yaml(&text).unwrap()).unwrap();
        let Plan { steps, .. } = build_plan(find_layout(&config, "solo").unwrap(), None);
        assert_eq!(
            steps,
            vec![
                Step::ReuseTab { tab: s("t0"), title: Some(s("only")) },
                Step::RenamePane { pane: s("t0p0"), title: s("shell") },
                Step::Run { pane: s("t0p0"), command: s("htop") },
            ]
        );
    }

    #[test]
    fn split_steps_carry_a_normalized_pane_size() {
        let text = [
            "layouts:",
            "  - id: sized",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "          - title: cells",
            "            split: vertical",
            "            size: 40",
            "          - title: percent",
            "            split: vertical",
            "            size: \"30%\"",
            "          - title: fraction",
            "            split: vertical",
            "            size: 0.25",
        ]
        .join("\n");
        let config = validate_config(&parse_yaml(&text).unwrap()).unwrap();
        let Plan { steps, .. } = build_plan(find_layout(&config, "sized").unwrap(), None);
        let sizes: Vec<Option<Size>> = steps
            .iter()
            .filter_map(|st| match st {
                Step::Split { size, .. } => Some(size.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            sizes,
            vec![
                Some(Size::Cells(40)),
                Some(Size::Percent(30.0)),
                Some(Size::Percent(25.0)),
            ]
        );
    }

    #[test]
    fn clamp_ratio_keeps_ratios_inside_herdrs_open_interval() {
        assert_eq!(clamp_ratio(0.5), Some(0.5));
        assert_eq!(clamp_ratio(-3.0), Some(0.01)); // a cell size >= the whole pane
        assert_eq!(clamp_ratio(5.0), Some(0.99));
        assert_eq!(clamp_ratio(f64::NAN), None);
    }

    #[test]
    fn split_ratio_arg_inverts_a_pane_size_into_the_from_panes_share() {
        // Nothing to size -> no --ratio.
        assert_eq!(split_ratio_arg(None, None, None), None);
        // Legacy ratio is the from-pane share already: passed through untouched.
        assert_eq!(split_ratio_arg(Some(0.3), None, None), Some(0.3));
        // A 30%-wide new pane means the from pane keeps 70%.
        assert_eq!(split_ratio_arg(None, Some(&Size::Percent(30.0)), None), Some(0.7));
        // A fixed 50-cell pane out of 200 -> new pane 25% -> from pane keeps 75%.
        assert_eq!(split_ratio_arg(None, Some(&Size::Cells(50)), Some(200.0)), Some(0.75));
        // A cell size >= the available extent clamps rather than going degenerate.
        assert_eq!(split_ratio_arg(None, Some(&Size::Cells(300)), Some(200.0)), Some(0.01));
        // A cell size with no known extent yields no --ratio (herdr's default split).
        assert_eq!(split_ratio_arg(None, Some(&Size::Cells(40)), None), None);
    }
}
