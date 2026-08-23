// herdr-workspace-manager — the Workspace Manager plugin binary.
//
// One executable serves both roles:
//   - plugin entrypoints, invoked by herdr via bin/herdr-workspace-manager
//     (`event`, `apply`, `validate`, `remove-gone --preview`)
//   - the user-facing CLI (`remove-gone` with flags + confirmation prompt)

mod apply_core;
mod config;
mod env;
mod herdr;
mod plan;
mod remove_gone;
mod runner;
mod socket;
mod yaml;

use std::io::{BufRead, Write};

use serde_json::Value;

use crate::apply_core::{
    apply_layout, claim_apply, clear_decided, get_event_payload, get_worktree_branch,
    get_workspace_info, is_decided, mark_decided, reap_orphan_claims, release_apply,
    resolve_target, state_dir,
};
use crate::config::{
    config_candidates, find_layout, load_config, match_workspace_layout, Config, MatchTarget,
    Pane, Size,
};
use crate::env::Env;
use crate::remove_gone::{
    apply_removals, collect_gone_worktrees, format_apply_result, format_preview,
    removable_candidates, repo_display_name,
};
use crate::runner::{notify, reap_setup_status_files};

const USAGE: &str = "herdr-workspace-manager — Workspace Manager plugin CLI

Usage:
  herdr-workspace-manager <command> [options]

Commands:
  remove-gone   Remove the current repo's worktrees whose remote branch was
                deleted (\"gone\"), after a y/n confirmation. Worktrees that never
                pushed/tracked a remote, the repo's main checkout, and the
                workspace you run it from are never removed.
  apply [ID]    Apply a layout to the current (or a target) workspace; with no
                ID, the workspace's default layout is matched. (plugin action)
  validate      Parse + validate the config file and print the resolved
                layouts/workspaces. (plugin action)
  event         Worktree/workspace event hook. (plugin internal)
  startup       One-shot state cleanup after the herdr server starts.
                (plugin internal)

Options (remove-gone):
  --dry-run            Only print the list; remove nothing and don't prompt.
  --confirm            Remove without the interactive confirmation prompt.
  --force              Also remove worktrees with uncommitted changes.
  --no-fetch           Skip the pre-run `git fetch --prune` (use cached refs).
  --workspace <id>     Target a specific workspace's repo (default: current pane's).
  --preview            Preview only (used by the plugin action); removes nothing.
  -h, --help           Show this help.
";

fn log(msg: &str) {
    eprintln!("[workspace-manager] {}", msg);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = Env::real();
    let code = match args.first().map(String::as_str) {
        Some("event") => run_logged(|| cmd_event(&env)),
        Some("startup") => run_logged(|| cmd_startup(&env)),
        Some("apply") => run_logged(|| cmd_apply(&env, args.get(1).map(String::as_str))),
        Some("validate") => cmd_validate(&env),
        Some("remove-gone") => run_logged(|| cmd_remove_gone(&env, &args[1..])),
        None | Some("help") | Some("-h") | Some("--help") => {
            print!("{}", USAGE);
            0
        }
        Some(other) => {
            eprint!("unknown command \"{}\"\n\n{}", other, USAGE);
            2
        }
    };
    std::process::exit(code);
}

fn run_logged(f: impl FnOnce() -> Result<(), String>) -> i32 {
    match f() {
        Ok(()) => 0,
        Err(err) => {
            log(&format!("error: {}", err));
            1
        }
    }
}

