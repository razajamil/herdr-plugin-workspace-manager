// Live integration test: applies the plugin to a REAL herdr linked worktree
// with REAL panes, driving the real event hook the way the herdr UI does
// (a workspace.focused event carrying only a workspace id), and verifies the
// layout + that the pane commands actually ran.
//
// Fully isolated and self-cleaning:
//   - creates a throwaway git repo + a real `herdr worktree create`
//   - drives the `event` subcommand with a synthetic workspace.focused payload
//     + a temp config/state dir, so the hook must query the workspace for facts
//   - asserts tab/pane structure, command execution (marker FILES), blocking
//     setup ordering, idempotency (a second event is a no-op)
//   - removes the worktree, closes the source workspace, deletes temp dirs
//
// Skips automatically when no herdr server is running.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

const EVENT_BIN: &str = env!("CARGO_BIN_EXE_herdr-workspace-manager");

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

fn herdr(args: &[&str]) -> Value {
    let out = Command::new(herdr_bin()).args(args).output().expect("spawn herdr");
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() && stdout.trim().is_empty() {
        panic!("herdr {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    if stdout.trim().is_empty() {
        return Value::Null;
    }
    let parsed: Value = serde_json::from_str(stdout.trim()).expect("herdr JSON");
    parsed.get("result").cloned().unwrap_or(Value::Null)
}

fn server_up() -> bool {
    Command::new(herdr_bin())
        .args(["workspace", "list"])
        .output()
        .map(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("\"workspaces\"")
        })
        .unwrap_or(false)
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {:?} failed", args);
}

