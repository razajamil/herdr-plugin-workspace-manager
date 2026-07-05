// Executes a symbolic plan against the live herdr server.

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config::{Direction, Size};
use crate::env::Env;
use crate::herdr::{pane_id_of, run_herdr_json, tab_id_of};
use crate::plan::{split_ratio_arg, Plan, Step, ROOT_PANE, ROOT_TAB};

pub type Logger<'a> = &'a dyn Fn(&str);

// Current extent (in cells) of a pane along the axis a split would divide:
// columns for a "right" split, rows for a "down" split. Used to convert a fixed
// cell `size` into a --ratio. Returns None if it can't be determined, in which
// case the split falls back to herdr's default sizing.
fn pane_extent(pane_id: &str, direction: Direction, env: &Env) -> Option<f64> {
    let layout = run_herdr_json(&["pane", "layout", "--pane", pane_id], env).ok()?;
    let rect = layout
        .get("layout")?
        .get("panes")?
        .as_array()?
        .iter()
        .find(|p| p.get("pane_id").and_then(Value::as_str) == Some(pane_id))?
        .get("rect")?;
    let side = if direction == Direction::Down { "height" } else { "width" };
    rect.get(side)?.as_f64()
}

// Format a ratio for the CLI: trim to 4 decimals so a converted cell size
// doesn't emit a long repeating fraction.
fn fmt_ratio(r: f64) -> String {
    format!("{}", (r * 1e4).round() / 1e4)
}

fn int_env(env: &Env, key: &str, fallback: u64) -> u64 {
    match env.get(key) {
        None | Some("") => fallback,
        Some(v) => v.trim().parse::<u64>().unwrap_or(fallback),
    }
}

