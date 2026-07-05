// Shared logic for the event hook and the manual `apply` action: figure out
// which workspace/tab/pane to build into, then build + run the plan.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use crate::config::{resolve_path, Layout};
use crate::env::Env;
use crate::herdr::run_herdr_json;
use crate::plan::build_plan;
use crate::runner::{execute_plan, Logger, Target};

fn try_json(text: Option<&str>) -> Option<Value> {
    serde_json::from_str(text?).ok()
}

#[derive(Debug, Default)]
pub struct EventPayload {
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub root_pane_id: Option<String>,
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str().map(String::from)
}

// Pull whatever the worktree.created event gives us out of the environment.
// The real event wraps its records under `data` and carries `workspace`
// (with `active_tab_id` + nested `worktree`) and a top-level `worktree`; it
// does NOT include tab/root_pane records, so those are resolved live later.
//
// Crucially this reads ONLY the event payload -- never the ambient
// HERDR_PANE_ID / HERDR_TAB_ID / HERDR_WORKSPACE_ID. Those describe whichever
// pane happened to be focused when the event fired (e.g. your agent pane), and
// using them would build the layout into the wrong pane. The target pane is
// always resolved from the new worktree's workspace.
pub fn get_event_payload(env: &Env) -> EventPayload {
    let ev = try_json(env.get("HERDR_PLUGIN_EVENT_JSON")).unwrap_or(Value::Null);
    let data = ev.get("data").filter(|d| !d.is_null()).unwrap_or(&ev);
    let workspace = data.get("workspace").cloned().unwrap_or(Value::Null);
    EventPayload {
        // workspace.focused carries only data.workspace_id; created events nest it.
        workspace_id: str_at(&workspace, &["workspace_id"])
            .or_else(|| str_at(&workspace, &["id"]))
            .or_else(|| str_at(data, &["workspace_id"])),
        tab_id: str_at(data, &["tab", "tab_id"]).or_else(|| str_at(&workspace, &["active_tab_id"])),
        root_pane_id: str_at(data, &["root_pane", "pane_id"])
            .or_else(|| str_at(data, &["pane", "pane_id"])),
    }
}

#[derive(Debug)]
pub struct WorkspaceInfo {
    pub checkout_path: Option<String>,
    pub repo_root: Option<String>,
    pub repo_name: Option<String>,
    pub is_linked_worktree: bool,
    pub tab_count: Option<i64>,
    pub pane_count: Option<i64>,
    pub active_tab_id: Option<String>,
}

// Look up a workspace's worktree facts + freshness by id. This is the source of
// truth (the workspace.focused payload only gives an id), and it's also more
// reliable than parsing per-event payload shapes for the created events.
pub fn get_workspace_info(env: &Env, workspace_id: &str) -> Result<Option<WorkspaceInfo>, String> {
    let mut ws = run_herdr_json(&["workspace", "get", workspace_id], env)
        .ok()
        .and_then(|r| r.get("workspace").cloned())
        .filter(|w| !w.is_null());
    if ws.is_none() {
        let list = run_herdr_json(&["workspace", "list"], env)?;
        ws = list
            .get("workspaces")
            .and_then(Value::as_array)
            .and_then(|all| {
                all.iter()
                    .find(|w| w.get("workspace_id").and_then(Value::as_str) == Some(workspace_id))
            })
            .cloned();
    }
    let Some(ws) = ws else { return Ok(None) };
    let wt = ws.get("worktree").cloned().unwrap_or(Value::Null);
    Ok(Some(WorkspaceInfo {
        checkout_path: str_at(&wt, &["checkout_path"]),
        repo_root: str_at(&wt, &["repo_root"]),
        repo_name: str_at(&wt, &["repo_name"]),
        is_linked_worktree: wt
            .get("is_linked_worktree")
            .map(|v| v.as_bool().unwrap_or(!v.is_null()))
            .unwrap_or(false),
        tab_count: ws.get("tab_count").and_then(Value::as_i64),
        pane_count: ws.get("pane_count").and_then(Value::as_i64),
        active_tab_id: str_at(&ws, &["active_tab_id"]),
    }))
}

