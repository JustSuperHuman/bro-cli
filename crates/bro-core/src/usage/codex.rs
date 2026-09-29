//! Codex subscription meters via the installed CLI's app-server (v1 `codex-usage.js`):
//! spawn `codex app-server --stdio` with `CODEX_HOME`, speak newline-delimited
//! JSON-RPC (`initialize` → `initialized` → `account/rateLimits/read`), then kill it.
use super::{Usage, Window};
use crate::util::{now_secs, num_of, parse_iso_secs};
use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

fn reset_ts(value: Option<&Value>, after_secs: Option<&Value>) -> Option<i64> {
    if let Some(n) = num_of(value) {
        return Some(n as i64);
    }
    if let Some(s) = value.and_then(Value::as_str).and_then(parse_iso_secs) {
        return Some(s);
    }
    num_of(after_secs).map(|a| now_secs() + a as i64)
}

fn first<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null()))
}

fn normalize_window(v: Option<&Value>) -> Option<Window> {
    let v = v.filter(|v| v.is_object())?;
    let used = num_of(first(v, &["used_percent", "usedPercent"]))?;
    let mins = match num_of(first(v, &["limit_window_seconds", "window_duration_seconds"])) {
        Some(secs) => Some((secs / 60.0).round()),
        None => num_of(first(v, &["window_minutes", "windowDurationMins"])),
    };
    Some(Window {
        used_pct: used as f32,
        resets_at: reset_ts(first(v, &["reset_at", "resets_at", "resetsAt"]), v.get("reset_after_seconds")),
        window_mins: mins.map(|m| m.max(0.0) as u32),
    })
}

/// Parse a rate-limit payload in either the backend's snake_case shape or the
/// app-server's camelCase projection (v1 `codexUsageSummary`). Windows are placed by
/// length (≥ 24h is weekly), falling back to primary = 5h, secondary = weekly.
pub fn parse(payload: &Value) -> Usage {
    let rate = payload
        .pointer("/rateLimitsByLimitId/codex")
        .or_else(|| first(payload, &["rate_limit", "rateLimits"]))
        .unwrap_or(payload);
    let primary = normalize_window(first(rate, &["primary_window", "primary"]));
    let secondary = normalize_window(first(rate, &["secondary_window", "secondary"]));
    let mut usage = Usage { fetched_at: now_secs(), ..Default::default() };
    for (w, weekly_fallback) in [(primary, false), (secondary, true)] {
        let Some(w) = w else { continue };
        let weekly = w.window_mins.map(|m| m >= 24 * 60).unwrap_or(weekly_fallback);
        let slot = if weekly { &mut usage.weekly } else { &mut usage.five_hour };
        if slot.is_none() {
            *slot = Some(w);
        }
    }
    usage.plan = first(payload, &["plan_type", "planType"])
        .or_else(|| first(rate, &["planType", "plan_type"]))
        .and_then(Value::as_str)
        .map(str::to_string);
    let credits = first(payload, &["credits"]).or_else(|| rate.get("credits"));
    usage.credits = credits.and_then(|c| num_of(c.get("balance")));
    usage
}

