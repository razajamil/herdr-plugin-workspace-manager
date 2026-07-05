// Logic for removing "gone" worktrees, shared by the `herdr-workspace-manager
// remove-gone` CLI and the `remove-gone` (preview) plugin action: find the
// *linked* worktrees of the current repo whose remote-tracking branch was
// deleted ("gone"), and remove them.
//
// "Gone" is git's own term: a branch whose configured upstream no longer exists
// after a prune. A branch that never had an upstream — never pushed, never
// tracked — is NOT gone, so those worktrees are left alone, exactly as required.
// The repo's main checkout and the workspace we're invoked from are never
// candidates either.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

use crate::env::Env;
use crate::herdr::run_herdr_json;
use crate::runner::Logger;

pub struct GitOut {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

// Run git in a given directory. Never fails on a non-zero exit — callers decide
// what a failure means (a missing remote shouldn't abort the whole sweep).
pub fn run_git(args: &[&str], cwd: Option<&str>, env: &Env) -> GitOut {
    let mut cmd = Command::new("git");
    cmd.args(args).env_clear().envs(env.iter());
    // Fail fast instead of hanging on a credential prompt in this non-interactive
    // action context; we fall back to cached refs if a fetch can't authenticate.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    match cmd.output() {
        Ok(out) => GitOut {
            status: out.status.code().unwrap_or(0),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            error: None,
        },
        Err(e) => GitOut { status: -1, stdout: String::new(), stderr: String::new(), error: Some(e.to_string()) },
    }
}

// Parse the output of
//   git for-each-ref --format='%(refname:short)\t%(upstream:track,nobracket)' refs/heads
// into the set of local branch names whose upstream is "gone". `upstream:track`
// is empty for a branch with no upstream OR one still in sync — neither appears
// here, which is precisely the "never pushed / still tracked" exclusion.
pub fn parse_gone_branches(text: &str) -> BTreeSet<String> {
    let mut gone = BTreeSet::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let (branch, track) = match line.find('\t') {
            Some(tab) => (line[..tab].trim(), line[tab + 1..].trim()),
            None => (line.trim(), ""),
        };
        if !branch.is_empty() && track == "gone" {
            gone.insert(branch.to_string());
        }
    }
    gone
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub branch: String,
    pub path: Option<String>,
    pub workspace_id: Option<String>,
    pub is_current: bool,
    pub dirty: bool,
    /// The workspace name as shown in herdr; falls back to the branch.
    pub label: String,
}

// From a herdr `worktree list` and the gone-branch set, pick removal candidates.
// Excludes: the repo's main checkout (is_linked_worktree:false), detached
// worktrees (no branch -> no upstream -> never "gone"), and branches whose
// upstream still exists. The invoking workspace is flagged (`is_current`) rather
// than dropped, so the preview can explain why it's left in place.
pub fn select_gone_worktrees(
    worktrees: &[Value],
    gone_branches: &BTreeSet<String>,
    current_workspace_id: Option<&str>,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for wt in worktrees {
        let linked = wt.get("is_linked_worktree").and_then(Value::as_bool).unwrap_or(false);
        if !linked {
            continue;
        }
        let detached = wt.get("is_detached").and_then(Value::as_bool).unwrap_or(false);
        let branch = wt.get("branch").and_then(Value::as_str).unwrap_or("");
        if detached || branch.is_empty() {
            continue;
        }
        if !gone_branches.contains(branch) {
            continue;
        }
        let workspace_id = wt.get("open_workspace_id").and_then(Value::as_str).map(String::from);
        let is_current = matches!((current_workspace_id, workspace_id.as_deref()),
            (Some(cur), Some(open)) if cur == open);
        candidates.push(Candidate {
            branch: branch.to_string(),
            path: wt.get("path").and_then(Value::as_str).map(String::from),
            workspace_id,
            is_current,
            dirty: false,
            label: branch.to_string(),
        });
    }
    candidates
}