fn context_json(env: &Env) -> Value {
    env.get("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

fn non_empty(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.is_empty())
}

// --- startup -----------------------------------------------------------------
// `[[startup]]` hook: runs once per enabled plugin after herdr restores its
// session and the API socket is ready (and again after a live handoff), never on
// client attach or config reload.
//
// This is where periodic state maintenance belongs. It used to run on every
// non-hot event invocation, which meant a filesystem sweep during ordinary
// workspace activity; once per server start is both cheaper and better timed,
// because a worktree removed while herdr was not running is now noticed before
// any event can consult a stale claim.
fn cmd_startup(env: &Env) -> Result<(), String> {
    let reaped = reap_orphan_claims(env);
    let forgotten = clear_decided(env);
    let stale = reap_setup_status_files(&state_dir(env));
    log(&format!(
        "startup: reaped {} orphan claim(s), forgot {} decided workspace(s), removed {} stale setup file(s)",
        reaped, forgotten, stale
    ));
    Ok(())
}

// --- event -------------------------------------------------------------------
// Event hook for worktree.created, workspace.created AND workspace.focused.
//
// Why three events: the `herdr worktree create` CLI emits worktree.created (and
// workspace.created), but the herdr **UI** "new worktree" command has
// historically created the worktree in-app and emitted NEITHER to plugins — the
// only thing it dispatches is workspace.focused (it focuses the new worktree
// right after creating it). So we listen for all three and resolve the worktree
// facts by querying the workspace.
//
// workspace.focused fires on every workspace switch, which makes it by far the
// most frequent reason this binary runs, so its early-exit path does no work
// beyond a single `stat`: no config parse, no herdr query. Set
// HERDR_WSM_FOCUS_HOOK=0 to drop the focus path entirely on a herdr build whose
// UI worktree creation does emit worktree.created (see the README).
//
// Guards (a layout is applied exactly once, only to a brand-new linked worktree):
//   - linked-worktree only (never the repo's main checkout)
//   - repo/path must match the config
//   - fresh 1-tab/1-pane workspace only (don't clobber arranged/restored ones)
//   - atomic claim by checkout path (dedupe across the multiple events + restarts)
//   - "decided" cache by workspace id so repeat focuses don't re-query
fn cmd_event(env: &Env) -> Result<(), String> {
    let is_focus = env.get("HERDR_PLUGIN_EVENT").unwrap_or("").contains("focus");
    if is_focus && env.get("HERDR_WSM_FOCUS_HOOK") == Some("0") {
        return Ok(());
    }

    let p = get_event_payload(env);
    let Some(workspace_id) = p.workspace_id else { return Ok(()) };

    // Hot path first: a workspace we've already ruled on costs one stat and
    // nothing else. Reading the config here would parse YAML on every focus.
    if is_focus && is_decided(env, &workspace_id) {
        return Ok(());
    }

    let (config_path, config) = load_config(env)?;
    if config_path.is_none() {
        return Ok(());
    }

    let done = |msg: Option<&str>| {
        if let Some(msg) = msg {
            log(msg);
        }
        if is_focus {
            mark_decided(env, &workspace_id);
        }
        Ok(())
    };

    let Some(info) = get_workspace_info(env, &workspace_id)? else {
        return done(None); // not a worktree workspace
    };
    let Some(checkout_path) = info.checkout_path.clone() else {
        return done(None); // not a worktree workspace
    };
    if !info.is_linked_worktree {
        return done(None); // the repo's main checkout — never touch
    }

    // The branch is only needed (and only worth an extra herdr query) when some
    // workspace uses branch-based layoutMatching.
    let branch = if config.workspaces.iter().any(|ws| !ws.layout_matching.is_empty()) {
        get_worktree_branch(env, Some(&workspace_id), Some(&checkout_path))
    } else {
        None
    };

    let target = MatchTarget {
        checkout_path: Some(checkout_path.clone()),
        repo_root: info.repo_root.clone(),
        repo_name: info.repo_name.clone(),
        branch,
    };
    let Some(layout) = match_workspace_layout(&config, &target) else {
        return done(Some(&format!(
            "no workspace/default layout matches {}; skipping",
            checkout_path
        )));
    };

    if info.tab_count != Some(1) || info.pane_count != Some(1) {
        return done(Some(&format!(
            "workspace {} is not a fresh 1-tab/1-pane workspace; skipping",
            workspace_id
        )));
    }

    if !claim_apply(env, &checkout_path)? {
        return done(Some(&format!("layout already applied for {}; skipping", checkout_path)));
    }

    log(&format!("applying layout \"{}\" to {}", layout.id, checkout_path));
    let apply = || -> Result<Value, String> {
        let target = resolve_target(
            env,
            Some(&workspace_id),
            p.tab_id.as_deref().or(info.active_tab_id.as_deref()),
            p.root_pane_id.as_deref(),
            Some(&checkout_path),
        )?;
        apply_layout(env, layout, &target, &|m| log(m))
    };
    match apply() {
        Ok(summary) => {
            if is_focus {
                mark_decided(env, &workspace_id);
            }
            println!("{}", summary);
            Ok(())
        }
        Err(err) => {
            release_apply(env, &checkout_path); // allow a retry on transient failure
            // A failed apply is otherwise invisible: the hook runs headless and
            // its stderr only reaches the plugin log.
            notify(
                env,
                "Workspace layout failed",
                &format!("Layout \"{}\" for {}: {}", layout.id, checkout_path, err),
            );
            Err(err)
        }
    }
}

// --- apply ---------------------------------------------------------------------
// Manual `apply` action. Applies a layout to the current (or a target)
// workspace. Layout selection order:
//   1. CLI argument           (herdr-workspace-manager apply <layoutId>)
//   2. HERDR_WSM_LAYOUT env
//   3. the workspace's default layout, matched by its checkout path
//
// Target selection (each overridable for testing):
//   workspace: HERDR_WSM_WORKSPACE | HERDR_WORKSPACE_ID | context
//   tab:       HERDR_WSM_TAB       | HERDR_TAB_ID       | workspace's first tab
//   pane:      HERDR_WSM_PANE      | HERDR_PANE_ID      | tab's first pane
//   cwd:       HERDR_WSM_CWD       | workspace checkout path
fn cmd_apply(env: &Env, layout_arg: Option<&str>) -> Result<(), String> {
    let (config_path, config) = load_config(env)?;
    if config_path.is_none() {
        return Err("no config file found".to_string());
    }

    let ctx = context_json(env);
    let ctx_str = |path: &[&str]| -> Option<String> {
        let mut cur = &ctx;
        for key in path {
            cur = cur.get(key)?;
        }
        cur.as_str().map(String::from)
    };
    let workspace_id = non_empty(env.get("HERDR_WSM_WORKSPACE"))
        .map(String::from)
        .or_else(|| non_empty(env.get("HERDR_WORKSPACE_ID")).map(String::from))
        .or_else(|| ctx_str(&["workspace", "workspace_id"]))
        .or_else(|| ctx_str(&["workspace_id"]));
    let tab_id = non_empty(env.get("HERDR_WSM_TAB"))
        .map(String::from)
        .or_else(|| non_empty(env.get("HERDR_TAB_ID")).map(String::from))
        .or_else(|| ctx_str(&["tab", "tab_id"]));
    let pane_id = non_empty(env.get("HERDR_WSM_PANE"))
        .map(String::from)
        .or_else(|| non_empty(env.get("HERDR_PANE_ID")).map(String::from))
        .or_else(|| ctx_str(&["pane", "pane_id"]));
    let cwd = non_empty(env.get("HERDR_WSM_CWD"))
        .map(String::from)
        .or_else(|| ctx_str(&["workspace", "worktree", "checkout_path"]));

    let target = resolve_target(
        env,
        workspace_id.as_deref(),
        tab_id.as_deref(),
        pane_id.as_deref(),
        cwd.as_deref(),
    )?;

    let explicit_id = layout_arg
        .filter(|a| !a.is_empty())
        .map(String::from)
        .or_else(|| non_empty(env.get("HERDR_WSM_LAYOUT")).map(String::from));
    let layout = match explicit_id {
        Some(id) => find_layout(&config, &id).ok_or_else(|| {
            let have: Vec<&str> = config.layouts.iter().map(|l| l.id.as_str()).collect();
            format!(
                "layout \"{}\" not found (have: {})",
                id,
                if have.is_empty() { "none".to_string() } else { have.join(", ") }
            )
        })?,
        None => {
            let branch = if config.workspaces.iter().any(|ws| !ws.layout_matching.is_empty()) {
                get_worktree_branch(env, Some(&target.workspace_id), target.cwd.as_deref())
            } else {
                None
            };
            let match_target = MatchTarget {
                checkout_path: target.cwd.clone(),
                repo_root: ctx_str(&["workspace", "worktree", "repo_root"]),
                repo_name: ctx_str(&["workspace", "worktree", "repo_name"]),
                branch,
            };
            let Some(layout) = match_workspace_layout(&config, &match_target) else {
                return Err(format!(
                    "no layout id given and no workspace default matches {}",
                    target.cwd.as_deref().unwrap_or("null")
                ));
            };
            layout
        }
    };

    log(&format!("applying layout \"{}\" to workspace {}", layout.id, target.workspace_id));
    let summary = apply_layout(env, layout, &target, &|m| log(m))?;
    println!("{}", summary);
    Ok(())
}

// --- validate ------------------------------------------------------------------
// `validate` action: parse + validate the config and print a human-readable
// summary of the resolved layouts and workspace mappings.
fn cmd_validate(env: &Env) -> i32 {
    let (config_path, config) = match load_config(env) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("Config error: {}", err);
            return 1;
        }
    };
    let Some(config_path) = config_path else {
        println!("No config file found. Looked in:");
        for c in config_candidates(env) {
            println!("  - {}", c.display());
        }
        return 0;
    };

    println!("Config: {}\n", config_path.display());

    print_layouts(&config);
    print_workspaces(&config);
    println!("\nConfig is valid.");
    0
}

