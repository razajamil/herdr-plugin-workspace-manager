// Config loading, validation, and workspace/layout matching.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::env::Env;
use crate::yaml::parse_yaml;

pub const PLUGIN_ID: &str = "herdr-plugin-workspace-manager";

#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub command: String,
    pub blocking: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Right,
    Down,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::Down => "down",
        }
    }
}

// A pane `size` (the extent of THIS pane along the split axis: columns for a
// vertical/right split, rows for a horizontal/down split), normalized from the
// three accepted config forms:
//   "30%" (string)      -> Percent(30.0)
//   0.3   (0 < n < 1)   -> Percent(30.0)   (a fraction)
//   40    (integer >=1) -> Cells(40)        (fixed columns/rows)
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    Percent(f64),
    Cells(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub title: Option<String>,
    pub command: Option<String>,
    pub setup: bool,
    pub split: Option<Direction>,
    pub ratio: Option<f64>,
    pub size: Option<Size>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub title: Option<String>,
    pub panes: Vec<Pane>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub id: String,
    pub setup: Option<Setup>,
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchRule {
    pub title: Option<String>,
    pub worktree_pattern: String,
    pub layout: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub repo: Option<String>,
    pub path: Option<String>,
    pub default_layout: Option<String>,
    pub layout_matching: Vec<MatchRule>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub layouts: Vec<Layout>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Clone, Debug, Default)]
pub struct MatchTarget {
    pub checkout_path: Option<String>,
    pub repo_root: Option<String>,
    pub repo_name: Option<String>,
    pub branch: Option<String>,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn expand_home(p: &str) -> String {
    if p == "~" {
        return home_dir().to_string_lossy().into_owned();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home_dir().join(rest).to_string_lossy().into_owned();
    }
    p.to_string()
}

// Node's path.resolve: absolutize against the cwd, then normalize `.`/`..`
// lexically (never touching the filesystem).
pub fn resolve_path(p: &str) -> String {
    let path = Path::new(p);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.to_string_lossy().into_owned()
}

// Candidate config file locations, most-preferred first. The herdr-managed
// config directory (HERDR_PLUGIN_CONFIG_DIR) is canonical; the ~/.herdr path is
// supported as a convenience fallback. HERDR_WSM_CONFIG overrides everything
// (used by tests).
pub fn config_candidates(env: &Env) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(explicit) = env.get("HERDR_WSM_CONFIG") {
        candidates.push(PathBuf::from(explicit));
    }
    if let Some(config_dir) = env.get("HERDR_PLUGIN_CONFIG_DIR") {
        candidates.push(Path::new(config_dir).join("config.yml"));
        candidates.push(Path::new(config_dir).join("config.yaml"));
    }
    let fallback_dir = home_dir().join(".herdr").join("plugins").join(PLUGIN_ID);
    candidates.push(fallback_dir.join("config.yml"));
    candidates.push(fallback_dir.join("config.yaml"));
    candidates
}

pub fn resolve_config_path(env: &Env) -> Option<PathBuf> {
    config_candidates(env).into_iter().find(|c| c.exists())
}

// --- Value helpers mirroring the JS dynamic checks -------------------------

/// A field that is absent or explicitly null, per JS `value != null`.
fn opt<'a>(raw: &'a Value, key: &str) -> Option<&'a Value> {
    match raw.get(key) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}

/// JS truthiness (`Boolean(v)`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// JS `Number(v)` for the value shapes YAML can produce.
fn js_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        _ => f64::NAN,
    }
}

/// JS `String(v)` / template interpolation for error messages.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn as_string(value: Option<&Value>, what: &str) -> Result<String, String> {
    match value {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        _ => Err(format!("{} must be a non-empty string", what)),
    }
}

fn is_mapping(v: &Value) -> bool {
    v.is_object()
}

// --- Glob matching ----------------------------------------------------------

// Match a glob pattern (workspaces[].layoutMatching[].worktreePattern) against
// a worktree's branch name. Only `*` (any run of characters, including "/") and
// `?` (a single character) are special; every other character is matched
// literally. The match is full-string, so `fix/rwr-*` matches `fix/rwr-123-foo`
// but not `hotfix/rwr-123-foo`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || (p[pi] != '*' && p[pi] == t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