// The workspace this action was invoked from. Mirrors the `apply` action's
// resolution order; the env overrides exist for tests.
pub fn resolve_workspace_id(env: &Env) -> Option<String> {
    let ctx: Value = env
        .get("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null);
    let non_empty = |v: Option<&str>| v.filter(|s| !s.is_empty()).map(String::from);
    non_empty(env.get("HERDR_WSM_WORKSPACE"))
        .or_else(|| non_empty(env.get("HERDR_WORKSPACE_ID")))
        .or_else(|| non_empty(ctx.get("workspace").and_then(|w| w.get("workspace_id")).and_then(Value::as_str)))
        .or_else(|| non_empty(ctx.get("workspace_id").and_then(Value::as_str)))
}

// All worktrees of the current repo, plus its `source` (repo_root/repo_name).
// Scoped to the invoking workspace's repo, so this is "the current repo".
pub fn list_repo_worktrees(env: &Env, workspace_id: Option<&str>) -> Result<(Value, Vec<Value>), String> {
    let mut args = vec!["worktree", "list", "--json"];
    if let Some(id) = workspace_id {
        args.push("--workspace");
        args.push(id);
    }
    let result = run_herdr_json(&args, env)?;
    let source = result.get("source").cloned().unwrap_or(Value::Null);
    let worktrees = result.get("worktrees").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok((source, worktrees))
}

// Map of workspace_id -> its display name (label), so candidates can be shown by
// the name you see in herdr rather than just a branch. Best-effort.
pub fn workspace_labels(env: &Env) -> Vec<(String, Option<String>)> {
    let list = run_herdr_json(&["workspace", "list"], env)
        .ok()
        .and_then(|r| r.get("workspaces").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    list.iter()
        .filter_map(|w| {
            let id = w.get("workspace_id").and_then(Value::as_str)?;
            Some((id.to_string(), w.get("label").and_then(Value::as_str).map(String::from)))
        })
        .collect()
}

// Prune stale remote-tracking refs across all remotes so a deleted upstream
// actually reads as "gone". Best-effort: on failure we keep going with whatever
// refs are already cached (and tell the caller).
pub fn fetch_prune(env: &Env, repo_root: &str, log: Logger) -> bool {
    let r = run_git(&["fetch", "--all", "--prune"], Some(repo_root), env);
    if r.status != 0 {
        let detail = if r.stderr.trim().is_empty() {
            r.error.clone().unwrap_or_else(|| "unknown error".to_string())
        } else {
            r.stderr.trim().to_string()
        };
        log(&format!(
            "git fetch --prune failed; using cached refs (a still-present upstream may read as gone): {}",
            detail
        ));
    }
    r.status == 0
}

pub fn gone_branch_set(env: &Env, repo_root: &str) -> Result<BTreeSet<String>, String> {
    let r = run_git(
        &["for-each-ref", "--format=%(refname:short)\t%(upstream:track,nobracket)", "refs/heads"],
        Some(repo_root),
        env,
    );
    if r.status != 0 {
        let detail = if r.stderr.trim().is_empty() {
            r.error.unwrap_or_else(|| "unknown error".to_string())
        } else {
            r.stderr.trim().to_string()
        };
        return Err(format!("git for-each-ref failed: {}", detail));
    }
    Ok(parse_gone_branches(&r.stdout))
}

// Uncommitted changes (modified tracked files or untracked files). A "gone"
// branch's committed work lives on in the repo regardless of removal; only an
// unclean working tree risks data loss, so it's what we guard on.
pub fn is_dirty(env: &Env, worktree_path: Option<&str>) -> bool {
    let Some(path) = worktree_path else { return false };
    let r = run_git(&["status", "--porcelain"], Some(path), env);
    if r.status != 0 {
        return false; // can't tell -> don't block on a guess
    }
    !r.stdout.trim().is_empty()
}

pub struct Collected {
    pub repo: Value,
    pub candidates: Vec<Candidate>,
}

// Gather the removal candidates for the current repo. Pure-ish orchestration:
// query herdr, (optionally) fetch+prune, diff against the gone set, flag dirty.
pub fn collect_gone_worktrees(env: &Env, fetch: bool, log: Logger) -> Result<Collected, String> {
    let workspace_id = resolve_workspace_id(env);
    let (source, worktrees) = list_repo_worktrees(env, workspace_id.as_deref())?;
    let repo_root = source.get("repo_root").and_then(Value::as_str).map(String::from);
    let Some(repo_root) = repo_root else {
        return Err("could not determine the current repo (the workspace has no git worktree)".to_string());
    };

    if fetch {
        fetch_prune(env, &repo_root, log);
    }
    let gone = gone_branch_set(env, &repo_root)?;
    let mut candidates = select_gone_worktrees(&worktrees, &gone, workspace_id.as_deref());
    let labels = workspace_labels(env);
    for c in &mut candidates {
        c.dirty = is_dirty(env, c.path.as_deref());
        // The workspace name as shown in herdr; fall back to the branch if the
        // worktree has no open workspace (or it's unnamed).
        c.label = c
            .workspace_id
            .as_deref()
            .and_then(|id| labels.iter().find(|(lid, _)| lid == id))
            .and_then(|(_, label)| label.clone())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| c.branch.clone());
    }

    Ok(Collected { repo: source, candidates })
}

// Remove one worktree by its open workspace id. Errors so the caller can report
// failures per-worktree.
pub fn remove_worktree(env: &Env, workspace_id: &str, force: bool) -> Result<Value, String> {
    let mut args = vec!["worktree", "remove", "--workspace", workspace_id];
    if force {
        args.push("--force");
    }
    args.push("--json");
    run_herdr_json(&args, env)
}

// --- shared rendering / execution (used by the CLI and the plugin actions) ---

pub fn repo_display_name(repo: &Value) -> String {
    repo.get("repo_name")
        .and_then(Value::as_str)
        .or_else(|| repo.get("repo_root").and_then(Value::as_str))
        .unwrap_or("the current repo")
        .to_string()
}

// Why a candidate would be skipped during removal, or None if it's removable.
// Centralizes the safety policy so the preview, the prompt count, and the actual
// removal all agree on what's eligible.
pub fn removal_skip_reason(c: &Candidate, force: bool) -> Option<&'static str> {
    if c.is_current {
        return Some("current workspace — switch away, then re-run");
    }
    if c.workspace_id.is_none() {
        return Some("no open workspace — run `git worktree remove`");
    }
    if c.dirty && !force {
        return Some("uncommitted changes — re-run with --force to remove anyway");
    }
    None
}

