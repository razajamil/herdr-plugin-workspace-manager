// Raw socket client for the parts of herdr's API the CLI doesn't expose.
//
// Almost everything this plugin needs is reachable through `herdr <subcommand>`
// (see herdr.rs), which is the portable path herdr recommends for plugins. But
// `layout.apply` -- the one call that builds a whole tab in a single request --
// has no CLI wrapper, so it has to go over the socket directly.
//
// The transport is deliberately tiny: newline-delimited JSON, one request line
// in, one response line out, connection closed. That's the whole protocol for
// non-subscription methods.
//
// Unix only, which is what the manifest declares. On Windows the same API lives
// behind a named pipe and would need a different connector.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::env::Env;

// Long enough for a big layout to be built (each pane is a real PTY spawn),
// short enough that a wedged server surfaces as an error rather than a hang.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

// Where the running server is listening. herdr injects HERDR_SOCKET_PATH into
// every plugin command and every managed pane process, so in practice the first
// branch always wins; the rest mirror herdr's own resolution order so the CLI
// and the test suite still work when invoked from an ordinary shell.
pub fn socket_path(env: &Env) -> PathBuf {
    if let Some(explicit) = env.get("HERDR_SOCKET_PATH").filter(|v| !v.is_empty()) {
        return PathBuf::from(explicit);
    }
    let config_dir = match env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(xdg) => PathBuf::from(xdg).join("herdr"),
        None => home_dir().join(".config").join("herdr"),
    };
    match env.get("HERDR_SESSION").filter(|v| !v.is_empty()) {
        Some(session) => config_dir.join("sessions").join(session).join("herdr.sock"),
        None => config_dir.join("herdr.sock"),
    }
}

fn next_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("wsm-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn value_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// Send one request and return its `result`. Errors carry herdr's machine-readable
// error code (e.g. `invalid_target`, `protocol_mismatch`) so failures are
// diagnosable from the plugin log without re-running anything.
pub fn request(env: &Env, method: &str, params: Value, timeout_ms: u64) -> Result<Value, String> {
    let path = socket_path(env);
    let timeout = Duration::from_millis(timeout_ms.max(1));

    let stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "cannot connect to the herdr socket at {}: {} (is the server running?)",
            path.display(),
            e
        )
    })?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| format!("socket setup failed: {}", e))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| format!("socket setup failed: {}", e))?;

    let id = next_request_id();
    let line = json!({ "id": id, "method": method, "params": params }).to_string();

    let mut writer = &stream;
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|e| format!("{} request failed to send: {}", method, e))?;

    let mut response = String::new();
    BufReader::new(&stream)
        .read_line(&mut response)
        .map_err(|e| format!("{} got no response within {}ms: {}", method, timeout_ms, e))?;
    if response.trim().is_empty() {
        return Err(format!("{}: the herdr server closed the connection", method));
    }

    let parsed: Value = serde_json::from_str(response.trim())
        .map_err(|e| format!("{}: unreadable response: {}", method, e))?;
    if let Some(err) = parsed.get("error").filter(|e| !e.is_null()) {
        return Err(format!(
            "{} -> {}: {}",
            method,
            value_display(err.get("code").unwrap_or(&Value::Null)),
            value_display(err.get("message").unwrap_or(&Value::Null)),
        ));
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_prefers_the_injected_override() {
        let env = Env::from_pairs(&[("HERDR_SOCKET_PATH", "/run/herdr-custom.sock")]);
        assert_eq!(socket_path(&env), PathBuf::from("/run/herdr-custom.sock"));
    }

    #[test]
    fn socket_path_falls_back_to_the_config_dir_and_honours_named_sessions() {
        let env = Env::from_pairs(&[("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(socket_path(&env), PathBuf::from("/cfg/herdr/herdr.sock"));

        let named = Env::from_pairs(&[("XDG_CONFIG_HOME", "/cfg"), ("HERDR_SESSION", "work")]);
        assert_eq!(socket_path(&named), PathBuf::from("/cfg/herdr/sessions/work/herdr.sock"));
    }

    #[test]
    fn socket_path_ignores_empty_overrides() {
        let env = Env::from_pairs(&[("HERDR_SOCKET_PATH", ""), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(socket_path(&env), PathBuf::from("/cfg/herdr/herdr.sock"));
    }

    #[test]
    fn request_ids_are_unique_per_call() {
        assert_ne!(next_request_id(), next_request_id());
    }

    #[test]
    fn a_missing_socket_is_a_clear_error_not_a_hang() {
        let env = Env::from_pairs(&[("HERDR_SOCKET_PATH", "/nonexistent/herdr.sock")]);
        let err = request(&env, "ping", json!({}), 500).unwrap_err();
        assert!(err.contains("cannot connect"), "got: {}", err);
    }
}