fn describe_size(size: &Size) -> String {
    match size {
        Size::Cells(v) => format!("size:{}c", v),
        Size::Percent(v) => format!("size:{}%", v),
    }
}

fn describe_env(env: &std::collections::BTreeMap<String, String>) -> String {
    let names: Vec<&str> = env.keys().map(String::as_str).collect();
    format!("env:{}", names.join(","))
}

fn describe_pane(pane: &Pane) -> String {
    let mut bits = vec![pane.title.clone().unwrap_or_else(|| "(untitled)".to_string())];
    if let Some(split) = pane.split {
        bits.push(format!("split:{}", split.as_str()));
    }
    if let Some(size) = &pane.size {
        bits.push(describe_size(size));
    } else if let Some(ratio) = pane.ratio {
        bits.push(format!("ratio:{}", ratio));
    }
    if pane.setup {
        bits.push("setup".to_string());
    }
    if !pane.env.is_empty() {
        bits.push(describe_env(&pane.env));
    }
    if let Some(agent) = &pane.agent {
        let mut agent_bits = format!("@{}", agent.kind);
        if let Some(name) = &agent.name {
            agent_bits.push_str(&format!(" as \"{}\"", name));
        }
        if !agent.args.is_empty() {
            agent_bits.push_str(&format!(" -- {}", agent.args.join(" ")));
        }
        bits.push(agent_bits);
        if let Some(prompt) = &agent.prompt {
            let wait = match agent.prompt_timeout_ms {
                Some(ms) => format!(" (waits up to {}ms)", ms),
                None => String::new(),
            };
            bits.push(format!("prompt: \"{}\"{}", prompt, wait));
        }
    }
    if let Some(command) = &pane.command {
        bits.push(format!("$ {}", command));
        if !pane.persist {
            bits.push("(pane closes when it exits)".to_string());
        }
    }
    bits.join("  ")
}