// The subset of candidates that would actually be removed under `force`.
pub fn removable_candidates<'a>(candidates: &'a [Candidate], force: bool) -> Vec<&'a Candidate> {
    candidates.iter().filter(|c| removal_skip_reason(c, force).is_none()).collect()
}

// Human-readable list of removal candidates, each led by its workspace name.
// An empty list renders the "nothing to do" line. `force` tunes the dirty note.
pub fn format_preview(repo_name: &str, candidates: &[Candidate], force: bool) -> String {
    if candidates.is_empty() {
        return format!("No worktrees with a deleted remote branch in {}.\n", repo_name);
    }
    let mut out = format!(
        "Workspaces in {} whose remote branch is gone ({}):\n\n",
        repo_name,
        candidates.len()
    );
    for c in candidates {
        let mut flags: Vec<&str> = Vec::new();
        if c.is_current {
            flags.push("CURRENT workspace — switch away first; will be skipped");
        }
        if c.workspace_id.is_none() {
            flags.push("no open workspace — remove with `git worktree remove`");
        }
        if c.dirty {
            flags.push(if force {
                "uncommitted changes — will be force-removed"
            } else {
                "uncommitted changes — will be skipped unless --force"
            });
        }
        let branch = if c.label == c.branch {
            String::new()
        } else {
            format!("  (branch {})", c.branch)
        };
        let tag = if flags.is_empty() {
            String::new()
        } else {
            format!("\n    ⚠ {}", flags.join("; "))
        };
        out.push_str(&format!(
            "  • {}{}\n    {}{}\n",
            c.label,
            branch,
            c.path.as_deref().unwrap_or("(unknown path)"),
            tag
        ));
    }
    out
}

pub struct Skipped {
    pub candidate: Candidate,
    pub reason: String,
}

// Remove the eligible candidates, returning (removed, skipped). Skips (never
// destroys silently) the invoking workspace, worktrees with no open workspace,
// and — unless `force` — worktrees with uncommitted changes.
pub fn apply_removals(
    env: &Env,
    candidates: &[Candidate],
    force: bool,
    log: Logger,
) -> (Vec<Candidate>, Vec<Skipped>) {
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for c in candidates {
        if let Some(reason) = removal_skip_reason(c, force) {
            skipped.push(Skipped { candidate: c.clone(), reason: reason.to_string() });
            continue;
        }
        // Eligibility guaranteed workspace_id is present.
        let workspace_id = c.workspace_id.as_deref().unwrap_or_default();
        match remove_worktree(env, workspace_id, c.dirty) {
            Ok(_) => {
                removed.push(c.clone());
                log(&format!("removed {} ({})", c.label, c.path.as_deref().unwrap_or("")));
            }
            Err(err) => skipped.push(Skipped { candidate: c.clone(), reason: err }),
        }
    }
    (removed, skipped)
}