// The git branch of a workspace's worktree. herdr's `workspace get`/`list` omit
// the branch from the nested worktree record, so we read it from `worktree list`
// (scoped to the workspace's repo) and match on the open workspace id, falling
// back to the checkout path. Returns None for a detached HEAD or when it can't
// be resolved -- callers then fall back to the workspace's defaultLayout. This
// is only queried when the config actually uses branch-based layoutMatching.
pub fn get_worktree_branch(
    env: &Env,
    workspace_id: Option<&str>,
    checkout_path: Option<&str>,
) -> Option<String> {
    let mut args = vec!["worktree", "list", "--json"];
    if let Some(id) = workspace_id {
        args.push("--workspace");
        args.push(id);
    }
    let result = run_herdr_json(&args, env).ok()?;
    let worktrees = result.get("worktrees").and_then(Value::as_array)?;
    let by_workspace = workspace_id.and_then(|id| {
        worktrees
            .iter()
            .find(|w| w.get("open_workspace_id").and_then(Value::as_str) == Some(id))
    });
    let by_path = checkout_path.and_then(|checkout| {
        worktrees.iter().find(|w| {
            w.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| resolve_path(p) == resolve_path(checkout))
        })
    });
    let wt = by_workspace.or(by_path)?;
    str_at(wt, &["branch"]).filter(|b| !b.is_empty()) // "" (detached) -> None
}

// --- Idempotency + freshness guards --------------------------------------
// Both worktree.created and workspace.created fire for one CLI creation, and
// workspace.created can also fire on restore. These guards ensure a layout is
// applied exactly once per worktree, only when the workspace is brand new.

fn state_dir(env: &Env) -> PathBuf {
    match env.get("HERDR_PLUGIN_STATE_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("herdr-wsc-state"),
    }
}

// "Decided" cache (by workspace id) for the high-frequency workspace.focused
// event: once we've handled a workspace once, repeat focuses skip immediately
// without re-querying. Workspace ids are stable per logical workspace, so this
// is safe; the persistent `claim` (by checkout path) remains the real guard.
fn decided_path(env: &Env, workspace_id: &str) -> PathBuf {
    let safe: String = workspace_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' })
        .collect();
    state_dir(env).join("decided").join(safe)
}

pub fn is_decided(env: &Env, workspace_id: &str) -> bool {
    decided_path(env, workspace_id).exists()
}

pub fn mark_decided(env: &Env, workspace_id: &str) {
    let _ = fs::create_dir_all(state_dir(env).join("decided"));
    let _ = fs::create_dir(decided_path(env, workspace_id)); // already marked -> fine
}

fn applied_dir(env: &Env) -> PathBuf {
    state_dir(env).join("applied")
}

fn claim_path(env: &Env, checkout_path: &str) -> PathBuf {
    let key = sha1_smol::Sha1::from(resolve_path(checkout_path).as_bytes()).digest().to_string();
    applied_dir(env).join(key)
}

fn claim_meta_path(dir: &Path) -> PathBuf {
    dir.join("meta.json")
}

struct Identity {
    ino: Option<String>,
    birthtime_ms: Option<i64>,
}

// The worktree's filesystem identity. A delete+recreate at the same path gets a
// new inode (a brand-new directory) and, on filesystems that record it, a new
// birth time -- either of which marks the claim from the previous worktree as
// stale. Returns nothing if the path can't be stat'd (e.g. already gone), in
// which case callers treat the identity as unknowable.
fn worktree_identity(checkout_path: &str) -> Identity {
    match fs::metadata(checkout_path) {
        Ok(md) => {
            #[cfg(unix)]
            let ino = {
                use std::os::unix::fs::MetadataExt;
                Some(md.ino().to_string())
            };
            #[cfg(not(unix))]
            let ino: Option<String> = None;
            let birthtime_ms = md
                .created()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .filter(|&ms| ms > 0);
            Identity { ino, birthtime_ms }
        }
        Err(_) => Identity { ino: None, birthtime_ms: None },
    }
}

fn read_claim_meta(dir: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(claim_meta_path(dir)).ok()?).ok()
}

fn write_claim_meta(dir: &Path, checkout_path: &str) {
    // Best effort -- a missing record just falls back to the mtime heuristic.
    let identity = worktree_identity(checkout_path);
    let meta = json!({
        "path": resolve_path(checkout_path),
        "ino": identity.ino,
        "birthtimeMs": identity.birthtime_ms,
    });
    let _ = fs::write(claim_meta_path(dir), meta.to_string());
}

