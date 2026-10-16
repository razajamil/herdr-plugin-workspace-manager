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
// The server listens on a Unix socket on Linux and macOS. On Windows the same
// protocol is served from a named pipe whose name is `\\.\pipe\` followed by the
// socket path herdr hands out (HERDR_SOCKET_PATH), so only the connector differs.

use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::home_dir;
use crate::env::Env;

// Long enough for a big layout to be built (each pane is a real PTY spawn),
// short enough that a wedged server surfaces as an error rather than a hang.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

// herdr's own config directory: ~/.config/herdr (or $XDG_CONFIG_HOME/herdr) on
// Unix, %APPDATA%\herdr on Windows.
fn herdr_config_dir(env: &Env) -> PathBuf {
    if cfg!(windows) {
        return match env.get("APPDATA").filter(|v| !v.is_empty()) {
            Some(appdata) => PathBuf::from(appdata).join("herdr"),
            None => home_dir().join("AppData").join("Roaming").join("herdr"),
        };
    }
    match env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(xdg) => PathBuf::from(xdg).join("herdr"),
        None => home_dir().join(".config").join("herdr"),
    }
}

// Where the running server is listening. herdr injects HERDR_SOCKET_PATH into
// every plugin command and every managed pane process, so in practice the first
// branch always wins; the rest mirror herdr's own resolution order so the CLI
// and the test suite still work when invoked from an ordinary shell.
pub fn socket_path(env: &Env) -> PathBuf {
    if let Some(explicit) = env.get("HERDR_SOCKET_PATH").filter(|v| !v.is_empty()) {
        return PathBuf::from(explicit);
    }
    let config_dir = herdr_config_dir(env);
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

// Send one request line and read one response line back.
#[cfg(unix)]
fn round_trip(path: &Path, line: &str, method: &str, timeout_ms: u64) -> Result<String, String> {
    let timeout = Duration::from_millis(timeout_ms.max(1));

    let stream = UnixStream::connect(path).map_err(|e| {
        format!(
            "cannot connect to the herdr socket at {}: {} (is the server running?)",
            path.display(),
            e
        )
    })?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| format!("socket setup failed: {}", e))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| format!("socket setup failed: {}", e))?;

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
    Ok(response)
}

// The named pipe a Windows herdr serves its API on. A value that already names a
// pipe is used as is.
#[cfg(windows)]
fn pipe_address(path: &Path) -> String {
    let raw = path.display().to_string();
    if raw.starts_with(r"\\.\pipe\") {
        raw
    } else {
        format!(r"\\.\pipe\{}", raw)
    }
}

// Opening a pipe fails at once with ERROR_PIPE_BUSY when the server has no free
// instance at that instant (several hooks firing together). A Unix listener would
// queue the connection, so wait for a free instance briefly instead of failing.
#[cfg(windows)]
fn open_pipe(pipe: &str) -> std::io::Result<std::fs::File> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let started = std::time::Instant::now();
    loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(pipe) {
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && started.elapsed() < Duration::from_secs(2) =>
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

// A pipe handle has no read timeout, so the exchange runs on a helper thread and
// the caller gives up after `timeout_ms`. The process is short-lived, so a thread
// stuck on a wedged server is reclaimed when it exits.
#[cfg(windows)]
fn round_trip(path: &Path, line: &str, method: &str, timeout_ms: u64) -> Result<String, String> {
    let pipe = pipe_address(path);
    let line = line.to_string();
    let who = method.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(pipe_exchange(&pipe, &line, &who));
    });
    rx.recv_timeout(Duration::from_millis(timeout_ms.max(1)))
        .unwrap_or_else(|_| Err(format!("{} got no response within {}ms", method, timeout_ms)))
}

#[cfg(windows)]
fn pipe_exchange(pipe: &str, line: &str, method: &str) -> Result<String, String> {
    let mut file = open_pipe(pipe).map_err(|e| {
        format!("cannot connect to the herdr pipe at {}: {} (is the server running?)", pipe, e)
    })?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.flush())
        .map_err(|e| format!("{} request failed to send: {}", method, e))?;

    let mut response = String::new();
    BufReader::new(&file)
        .read_line(&mut response)
        .map_err(|e| format!("{} got no response: {}", method, e))?;
    Ok(response)
}

// Send one request and return its `result`. Errors carry herdr's machine-readable
// error code (e.g. `invalid_target`, `protocol_mismatch`) so failures are
// diagnosable from the plugin log without re-running anything.
pub fn request(env: &Env, method: &str, params: Value, timeout_ms: u64) -> Result<Value, String> {
    let path = socket_path(env);
    let id = next_request_id();
    let line = json!({ "id": id, "method": method, "params": params }).to_string();
    let response = round_trip(&path, &line, method, timeout_ms)?;
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

    #[cfg(unix)]
    #[test]
    fn socket_path_falls_back_to_the_config_dir_and_honours_named_sessions() {
        let env = Env::from_pairs(&[("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(socket_path(&env), PathBuf::from("/cfg/herdr/herdr.sock"));

        let named = Env::from_pairs(&[("XDG_CONFIG_HOME", "/cfg"), ("HERDR_SESSION", "work")]);
        assert_eq!(socket_path(&named), PathBuf::from("/cfg/herdr/sessions/work/herdr.sock"));
    }

    #[cfg(unix)]
    #[test]
    fn socket_path_ignores_empty_overrides() {
        let env = Env::from_pairs(&[("HERDR_SOCKET_PATH", ""), ("XDG_CONFIG_HOME", "/cfg")]);
        assert_eq!(socket_path(&env), PathBuf::from("/cfg/herdr/herdr.sock"));
    }

    #[cfg(windows)]
    #[test]
    fn socket_path_falls_back_to_appdata_and_honours_named_sessions() {
        let env = Env::from_pairs(&[("APPDATA", r"C:\cfg")]);
        assert_eq!(socket_path(&env), PathBuf::from(r"C:\cfg\herdr\herdr.sock"));

        let named = Env::from_pairs(&[("APPDATA", r"C:\cfg"), ("HERDR_SESSION", "work")]);
        assert_eq!(socket_path(&named), PathBuf::from(r"C:\cfg\herdr\sessions\work\herdr.sock"));
    }

    #[cfg(windows)]
    #[test]
    fn socket_path_ignores_empty_overrides() {
        let env = Env::from_pairs(&[("HERDR_SOCKET_PATH", ""), ("APPDATA", r"C:\cfg")]);
        assert_eq!(socket_path(&env), PathBuf::from(r"C:\cfg\herdr\herdr.sock"));
    }

    #[cfg(windows)]
    #[test]
    fn the_pipe_name_is_the_socket_path_under_the_pipe_namespace() {
        let path = PathBuf::from(r"C:\Users\me\AppData\Roaming\herdr\herdr.sock");
        assert_eq!(pipe_address(&path), r"\\.\pipe\C:\Users\me\AppData\Roaming\herdr\herdr.sock");
    }

    #[cfg(windows)]
    #[test]
    fn a_value_that_already_names_a_pipe_is_not_prefixed_twice() {
        let path = PathBuf::from(r"\\.\pipe\herdr-main");
        assert_eq!(pipe_address(&path), r"\\.\pipe\herdr-main");
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