pub fn format_apply_result(repo_name: &str, removed: &[Candidate], skipped: &[Skipped]) -> String {
    let mut out = format!("Removed {} gone worktree(s) in {}:\n", removed.len(), repo_name);
    for c in removed {
        out.push_str(&format!("  ✓ {}  {}\n", c.label, c.path.as_deref().unwrap_or("")));
    }
    if !skipped.is_empty() {
        out.push_str(&format!("\nSkipped {}:\n", skipped.len()));
        for s in skipped {
            out.push_str(&format!("  • {} — {}\n", s.candidate.label, s.reason));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_gone_branches_keeps_only_branches_whose_upstream_is_gone() {
        // Tab-separated: <branch>\t<upstream:track,nobracket>.
        let text = [
            "main\t",            // upstream exists, in sync -> keep (not gone)
            "feature-a\tgone",   // deleted upstream -> gone
            "feature-b\tahead 2", // ahead of an existing upstream -> not gone
            "local-only\t",      // never pushed / no upstream -> not gone
            "feature-c\tgone",   // gone
            "",                  // blank line ignored
        ]
        .join("\n");
        let gone = parse_gone_branches(&text);
        assert_eq!(gone.iter().cloned().collect::<Vec<_>>(), vec!["feature-a", "feature-c"]);
    }

    #[test]
    fn parse_gone_branches_tolerates_empty_whitespace_input() {
        assert!(parse_gone_branches("").is_empty());
        assert!(parse_gone_branches("   \n\n").is_empty());
    }

    fn worktrees() -> Vec<Value> {
        vec![
            // main checkout — never a candidate even if (impossibly) "gone".
            json!({ "branch": "main", "is_linked_worktree": false, "path": "/repo", "open_workspace_id": "w1" }),
            // linked worktree, branch gone -> candidate.
            json!({ "branch": "feature-a", "is_linked_worktree": true, "path": "/wt/feature-a", "open_workspace_id": "w2" }),
            // linked, branch NOT gone -> excluded.
            json!({ "branch": "feature-keep", "is_linked_worktree": true, "path": "/wt/feature-keep", "open_workspace_id": "w3" }),
            // linked, detached -> excluded (no branch).
            json!({ "branch": "", "is_detached": true, "is_linked_worktree": true, "path": "/wt/detached", "open_workspace_id": "w4" }),
            // linked, gone, but no open workspace -> still a candidate (workspace_id None).
            json!({ "branch": "feature-orphan", "is_linked_worktree": true, "path": "/wt/orphan", "open_workspace_id": null }),
        ]
    }

    fn gone(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn select_gone_worktrees_picks_linked_gone_branch_bearing_worktrees() {
        let picked =
            select_gone_worktrees(&worktrees(), &gone(&["feature-a", "feature-orphan", "main"]), None);
        let mut branches: Vec<&str> = picked.iter().map(|c| c.branch.as_str()).collect();
        branches.sort();
        assert_eq!(branches, vec!["feature-a", "feature-orphan"]);
        let orphan = picked.iter().find(|c| c.branch == "feature-orphan").unwrap();
        assert_eq!(orphan.workspace_id, None);
    }

    #[test]
    fn select_gone_worktrees_flags_the_invoking_workspace() {
        let picked = select_gone_worktrees(&worktrees(), &gone(&["feature-a"]), Some("w2"));
        assert_eq!(picked.len(), 1);
        assert!(picked[0].is_current);
    }

    #[test]
    fn select_gone_worktrees_handles_empty_inputs() {
        assert!(select_gone_worktrees(&[], &BTreeSet::new(), None).is_empty());
        assert!(select_gone_worktrees(&worktrees(), &BTreeSet::new(), None).is_empty());
    }

    #[test]
    fn repo_display_name_prefers_name_then_root_then_a_fallback() {
        assert_eq!(repo_display_name(&json!({ "repo_name": "r", "repo_root": "/x" })), "r");
        assert_eq!(repo_display_name(&json!({ "repo_root": "/x" })), "/x");
        assert_eq!(repo_display_name(&Value::Null), "the current repo");
    }

    fn candidate(label: &str, branch: &str, path: &str, workspace_id: Option<&str>) -> Candidate {
        Candidate {
            branch: branch.to_string(),
            path: Some(path.to_string()),
            workspace_id: workspace_id.map(String::from),
            is_current: false,
            dirty: false,
            label: label.to_string(),
        }
    }

    #[test]
    fn format_preview_lists_candidates_by_workspace_name_and_flags_risks() {
        let mut dirty = candidate("nice-name", "feature/x", "/wt/x", Some("w3"));
        dirty.dirty = true;
        let out = format_preview(
            "myrepo",
            &[candidate("feat-a", "feat-a", "/wt/a", Some("w2")), dirty],
            false,
        );
        assert!(out.contains("gone (2)"));
        assert!(out.contains("• feat-a\n    /wt/a"));
        // Branch shown in parens only when it differs from the workspace name.
        assert!(out.contains("• nice-name  (branch feature/x)"));
        assert!(!out.contains("feat-a  (branch"));
        assert!(out.contains("uncommitted changes"));
    }

    #[test]
    fn format_preview_renders_the_empty_case() {
        assert_eq!(
            format_preview("myrepo", &[], false),
            "No worktrees with a deleted remote branch in myrepo.\n"
        );
    }

    #[test]
    fn format_apply_result_summarizes_removed_and_skipped() {
        let out = format_apply_result(
            "myrepo",
            &[candidate("a", "a", "/wt/a", Some("w2"))],
            &[Skipped {
                candidate: candidate("b", "b", "/wt/b", Some("w3")),
                reason: "uncommitted changes".to_string(),
            }],
        );
        assert!(out.contains("Removed 1 gone worktree(s) in myrepo"));
        assert!(out.contains("✓ a  /wt/a"));
        assert!(out.contains("Skipped 1"));
        assert!(out.contains("• b — uncommitted changes"));
    }

    #[test]
    fn removal_skip_reason_encodes_the_safety_policy() {
        let clean = candidate("c", "c", "/wt/c", Some("w2"));
        assert_eq!(removal_skip_reason(&clean, false), None);
        let mut current = clean.clone();
        current.is_current = true;
        assert!(removal_skip_reason(&current, false).unwrap().contains("current workspace"));
        let mut orphan = clean.clone();
        orphan.workspace_id = None;
        assert!(removal_skip_reason(&orphan, false).unwrap().contains("no open workspace"));
        let mut dirty = clean.clone();
        dirty.dirty = true;
        assert!(removal_skip_reason(&dirty, false).unwrap().contains("uncommitted changes"));
        // --force makes a dirty worktree removable.
        assert_eq!(removal_skip_reason(&dirty, true), None);
    }

    #[test]
    fn removable_candidates_filters_to_what_would_actually_be_removed() {
        let removable = candidate("a", "a", "/wt/a", Some("w2"));
        let mut current = candidate("b", "b", "/wt/b", Some("w3"));
        current.is_current = true;
        let mut dirty = candidate("c", "c", "/wt/c", Some("w4"));
        dirty.dirty = true;
        let candidates = vec![removable, current, dirty];
        let labels = |v: Vec<&Candidate>| v.iter().map(|c| c.label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(removable_candidates(&candidates, false)), vec!["a"]);
        assert_eq!(labels(removable_candidates(&candidates, true)), vec!["a", "c"]);
        assert!(removable_candidates(&[], false).is_empty());
    }

    #[test]
    fn format_preview_reflects_force_in_the_dirty_note() {
        let mut dirty = candidate("x", "x", "/wt/x", Some("w3"));
        dirty.dirty = true;
        let dirty = vec![dirty];
        assert!(format_preview("r", &dirty, false).contains("will be skipped unless --force"));
        assert!(format_preview("r", &dirty, true).contains("will be force-removed"));
    }

    #[test]
    fn format_apply_result_omits_the_skipped_section_when_none_skipped() {
        let out = format_apply_result("myrepo", &[candidate("a", "a", "/wt/a", Some("w2"))], &[]);
        assert!(!out.contains("Skipped"));
    }

    #[test]
    fn resolve_workspace_id_prefers_env_overrides_then_context_json() {
        assert_eq!(
            resolve_workspace_id(&Env::from_pairs(&[("HERDR_WSM_WORKSPACE", "wA")])).as_deref(),
            Some("wA")
        );
        assert_eq!(
            resolve_workspace_id(&Env::from_pairs(&[("HERDR_WORKSPACE_ID", "wB")])).as_deref(),
            Some("wB")
        );
        assert_eq!(
            resolve_workspace_id(&Env::from_pairs(&[(
                "HERDR_PLUGIN_CONTEXT_JSON",
                r#"{"workspace":{"workspace_id":"wC"}}"#
            )]))
            .as_deref(),
            Some("wC")
        );
        assert_eq!(
            resolve_workspace_id(&Env::from_pairs(&[("HERDR_PLUGIN_CONTEXT_JSON", "not json")])),
            None
        );
        assert_eq!(resolve_workspace_id(&Env::from_pairs(&[])), None);
    }
}