fn wait_for_files(files: &[PathBuf], deadline: Duration) {
    let start = Instant::now();
    loop {
        let missing: Vec<&PathBuf> = files.iter().filter(|f| !f.exists()).collect();
        if missing.is_empty() {
            return;
        }
        if start.elapsed() > deadline {
            panic!("timed out waiting for marker files: {:?}", missing);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn base36(mut n: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    if out.is_empty() {
        out.push(b'0');
    }
    String::from_utf8(out).unwrap()
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

// Cleanup that also runs when an assertion panics mid-test.
struct Cleanup {
    workspace_id: Option<String>,
    source_workspace_id: Option<String>,
    worktree_parent_dir: Option<PathBuf>,
    tmp_root: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(id) = &self.workspace_id {
            let _ = Command::new(herdr_bin())
                .args(["worktree", "remove", "--workspace", id, "--force"])
                .output();
        }
        if let Some(id) = &self.source_workspace_id {
            let _ = Command::new(herdr_bin()).args(["workspace", "close", id]).output();
        }
        let _ = fs::remove_dir_all(&self.tmp_root);
        // Remove the now-empty ~/.herdr/worktrees/<repo> parent dir herdr created.
        if let Some(parent) = &self.worktree_parent_dir {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

#[test]
fn applies_a_layout_to_a_real_worktree_via_a_workspace_focused_event() {
    if !server_up() {
        eprintln!("skipping: no herdr server running");
        return;
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let tmp_root =
        std::env::temp_dir().join(format!("wsc-itest-{}-{}", std::process::id(), base36(now)));
    fs::create_dir_all(&tmp_root).unwrap();
    let tmp_root = tmp_root.canonicalize().unwrap();
    let repo_name = format!("wsc-it-{}", tmp_root.file_name().unwrap().to_string_lossy());
    let repo = tmp_root.join(&repo_name);
    let markers = tmp_root.join("markers");
    let state_dir = tmp_root.join("state");
    let config_path = tmp_root.join("config.yml");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&markers).unwrap();

    let mut cleanup = Cleanup {
        workspace_id: None,
        source_workspace_id: None,
        worktree_parent_dir: None,
        tmp_root: tmp_root.clone(),
    };

    // A real git repo so herdr can create a linked worktree from it.
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.email=t@t.co",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );

    let m = |name: &str| markers.join(name);
    let token = format!("TOK{}", base36(now));
    // Match by repo root; structure mirrors a real layout but with cheap marker
    // commands. setup sleeps 1s so the blocking-ordering check is observable.
    //
    // The `itest` layout is selected by a layoutMatching rule on the branch
    // (`itest`), NOT by defaultLayout (a 1-tab decoy). This proves the hook
    // resolves the worktree's branch live -- if it couldn't, it would fall back
    // to the decoy and the tab/pane assertions below would fail.
    fs::write(
        &config_path,
        format!(
            r#"
layouts:
  - id: itest
    setup:
      command: sleep 1; echo done > {setup_done}
      blocking: true
    tabs:
      - title: alpha
        panes:
          - title: a0
            setup: true
            command: echo a0 > {a0}; printf 'A0OUT_%s\n' '{token}'
          - title: a1
            split: vertical
            size: 10
            command: echo a1 > {a1}
      - title: beta
        panes:
          - title: b0
            command: echo b0 > {b0}
          - title: b1
            split: horizontal
            command: echo b1 > {b1}
  - id: itest-decoy
    tabs:
      - title: decoy
        panes:
          - title: only
workspaces:
  - repo: {repo}
    defaultLayout: itest-decoy
    layoutMatching:
      - title: branch match
        worktreePattern: itest
        layout: itest
"#,
            setup_done = m("setup.done").display(),
            a0 = m("a0.cmd").display(),
            a1 = m("a1.cmd").display(),
            b0 = m("b0.cmd").display(),
            b1 = m("b1.cmd").display(),
            token = token,
            repo = repo.display(),
        ),
    )
    .unwrap();

    // 1. Create a real linked worktree (with --no-focus, so the real installed
    //    plugin doesn't act — and it wouldn't match this temp repo anyway).
    let repo_str = repo.to_str().unwrap();
    let created = herdr(&[
        "worktree", "create", "--cwd", repo_str, "--branch", "itest", "--no-focus", "--json",
    ]);
    let workspace_id = created["worktree"]["open_workspace_id"].as_str().unwrap().to_string();
    cleanup.workspace_id = Some(workspace_id.clone());
    cleanup.worktree_parent_dir = created["worktree"]["path"]
        .as_str()
        .map(|p| Path::new(p).parent().unwrap().to_path_buf()); // ~/.herdr/worktrees/<repo>
    // herdr also opens the source repo as a workspace; find it for cleanup.
    let all = herdr(&["workspace", "list"]);
    cleanup.source_workspace_id = all["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| {
            w["worktree"]["checkout_path"].as_str() == Some(repo_str)
                && w["workspace_id"].as_str() != Some(workspace_id.as_str())
        })
        .and_then(|w| w["workspace_id"].as_str().map(String::from));

    // 2. Drive the hook the way the UI does: a workspace.focused event that
    //    carries only the workspace id. The hook must query the workspace.
    let run_event = |event_name: &str| {
        let event_json = format!(
            r#"{{"event":"{ev}","data":{{"type":"{ev}","workspace_id":"{ws}"}}}}"#,
            ev = event_name.replace('.', "_"),
            ws = workspace_id,
        );
        Command::new(EVENT_BIN)
            .arg("event")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_TAB_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env("HERDR_WSM_CONFIG", &config_path)
            .env("HERDR_PLUGIN_STATE_DIR", &state_dir)
            .env("HERDR_PLUGIN_EVENT", event_name)
            .env("HERDR_PLUGIN_EVENT_JSON", event_json)
            .env("HERDR_WSM_SETUP_TIMEOUT_MS", "20000")
            .output()
            .expect("spawn event hook")
    };

    let run = run_event("workspace.focused");
    assert!(
        run.status.success(),
        "event hook failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let summary: Value =
        serde_json::from_str(stdout.trim().lines().last().unwrap()).expect("summary JSON");
    // The branch-matched layout won over the decoy defaultLayout -> the hook
    // resolved the worktree's branch (`itest`) from the live server.
    assert_eq!(summary["layoutId"], "itest");

    // 3. Structure: two tabs (alpha, beta), 2 panes each.
    let tabs = herdr(&["tab", "list", "--workspace", &workspace_id]);
    let mut labels: Vec<&str> =
        tabs["tabs"].as_array().unwrap().iter().filter_map(|t| t["label"].as_str()).collect();
    labels.sort();
    assert_eq!(labels, vec!["alpha", "beta"]);
    let panes = herdr(&["pane", "list", "--workspace", &workspace_id]);
    let pane_list = panes["panes"].as_array().unwrap();
    assert_eq!(pane_list.len(), 4, "expected 4 panes total");
    for tab in tabs["tabs"].as_array().unwrap() {
        let count =
            pane_list.iter().filter(|p| p["tab_id"] == tab["tab_id"]).count();
        assert_eq!(count, 2, "tab {} should have 2 panes", tab["label"]);
    }

    // 3b. Sizing: a1 asked for a fixed 10-column width, so it must end up
    //     narrower than its sibling a0 (which keeps the rest). This exercises
    //     the live cells->ratio conversion (pane_extent) end-to-end.
    let t0p0 = summary["handles"]["t0p0"].as_str().unwrap();
    let t0p1 = summary["handles"]["t0p1"].as_str().unwrap();
    let alpha = herdr(&["pane", "layout", "--pane", t0p0]);
    let width_of = |id: &str| -> Option<f64> {
        alpha["layout"]["panes"]
            .as_array()?
            .iter()
            .find(|p| p["pane_id"].as_str() == Some(id))?["rect"]["width"]
            .as_f64()
    };
    let a0w = width_of(t0p0);
    let a1w = width_of(t0p1);
    assert!(
        matches!((a0w, a1w), (Some(a0), Some(a1)) if a1 < a0),
        "sized pane a1 ({:?}) should be narrower than a0 ({:?})",
        a1w,
        a0w
    );

    // 4. Idempotency: a second event (e.g. the workspace.created the CLI also
    //    fires) must be a no-op via the claim, not a doubled layout.
    let dup = run_event("workspace.created");
    let dup_stderr = String::from_utf8_lossy(&dup.stderr).into_owned();
    assert!(dup.status.success(), "dedupe run failed:\n{}", dup_stderr);
    // No re-apply: the claim (race) or the freshness guard (sequential) skips it.
    assert!(
        dup_stderr.contains("already applied") || dup_stderr.contains("not a fresh"),
        "duplicate event should be a no-op, got:\n{}",
        dup_stderr
    );
    assert_eq!(
        herdr(&["pane", "list", "--workspace", &workspace_id])["panes"].as_array().unwrap().len(),
        4,
        "duplicate event must not add panes"
    );

    // 5. Command execution: each pane command wrote its marker file.
    let files: Vec<PathBuf> =
        ["setup.done", "a0.cmd", "a1.cmd", "b0.cmd", "b1.cmd"].iter().map(|f| m(f)).collect();
    wait_for_files(&files, Duration::from_secs(20));

    // 6. Blocking setup finished (1s sleep) before the later panes were built.
    assert!(
        mtime(&m("b1.cmd")) >= mtime(&m("setup.done")),
        "blocking setup should complete before later panes"
    );

    // 7. Terminal-level proof: the setup pane's command produced output
    //    (A0OUT_<token> via printf %s — present in output, not the echoed input).
    let marker = format!("A0OUT_{}", token);
    let w = Command::new(herdr_bin())
        .args(["wait", "output", t0p0, "--match", &marker, "--timeout", "10000"])
        .output()
        .expect("spawn herdr wait");
    let w_stdout = String::from_utf8_lossy(&w.stdout);
    assert!(
        w_stdout.contains("output_matched") || w_stdout.contains("matched_line"),
        "setup pane should print {}",
        marker
    );
}
