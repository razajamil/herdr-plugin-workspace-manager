// Thin wrapper around the herdr CLI (JSON in, JSON out).

use std::process::Command;

use serde_json::Value;

use crate::env::Env;

pub struct CmdOut {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn herdr_bin(env: &Env) -> String {
    env.get("HERDR_BIN_PATH").filter(|v| !v.is_empty()).unwrap_or("herdr").to_string()
}

// Run a herdr CLI command. Returns { status, stdout, stderr }.
pub fn run_herdr(args: &[&str], env: &Env) -> Result<CmdOut, String> {
    let bin = herdr_bin(env);
    let output = Command::new(&bin)
        .args(args)
        .env_clear()
        .envs(env.iter())
        .output()
        .map_err(|e| format!("failed to spawn {}: {}", bin, e))?;
    Ok(CmdOut {
        status: output.status.code().unwrap_or(0),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn value_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// Run a herdr command that returns JSON; return its `result` object.
// Errors on a non-zero exit or an `{ error: ... }` envelope.
pub fn run_herdr_json(args: &[&str], env: &Env) -> Result<Value, String> {
    let CmdOut { status, stdout, stderr } = run_herdr(args, env)?;
    let trimmed = stdout.trim();
    let parsed: Option<Value> =
        if trimmed.is_empty() { None } else { serde_json::from_str(trimmed).ok() };
    if let Some(err) = parsed.as_ref().and_then(|p| p.get("error")).filter(|e| !e.is_null()) {
        return Err(format!(
            "herdr {} -> {}: {}",
            args.join(" "),
            value_display(err.get("code").unwrap_or(&Value::Null)),
            value_display(err.get("message").unwrap_or(&Value::Null)),
        ));
    }
    if status != 0 {
        let detail = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
        return Err(format!("herdr {} exited {}: {}", args.join(" "), status, detail));
    }
    Ok(parsed.map(|p| p.get("result").cloned().unwrap_or(Value::Null)).unwrap_or(Value::Null))
}

// Extract a pane id from any herdr result shape we care about
// (pane split -> result.pane.pane_id, tab create -> result.root_pane.pane_id).
pub fn pane_id_of(result: &Value) -> Option<String> {
    for candidate in [
        result.get("pane_id"),
        result.get("pane").and_then(|p| p.get("pane_id")),
        result.get("root_pane").and_then(|p| p.get("pane_id")),
    ] {
        if let Some(Value::String(id)) = candidate {
            return Some(id.clone());
        }
    }
    None
}

pub fn tab_id_of(result: &Value) -> Option<String> {
    for candidate in [result.get("tab_id"), result.get("tab").and_then(|t| t.get("tab_id"))] {
        if let Some(Value::String(id)) = candidate {
            return Some(id.clone());
        }
    }
    None
}