impl MatchRule {
    pub fn matches(&self, branch: &str) -> bool {
        glob_match(&self.worktree_pattern, branch)
    }
}

// --- Normalization -----------------------------------------------------------

fn normalize_setup(raw: Option<&Value>, layout_id: &str) -> Result<Option<Setup>, String> {
    let Some(raw) = raw else { return Ok(None) };
    if !is_mapping(raw) {
        return Err(format!("layout \"{}\": setup must be a mapping", layout_id));
    }
    let command = as_string(raw.get("command"), &format!("layout \"{}\": setup.command", layout_id))?;
    let blocking = raw.get("blocking").map(truthy).unwrap_or(false);
    Ok(Some(Setup { command, blocking }))
}

fn size_form_error(where_: &str) -> String {
    format!(
        "{}: must be a number of cells (e.g. 40), a fraction (e.g. 0.3), or a percentage (e.g. \"30%\")",
        where_
    )
}

fn numeric_size(n: f64, raw: &Value, where_: &str) -> Result<Size, String> {
    if !n.is_finite() || n <= 0.0 {
        return Err(format!(
            "{}: must be a positive number (got {})",
            where_,
            serde_json::to_string(raw).unwrap_or_else(|_| js_string(raw))
        ));
    }
    if n < 1.0 {
        return Ok(Size::Percent(n * 100.0)); // fraction of the axis
    }
    if n.fract() != 0.0 {
        return Err(format!(
            "{}: a fixed cell count must be a whole number (got {}); use a value below 1 or an \"N%\" string for a proportion",
            where_, n
        ));
    }
    Ok(Size::Cells(n as u64))
}

fn normalize_size(raw: &Value, where_: &str) -> Result<Size, String> {
    match raw {
        Value::String(raw_s) => {
            let s = raw_s.trim();
            if let Some(prefix) = s.strip_suffix('%') {
                let pct = js_number(&Value::String(prefix.trim().to_string()));
                if !pct.is_finite() || pct <= 0.0 || pct >= 100.0 {
                    return Err(format!(
                        "{}: percentage must be between 0 and 100 (got \"{}\")",
                        where_, raw_s
                    ));
                }
                return Ok(Size::Percent(pct));
            }
            let n = js_number(&Value::String(s.to_string()));
            if s.is_empty() || !n.is_finite() {
                return Err(size_form_error(where_));
            }
            numeric_size(n, raw, where_)
        }
        Value::Number(n) => numeric_size(n.as_f64().unwrap_or(f64::NAN), raw, where_),
        _ => Err(size_form_error(where_)),
    }
}

fn normalize_pane(raw: &Value, layout_id: &str, tab_title: &str, index: usize) -> Result<Pane, String> {
    if !is_mapping(raw) {
        return Err(format!(
            "layout \"{}\", tab \"{}\": pane {} must be a mapping",
            layout_id, tab_title, index
        ));
    }
    let mut pane = Pane {
        title: opt(raw, "title").map(|v| as_string(Some(v), "pane title")).transpose()?,
        command: opt(raw, "command").map(|v| as_string(Some(v), "pane command")).transpose()?,
        setup: raw.get("setup").map(truthy).unwrap_or(false),
        split: None,
        ratio: None,
        size: None,
    };
    if let Some(split_raw) = opt(raw, "split") {
        pane.split = match js_string(split_raw).to_lowercase().as_str() {
            "vertical" | "right" => Some(Direction::Right),
            "horizontal" | "down" => Some(Direction::Down),
            _ => {
                return Err(format!(
                    "layout \"{}\", tab \"{}\": unsupported split \"{}\" (use vertical, horizontal, right, or down)",
                    layout_id,
                    tab_title,
                    js_string(split_raw)
                ))
            }
        };
    }
    if opt(raw, "ratio").is_some() && opt(raw, "size").is_some() {
        return Err(format!(
            "layout \"{}\", tab \"{}\": set either \"ratio\" or \"size\", not both",
            layout_id, tab_title
        ));
    }
    if let Some(ratio_raw) = opt(raw, "ratio") {
        let ratio = js_number(ratio_raw);
        if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
            return Err(format!(
                "layout \"{}\", tab \"{}\": ratio must be a number between 0 and 1",
                layout_id, tab_title
            ));
        }
        pane.ratio = Some(ratio);
    }
    if let Some(size_raw) = opt(raw, "size") {
        pane.size = Some(normalize_size(
            size_raw,
            &format!("layout \"{}\", tab \"{}\": size", layout_id, tab_title),
        )?);
    }
    Ok(pane)
}