// Is an existing claim stale -- i.e. left over from a *previous* worktree that
// lived at this same path and has since been removed and recreated? A worktree
// can be removed by this plugin, another plugin, or the user directly, and none
// of those reliably emit an event, so we decide from filesystem ground truth at
// claim time rather than trying to catch the removal.
fn is_stale_claim(dir: &Path, checkout_path: &str) -> bool {
    let cur = worktree_identity(checkout_path);
    if cur.ino.is_none() && cur.birthtime_ms.is_none() {
        return false; // identity unknowable
    }
    let meta = read_claim_meta(dir);
    // A different inode means the directory was recreated.
    let meta_ino = meta.as_ref().and_then(|m| m.get("ino")).and_then(Value::as_str);
    if let (Some(recorded), Some(current)) = (meta_ino, cur.ino.as_deref()) {
        if recorded != current {
            return true;
        }
    }
    // The current worktree was born after the claim was established -> it's a
    // newer instance at the same path. Use the recorded birth time, or for legacy
    // claims (no record) fall back to the claim directory's own mtime.
    let mut claim_at = meta.as_ref().and_then(|m| m.get("birthtimeMs")).and_then(Value::as_i64);
    if claim_at.is_none() {
        claim_at = fs::metadata(dir)
            .ok()
            .and_then(|md| md.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
    }
    matches!((claim_at, cur.birthtime_ms), (Some(at), Some(born)) if born > at)
}

// Atomically claim a worktree for application. Returns true if we won the claim
// (first to see it), false if a valid claim already exists. mkdir is atomic
// across processes, so concurrent worktree.created/workspace.created hooks can't
// both win. The claim persists across restarts so *restored* worktrees are
// skipped -- but a stale claim left by a removed-and-recreated worktree at the
// same path is detected and reset, so the layout is re-applied on recreate.
pub fn claim_apply(env: &Env, checkout_path: &str) -> Result<bool, String> {
    let dir = claim_path(env, checkout_path);
    fs::create_dir_all(applied_dir(env))
        .map_err(|e| format!("cannot create state dir: {}", e))?;
    match fs::create_dir(&dir) {
        Ok(()) => {
            write_claim_meta(&dir, checkout_path);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if is_stale_claim(&dir, checkout_path) {
                let _ = fs::remove_dir_all(&dir);
                fs::create_dir(&dir).map_err(|e| format!("cannot reset stale claim: {}", e))?;
                write_claim_meta(&dir, checkout_path);
                return Ok(true);
            }
            Ok(false)
        }
        Err(e) => Err(format!("cannot claim {}: {}", dir.display(), e)),
    }
}

pub fn release_apply(env: &Env, checkout_path: &str) {
    let _ = fs::remove_dir_all(claim_path(env, checkout_path));
}

#[cfg_attr(not(test), allow(dead_code))] // exercised by the claim tests
pub fn already_applied(env: &Env, checkout_path: &str) -> bool {
    claim_path(env, checkout_path).exists()
}

// Opportunistic GC: drop claims whose worktree no longer exists on disk, so the
// `applied/` directory doesn't grow without bound and a future worktree at a
// reclaimed path starts clean. Pure filesystem, no herdr query -- cheap enough
// to run on each (non-hot-path) hook invocation. Legacy claims with no record
// can't be mapped back to a path, so they're left for is_stale_claim to handle
// on recreate. Returns the number of claims reaped.
pub fn reap_orphan_claims(env: &Env) -> usize {
    let Ok(entries) = fs::read_dir(applied_dir(env)) else {
        return 0; // nothing claimed yet
    };
    let mut reaped = 0;
    for entry in entries.flatten() {
        let claim = entry.path();
        let Some(meta) = read_claim_meta(&claim) else { continue };
        let Some(path) = meta.get("path").and_then(Value::as_str) else {
            continue; // legacy or in-progress claim -- leave it
        };
        if !Path::new(path).exists() {
            let _ = fs::remove_dir_all(&claim);
            reaped += 1;
        }
    }
    reaped
}

fn first_by_number(items: &[Value], key: &str) -> Option<String> {
    let mut sorted: Vec<&Value> = items.iter().collect();
    sorted.sort_by_key(|item| item.get("number").and_then(Value::as_i64).unwrap_or(0));
    sorted.first().and_then(|item| str_at(item, &[key]))
}

// Fill in any missing tab/pane/cwd for a workspace by querying the live server.
pub fn resolve_target(
    env: &Env,
    workspace_id: Option<&str>,
    tab_id: Option<&str>,
    root_pane_id: Option<&str>,
    cwd: Option<&str>,
) -> Result<Target, String> {
    let Some(workspace_id) = workspace_id.filter(|id| !id.is_empty()) else {
        return Err("could not determine target workspace id".to_string());
    };

    // herdr ids are workspace-prefixed ("w9:t1", "w9:p1"). Discard any tab/pane
    // id that doesn't belong to this workspace -- it's stale/ambient context and
    // must never be built into. Re-resolve from the workspace instead.
    let prefix = format!("{}:", workspace_id);
    let mut resolved_tab = tab_id.filter(|t| t.starts_with(&prefix)).map(String::from);
    let mut resolved_pane = root_pane_id.filter(|p| p.starts_with(&prefix)).map(String::from);
    let mut resolved_cwd = cwd.map(String::from);

    if resolved_cwd.is_none() || resolved_tab.is_none() {
        let ws_list = run_herdr_json(&["workspace", "list"], env)?;
        let ws = ws_list.get("workspaces").and_then(Value::as_array).and_then(|all| {
            all.iter()
                .find(|w| w.get("workspace_id").and_then(Value::as_str) == Some(workspace_id))
        });
        if let Some(ws) = ws {
            resolved_tab = resolved_tab.or_else(|| str_at(ws, &["active_tab_id"]));
            resolved_cwd = resolved_cwd.or_else(|| str_at(ws, &["worktree", "checkout_path"]));
        }
    }

    if resolved_tab.is_none() {
        let tabs = run_herdr_json(&["tab", "list", "--workspace", workspace_id], env)?;
        resolved_tab = tabs
            .get("tabs")
            .and_then(Value::as_array)
            .and_then(|t| first_by_number(t, "tab_id"));
    }
    let Some(resolved_tab) = resolved_tab else {
        return Err(format!("no tab found for workspace {}", workspace_id));
    };

    if resolved_pane.is_none() {
        let panes = run_herdr_json(&["pane", "list", "--workspace", workspace_id], env)?;
        let in_tab: Vec<&Value> = panes
            .get("panes")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter(|p| {
                        p.get("tab_id").and_then(Value::as_str) == Some(resolved_tab.as_str())
                    })
                    .collect()
            })
            .unwrap_or_default();
        resolved_pane = in_tab.first().and_then(|p| str_at(p, &["pane_id"]));
        if resolved_cwd.is_none() {
            resolved_cwd = in_tab.first().and_then(|p| str_at(p, &["cwd"]));
        }
    }
    let Some(resolved_pane) = resolved_pane else {
        return Err(format!("no root pane found for tab {}", resolved_tab));
    };

    Ok(Target {
        workspace_id: workspace_id.to_string(),
        root_tab: resolved_tab,
        root_pane: resolved_pane,
        cwd: resolved_cwd,
    })
}

