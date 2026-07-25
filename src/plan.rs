// Pure layout planner.
//
// Turns a normalized layout into one declarative tree per tab, in the shape
// herdr's `layout.apply` takes: a BSP tree of `pane` and `split` nodes carrying
// labels, cwd, env, and argv commands. The runner hands each tree to herdr in a
// single request instead of walking the layout with `pane split` / `pane rename`
// / `pane run`, so a whole tab is built in one server-side operation.
//
// The config's per-tab pane list stays linear -- pane 0 is the tab's root pane
// and each later pane splits off the pane before it -- which maps onto a
// right-nested tree: splitting pane j replaces the *leaf* j with a split whose
// first child is pane j and whose second child is everything after it.
//
//   panes [a, b(right), c(down)]  ->  split(right, a, split(down, b, c))
//
// Handles:
//   tab  ti  -> "t0", "t1", ...      ("t0" is the worktree's existing root tab)
//   pane pj  -> "t<ti>p<pj>"         ("t0p0" is the worktree's existing root pane)
//
// Sizing is resolved here rather than by querying herdr between splits: given
// the tab's cell area, every nested region is arithmetic, so a `size:` in cells
// converts to a ratio without a round-trip per split.

use std::collections::BTreeMap;

use crate::config::{Agent, Direction, Layout, Pane, Size};

pub const ROOT_TAB: &str = "t0";
/// The worktree's existing root pane. Only named in tests today; the runner
/// reaches it through the plan's tree order.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROOT_PANE: &str = "t0p0";

// herdr's own even split, used when a pane doesn't ask for a size. `layout.apply`
// requires an explicit ratio on every split node, so there is no "let herdr
// decide" to defer to.
pub const DEFAULT_RATIO: f64 = 0.5;

pub fn tab_handle(ti: usize) -> String {
    format!("t{}", ti)
}

pub fn pane_handle(ti: usize, pj: usize) -> String {
    format!("t{}p{}", ti, pj)
}

/// A leaf of the layout tree: one terminal pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneSpec {
    pub handle: String,
    pub title: Option<String>,
    /// argv for herdr to launch, or None to leave herdr's default shell running.
    pub command: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Pane(PaneSpec),
    Split { direction: Direction, ratio: f64, first: Box<Node>, second: Box<Node> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabPlan {
    pub handle: String,
    pub title: Option<String>,
    pub root: Node,
}

/// An agent to start (and optionally prompt) once its pane exists.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentAction {
    pub handle: String,
    pub agent: Agent,
}

/// The layout's setup command, which runs as part of the setup pane's own
/// process and records its exit status to `status_path` when it finishes.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupPlan {
    pub handle: String,
    pub blocking: bool,
    pub status_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub layout_id: String,
    pub tabs: Vec<TabPlan>,
    pub setup: Option<SetupPlan>,
    pub agents: Vec<AgentAction>,
}

/// Everything the planner needs from the live world.
#[derive(Clone, Debug, Default)]
pub struct PlanContext {
    pub cwd: Option<String>,
    /// The tab's usable area in cells (columns, rows), when known. Only needed
    /// to convert a fixed `size:` in cells into a ratio.
    pub area: Option<(f64, f64)>,
    /// Where the setup command should record its exit status.
    pub setup_status_path: Option<String>,
}

// --- Shell wrapping ----------------------------------------------------------

// Quote a script for embedding inside a single-quoted shell word.
fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// Wrap a user command as argv for herdr to launch.
//
// The command runs inside the user's own login+interactive shell rather than
// bare `sh`, because that is what it used to get: the previous implementation
// typed the command into the pane's interactive shell, so `.zshrc`/`.bash_profile`
// setup -- mise/asdf shims, nvm, aliases, PATH edits -- was already applied.
// Launching the argv directly would silently drop all of it, and `npm run dev`
// would stop resolving for anyone whose toolchain comes from a shell hook.
fn shell_argv(script: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("exec \"${{SHELL:-/bin/sh}}\" -lic {}", single_quote(script)),
    ]
}

// Hand the pane back to an interactive shell once the command finishes, so a
// pane whose command exits behaves like one you ran the command in yourself.
//
// Interactive but NOT login: the wrapper above already ran the login files, and
// this shell inherits their exported environment. Re-running them would fire
// login-only side effects a second time -- starting another ssh-agent, printing
// another MOTD -- for a shell that gains nothing from it.
const HAND_BACK: &str = "exec \"${SHELL:-/bin/sh}\" -i";

