//! Small self-contained helpers: paths, PATH lookup, the default shell, time formatting and atomic writes.
//!
//! Nothing here calls into bro-core, so it keeps working while the other crates are still being built.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// `~/.bro` (override `BRO_DIR`). Mirrors `bro_core::paths::bro_dir` without depending on it.
pub fn bro_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BRO_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".bro")
}

/// Find a program on PATH (tries .exe/.cmd/.bat on Windows).
pub fn which(prog: &str) -> Option<PathBuf> {
    let p = Path::new(prog);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) && p.extension().is_none() { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    for dir in std::env::split_paths(&path) {
        for e in exts {
            let c = dir.join(format!("{prog}{e}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// The user's shell: `Settings.shell` if set, else pwsh / powershell / cmd on Windows, `$SHELL` elsewhere.
pub fn default_shell(configured: Option<&str>) -> (String, Vec<String>) {
    if let Some(s) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        let mut parts = s.split_whitespace().map(String::from);
        let prog = parts.next().unwrap_or_default();
        return (prog, parts.collect());
    }
    if cfg!(windows) {
        for p in ["pwsh.exe", "powershell.exe"] {
            if which(p).is_some() {
                return (p.into(), vec!["-NoLogo".into()]);
            }
        }
        ("cmd.exe".into(), vec![])
    } else {
        (std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()), vec![])
    }
}

/// Unix seconds now.
pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Seconds since `t` (0 if in the future).
pub fn secs_since(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0)
}

/// Compact duration: "now", "42s", "7m", "3h", "2d", "5w".
pub fn short_dur(secs: u64) -> String {
    match secs {
        0..=4 => "now".into(),
        5..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=1_209_599 => format!("{}d", secs / 86_400),
        _ => format!("{}w", secs / 604_800),
    }
}

/// A reset countdown: "2h14m", "3d4h", "12m", "now".
pub fn countdown(secs: i64) -> String {
    if secs <= 0 {
        return "now".into();
    }
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m:02}m")
    } else {
        format!("{}m", m.max(1))
    }
}

/// `C:\Users\me\code\x` → `~\code\x`; long paths keep the drive/root and the last two components.
pub fn short_path(p: &Path, max: usize) -> String {
    let mut s = p.to_string_lossy().to_string();
    if let Some(home) = dirs::home_dir() {
        let h = home.to_string_lossy().to_string();
        if !h.is_empty() && s.to_lowercase().starts_with(&h.to_lowercase()) {
            s = format!("~{}", &s[h.len()..]);
        }
    }
    if s.chars().count() <= max {
        return s;
    }
    let sep = if s.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = s.split(sep).filter(|x| !x.is_empty()).collect();
    if parts.len() >= 3 {
        let tail = format!("{}{sep}{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        let cand = format!("{}{sep}…{sep}{tail}", parts[0]);
        if cand.chars().count() <= max {
            return cand;
        }
        let cand = format!("…{sep}{tail}");
        if cand.chars().count() <= max {
            return cand;
        }
    }
    let n = s.chars().count();
    format!("…{}", s.chars().skip(n + 1 - max.max(2)).collect::<String>())
}

/// Write `data` to `path` atomically (temp file in the same dir + rename).
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Local clock "HH:MM" (UTC offset asked from the OS once, off the UI thread at startup).
pub fn clock() -> String {
    let s = (now_secs() + local_offset_secs()).rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

static OFFSET: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Seconds east of UTC (0 until [`probe_local_offset`] has run).
pub fn local_offset_secs() -> i64 {
    OFFSET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Ask the OS for the local UTC offset on a background thread (it spawns a process on Windows).
pub fn probe_local_offset() {
    std::thread::spawn(|| {
        #[cfg(windows)]
        let off: Option<i64> = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", "[int][TimeZoneInfo]::Local.GetUtcOffset([DateTime]::Now).TotalSeconds"])
                .creation_flags(0x0800_0000)
                .output()
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        };
        #[cfg(not(windows))]
        let off: Option<i64> = std::process::Command::new("date").arg("+%z").output().ok().and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let d = s.trim_start_matches(['+', '-']);
            let h: i64 = d.get(0..2)?.parse().ok()?;
            let m: i64 = d.get(2..4)?.parse().ok()?;
            Some(sign * (h * 3600 + m * 60))
        });
        if let Some(o) = off {
            OFFSET.store(o, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(short_dur(3), "now");
        assert_eq!(short_dur(90), "1m");
        assert_eq!(short_dur(7200), "2h");
        assert_eq!(short_dur(3 * 86_400), "3d");
        assert_eq!(countdown(0), "now");
        assert_eq!(countdown(2 * 3600 + 14 * 60), "2h14m");
        assert_eq!(countdown(3 * 86_400 + 4 * 3600), "3d4h");
        assert_eq!(countdown(30), "1m");
    }

    #[test]
    fn short_paths() {
        let p = Path::new("/very/long/path/to/some/project-dir");
        let s = short_path(p, 24);
        assert!(s.chars().count() <= 24, "{s}");
        assert!(s.ends_with("project-dir"), "{s}");
    }
}