fn normalize_tab(raw: &Value, layout_id: &str, index: usize) -> Result<Tab, String> {
    if !is_mapping(raw) {
        return Err(format!("layout \"{}\": tab {} must be a mapping", layout_id, index));
    }
    let title = opt(raw, "title").map(|v| as_string(Some(v), "tab title")).transpose()?;
    let tab_label = title.clone().unwrap_or_else(|| index.to_string());
    let panes_raw = match raw.get("panes") {
        Some(Value::Array(panes)) if !panes.is_empty() => panes,
        _ => {
            return Err(format!(
                "layout \"{}\", tab \"{}\": needs at least one pane",
                layout_id, tab_label
            ))
        }
    };
    let panes = panes_raw
        .iter()
        .enumerate()
        .map(|(i, p)| normalize_pane(p, layout_id, &tab_label, i))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Tab { title, panes })
}

fn normalize_layout(raw: &Value, index: usize) -> Result<Layout, String> {
    if !is_mapping(raw) {
        return Err(format!("layout {} must be a mapping", index));
    }
    let id = as_string(raw.get("id"), &format!("layout {}: id", index))?;
    let setup = normalize_setup(opt(raw, "setup"), &id)?;
    let tabs_raw = match raw.get("tabs") {
        Some(Value::Array(tabs)) if !tabs.is_empty() => tabs,
        _ => return Err(format!("layout \"{}\": needs at least one tab", id)),
    };
    let tabs = tabs_raw
        .iter()
        .enumerate()
        .map(|(i, t)| normalize_tab(t, &id, i))
        .collect::<Result<Vec<_>, _>>()?;

    let setup_panes = tabs.iter().flat_map(|t| &t.panes).filter(|p| p.setup).count();
    if setup_panes > 1 {
        return Err(format!(
            "layout \"{}\": only one pane may set \"setup: true\" (found {})",
            id, setup_panes
        ));
    }
    if setup.is_some() && setup_panes == 0 {
        return Err(format!(
            "layout \"{}\": defines a setup command but no pane has \"setup: true\"",
            id
        ));
    }
    Ok(Layout { id, setup, tabs })
}

fn normalize_match_rule(raw: &Value, ws_index: usize, i: usize) -> Result<MatchRule, String> {
    if !is_mapping(raw) {
        return Err(format!("workspace {}: layoutMatching[{}] must be a mapping", ws_index, i));
    }
    let title = opt(raw, "title")
        .map(|v| as_string(Some(v), &format!("workspace {}: layoutMatching[{}].title", ws_index, i)))
        .transpose()?;
    let worktree_pattern = as_string(
        raw.get("worktreePattern"),
        &format!("workspace {}: layoutMatching[{}].worktreePattern", ws_index, i),
    )?;
    let layout = as_string(
        raw.get("layout"),
        &format!("workspace {}: layoutMatching[{}].layout", ws_index, i),
    )?;
    Ok(MatchRule { title, worktree_pattern, layout })
}

fn normalize_workspace(raw: &Value, index: usize) -> Result<Workspace, String> {
    if !is_mapping(raw) {
        return Err(format!("workspace {} must be a mapping", index));
    }
    let repo = opt(raw, "repo").map(|v| as_string(Some(v), &format!("workspace {}: repo", index))).transpose()?;
    let ws_path = opt(raw, "path").map(|v| as_string(Some(v), &format!("workspace {}: path", index))).transpose()?;
    if repo.is_none() && ws_path.is_none() {
        return Err(format!(
            "workspace {} needs \"repo\" (repo root/name — recommended) or \"path\" (worktree dir prefix)",
            index
        ));
    }
    let default_layout = opt(raw, "defaultLayout")
        .map(|v| as_string(Some(v), &format!("workspace {}: defaultLayout", index)))
        .transpose()?;
    let layout_matching = match raw.get("layoutMatching") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(rules)) => rules
            .iter()
            .enumerate()
            .map(|(i, r)| normalize_match_rule(r, index, i))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(format!("workspace {}: layoutMatching must be a list", index)),
    };
    Ok(Workspace { repo, path: ws_path, default_layout, layout_matching })
}