pub fn apply_layout(env: &Env, layout: &Layout, target: &Target, log: Logger) -> Result<Value, String> {
    let plan = build_plan(layout, target.cwd.as_deref());
    execute_plan(&plan, target, env, log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Unique temp dir per call (no external tempdir dep).
    fn mkdtemp(prefix: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            n
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // The on-disk claim directory for a checkout path (mirrors claim keying).
    fn claim_dir(state: &Path, checkout: &str) -> PathBuf {
        let key = sha1_smol::Sha1::from(resolve_path(checkout).as_bytes()).digest().to_string();
        state.join("applied").join(key)
    }

    fn state_env(dir: &Path) -> Env {
        Env::from_pairs(&[("HERDR_PLUGIN_STATE_DIR", dir.to_str().unwrap())])
    }

    #[test]
    fn get_event_payload_reads_the_workspace_focused_id_only_shape() {
        let env = Env::from_pairs(&[(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"event":"workspace_focused","data":{"type":"workspace_focused","workspace_id":"wY"}}"#,
        )]);
        assert_eq!(get_event_payload(&env).workspace_id.as_deref(), Some("wY"));
    }

    #[test]
    fn get_event_payload_reads_the_nested_created_shape() {
        let env = Env::from_pairs(&[(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"event":"worktree_created","data":{"workspace":{"workspace_id":"wZ","active_tab_id":"wZ:t1"}}}"#,
        )]);
        let p = get_event_payload(&env);
        assert_eq!(p.workspace_id.as_deref(), Some("wZ"));
        assert_eq!(p.tab_id.as_deref(), Some("wZ:t1"));
    }

    #[test]
    fn claim_apply_is_atomic_and_idempotent_per_checkout_path() {
        let dir = mkdtemp("wsc-claim");
        let env = state_env(&dir);
        let checkout = "/some/worktree/path";
        assert!(!already_applied(&env, checkout));
        assert!(claim_apply(&env, checkout).unwrap(), "first claim wins");
        assert!(already_applied(&env, checkout));
        assert!(!claim_apply(&env, checkout).unwrap(), "second claim loses");
        // a different path is independent
        assert!(claim_apply(&env, "/another/path").unwrap());
        // releasing allows a re-claim (used on transient apply failure)
        release_apply(&env, checkout);
        assert!(!already_applied(&env, checkout));
        assert!(claim_apply(&env, checkout).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn claim_apply_keeps_the_claim_for_the_same_worktree() {
        let state = mkdtemp("wsc-restore");
        let wt = mkdtemp("wsc-wt");
        let env = state_env(&state);
        let wt_str = wt.to_str().unwrap();
        assert!(claim_apply(&env, wt_str).unwrap(), "first claim wins");
        // Same directory, untouched: a restored worktree must still be skipped so we
        // don't clobber an already-arranged workspace.
        assert!(!claim_apply(&env, wt_str).unwrap(), "unchanged worktree stays claimed");
        fs::remove_dir_all(&state).unwrap();
        fs::remove_dir_all(&wt).unwrap();
    }

    #[test]
    fn claim_apply_reclaims_a_recreated_worktree_at_the_same_path() {
        let state = mkdtemp("wsc-stale");
        let wt = mkdtemp("wsc-wt2");
        let env = state_env(&state);
        let wt_str = wt.to_str().unwrap();
        assert!(claim_apply(&env, wt_str).unwrap());
        assert!(!claim_apply(&env, wt_str).unwrap());

        // Simulate the worktree having been removed and recreated at the same path:
        // the recorded identity (here, the inode) no longer matches the live dir.
        fs::write(
            claim_meta_path(&claim_dir(&state, wt_str)),
            json!({ "path": resolve_path(wt_str), "ino": "0", "birthtimeMs": 0 }).to_string(),
        )
        .unwrap();
        assert!(
            claim_apply(&env, wt_str).unwrap(),
            "identity mismatch -> stale claim reset -> re-applies"
        );
        // ...and the refreshed claim now matches again.
        assert!(!claim_apply(&env, wt_str).unwrap(), "fresh claim is honoured");
        fs::remove_dir_all(&state).unwrap();
        fs::remove_dir_all(&wt).unwrap();
    }

    #[test]
    fn reap_orphan_claims_drops_gone_worktrees_keeps_live_ones() {
        let state = mkdtemp("wsc-reap");
        let live = mkdtemp("wsc-live");
        let gone = mkdtemp("wsc-gone");
        let env = state_env(&state);
        let live_str = live.to_str().unwrap().to_string();
        let gone_str = gone.to_str().unwrap().to_string();
        assert!(claim_apply(&env, &live_str).unwrap());
        assert!(claim_apply(&env, &gone_str).unwrap());
        fs::remove_dir_all(&gone).unwrap(); // worktree removed out-of-band

        assert_eq!(reap_orphan_claims(&env), 1, "only the orphaned claim is reaped");
        assert!(!already_applied(&env, &gone_str), "orphan claim removed");
        assert!(already_applied(&env, &live_str), "live claim kept");
        assert_eq!(reap_orphan_claims(&env), 0, "second sweep is a no-op");
        fs::remove_dir_all(&state).unwrap();
        fs::remove_dir_all(&live).unwrap();
    }

    #[test]
    fn decided_cache_is_per_workspace_id() {
        let dir = mkdtemp("wsc-decided");
        let env = state_env(&dir);
        assert!(!is_decided(&env, "w5"));
        mark_decided(&env, "w5");
        assert!(is_decided(&env, "w5"));
        assert!(!is_decided(&env, "w6"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