/// The `codex` executable and any args that must precede ours (`.cmd` shims on
/// Windows run through `cmd.exe /d /s /c`).
fn codex_command() -> anyhow::Result<Command> {
    let exe = crate::launch::which("codex").ok_or_else(|| anyhow!("Codex CLI is not installed"))?;
    let ext = exe.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut cmd = if cfg!(windows) && (ext == "cmd" || ext == "bat") {
        let mut c = Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()));
        c.arg("/d").arg("/s").arg("/c").arg(&exe);
        c
    } else {
        Command::new(&exe)
    };
    cmd.args(["app-server", "--stdio"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

/// A Codex home the app-server can read. The machine's own login may live only in
/// bro v1's `~/.bro/codex-auth.json`; then a temporary home with a copy is staged.
fn app_server_home(home: &Path) -> anyhow::Result<(PathBuf, Option<tempfile_like::TempHome>)> {
    let login = crate::creds::codex_login(home);
    let source = login.source.ok_or_else(|| anyhow!("Codex is not logged in"))?;
    if source.file_name().is_some_and(|n| n.eq_ignore_ascii_case("auth.json")) {
        return Ok((source.parent().map(Path::to_path_buf).unwrap_or_else(|| home.to_path_buf()), None));
    }
    crate::creds::codex_auth(home, false)?;
    let temp = tempfile_like::TempHome::new()?;
    std::fs::copy(&source, temp.path.join("auth.json")).context("staging codex auth")?;
    Ok((temp.path.clone(), Some(temp)))
}

/// A throwaway directory under the OS temp dir, removed on drop.
mod tempfile_like {
    use std::path::PathBuf;
    pub struct TempHome {
        pub path: PathBuf,
    }
    impl TempHome {
        pub fn new() -> std::io::Result<TempHome> {
            let path = std::env::temp_dir().join(format!("bro-codex-usage-{}-{:08x}", std::process::id(), rand::random::<u32>()));
            std::fs::create_dir_all(&path)?;
            Ok(TempHome { path })
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Newline-delimited JSON-RPC over the child's stdio.
struct Rpc {
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<Value>,
    deadline: Instant,
}

impl Rpc {
    fn send(&mut self, msg: &Value) -> anyhow::Result<()> {
        writeln!(self.stdin, "{msg}").context("writing to codex app-server")?;
        self.stdin.flush().ok();
        Ok(())
    }

    fn notify(&mut self, method: &str) -> anyhow::Result<()> {
        self.send(&json!({ "method": method }))
    }

    fn call(&mut self, id: u64, method: &str, params: Option<Value>) -> anyhow::Result<Value> {
        let mut msg = json!({ "method": method, "id": id });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.send(&msg)?;
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("Codex app-server timed out during {method}");
            }
            let v = self.rx.recv_timeout(left).map_err(|_| anyhow!("Codex app-server timed out or exited during {method}"))?;
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                bail!("{}", err.get("message").and_then(Value::as_str).unwrap_or("Codex app-server request failed"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// Fetch a Codex login's rate limits through `codex app-server` (~15 s timeout; the
/// child is always killed).
pub fn fetch(home: &Path) -> anyhow::Result<Usage> {
    let (server_home, _temp) = app_server_home(home)?;
    let mut cmd = codex_command()?;
    cmd.env("CODEX_HOME", &server_home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = KillOnDrop(cmd.spawn().context("starting codex app-server")?);
    let stdout = child.0.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let stdin = child.0.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut rpc = Rpc { stdin, rx, deadline: Instant::now() + TIMEOUT };
    let info = json!({ "clientInfo": { "name": "bro_cli", "title": "bro", "version": env!("CARGO_PKG_VERSION") } });
    rpc.call(0, "initialize", Some(info))?;
    rpc.notify("initialized")?;
    let result = rpc.call(1, "account/rateLimits/read", None)?;
    drop(child);
    Ok(parse(&result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_camel_case_app_server_shape() {
        let v = json!({
            "rateLimits": {
                "primary": { "usedPercent": 12.5, "windowDurationMins": 300, "resetsAt": 1790000000 },
                "secondary": { "usedPercent": 40, "windowDurationMins": 10080, "resetsAt": 1790500000 },
                "planType": "pro",
                "credits": { "balance": "3.5", "unlimited": false }
            }
        });
        let u = parse(&v);
        assert_eq!(u.five_hour.as_ref().unwrap().used_pct, 12.5);
        assert_eq!(u.five_hour.as_ref().unwrap().resets_at, Some(1790000000));
        assert_eq!(u.weekly.as_ref().unwrap().window_mins, Some(10080));
        assert_eq!(u.plan.as_deref(), Some("pro"));
        assert_eq!(u.credits, Some(3.5));
    }

    #[test]
    fn parses_snake_case_backend_shape_and_places_by_length() {
        // Pro-style: only a weekly window, reported as "primary".
        let v = json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": { "used_percent": 70, "limit_window_seconds": 604800, "reset_at": "2026-10-01T00:00:00Z" }
            }
        });
        let u = parse(&v);
        assert!(u.five_hour.is_none());
        let w = u.weekly.unwrap();
        assert_eq!(w.used_pct, 70.0);
        assert_eq!(w.window_mins, Some(10080));
        assert_eq!(w.resets_at, crate::util::parse_iso_secs("2026-10-01T00:00:00Z"));
        assert_eq!(u.plan.as_deref(), Some("plus"));

        // No lengths: primary = 5h, secondary = weekly; reset_after_seconds honoured.
        let v = json!({ "rateLimitsByLimitId": { "codex": {
            "primary": { "used_percent": 1, "reset_after_seconds": 60 },
            "secondary": { "used_percent": 2 }
        } } });
        let u = parse(&v);
        assert_eq!(u.five_hour.as_ref().unwrap().used_pct, 1.0);
        assert!(u.five_hour.as_ref().unwrap().resets_at.unwrap() >= now_secs() + 59);
        assert_eq!(u.weekly.as_ref().unwrap().used_pct, 2.0);
    }
}