// Record the setup command's exit status where the runner can read it. Writing a
// file is deliberate: the previous approach printed a sentinel and scraped it
// back out of the terminal with `pane wait-output`, which depended on the marker
// surviving in the last rendered rows and on the shell not echoing it early.
fn record_status(status_path: &str) -> String {
    format!("printf '%s' \"$?\" > {}", single_quote(status_path))
}

// The script a pane runs, or None for a plain shell pane.
fn pane_script(pane: &Pane, setup: Option<&str>, status_path: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(setup) = setup {
        parts.push(setup.to_string());
        // `$?` has to be captured immediately after the setup command.
        parts.push(record_status(status_path.unwrap_or("/dev/null")));
    }
    if let Some(command) = &pane.command {
        parts.push(command.clone());
    }
    if parts.is_empty() {
        return None;
    }
    // An agent pane, or a `persist` command pane, must be left at a prompt: the
    // agent is started into that shell afterwards by `herdr agent start`.
    let hand_back = pane.agent.is_some() || pane.command.is_none() || pane.persist;
    if hand_back {
        parts.push(HAND_BACK.to_string());
    }
    Some(parts.join("; "))
}

// --- Tree building -----------------------------------------------------------

// The share of the region the *first* child keeps, matching herdr's split
// semantics. A pane's `size` describes the pane itself (the second child), so it
// is inverted; a legacy `ratio` is already the first child's share.
fn ratio_for(pane: &Pane, region: (f64, f64), direction: Direction) -> f64 {
    let extent = match direction {
        Direction::Down => region.1,
        Direction::Right => region.0,
    };
    split_ratio_arg(pane.ratio, pane.size.as_ref(), Some(extent).filter(|e| *e > 0.0))
        .unwrap_or(DEFAULT_RATIO)
}

// What's left for everything after a split that gave `ratio` to the first child.
fn remaining(region: (f64, f64), direction: Direction, ratio: f64) -> (f64, f64) {
    match direction {
        Direction::Right => (region.0 * (1.0 - ratio), region.1),
        Direction::Down => (region.0, region.1 * (1.0 - ratio)),
    }
}

fn leaf(ti: usize, pj: usize, pane: &Pane, layout: &Layout, ctx: &PlanContext, setup: Option<&str>) -> Node {
    // Layout-level env first so a pane's own entries win on conflict.
    let mut env = layout.env.clone();
    env.extend(pane.env.clone());
    Node::Pane(PaneSpec {
        handle: pane_handle(ti, pj),
        title: pane.title.clone(),
        command: pane_script(pane, setup, ctx.setup_status_path.as_deref()).map(|s| shell_argv(&s)),
        env,
        cwd: ctx.cwd.clone(),
    })
}

// Build the right-nested tree for one tab's pane list, tracking the region each
// split divides so a fixed cell `size` can be resolved without asking herdr.
fn build_tab_tree(
    ti: usize,
    panes: &[Pane],
    layout: &Layout,
    ctx: &PlanContext,
    setup_command: Option<&str>,
    pj: usize,
    region: (f64, f64),
) -> Node {
    let pane = &panes[pj];
    let setup = if pane.setup { setup_command } else { None };
    let node = leaf(ti, pj, pane, layout, ctx, setup);
    let Some(next) = panes.get(pj + 1) else { return node };

    let direction = next.split.unwrap_or(Direction::Right);
    let ratio = ratio_for(next, region, direction);
    Node::Split {
        direction,
        ratio,
        first: Box::new(node),
        second: Box::new(build_tab_tree(
            ti,
            panes,
            layout,
            ctx,
            setup_command,
            pj + 1,
            remaining(region, direction, ratio),
        )),
    }
}