fn base36(mut n: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

// Build a setup command wrapped with a completion sentinel printed after it.
// The marker (HERDR_WSM_DONE_<token>) is assembled by printf's `%s`, so the
// full literal never appears in the *echoed* command line -- only in the
// command's actual output. That matters because `herdr wait output` also sees
// the echoed input, and would otherwise match immediately (before the command
// finished). Using printf %s instead of shell quote tricks keeps this correct
// across bash and zsh (including zsh's RC_QUOTES, where 'a''b' is a literal
// quote, not concatenation). The token is base36 [0-9a-z], safe to single-quote.
fn wrap_setup(command: &str, token: &str) -> String {
    format!("( {} ) ; printf 'HERDR_WSM_DONE_%s %s\\n' '{}' \"$?\"", command, token)
}

// Ordered handle -> id map (insertion order matters for the summary).
struct Handles(Vec<(String, String)>);

impl Handles {
    fn get(&self, handle: &str) -> Option<&str> {
        self.0.iter().find(|(h, _)| h == handle).map(|(_, id)| id.as_str())
    }

    fn set(&mut self, handle: &str, id: String) {
        match self.0.iter_mut().find(|(h, _)| h == handle) {
            Some(entry) => entry.1 = id,
            None => self.0.push((handle.to_string(), id)),
        }
    }
}

pub struct Target {
    pub workspace_id: String,
    pub root_tab: String,
    pub root_pane: String,
    pub cwd: Option<String>,
}

// Execute a symbolic plan against the live herdr server.
//
// target.root_tab / target.root_pane are the worktree's existing first tab +
// pane (from the event payload or queried live).
pub fn execute_plan(plan: &Plan, target: &Target, env: &Env, log: Logger) -> Result<Value, String> {
    let ready_ms = int_env(env, "HERDR_WSM_PANE_READY_MS", 700);
    let setup_timeout_ms = int_env(env, "HERDR_WSM_SETUP_TIMEOUT_MS", 600_000);

    let mut handles = Handles(Vec::new());
    handles.set(ROOT_TAB, target.root_tab.clone());
    handles.set(ROOT_PANE, target.root_pane.clone());

    let mut readied: HashSet<String> = HashSet::new();

    let resolve_pane = |handles: &Handles, handle: &str| -> Result<String, String> {
        handles
            .get(handle)
            .map(String::from)
            .ok_or_else(|| format!("unresolved pane handle \"{}\"", handle))
    };

    // Wait for a freshly-spawned pane's shell to be ready before typing into it.
    let mut ensure_ready = |pane_id: &str| {
        if readied.insert(pane_id.to_string()) && ready_ms > 0 {
            std::thread::sleep(Duration::from_millis(ready_ms));
        }
    };

    let cwd_args = |step_cwd: &Option<String>| -> Vec<String> {
        match step_cwd.as_deref().or(target.cwd.as_deref()) {
            Some(cwd) => vec!["--cwd".to_string(), cwd.to_string()],
            None => Vec::new(),
        }
    };

    for step in &plan.steps {
        match step {
            Step::ReuseTab { tab, title } => {
                let tab_id = handles
                    .get(tab)
                    .map(String::from)
                    .ok_or_else(|| format!("unresolved tab handle \"{}\"", tab))?;
                if let Some(title) = title {
                    run_herdr_json(&["tab", "rename", &tab_id, title], env)?;
                }
                log(&format!(
                    "reuse tab {}{}",
                    tab_id,
                    title.as_ref().map(|t| format!(" as \"{}\"", t)).unwrap_or_default()
                ));
            }
            Step::CreateTab { tab, pane, title, cwd } => {
                let mut args: Vec<String> = vec![
                    "tab".into(),
                    "create".into(),
                    "--workspace".into(),
                    target.workspace_id.clone(),
                    "--no-focus".into(),
                ];
                if let Some(title) = title {
                    args.push("--label".into());
                    args.push(title.clone());
                }
                args.extend(cwd_args(cwd));
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let result = run_herdr_json(&arg_refs, env)?;
                let tab_id = tab_id_of(&result);
                let pane_id = pane_id_of(&result);
                let (Some(tab_id), Some(pane_id)) = (tab_id, pane_id) else {
                    return Err(format!("tab create did not return ids: {}", result));
                };
                handles.set(tab, tab_id.clone());
                handles.set(pane, pane_id.clone());
                log(&format!(
                    "create tab {} (pane {}){}",
                    tab_id,
                    pane_id,
                    title.as_ref().map(|t| format!(" \"{}\"", t)).unwrap_or_default()
                ));
            }
            Step::Split { pane, from, direction, ratio, size, cwd } => {
                let from_id = resolve_pane(&handles, from)?;
                let mut args: Vec<String> = vec![
                    "pane".into(),
                    "split".into(),
                    from_id.clone(),
                    "--direction".into(),
                    direction.as_str().into(),
                    "--no-focus".into(),
                ];
                // A fixed cell size needs the from pane's live extent to become a ratio.
                let extent = match size {
                    Some(Size::Cells(_)) => pane_extent(&from_id, *direction, env),
                    _ => None,
                };
                let ratio = split_ratio_arg(*ratio, size.as_ref(), extent);
                if let Some(r) = ratio {
                    args.push("--ratio".into());
                    args.push(fmt_ratio(r));
                }
                args.extend(cwd_args(cwd));
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let result = run_herdr_json(&arg_refs, env)?;
                let Some(pane_id) = pane_id_of(&result) else {
                    return Err(format!("pane split did not return a pane id: {}", result));
                };
                handles.set(pane, pane_id.clone());
                log(&format!(
                    "split {} {}{} -> {}",
                    from_id,
                    direction.as_str(),
                    ratio.map(|r| format!(" @{}", fmt_ratio(r))).unwrap_or_default(),
                    pane_id
                ));
            }
            Step::RenamePane { pane, title } => {
                let pane_id = resolve_pane(&handles, pane)?;
                run_herdr_json(&["pane", "rename", &pane_id, title], env)?;
                log(&format!("rename pane {} -> \"{}\"", pane_id, title));
            }
            Step::RunSetup { pane, command, blocking } => {
                let pane_id = resolve_pane(&handles, pane)?;
                ensure_ready(&pane_id);
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                let token = format!("{}{}", base36(std::process::id() as u128), base36(now_ms));
                let marker = format!("HERDR_WSM_DONE_{}", token);
                run_herdr_json(&["pane", "run", &pane_id, &wrap_setup(command, &token)], env)?;
                log(&format!(
                    "run setup in {}: {}{}",
                    pane_id,
                    command,
                    if *blocking { " (blocking)" } else { "" }
                ));
                if *blocking {
                    let timeout = setup_timeout_ms.to_string();
                    let wait_res = run_herdr_json(
                        &["wait", "output", &pane_id, "--match", &marker, "--timeout", &timeout],
                        env,
                    )?;
                    let line = wait_res
                        .get("matched_line")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let code = line.trim().split_whitespace().last().and_then(|t| t.parse::<i64>().ok());
                    match code {
                        Some(code) if code != 0 => {
                            log(&format!("WARNING: setup command exited {} in {}", code, pane_id))
                        }
                        _ => log(&format!("setup finished in {}", pane_id)),
                    }
                }
            }
            Step::Run { pane, command } => {
                let pane_id = resolve_pane(&handles, pane)?;
                ensure_ready(&pane_id);
                run_herdr_json(&["pane", "run", &pane_id, command], env)?;
                log(&format!("run in {}: {}", pane_id, command));
            }
        }
    }

    // Build a handle -> id summary for callers/tests.
    let panes: Vec<Value> = handles
        .0
        .iter()
        .filter(|(handle, _)| handle.contains('p'))
        .map(|(handle, id)| json!({ "handle": handle, "paneId": id }))
        .collect();
    let handle_map: Map<String, Value> = handles
        .0
        .iter()
        .map(|(handle, id)| (handle.clone(), Value::String(id.clone())))
        .collect();
    Ok(json!({ "layoutId": plan.layout_id, "panes": panes, "handles": handle_map }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ratio_trims_to_four_decimals() {
        assert_eq!(fmt_ratio(0.5), "0.5");
        assert_eq!(fmt_ratio(1.0 - 50.0 / 200.0), "0.75");
        assert_eq!(fmt_ratio(2.0 / 3.0), "0.6667");
    }

    #[test]
    fn wrap_setup_hides_the_marker_from_the_echoed_command() {
        let wrapped = wrap_setup("make setup", "abc123");
        assert!(wrapped.starts_with("( make setup ) ;"));
        // The literal marker never appears in the command line; printf assembles it.
        assert!(!wrapped.contains("HERDR_WSM_DONE_abc123"));
        assert!(wrapped.contains("'abc123'"));
    }
}