fn print_layouts(config: &Config) {
    println!("Layouts ({}):", config.layouts.len());
    for layout in &config.layouts {
        let is_global = config.global_layout.as_deref() == Some(layout.id.as_str());
        println!("  {}{}", layout.id, if is_global { " (global default)" } else { "" });
        if !layout.env.is_empty() {
            println!("    {}", describe_env(&layout.env));
        }
        if let Some(setup) = &layout.setup {
            println!(
                "    setup: {}{}",
                setup.script(),
                if setup.blocking { " (blocking)" } else { "" }
            );
        }
        for (ti, tab) in layout.tabs.iter().enumerate() {
            println!("    tab {}: {}", ti, tab.title.as_deref().unwrap_or("(untitled)"));
            for (pj, pane) in tab.panes.iter().enumerate() {
                println!("      pane {}: {}", pj, describe_pane(pane));
            }
        }
    }
}

fn print_workspaces(config: &Config) {
    println!("\nWorkspaces ({}):", config.workspaces.len());
    for ws in &config.workspaces {
        let target: Vec<String> = [
            ws.repo.as_ref().map(|r| format!("repo:{}", r)),
            ws.path.as_ref().map(|p| format!("path:{}", p)),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!("  {} -> {}", target.join("  "), ws.default_layout.as_deref().unwrap_or("(none)"));
        for rule in &ws.layout_matching {
            let title = rule.title.as_ref().map(|t| format!(" ({})", t)).unwrap_or_default();
            println!("      branch ~ {} -> {}{}", rule.worktree_pattern, rule.layout, title);
        }
    }
}

// --- remove-gone -----------------------------------------------------------------

#[derive(Default)]
struct RemoveGoneFlags {
    dry_run: bool,
    confirm: bool,
    force: bool,
    no_fetch: bool,
    workspace: Option<String>,
    preview: bool,
    help: bool,
}

fn parse_remove_gone_flags(argv: &[String]) -> Result<RemoveGoneFlags, String> {
    let mut flags = RemoveGoneFlags::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--dry-run" => flags.dry_run = true,
            "--confirm" => flags.confirm = true,
            "--force" => flags.force = true,
            "--no-fetch" => flags.no_fetch = true,
            "--preview" => flags.preview = true,
            "--workspace" => {
                i += 1;
                flags.workspace = argv.get(i).cloned();
            }
            "-h" | "--help" => flags.help = true,
            other => return Err(format!("unknown option \"{}\" for remove-gone (see --help)", other)),
        }
        i += 1;
    }
    Ok(flags)
}

