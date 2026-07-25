// Executes a plan against the live herdr server.
//
// One `layout.apply` request per tab builds that tab's whole pane tree --
// structure, labels, cwd, env and argv commands -- server-side. Agents are then
// started into their panes with `herdr agent start`, which returns only once
// herdr has detected the agent and marked it ready for input.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::config::PLUGIN_ID;
use crate::env::Env;
use crate::herdr::run_herdr_json;
use crate::plan::{handles_in_tree_order, Node, Plan, SetupPlan, ROOT_TAB};
use crate::socket;

pub type Logger<'a> = &'a dyn Fn(&str);

pub fn int_env(env: &Env, key: &str, fallback: u64) -> u64 {
    match env.get(key) {
        None | Some("") => fallback,
        Some(v) => v.trim().parse::<u64>().unwrap_or(fallback),
    }
}

pub struct Target {
    pub workspace_id: String,
    pub root_tab: String,
    pub root_pane: String,
    pub cwd: Option<String>,
}

// Handle -> id, in insertion order (the summary reports them in build order).
#[derive(Default)]
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

// --- Request/response shapes -------------------------------------------------

// A plan node as `layout.apply` wants it. Optional fields are omitted rather
// than sent as null so herdr applies its own defaults.
fn node_json(node: &Node) -> Value {
    match node {
        Node::Pane(spec) => {
            let mut obj = Map::new();
            obj.insert("type".into(), json!("pane"));
            if let Some(title) = &spec.title {
                obj.insert("label".into(), json!(title));
            }
            if let Some(cwd) = &spec.cwd {
                obj.insert("cwd".into(), json!(cwd));
            }
            if let Some(command) = &spec.command {
                obj.insert("command".into(), json!(command));
            }
            if !spec.env.is_empty() {
                obj.insert("env".into(), json!(spec.env));
            }
            Value::Object(obj)
        }
        Node::Split { direction, ratio, first, second } => json!({
            "type": "split",
            "direction": direction.as_str(),
            "ratio": ratio,
            "first": node_json(first),
            "second": node_json(second),
        }),
    }
}