pub fn build_plan(layout: &Layout, ctx: &PlanContext) -> Plan {
    // Without a known tab area, treat every region as unmeasurable: percentages
    // still work (they're relative), and a cell size falls back to an even split.
    let area = ctx.area.unwrap_or((0.0, 0.0));
    let setup_command = layout.setup.as_ref().map(|s| s.command.as_str());

    let tabs = layout
        .tabs
        .iter()
        .enumerate()
        .map(|(ti, tab)| TabPlan {
            handle: tab_handle(ti),
            title: tab.title.clone(),
            root: build_tab_tree(ti, &tab.panes, layout, ctx, setup_command, 0, area),
        })
        .collect();

    let mut setup = None;
    let mut agents = Vec::new();
    for (ti, tab) in layout.tabs.iter().enumerate() {
        for (pj, pane) in tab.panes.iter().enumerate() {
            if pane.setup {
                if let Some(spec) = &layout.setup {
                    setup = Some(SetupPlan {
                        handle: pane_handle(ti, pj),
                        blocking: spec.blocking,
                        status_path: ctx.setup_status_path.clone(),
                    });
                }
            }
            if let Some(agent) = &pane.agent {
                agents.push(AgentAction {
                    handle: pane_handle(ti, pj),
                    agent: agent.clone(),
                });
            }
        }
    }

    Plan { layout_id: layout.id.clone(), tabs, setup, agents }
}

/// Pane handles in the tree order herdr echoes back, so a response tree can be
/// zipped against the plan to recover each handle's real pane id.
pub fn handles_in_tree_order(node: &Node) -> Vec<&str> {
    match node {
        Node::Pane(spec) => vec![spec.handle.as_str()],
        Node::Split { first, second, .. } => {
            let mut out = handles_in_tree_order(first);
            out.extend(handles_in_tree_order(second));
            out
        }
    }
}

// --- Ratio helpers -----------------------------------------------------------

// Clamp a split ratio into herdr's usable open interval (0, 1). A fixed cell
// size that meets or exceeds the available space would otherwise produce a
// degenerate 0-width pane; clamping keeps both panes visible.
pub fn clamp_ratio(r: f64) -> Option<f64> {
    if !r.is_finite() {
        return None;
    }
    Some(r.clamp(0.01, 0.99))
}