// Ask a y/n question on the terminal. Reads from stdin so a piped `yes` works
// too; an empty answer or closed stdin (no input) counts as "no".
fn confirm(question: &str) -> bool {
    print!("{}", question);
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

fn cmd_remove_gone(env: &Env, argv: &[String]) -> Result<(), String> {
    let flags = parse_remove_gone_flags(argv)?;
    if flags.help {
        print!("{}", USAGE);
        return Ok(());
    }

    // `remove-gone --preview` is the plugin action: list only, remove NOTHING —
    // plugin actions run headless and can't prompt, so removal lives in the CLI.
    if flags.preview {
        let collected = collect_gone_worktrees(env, !env.is_set("HERDR_WSM_NO_FETCH"), &|m| log(m))?;
        print!("{}", format_preview(&repo_display_name(&collected.repo), &collected.candidates, false));
        if !collected.candidates.is_empty() {
            println!(
                "\nPreview only — nothing was removed. Run `herdr-workspace-manager remove-gone` to remove the eligible ones."
            );
        }
        return Ok(());
    }

    let cli_env = match &flags.workspace {
        Some(ws) => {
            let mut e = env.clone();
            e.set("HERDR_WSM_WORKSPACE", ws);
            e
        }
        None => env.clone(),
    };
    let fetch = !(flags.no_fetch || env.is_set("HERDR_WSM_NO_FETCH"));
    let collected = collect_gone_worktrees(&cli_env, fetch, &|m| log(m))?;
    let repo_name = repo_display_name(&collected.repo);

    print!("{}", format_preview(&repo_name, &collected.candidates, flags.force));
    if collected.candidates.is_empty() {
        return Ok(());
    }

    if flags.dry_run {
        println!("\nDry run — nothing was removed.");
        return Ok(());
    }

    let removable = removable_candidates(&collected.candidates, flags.force);
    if removable.is_empty() {
        println!("\nNothing eligible to remove (see the notes above).");
        return Ok(());
    }

    if !flags.confirm {
        let ok = confirm(&format!("\nRemove {} worktree(s)? [y/N] ", removable.len()));
        if !ok {
            println!("Aborted — nothing was removed.");
            return Ok(());
        }
    }

    let (removed, skipped) =
        apply_removals(&cli_env, &collected.candidates, flags.force, &|m| log(m));
    print!("\n{}", format_apply_result(&repo_name, &removed, &skipped));
    Ok(())
}