pub fn validate_config(raw: &Value) -> Result<Config, String> {
    if raw.is_null() {
        return Ok(Config::default());
    }
    if !is_mapping(raw) {
        return Err("config root must be a mapping".to_string());
    }
    let layouts = match raw.get("layouts") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, l)| normalize_layout(l, i))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("layouts must be a list".to_string()),
    };

    let mut seen = std::collections::BTreeSet::new();
    for layout in &layouts {
        if !seen.insert(layout.id.clone()) {
            return Err(format!("duplicate layout id \"{}\"", layout.id));
        }
    }

    let workspaces = match raw.get("workspaces") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, w)| normalize_workspace(w, i))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("workspaces must be a list".to_string()),
    };

    // Cross-check: every layout referenced by a workspace must exist.
    for ws in &workspaces {
        let label = ws.repo.as_deref().or(ws.path.as_deref()).unwrap_or("");
        if let Some(default) = &ws.default_layout {
            if !seen.contains(default) {
                return Err(format!(
                    "workspace \"{}\" references unknown layout \"{}\"",
                    label, default
                ));
            }
        }
        for rule in &ws.layout_matching {
            if !seen.contains(&rule.layout) {
                return Err(format!(
                    "workspace \"{}\" layoutMatching references unknown layout \"{}\"",
                    label, rule.layout
                ));
            }
        }
    }

    Ok(Config { layouts, workspaces })
}

pub fn load_config(env: &Env) -> Result<(Option<PathBuf>, Config), String> {
    let Some(file) = resolve_config_path(env) else {
        return Ok((None, Config::default()));
    };
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("cannot read config file {}: {}", file.display(), e))?;
    let config = validate_config(&parse_yaml(&text)?)?;
    Ok((Some(file), config))
}

pub fn find_layout<'a>(config: &'a Config, id: &str) -> Option<&'a Layout> {
    config.layouts.iter().find(|l| l.id == id)
}

// Is `checkout_path` inside (or equal to) the configured workspace `ws_path`?
fn is_under(checkout_path: &str, ws_path: &str) -> bool {
    let a = resolve_path(&expand_home(checkout_path));
    let b = resolve_path(&expand_home(ws_path));
    a == b || a.starts_with(&format!("{}/", b))
}

// Score how specifically a workspace rule matches a target, or None for no
// match. Higher score wins. `repo` matches (by repo root or repo name) are
// preferred over `path` prefix matches, and longer path prefixes win among
// path matches.
fn match_score(ws: &Workspace, target: &MatchTarget) -> Option<i64> {
    if let Some(repo) = &ws.repo {
        let repo_resolved = resolve_path(&expand_home(repo));
        if let Some(root) = &target.repo_root {
            if resolve_path(&expand_home(root)) == repo_resolved {
                return Some(1_000_000);
            }
        }
        if target.repo_name.as_deref() == Some(repo.as_str()) {
            return Some(900_000);
        }
    }
    if let (Some(ws_path), Some(checkout)) = (&ws.path, &target.checkout_path) {
        if is_under(checkout, ws_path) {
            return Some(resolve_path(&expand_home(ws_path)).len() as i64);
        }
    }
    None
}

// Which layout does a matched workspace apply to a worktree on `branch`?
// layoutMatching rules are tried in the order the user wrote them; the first
// whose glob matches the branch (and whose layout exists) wins. When no rule
// matches — or there's no branch to match against (e.g. a detached HEAD) — the
// workspace's defaultLayout is used. Returns the layout, or None if the
// workspace yields no applicable layout.
fn resolve_layout_for<'a>(config: &'a Config, ws: &Workspace, branch: Option<&str>) -> Option<&'a Layout> {
    if let Some(branch) = branch {
        for rule in &ws.layout_matching {
            if rule.matches(branch) {
                if let Some(layout) = find_layout(config, &rule.layout) {
                    return Some(layout);
                }
            }
        }
    }
    if let Some(default) = &ws.default_layout {
        return find_layout(config, default);
    }
    None
}