// Resolve the ratio for a split. herdr's ratio is the fraction the FIRST pane
// keeps; the new pane gets the rest. A pane's `size` sizes the NEW pane, so it's
// inverted here:
//   percent p -> new pane is p%      -> first pane keeps (1 - p/100)
//   cells   w -> new pane is w cells -> first pane keeps (1 - w/extent)
// `extent` is the region's size (columns/rows) along the split axis, needed only
// for a cell size. Legacy `ratio` (already the first-pane share) is passed
// through unchanged. Returns None when nothing sizes the split (or a cell size
// can't be converted because the extent is unknown), leaving an even split.
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
        ]
        .join("\n");
        validate_config(&parse_yaml(&text).unwrap()).unwrap()
    }

    fn ctx(cwd: Option<&str>) -> PlanContext {
        PlanContext {
            cwd: cwd.map(String::from),
            area: Some((200.0, 50.0)),
            setup_status_path: Some("/state/setup.status".to_string()),
        }
    }

    fn plan_of(config: &Config, id: &str, ctx: &PlanContext) -> Plan {
        build_plan(find_layout(config, id).unwrap(), ctx)
    }

    fn parse(text: &str) -> Config {
        validate_config(&parse_yaml(text).unwrap()).unwrap()
    }

    const OUTER: &str = "exec \"${SHELL:-/bin/sh}\" -lic ";

    // The script the pane actually runs, unwrapped from its login-shell argv.
    // Asserting on the raw argv would be misleading: the outer wrapper starts
    // with the same `exec "$SHELL" -li` text the hand-back uses.
    fn inner_script(spec: &PaneSpec) -> String {
        let argv = spec.command.as_ref().expect("pane has a command");
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[1], "-c");
        let rest = argv[2].strip_prefix(OUTER).expect("login-shell wrapper");
        let quoted = rest
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .expect("script is one quoted word");
        quoted.replace(r"'\''", "'")
    }

    fn pane_of<'a>(node: &'a Node, handle: &str) -> &'a PaneSpec {
        match node {
            Node::Pane(spec) => {
                assert_eq!(spec.handle, handle, "unexpected leaf");
                spec
            }
            Node::Split { first, second, .. } => {
                fn find<'a>(n: &'a Node, handle: &str) -> Option<&'a PaneSpec> {
                    match n {
                        Node::Pane(s) if s.handle == handle => Some(s),
                        Node::Pane(_) => None,
                        Node::Split { first, second, .. } => {
                            find(first, handle).or_else(|| find(second, handle))
                        }
                    }
                }
                find(first, handle)
                    .or_else(|| find(second, handle))
                    .unwrap_or_else(|| panic!("no pane {}", handle))
            }
        }
    }

    #[test]
    fn each_tab_becomes_one_right_nested_tree() {
        let config = sample_config();
        let plan = plan_of(&config, "web-app", &ctx(Some("/work")));
        assert_eq!(plan.layout_id, "web-app");
        assert_eq!(plan.tabs.len(), 2);
        assert_eq!(plan.tabs[0].handle, ROOT_TAB);
        assert_eq!(plan.tabs[0].title.as_deref(), Some("main"));

        // Two panes -> one split, first = the root pane, second = the new pane.
        match &plan.tabs[0].root {
            Node::Split { direction, first, second, .. } => {
                assert_eq!(*direction, Direction::Right);
                assert!(matches!(**first, Node::Pane(ref s) if s.handle == ROOT_PANE));
                assert!(matches!(**second, Node::Pane(ref s) if s.handle == "t0p1"));
            }
            other => panic!("expected a split, got {:?}", other),
        }
        // horizontal -> down on the second tab
        match &plan.tabs[1].root {
            Node::Split { direction, .. } => assert_eq!(*direction, Direction::Down),
            other => panic!("expected a split, got {:?}", other),
        }
    }

    #[test]
    fn three_panes_nest_so_each_split_divides_the_previous_pane() {
        let config = parse(
            &[
                "layouts:",
                "  - id: three",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: vertical",
                "          - title: c",
                "            split: horizontal",
            ]
            .join("\n"),
        );
        let plan = plan_of(&config, "three", &ctx(None));
        // split(right, a, split(down, b, c))
        match &plan.tabs[0].root {
            Node::Split { direction, first, second, .. } => {
                assert_eq!(*direction, Direction::Right);
                assert!(matches!(**first, Node::Pane(ref s) if s.handle == "t0p0"));
                match &**second {
                    Node::Split { direction, first, second, .. } => {
                        assert_eq!(*direction, Direction::Down);
                        assert!(matches!(**first, Node::Pane(ref s) if s.handle == "t0p1"));
                        assert!(matches!(**second, Node::Pane(ref s) if s.handle == "t0p2"));
                    }
                    other => panic!("expected a nested split, got {:?}", other),
                }
            }
            other => panic!("expected a split, got {:?}", other),
        }
        assert_eq!(handles_in_tree_order(&plan.tabs[0].root), vec!["t0p0", "t0p1", "t0p2"]);
    }

    #[test]
    fn a_cell_size_is_resolved_against_the_tracked_region_not_a_live_query() {
        let config = parse(
            &[
                "layouts:",
                "  - id: sized",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: vertical",
                "            size: 50",
                "          - title: c",
                "            split: vertical",
                "            size: 30",
            ]
            .join("\n"),
        );
        // 200 columns wide: b wants 50 -> a keeps 150/200 = 0.75.
        // The remaining region is 50 columns, so c's 30 -> b keeps 20/50 = 0.4.
        let plan = plan_of(&config, "sized", &ctx(None));
        match &plan.tabs[0].root {
            Node::Split { ratio, second, .. } => {
                assert_eq!(*ratio, 0.75);
                match &**second {
                    Node::Split { ratio, .. } => assert!((ratio - 0.4).abs() < 1e-9, "{}", ratio),
                    other => panic!("expected a nested split, got {:?}", other),
                }
            }
            other => panic!("expected a split, got {:?}", other),
        }
    }

    #[test]
    fn a_down_split_measures_rows_and_leaves_the_width_untouched() {
        let config = parse(
            &[
                "layouts:",
                "  - id: rows",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: horizontal",
                "            size: 10",
            ]
            .join("\n"),
        );
        // 50 rows tall: b wants 10 -> a keeps 40/50 = 0.8.
        let plan = plan_of(&config, "rows", &ctx(None));
        match &plan.tabs[0].root {
            Node::Split { direction, ratio, .. } => {
                assert_eq!(*direction, Direction::Down);
                assert_eq!(*ratio, 0.8);
            }
            other => panic!("expected a split, got {:?}", other),
        }
    }

    #[test]
    fn an_unsized_split_uses_an_even_ratio() {
        let config = sample_config();
        let plan = plan_of(&config, "web-app", &ctx(None));
        match &plan.tabs[0].root {
            Node::Split { ratio, .. } => assert_eq!(*ratio, DEFAULT_RATIO),
            other => panic!("expected a split, got {:?}", other),
        }
    }

    #[test]
    fn a_cell_size_with_no_known_area_falls_back_to_an_even_split() {
        let config = parse(
            &[
                "layouts:",
                "  - id: sized",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: vertical",
                "            size: 50",
            ]
            .join("\n"),
        );
        let plan = build_plan(
            find_layout(&config, "sized").unwrap(),
            &PlanContext { area: None, ..Default::default() },
        );
        match &plan.tabs[0].root {
            Node::Split { ratio, .. } => assert_eq!(*ratio, DEFAULT_RATIO),
            other => panic!("expected a split, got {:?}", other),
        }
    }

    #[test]
    fn a_percentage_size_needs_no_area_at_all() {
        let config = parse(
            &[
                "layouts:",
                "  - id: pct",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: vertical",
                "            size: \"30%\"",
            ]
            .join("\n"),
        );
        let plan = build_plan(
            find_layout(&config, "pct").unwrap(),
            &PlanContext { area: None, ..Default::default() },
        );
        match &plan.tabs[0].root {
            Node::Split { ratio, .. } => assert!((ratio - 0.7).abs() < 1e-9, "{}", ratio),
            other => panic!("expected a split, got {:?}", other),
        }
    }

    #[test]
    fn a_command_pane_runs_in_the_users_login_shell_and_hands_the_pane_back() {
        let config = sample_config();
        let plan = plan_of(&config, "web-app", &ctx(Some("/work")));
        let editor = pane_of(&plan.tabs[0].root, "t0p1");
        // The user's own interactive login shell, so .zshrc/.bash_profile PATH
        // setup (mise, asdf, nvm) applies exactly as it did when the command was
        // typed into the pane. inner_script asserts the wrapper shape.
        let script = inner_script(editor);
        assert_eq!(script, format!("nvim; {}", HAND_BACK));
        assert_eq!(editor.cwd.as_deref(), Some("/work"));
        assert_eq!(editor.title.as_deref(), Some("editor"));
    }

    #[test]
    fn persist_false_lets_the_pane_close_when_the_command_exits() {
        let config = parse(
            &[
                "layouts:",
                "  - id: p",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: once",
                "            command: just build",
                "            persist: false",
            ]
            .join("\n"),
        );
        let plan = plan_of(&config, "p", &ctx(None));
        let script = inner_script(pane_of(&plan.tabs[0].root, "t0p0"));
        assert_eq!(script, "just build", "nothing follows, so the pane exits with it");
    }

    #[test]
    fn a_plain_pane_gets_no_command_at_all() {
        let config = sample_config();
        let plan = plan_of(&config, "web-app", &ctx(None));
        assert_eq!(pane_of(&plan.tabs[1].root, "t1p0").command, None);
    }

    #[test]
    fn the_setup_pane_records_its_exit_status_before_its_own_command() {
        let config = sample_config();
        let plan = plan_of(&config, "web-app", &ctx(None));
        let script = inner_script(pane_of(&plan.tabs[0].root, ROOT_PANE));
        let setup_at = script.find("mise run setup").unwrap();
        let status_at = script.find("/state/setup.status").unwrap();
        let command_at = script.find("opencode").unwrap();
        assert!(setup_at < status_at, "status is captured right after setup");
        assert!(status_at < command_at, "the pane's own command runs after setup");

        let setup = plan.setup.unwrap();
        assert_eq!(setup.handle, ROOT_PANE);
        assert!(setup.blocking);
        assert_eq!(setup.status_path.as_deref(), Some("/state/setup.status"));
    }

    #[test]
    fn a_layout_without_setup_plans_no_setup_step() {
        let config = parse(
            &["layouts:", "  - id: bare", "    tabs:", "      - title: t", "        panes:", "          - title: a"]
                .join("\n"),
        );
        assert_eq!(plan_of(&config, "bare", &ctx(None)).setup, None);
    }

    #[test]
    fn layout_env_is_merged_into_every_pane_and_pane_env_wins() {
        let config = parse(
            &[
                "layouts:",
                "  - id: e",
                "    env:",
                "      SHARED: layout",
                "      ONLY_LAYOUT: yes",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "            env:",
                "              SHARED: pane",
                "              PORT: 3000",
            ]
            .join("\n"),
        );
        let plan = plan_of(&config, "e", &ctx(None));
        let env = &pane_of(&plan.tabs[0].root, "t0p0").env;
        assert_eq!(env.get("SHARED").map(String::as_str), Some("pane"));
        assert_eq!(env.get("ONLY_LAYOUT").map(String::as_str), Some("yes"));
        assert_eq!(env.get("PORT").map(String::as_str), Some("3000"));
    }

    #[test]
    fn agent_panes_are_planned_as_post_apply_actions_with_no_pane_command() {
        let config = parse(
            &[
                "layouts:",
                "  - id: a",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: agent",
                "            agent: claude",
                "            agentName: main",
                "            prompt: do the thing",
                "          - title: shell",
                "            split: vertical",
            ]
            .join("\n"),
        );
        let plan = plan_of(&config, "a", &ctx(None));
        // The pane is created as a plain shell; `herdr agent start` fills it.
        assert_eq!(pane_of(&plan.tabs[0].root, "t0p0").command, None);
        assert_eq!(plan.agents.len(), 1);
        assert_eq!(plan.agents[0].handle, "t0p0");
        assert_eq!(plan.agents[0].agent.kind, "claude");
        assert_eq!(plan.agents[0].agent.name.as_deref(), Some("main"));
        assert_eq!(plan.agents[0].agent.prompt.as_deref(), Some("do the thing"));
    }

    #[test]
    fn an_agent_pane_that_also_runs_setup_still_ends_at_a_prompt() {
        let config = parse(
            &[
                "layouts:",
                "  - id: a",
                "    setup:",
                "      command: npm ci",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: agent",
                "            agent: claude",
                "            setup: true",
            ]
            .join("\n"),
        );
        let plan = plan_of(&config, "a", &ctx(None));
        let script = inner_script(pane_of(&plan.tabs[0].root, "t0p0"));
        assert!(script.starts_with("npm ci; "));
        // `agent start` requires the pane to be back at its interactive prompt.
        assert!(script.ends_with(HAND_BACK), "{}", script);
    }

    #[test]
    fn single_pane_layouts_produce_a_bare_leaf() {
        let config = parse(
            &[
                "layouts:",
                "  - id: solo",
                "    tabs:",
                "      - title: only",
                "        panes:",
                "          - title: shell",
                "            command: htop",
            ]
            .join("\n"),
        );
        let plan = plan_of(&config, "solo", &ctx(None));
        assert_eq!(plan.tabs.len(), 1);
        assert!(matches!(plan.tabs[0].root, Node::Pane(_)));
        assert_eq!(handles_in_tree_order(&plan.tabs[0].root), vec![ROOT_PANE]);
    }

    #[test]
    fn single_quoting_survives_a_command_containing_quotes() {
        let script = "echo 'it''s fine'";
        let argv = shell_argv(script);
        // The whole script is one shell word; embedded quotes are escaped, not lost.
        assert!(argv[2].contains(r"'\''"), "{}", argv[2]);
        assert!(argv[2].starts_with("exec \"${SHELL:-/bin/sh}\" -lic '"));
        assert!(argv[2].ends_with('\''));
    }

    #[test]
    fn clamp_ratio_keeps_ratios_inside_herdrs_open_interval() {
        assert_eq!(clamp_ratio(0.5), Some(0.5));
        assert_eq!(clamp_ratio(-3.0), Some(0.01)); // a cell size >= the whole pane
        assert_eq!(clamp_ratio(5.0), Some(0.99));
        assert_eq!(clamp_ratio(f64::NAN), None);
    }

    #[test]
    fn split_ratio_arg_inverts_a_pane_size_into_the_first_panes_share() {
        assert_eq!(split_ratio_arg(None, None, None), None);
        assert_eq!(split_ratio_arg(Some(0.3), None, None), Some(0.3));
        assert_eq!(split_ratio_arg(None, Some(&Size::Percent(30.0)), None), Some(0.7));
        assert_eq!(split_ratio_arg(None, Some(&Size::Cells(50)), Some(200.0)), Some(0.75));
        assert_eq!(split_ratio_arg(None, Some(&Size::Cells(300)), Some(200.0)), Some(0.01));
        assert_eq!(split_ratio_arg(None, Some(&Size::Cells(40)), None), None);
    }
}