// herdr echoes the applied tree back with every `pane_id` filled in, in the same
// shape we sent. Walking both in the same order recovers each handle's real id
// without guessing at id allocation.
fn response_pane_ids(node: &Value, out: &mut Vec<String>) -> Result<(), String> {
    match node.get("type").and_then(Value::as_str) {
        Some("pane") => {
            let id = node
                .get("pane_id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("layout.apply returned a pane with no id: {}", node))?;
            out.push(id.to_string());
            Ok(())
        }
        Some("split") => {
            let first = node.get("first").ok_or("layout.apply split has no first child")?;
            let second = node.get("second").ok_or("layout.apply split has no second child")?;
            response_pane_ids(first, out)?;
            response_pane_ids(second, out)
        }
        other => Err(format!("layout.apply returned an unexpected node type {:?}", other)),
    }
}

// --- Setup gating ------------------------------------------------------------

// Setup's exit status, once its pane has written it. Polls for the status file
// the setup script writes: unlike matching a sentinel in terminal output, this
// can't be missed because the marker scrolled away, wrapped, or was echoed back
// by the shell before the command actually ran.
//
// A timeout is reported, not fatal -- the layout is already built, and a setup
// step that runs long shouldn't invalidate it.
enum SetupOutcome {
    Exited(i64),
    Finished,
    TimedOut,
}

fn wait_for_setup(status_path: &Path, timeout_ms: u64, log: Logger) -> SetupOutcome {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Ok(text) = fs::read_to_string(status_path) {
            let trimmed = text.trim();
            // The file is written by a single `printf` with no partial-write
            // window in practice, but an empty read just means "not yet".
            if !trimmed.is_empty() {
                let _ = fs::remove_file(status_path);
                return match trimmed.parse::<i64>() {
                    Ok(code) => SetupOutcome::Exited(code),
                    Err(_) => SetupOutcome::Finished,
                };
            }
        }
        if Instant::now() >= deadline {
            log(&format!("setup did not finish within {}ms; continuing", timeout_ms));
            return SetupOutcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// --- User-visible status -----------------------------------------------------

// Put a token on the pane's sidebar row so a long or failed setup is visible in
// the UI rather than only in the plugin log. Best effort: metadata is cosmetic,
// and a herdr build without it must not fail the apply.
fn report_pane_token(env: &Env, pane_id: &str, token: Option<&str>, ttl_ms: Option<u64>) {
    let mut args: Vec<String> = vec![
        "pane".into(),
        "report-metadata".into(),
        pane_id.into(),
        "--source".into(),
        PLUGIN_ID.into(),
    ];
    match token {
        Some(value) => {
            args.push("--token".into());
            args.push(format!("setup={}", value));
        }
        None => {
            args.push("--clear-token".into());
            args.push("setup".into());
        }
    }
    if let Some(ttl) = ttl_ms {
        args.push("--ttl-ms".into());
        args.push(ttl.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let _ = run_herdr_json(&refs, env);
}

// Surface a failure the user would otherwise only find in the plugin log. Also
// best effort: notifications can be disabled in herdr's config, in which case
// the server simply reports `shown: false`.
pub fn notify(env: &Env, title: &str, body: &str) {
    let _ = run_herdr_json(&["notification", "show", title, "--body", body], env);
}

// --- Agents ------------------------------------------------------------------

// `herdr agent start` types into the pane's shell and refuses a pane that isn't
// "an available shell" -- one where the shell itself owns the foreground with no
// command running. A freshly created pane isn't there yet: the shell is still
// sourcing rc files (mise/asdf activation, prompt setup), and a setup pane has
// to finish its command and exec back to a prompt first.
//
// So poll for that state rather than sleeping a fixed guess, which is what the
// old implementation did before typing anything into a new pane.
//
// The process group id alone isn't enough: while zsh sources its rc files it
// spawns children inside its own group, so the group still looks like the
// shell's. The pane is only idle once the shell is the *only* foreground
// process.
fn pane_is_at_a_prompt(result: &Value) -> bool {
    let Some(info) = result.get("process_info") else { return false };
    let field = |key: &str| info.get(key).and_then(Value::as_i64);
    let Some(shell) = field("shell_pid") else { return false };
    if field("foreground_process_group_id") != Some(shell) {
        return false;
    }
    match info.get("foreground_processes").and_then(Value::as_array).map(Vec::as_slice) {
        // Absent entirely: a platform that doesn't expose the list, so the
        // process group is all we have to go on.
        None => true,
        // Present but empty means herdr hasn't sampled the pane yet, which is
        // exactly the state a just-created pane is in -- not an idle shell.
        Some([]) => false,
        Some([only]) => only.get("pid").and_then(Value::as_i64) == Some(shell),
        Some(_) => false,
    }
}

fn wait_for_shell_prompt(env: &Env, pane_id: &str, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    // Require two consecutive idle samples. A shell sourcing rc files drops in
    // and out of having a child, so a single sample can catch a gap between two
    // of them and call the pane ready a moment before it is.
    let mut consecutive = 0;
    loop {
        match run_herdr_json(&["pane", "process-info", "--pane", pane_id], env) {
            Ok(result) if pane_is_at_a_prompt(&result) => {
                consecutive += 1;
                if consecutive >= 2 {
                    return true;
                }
            }
            _ => consecutive = 0,
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// herdr requires a unique agent name matching [a-z][a-z0-9_-]{0,31}. When the
// config doesn't pick one, derive a stable, readable name from the kind and the
// workspace so two worktrees running the same agent don't collide.
fn derive_agent_name(kind: &str, workspace_id: &str, taken: &[String]) -> String {
    let sanitized: String = workspace_id
        .chars()
        .filter_map(|c| {
            let c = c.to_ascii_lowercase();
            (c.is_ascii_lowercase() || c.is_ascii_digit()).then_some(c)
        })
        .collect();
    let base = format!("{}-{}", kind, sanitized);
    let base = base[..base.len().min(30)].to_string();
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{}-{}", base, n))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or(base)
}

// --- Execution ---------------------------------------------------------------

pub fn execute_plan(plan: &Plan, target: &Target, env: &Env, log: Logger) -> Result<Value, String> {
    let apply_timeout = int_env(env, "HERDR_WSM_APPLY_TIMEOUT_MS", socket::DEFAULT_TIMEOUT_MS);
    let setup_timeout = int_env(env, "HERDR_WSM_SETUP_TIMEOUT_MS", 600_000);
    let agent_timeout = int_env(env, "HERDR_WSM_AGENT_TIMEOUT_MS", 60_000);
    let shell_ready_timeout = int_env(env, "HERDR_WSM_SHELL_READY_MS", 15_000);

    let mut handles = Handles::default();
    let mut setup_done = plan.setup.is_none();

    for tab in &plan.tabs {
        let mut params = Map::new();
        if tab.handle == ROOT_TAB {
            // Replace the worktree's existing root tab. herdr builds the
            // replacement first and closes the old tab afterwards, so the
            // workspace is never briefly tabless. `tab_id` and `workspace_id`
            // are mutually exclusive.
            params.insert("tab_id".into(), json!(target.root_tab));
        } else {
            params.insert("workspace_id".into(), json!(target.workspace_id));
        }
        if let Some(title) = &tab.title {
            params.insert("tab_label".into(), json!(title));
        }
        // Never steal focus: the layout is built into whichever workspace the
        // event named, which is not necessarily the one the user is looking at.
        params.insert("focus".into(), json!(false));
        params.insert("root".into(), node_json(&tab.root));

        let result = socket::request(env, "layout.apply", Value::Object(params), apply_timeout)?;
        let layout = result.get("layout").ok_or("layout.apply returned no layout")?;
        let tab_id = layout
            .get("tab_id")
            .and_then(Value::as_str)
            .ok_or("layout.apply returned no tab id")?;
        let root = layout.get("root").ok_or("layout.apply returned no root node")?;

        let mut pane_ids = Vec::new();
        response_pane_ids(root, &mut pane_ids)?;
        let planned = handles_in_tree_order(&tab.root);
        if planned.len() != pane_ids.len() {
            return Err(format!(
                "layout.apply returned {} panes for tab {} but the plan has {}",
                pane_ids.len(),
                tab.handle,
                planned.len()
            ));
        }
        handles.set(&tab.handle, tab_id.to_string());
        for (handle, pane_id) in planned.iter().zip(pane_ids) {
            handles.set(handle, pane_id);
        }

        log(&format!(
            "applied tab {} -> {} ({} pane{}){}",
            tab.handle,
            tab_id,
            planned.len(),
            if planned.len() == 1 { "" } else { "s" },
            tab.title.as_ref().map(|t| format!(" \"{}\"", t)).unwrap_or_default(),
        ));

        // A blocking setup must finish before any later tab is spawned.
        if let Some(setup) = &plan.setup {
            if setup.blocking && !setup_done && planned.contains(&setup.handle.as_str()) {
                await_setup(setup, &handles, env, setup_timeout, log);
                setup_done = true;
            }
        }
    }

    let mut agent_names: Vec<String> = Vec::new();
    for action in &plan.agents {
        let Some(pane_id) = handles.get(&action.handle).map(String::from) else {
            log(&format!("no pane for agent handle {}; skipping", action.handle));
            continue;
        };

        // An agent is typed into the pane's shell, so setup running in that same
        // pane has to be finished first even when it isn't marked blocking.
        if let Some(setup) = &plan.setup {
            if !setup_done && setup.handle == action.handle {
                log("waiting for setup before starting the agent in its pane");
                await_setup(setup, &handles, env, setup_timeout, log);
                setup_done = true;
            }
        }

        if !wait_for_shell_prompt(env, &pane_id, shell_ready_timeout) {
            log(&format!(
                "WARNING: {} is not back at a shell prompt after {}ms; starting the agent anyway",
                pane_id, shell_ready_timeout
            ));
        }

        let agent = &action.agent;
        let name = agent
            .name
            .clone()
            .unwrap_or_else(|| derive_agent_name(&agent.kind, &target.workspace_id, &agent_names));
        let timeout = agent.start_timeout_ms.unwrap_or(agent_timeout).to_string();

        let mut args: Vec<String> = vec![
            "agent".into(),
            "start".into(),
            name.clone(),
            "--kind".into(),
            agent.kind.clone(),
            "--pane".into(),
            pane_id.clone(),
            "--timeout".into(),
            timeout,
        ];
        if !agent.args.is_empty() {
            args.push("--".into());
            args.extend(agent.args.iter().cloned());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();

        // A failed agent doesn't invalidate the layout that's already built, so
        // warn and keep going rather than tearing the whole apply down.
        if let Err(err) = run_herdr_json(&refs, env) {
            log(&format!("WARNING: could not start {} in {}: {}", agent.kind, pane_id, err));
            notify(
                env,
                "Workspace layout: agent did not start",
                &format!("{} in {}: {}", agent.kind, pane_id, err),
            );
            continue;
        }
        agent_names.push(name.clone());
        log(&format!("started {} as \"{}\" in {}", agent.kind, name, pane_id));

        if let Some(prompt) = &agent.prompt {
            let mut args: Vec<String> =
                vec!["agent".into(), "prompt".into(), name.clone(), prompt.clone()];
            // Waiting is opt-in: setting a prompt timeout means "block until the
            // agent settles or this elapses". Without one the prompt is
            // submitted and the hook returns, leaving the agent working.
            if let Some(timeout) = agent.prompt_timeout_ms {
                args.push("--wait".into());
                args.push("--until".into());
                args.push("idle".into());
                args.push("--until".into());
                args.push("done".into());
                args.push("--timeout".into());
                args.push(timeout.to_string());
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            match run_herdr_json(&refs, env) {
                Ok(_) => log(&format!("prompted \"{}\"", name)),
                Err(err) => log(&format!("WARNING: could not prompt \"{}\": {}", name, err)),
            }
        }
    }

    // A non-blocking setup is left running in its pane; its status file is
    // nobody's to read and gets reaped by the startup hook.
    if let (Some(setup), false) = (&plan.setup, setup_done) {
        if let Some(pane_id) = handles.get(&setup.handle) {
            log(&format!("setup is running in {} (not blocking)", pane_id));
        }
    }

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
    let tabs: Vec<Value> = plan
        .tabs
        .iter()
        .filter_map(|t| handles.get(&t.handle).map(|id| json!({ "handle": t.handle, "tabId": id })))
        .collect();
    Ok(json!({
        "layoutId": plan.layout_id,
        "tabs": tabs,
        "panes": panes,
        "handles": handle_map,
        "agents": agent_names,
    }))
}

// Block on the setup command, reporting progress and outcome on the pane itself.
fn await_setup(setup: &SetupPlan, handles: &Handles, env: &Env, timeout_ms: u64, log: Logger) {
    let Some(status_path) = setup.status_path.as_deref() else { return };
    let pane_id = handles.get(&setup.handle).map(String::from);

    if let Some(pane_id) = &pane_id {
        // TTL slightly beyond the wait so a crashed plugin can't leave the pane
        // labelled "running" forever.
        report_pane_token(env, pane_id, Some("running"), Some(timeout_ms + 30_000));
    }
    log("waiting for setup to finish");
    let outcome = wait_for_setup(Path::new(status_path), timeout_ms, log);

    let Some(pane_id) = pane_id else { return };
    match outcome {
        SetupOutcome::Exited(0) | SetupOutcome::Finished => {
            report_pane_token(env, &pane_id, None, None);
            log(&format!("setup finished in {}", pane_id));
        }
        SetupOutcome::Exited(code) => {
            report_pane_token(env, &pane_id, Some(&format!("failed-{}", code)), None);
            log(&format!("WARNING: setup command exited {} in {}", code, pane_id));
            notify(
                env,
                "Workspace layout: setup failed",
                &format!("The setup command exited {} in {}", code, pane_id),
            );
        }
        SetupOutcome::TimedOut => {
            report_pane_token(env, &pane_id, Some("timed-out"), None);
            notify(
                env,
                "Workspace layout: setup timed out",
                &format!("Setup did not finish within {}ms in {}", timeout_ms, pane_id),
            );
        }
    }
}

/// Where a run should record its setup exit status. Unique per invocation so
/// concurrent applies (two worktrees created at once) can't read each other's.
pub fn setup_status_path(state_dir: &Path, token: &str) -> Option<String> {
    let dir = state_dir.join("setup");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("{}.status", token)).to_string_lossy().into_owned())
}

/// Drop status files left behind by runs that died before reading them.
pub fn reap_setup_status_files(state_dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(state_dir.join("setup")) else { return 0 };
    let mut reaped = 0;
    for entry in entries.flatten() {
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or_default();
        if age > Duration::from_secs(24 * 60 * 60) && fs::remove_file(entry.path()).is_ok() {
            reaped += 1;
        }
    }
    reaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::config::{find_layout, validate_config, Direction};
    use crate::plan::{build_plan, PaneSpec, PlanContext};
    use crate::yaml::parse_yaml;

    fn leaf(handle: &str) -> Node {
        Node::Pane(PaneSpec {
            handle: handle.to_string(),
            title: Some(handle.to_string()),
            command: None,
            env: BTreeMap::new(),
            cwd: None,
        })
    }

    #[test]
    fn node_json_omits_absent_fields_and_keeps_argv_as_a_list() {
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), "3000".to_string());
        let node = Node::Pane(PaneSpec {
            handle: "t0p0".into(),
            title: Some("editor".into()),
            command: Some(vec!["sh".into(), "-c".into(), "nvim".into()]),
            env,
            cwd: Some("/work".into()),
        });
        let json = node_json(&node);
        assert_eq!(json["type"], "pane");
        assert_eq!(json["label"], "editor");
        assert_eq!(json["cwd"], "/work");
        assert_eq!(json["command"], json!(["sh", "-c", "nvim"]));
        assert_eq!(json["env"]["PORT"], "3000");
        assert!(json.get("pane_id").is_none());

        // A bare pane sends nothing but its type.
        let bare = node_json(&leaf("t0p1"));
        assert!(bare.get("command").is_none());
        assert!(bare.get("env").is_none());
        assert!(bare.get("cwd").is_none());
    }

    #[test]
    fn node_json_nests_splits_with_direction_and_ratio() {
        let node = Node::Split {
            direction: Direction::Down,
            ratio: 0.75,
            first: Box::new(leaf("t0p0")),
            second: Box::new(leaf("t0p1")),
        };
        let json = node_json(&node);
        assert_eq!(json["type"], "split");
        assert_eq!(json["direction"], "down");
        assert_eq!(json["ratio"], 0.75);
        assert_eq!(json["first"]["label"], "t0p0");
        assert_eq!(json["second"]["label"], "t0p1");
    }

    #[test]
    fn response_pane_ids_are_read_in_the_same_order_the_plan_lists_handles() {
        let response = json!({
            "type": "split", "direction": "right", "ratio": 0.5,
            "first": { "type": "pane", "pane_id": "w1:p1" },
            "second": {
                "type": "split", "direction": "down", "ratio": 0.5,
                "first": { "type": "pane", "pane_id": "w1:p2" },
                "second": { "type": "pane", "pane_id": "w1:p3" },
            },
        });
        let mut ids = Vec::new();
        response_pane_ids(&response, &mut ids).unwrap();
        assert_eq!(ids, vec!["w1:p1", "w1:p2", "w1:p3"]);

        let plan_tree = Node::Split {
            direction: Direction::Right,
            ratio: 0.5,
            first: Box::new(leaf("t0p0")),
            second: Box::new(Node::Split {
                direction: Direction::Down,
                ratio: 0.5,
                first: Box::new(leaf("t0p1")),
                second: Box::new(leaf("t0p2")),
            }),
        };
        assert_eq!(handles_in_tree_order(&plan_tree), vec!["t0p0", "t0p1", "t0p2"]);
    }

    #[test]
    fn response_pane_ids_rejects_a_malformed_tree() {
        let mut ids = Vec::new();
        assert!(response_pane_ids(&json!({ "type": "pane" }), &mut ids).is_err());
        assert!(response_pane_ids(&json!({ "type": "wat" }), &mut ids).is_err());
    }

    #[test]
    fn the_plan_and_the_request_agree_on_pane_count_for_a_real_layout() {
        let config = validate_config(
            &parse_yaml(
                &[
                    "layouts:",
                    "  - id: x",
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
            )
            .unwrap(),
        )
        .unwrap();
        let plan = build_plan(find_layout(&config, "x").unwrap(), &PlanContext::default());
        let request = node_json(&plan.tabs[0].root);
        // Same tree shape going out as we expect coming back.
        let mut leaves = 0;
        fn count(node: &Value, leaves: &mut usize) {
            match node["type"].as_str() {
                Some("pane") => *leaves += 1,
                _ => {
                    count(&node["first"], leaves);
                    count(&node["second"], leaves);
                }
            }
        }
        count(&request, &mut leaves);
        assert_eq!(leaves, handles_in_tree_order(&plan.tabs[0].root).len());
    }

    #[test]
    fn derived_agent_names_are_valid_stable_and_deduplicated() {
        let none: Vec<String> = Vec::new();
        assert_eq!(derive_agent_name("claude", "w5R", &none), "claude-w5r");
        // Same inputs -> same name, so a re-apply targets the same agent.
        assert_eq!(derive_agent_name("claude", "w5R", &none), "claude-w5r");
        // A second agent of the same kind in the same workspace gets a suffix.
        let taken = vec!["claude-w5r".to_string()];
        assert_eq!(derive_agent_name("claude", "w5R", &taken), "claude-w5r-2");

        let name = derive_agent_name("opencode", "w:12-ab", &none);
        assert!(name.starts_with("opencode-"));
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(name.len() <= 32);
    }

    #[test]
    fn a_pane_is_only_at_a_prompt_once_the_shell_is_the_lone_foreground_process() {
        let info = |fg: Value| json!({ "process_info": {
            "shell_pid": 100, "foreground_process_group_id": 100, "foreground_processes": fg,
        }});
        // Settled: just the shell.
        assert!(pane_is_at_a_prompt(&info(json!([{ "pid": 100, "name": "zsh" }]))));
        // Still sourcing rc files: the shell has a child in its own group, which
        // is exactly the window where `agent start` reports agent_pane_busy.
        assert!(!pane_is_at_a_prompt(&info(
            json!([{ "pid": 100, "name": "zsh" }, { "pid": 101, "name": "zsh" }])
        )));
        // Running a command.
        assert!(!pane_is_at_a_prompt(&info(json!([{ "pid": 202, "name": "nvim" }]))));
        // Not sampled yet -- what a pane looks like the instant it's created.
        // Reading this as "idle" is what made `agent start` race.
        assert!(!pane_is_at_a_prompt(&info(json!([]))));
        // A different foreground group means something else owns the terminal.
        assert!(!pane_is_at_a_prompt(&json!({ "process_info": {
            "shell_pid": 100, "foreground_process_group_id": 202,
        }})));
        // No process list exposed -> fall back to the group check.
        assert!(pane_is_at_a_prompt(&json!({ "process_info": {
            "shell_pid": 100, "foreground_process_group_id": 100,
        }})));
        assert!(!pane_is_at_a_prompt(&json!({})));
    }

    #[test]
    fn wait_for_setup_reads_the_exit_code_and_removes_the_file() {
        let dir = std::env::temp_dir().join(format!("wsm-setup-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.status");
        fs::write(&path, "3").unwrap();
        let quiet = |_: &str| {};
        assert!(matches!(wait_for_setup(&path, 1000, &quiet), SetupOutcome::Exited(3)));
        assert!(!path.exists(), "the status file is consumed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wait_for_setup_times_out_instead_of_blocking_forever() {
        let dir = std::env::temp_dir().join(format!("wsm-setup-to-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let quiet = |_: &str| {};
        let outcome = wait_for_setup(&dir.join("missing.status"), 250, &quiet);
        assert!(matches!(outcome, SetupOutcome::TimedOut));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn int_env_falls_back_on_missing_empty_and_garbage_values() {
        let env = Env::from_pairs(&[("A", "5"), ("B", ""), ("C", "nope")]);
        assert_eq!(int_env(&env, "A", 1), 5);
        assert_eq!(int_env(&env, "B", 1), 1);
        assert_eq!(int_env(&env, "C", 1), 1);
        assert_eq!(int_env(&env, "MISSING", 1), 1);
    }
}