// Find the layout to apply for a freshly created worktree. The most specific
// workspace (by repo/path) that actually yields a layout wins; within it,
// layoutMatching branch patterns are tried before defaultLayout.
pub fn match_workspace_layout<'a>(
    config: &'a Config,
    target: &MatchTarget,
) -> Option<(&'a Workspace, &'a Layout)> {
    let mut best: Option<(&Workspace, &Layout)> = None;
    let mut best_score = -1i64;
    for ws in &config.workspaces {
        let Some(score) = match_score(ws, target) else { continue };
        if score <= best_score {
            continue;
        }
        if let Some(layout) = resolve_layout_for(config, ws, target.branch.as_deref()) {
            best = Some((ws, layout));
            best_score = score;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        validate_config(&parse_yaml(text).unwrap()).unwrap()
    }

    fn parse_err(text: &str) -> String {
        validate_config(&parse_yaml(text).unwrap()).unwrap_err()
    }

    fn home(rest: &str) -> String {
        home_dir().join(rest).to_string_lossy().into_owned()
    }

    fn sample() -> String {
        [
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
            "workspaces:",
            "  - path: ~/.herdr/worktrees/web-app",
            "    defaultLayout: web-app",
        ]
        .join("\n")
    }

    fn target(checkout: &str) -> MatchTarget {
        MatchTarget { checkout_path: Some(checkout.to_string()), ..Default::default() }
    }

    #[test]
    fn normalizes_the_sample_config_and_maps_split_aliases() {
        let config = parse(&sample());
        let layout = find_layout(&config, "web-app").unwrap();
        assert_eq!(
            layout.setup,
            Some(Setup { command: "mise run setup".to_string(), blocking: true })
        );
        // vertical -> right, horizontal -> down
        assert_eq!(layout.tabs[0].panes[1].split, Some(Direction::Right));
        assert_eq!(layout.tabs[1].panes[1].split, Some(Direction::Down));
        // first panes have no split
        assert_eq!(layout.tabs[0].panes[0].split, None);
        assert!(layout.tabs[0].panes[0].setup);
    }

    #[test]
    fn accepts_literal_right_down_and_validates_ratio() {
        let config = parse(
            &[
                "layouts:",
                "  - id: x",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: down",
                "            ratio: 0.3",
            ]
            .join("\n"),
        );
        assert_eq!(config.layouts[0].tabs[0].panes[1].split, Some(Direction::Down));
        assert_eq!(config.layouts[0].tabs[0].panes[1].ratio, Some(0.3));
    }

    #[test]
    fn parses_pane_size_in_cells_percent_and_fraction_forms() {
        let config = parse(
            &[
                "layouts:",
                "  - id: x",
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
            .join("\n"),
        );
        let panes = &config.layouts[0].tabs[0].panes;
        assert_eq!(panes[0].size, None);
        assert_eq!(panes[1].size, Some(Size::Cells(40)));
        assert_eq!(panes[2].size, Some(Size::Percent(30.0)));
        assert_eq!(panes[3].size, Some(Size::Percent(25.0)));
    }

    #[test]
    fn rejects_setting_both_ratio_and_size_on_a_pane() {
        let text = [
            "layouts:",
            "  - id: x",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "          - title: b",
            "            split: vertical",
            "            ratio: 0.5",
            "            size: 40",
        ]
        .join("\n");
        assert!(parse_err(&text).contains("not both"));
    }

    #[test]
    fn rejects_out_of_range_percent_and_fractional_cell_count() {
        let sized = |v: &str| {
            [
                "layouts:",
                "  - id: x",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "          - title: b",
                "            split: vertical",
                &format!("            size: {}", v),
            ]
            .join("\n")
        };
        assert!(validate_config(&parse_yaml(&sized("\"150%\"")).unwrap()).is_err());
        assert!(validate_config(&parse_yaml(&sized("\"0%\"")).unwrap()).is_err());
        assert!(validate_config(&parse_yaml(&sized("40.5")).unwrap()).is_err()); // fixed cells must be whole
        assert!(validate_config(&parse_yaml(&sized("0")).unwrap()).is_err());
        assert!(validate_config(&parse_yaml(&sized("\"wide\"")).unwrap()).is_err());
    }

    #[test]
    fn rejects_two_setup_panes() {
        let text = [
            "layouts:",
            "  - id: x",
            "    setup:",
            "      command: echo hi",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "            setup: true",
            "          - title: b",
            "            setup: true",
        ]
        .join("\n");
        assert!(parse_err(&text).contains("only one pane"));
    }

    #[test]
    fn rejects_setup_command_with_no_setup_pane() {
        let text = [
            "layouts:",
            "  - id: x",
            "    setup:",
            "      command: echo hi",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
        ]
        .join("\n");
        assert!(parse_err(&text).contains("no pane has"));
    }

    #[test]
    fn rejects_duplicate_layout_ids() {
        let dup = [
            "layouts:",
            "  - id: dup",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "  - id: dup",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
        ]
        .join("\n");
        assert!(parse_err(&dup).contains("duplicate layout id"));
    }

    #[test]
    fn rejects_a_layout_with_no_tabs() {
        let text = ["layouts:", "  - id: empty"].join("\n");
        assert!(parse_err(&text).contains("needs at least one tab"));
    }

    #[test]
    fn rejects_workspace_referencing_unknown_layout() {
        let text = [
            "layouts:",
            "  - id: known",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "workspaces:",
            "  - path: ~/x",
            "    defaultLayout: nope",
        ]
        .join("\n");
        assert!(parse_err(&text).contains("unknown layout"));
    }

    #[test]
    fn expand_home_expands_tilde() {
        assert_eq!(expand_home("~/foo"), home("foo"));
        assert_eq!(expand_home("/abs"), "/abs");
    }

    #[test]
    fn match_workspace_layout_matches_paths_under_the_workspace_root() {
        let config = parse(&sample());
        let root = home(".herdr/worktrees/web-app");
        let m = match_workspace_layout(&config, &target(&format!("{}/my-branch", root))).unwrap();
        assert_eq!(m.1.id, "web-app");
        // exact path also matches
        assert!(match_workspace_layout(&config, &target(&root)).is_some());
        // unrelated path does not
        assert!(match_workspace_layout(&config, &target("/tmp/other")).is_none());
    }

    #[test]
    fn match_workspace_layout_prefers_the_most_specific_longest_path() {
        let cfg = parse(
            &[
                "layouts:",
                "  - id: parent",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "  - id: child",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "workspaces:",
                "  - path: /repos",
                "    defaultLayout: parent",
                "  - path: /repos/special",
                "    defaultLayout: child",
            ]
            .join("\n"),
        );
        let m = match_workspace_layout(&cfg, &target("/repos/special/branch")).unwrap();
        assert_eq!(m.1.id, "child");
    }

    fn repo_cfg() -> String {
        [
            "layouts:",
            "  - id: rf",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "workspaces:",
            "  - repo: ~/dev/web-app",
            "    defaultLayout: rf",
        ]
        .join("\n")
    }

    #[test]
    fn matches_a_worktree_by_repo_root() {
        let cfg = parse(&repo_cfg());
        let m = match_workspace_layout(
            &cfg,
            &MatchTarget {
                checkout_path: Some(home(".herdr/worktrees/web-app/some-branch")),
                repo_root: Some(home("dev/web-app")),
                repo_name: Some("web-app".to_string()),
                branch: None,
            },
        )
        .unwrap();
        assert_eq!(m.1.id, "rf");
    }

    #[test]
    fn matches_a_worktree_by_bare_repo_name() {
        let cfg = parse(&repo_cfg().replace("~/dev/web-app", "web-app"));
        let m = match_workspace_layout(
            &cfg,
            &MatchTarget {
                checkout_path: Some("/anywhere/else".to_string()),
                repo_root: Some("/some/other/path/web-app".to_string()),
                repo_name: Some("web-app".to_string()),
                branch: None,
            },
        )
        .unwrap();
        assert_eq!(m.1.id, "rf");
    }

    #[test]
    fn a_non_matching_repo_returns_none() {
        let cfg = parse(&repo_cfg());
        let m = match_workspace_layout(
            &cfg,
            &MatchTarget {
                checkout_path: Some("/x".to_string()),
                repo_root: Some("/Users/x/dev/other-repo".to_string()),
                repo_name: Some("other-repo".to_string()),
                branch: None,
            },
        );
        assert!(m.is_none());
    }

    #[test]
    fn repo_match_wins_over_a_path_match() {
        let cfg = parse(
            &[
                "layouts:",
                "  - id: byrepo",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "  - id: bypath",
                "    tabs:",
                "      - title: t",
                "        panes:",
                "          - title: a",
                "workspaces:",
                "  - path: /wt/web-app",
                "    defaultLayout: bypath",
                "  - repo: /dev/web-app",
                "    defaultLayout: byrepo",
            ]
            .join("\n"),
        );
        let m = match_workspace_layout(
            &cfg,
            &MatchTarget {
                checkout_path: Some("/wt/web-app/branch".to_string()),
                repo_root: Some("/dev/web-app".to_string()),
                repo_name: Some("web-app".to_string()),
                branch: None,
            },
        )
        .unwrap();
        assert_eq!(m.1.id, "byrepo");
    }

    #[test]
    fn rejects_a_workspace_with_neither_repo_nor_path() {
        let text = [
            "layouts:",
            "  - id: x",
            "    tabs:",
            "      - title: t",
            "        panes:",
            "          - title: a",
            "workspaces:",
            "  - defaultLayout: x",
        ]
        .join("\n");
        assert!(parse_err(&text).contains("needs \"repo\""));
    }

    // --- layoutMatching: branch-pattern -> layout ---------------------------

    // A minimal valid layout block in block YAML (the parser has no flow support).
    fn layout_y(id: &str) -> String {
        [
            &format!("  - id: {}", id),
            "    tabs:",
            "      - panes:",
            "          - title: a",
        ]
        .join("\n")
    }

    #[test]
    fn glob_matches_the_whole_branch_star_spans_any_chars_question_one() {
        assert!(glob_match("fix/rwr-*", "fix/rwr-142-login"));
        assert!(glob_match("fix/rwr-*", "fix/rwr-"));
        assert!(!glob_match("fix/rwr-*", "hotfix/rwr-1"), "not a prefix match");
        assert!(!glob_match("fix/rwr-*", "fix/rwr"), "full-string anchored");
        // regex metacharacters in the glob are matched literally
        assert!(glob_match("a.b+c", "a.b+c"));
        assert!(!glob_match("a.b", "axb"));
        // ? matches exactly one character
        assert!(glob_match("v?", "v2"));
        assert!(!glob_match("v?", "v12"));
    }

    // repo `rf` with three layouts: default, fix, docs.
    fn match_cfg() -> String {
        [
            "layouts:".to_string(),
            layout_y("rf"),
            layout_y("rf-fix"),
            layout_y("rf-docs"),
            "workspaces:".to_string(),
            "  - repo: ~/dev/web-app".to_string(),
            "    defaultLayout: rf".to_string(),
            "    layoutMatching:".to_string(),
            "      - title: Fix".to_string(),
            "        worktreePattern: fix/rwr-*".to_string(),
            "        layout: rf-fix".to_string(),
            "      - title: Docs".to_string(),
            "        worktreePattern: docs/*".to_string(),
            "        layout: rf-docs".to_string(),
        ]
        .join("\n")
    }

    fn match_branch(cfg_text: &str, branch: Option<&str>) -> Option<String> {
        let cfg = parse(cfg_text);
        match_workspace_layout(
            &cfg,
            &MatchTarget {
                checkout_path: Some(home(".herdr/worktrees/web-app/wt")),
                repo_root: Some(home("dev/web-app")),
                repo_name: Some("web-app".to_string()),
                branch: branch.map(String::from),
            },
        )
        .map(|(_, l)| l.id.clone())
    }

    #[test]
    fn layout_matching_applies_the_first_pattern_that_matches_the_branch() {
        assert_eq!(match_branch(&match_cfg(), Some("fix/rwr-9-login")).unwrap(), "rf-fix");
        assert_eq!(match_branch(&match_cfg(), Some("docs/architecture")).unwrap(), "rf-docs");
    }

    #[test]
    fn layout_matching_falls_back_to_default_layout_when_nothing_matches() {
        assert_eq!(match_branch(&match_cfg(), Some("main")).unwrap(), "rf");
    }

    #[test]
    fn a_worktree_with_no_branch_uses_default_layout() {
        assert_eq!(match_branch(&match_cfg(), None).unwrap(), "rf");
    }

    #[test]
    fn layout_matching_honors_user_order_first_match_wins() {
        // Both rules match `feat/x`; the first one listed must win.
        let cfg = [
            "layouts:".to_string(),
            layout_y("first"),
            layout_y("second"),
            "workspaces:".to_string(),
            "  - repo: /dev/r".to_string(),
            "    layoutMatching:".to_string(),
            "      - worktreePattern: feat/*".to_string(),
            "        layout: first".to_string(),
            "      - worktreePattern: feat/x".to_string(),
            "        layout: second".to_string(),
        ]
        .join("\n");
        let cfg = parse(&cfg);
        let m = match_workspace_layout(
            &cfg,
            &MatchTarget {
                checkout_path: None,
                repo_root: Some("/dev/r".to_string()),
                repo_name: Some("r".to_string()),
                branch: Some("feat/x".to_string()),
            },
        )
        .unwrap();
        assert_eq!(m.1.id, "first");
    }

    #[test]
    fn a_workspace_with_only_layout_matching_yields_none_when_nothing_matches() {
        let cfg = [
            "layouts:".to_string(),
            layout_y("only"),
            "workspaces:".to_string(),
            "  - repo: /dev/r".to_string(),
            "    layoutMatching:".to_string(),
            "      - worktreePattern: release/*".to_string(),
            "        layout: only".to_string(),
        ]
        .join("\n");
        let config = parse(&cfg);
        let with_branch = |branch: &str| {
            match_workspace_layout(
                &config,
                &MatchTarget {
                    checkout_path: None,
                    repo_root: Some("/dev/r".to_string()),
                    repo_name: Some("r".to_string()),
                    branch: Some(branch.to_string()),
                },
            )
        };
        // branch matches -> applies
        assert_eq!(with_branch("release/1").unwrap().1.id, "only");
        // branch doesn't match and there's no defaultLayout -> nothing applies
        assert!(with_branch("main").is_none());
    }

    #[test]
    fn a_more_specific_workspace_with_no_layout_defers_to_a_less_specific_default() {
        // path /wt/r/special (more specific) only has layoutMatching and won't match
        // `main`, so the broader /wt/r default applies.
        let cfg = [
            "layouts:".to_string(),
            layout_y("broad"),
            layout_y("narrow"),
            "workspaces:".to_string(),
            "  - path: /wt/r".to_string(),
            "    defaultLayout: broad".to_string(),
            "  - path: /wt/r/special".to_string(),
            "    layoutMatching:".to_string(),
            "      - worktreePattern: feat/*".to_string(),
            "        layout: narrow".to_string(),
        ]
        .join("\n");
        let config = parse(&cfg);
        let with_branch = |branch: &str| {
            match_workspace_layout(
                &config,
                &MatchTarget {
                    checkout_path: Some("/wt/r/special/x".to_string()),
                    branch: Some(branch.to_string()),
                    ..Default::default()
                },
            )
        };
        // under the more specific path, but on `main` -> narrow doesn't match -> broad
        assert_eq!(with_branch("main").unwrap().1.id, "broad");
        // on a feat branch -> the more specific workspace's rule wins
        assert_eq!(with_branch("feat/y").unwrap().1.id, "narrow");
    }

    #[test]
    fn rejects_layout_matching_referencing_an_unknown_layout() {
        let text = [
            "layouts:".to_string(),
            layout_y("known"),
            "workspaces:".to_string(),
            "  - repo: /dev/r".to_string(),
            "    layoutMatching:".to_string(),
            "      - worktreePattern: feat/*".to_string(),
            "        layout: nope".to_string(),
        ]
        .join("\n");
        assert!(parse_err(&text).contains("layoutMatching references unknown layout"));
    }

    #[test]
    fn rejects_a_layout_matching_rule_missing_worktree_pattern() {
        let text = [
            "layouts:".to_string(),
            layout_y("known"),
            "workspaces:".to_string(),
            "  - repo: /dev/r".to_string(),
            "    layoutMatching:".to_string(),
            "      - layout: known".to_string(),
        ]
        .join("\n");
        assert!(parse_err(&text).contains("worktreePattern"));
    }
}
