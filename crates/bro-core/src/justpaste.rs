//! JustPaste Smart Paste (part of JustTools; Windows): Ctrl+V in File Explorer saves a copied image
//! as a PNG, downloads copied links and clones copied repository links into the open folder. Copied
//! files keep Explorer's own paste unless JustPaste's copy filters are on. bro reads and switches it
//! through `justpaste --status`, `--enable` / `--disable` and `--startup on|off`; JustPaste keeps
//! its own settings, so bro saves nothing about it.
use crate::justtools::{Installed, exe, quiet};
use anyhow::{Context, bail};
use std::process::Command;
use std::time::{Duration, Instant};

/// Smart Paste hooks Windows' File Explorer.
pub fn supported() -> bool {
    cfg!(windows)
}

/// What `justpaste --status` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// its tray resident is listening for Ctrl+V
    pub running: bool,
    /// it starts at sign-in
    pub at_sign_in: bool,
    /// paused from its tray menu
    pub paused: bool,
    /// copy filters are on, so JustPaste copies files itself instead of Explorer
    pub filters: bool,
}

impl Status {
    /// Ctrl+V in Explorer goes through it right now.
    pub fn active(&self) -> bool {
        self.running && !self.paused
    }
}

/// A short word for a running Smart Paste: "on", "paused", "on · not at sign-in", "on · copy filters".
pub fn describe(s: &Status) -> &'static str {
    match s {
        _ if !s.running => "off",
        _ if s.paused => "paused",
        _ if s.filters => "on · copy filters",
        _ if !s.at_sign_in => "on · not at sign-in",
        _ => "on",
    }
}

/// What it means for copied files, for the line under bro's setting.
pub fn file_note(s: &Status) -> &'static str {
    if s.filters {
        "copy filters are on: JustPaste copies your files itself (skipping filtered folders) · turn them off in its tray settings to let Windows copy"
    } else {
        "copied files and folders still paste the normal Windows way: Smart Paste only takes images and links"
    }
}

/// Quick Install's step: Smart Paste on now and at sign-in, with the JustTools found here.
/// `say` gets progress lines. Blocking.
pub fn install(mut say: impl FnMut(&str)) -> Result<Status, String> {
    let jt = crate::justtools::installed().ok_or("install JustTools first: JustPaste comes with it")?;
    say("$ justpaste --enable && justpaste --startup on");
    let s = set(&jt, true).map_err(|e| format!("{e:#}"))?;
    if !s.running {
        return Err("Smart Paste didn't start · `justpaste --status` says why".into());
    }
    say("  Smart Paste is on: Ctrl+V in File Explorer saves a copied image as a PNG, downloads copied links and clones copied repos into that folder");
    say(&format!("  {}", file_note(&s)));
    Ok(s)
}

/// `justpaste --status` output. None when it isn't Smart Paste's status.
pub fn parse_status(out: &str) -> Option<Status> {
    let mut status: Option<Status> = None;
    for line in out.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if key == "JustPaste Smart Paste" {
            status = Some(Status { running: value == "running", ..Status::default() });
            continue;
        }
        let Some(s) = status.as_mut() else { continue };
        let yes = value == "true" || value.starts_with("on");
        match key {
            "Start at login" => s.at_sign_in = yes,
            "Paused" => s.paused = yes,
            "Copy filters" => s.filters = yes,
            _ => {}
        }
    }
    status
}

/// This JustTools has JustPaste (2.8.0 on).
pub fn available(jt: &Installed) -> bool {
    jt.bin.join(exe("justpaste")).is_file()
}

fn run(jt: &Installed, args: &[&str]) -> anyhow::Result<String> {
    let out = quiet(&mut Command::new(jt.bin.join(exe("justpaste"))))
        .args(args)
        .output()
        .with_context(|| format!("couldn't run justpaste {}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("{}", if stderr.is_empty() { stdout } else { stderr });
    }
    Ok(stdout)
}

/// A host whose job object forbids breakaway (some terminals, agent sandboxes) can't start the tray
/// resident itself: `justpaste --enable` refuses rather than start one that dies with the host. WMI
/// starts it outside every job, the way sign-in would.
fn enable_outside_job(jt: &Installed) -> anyhow::Result<()> {
    let line = format!("\"{}\" --enable", jt.bin.join(exe("justpaste")).display()).replace('\'', "''");
    let script = format!("$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{ CommandLine = '{line}' }}; exit $r.ReturnValue");
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let ps = format!(r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe");
    let out = quiet(&mut Command::new(ps)).args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().context("couldn't run PowerShell")?;
    if !out.status.success() {
        bail!("WMI couldn't start it either ({})", out.status);
    }
    Ok(())
}

/// Smart Paste's state; None when this JustTools has no JustPaste. Blocking: call off the UI thread.
pub fn status(jt: &Installed) -> anyhow::Result<Option<Status>> {
    if !available(jt) {
        return Ok(None);
    }
    let out = run(jt, &["--status"])?;
    parse_status(&out).map(Some).with_context(|| format!("unexpected justpaste --status output: {out}"))
}

/// Turn Smart Paste on now and at sign-in, or off and out of sign-in, then report where it ended up
/// (stopping is asynchronous: an active paste is cancelled first). Blocking.
pub fn set(jt: &Installed, on: bool) -> anyhow::Result<Status> {
    if !available(jt) {
        bail!("this JustTools has no JustPaste · update JustTools first");
    }
    if on {
        if let Err(e) = run(jt, &["--enable"]) {
            enable_outside_job(jt).with_context(|| format!("{e:#}"))?;
        }
        run(jt, &["--startup", "on"])?;
    } else {
        run(jt, &["--startup", "off"])?;
        run(jt, &["--disable"])?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = status(jt)?.context("JustPaste disappeared")?;
        if now.running == on || Instant::now() >= deadline {
            return Ok(now);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_status() {
        assert_eq!(parse_status("JustPaste Smart Paste: stopped\nStart at login: false\n"), Some(Status::default()));
        let on = "JustPaste Smart Paste: running\nStart at login: true\nCopy filters: off (Explorer copies files)\n\
                  Paused: false\nBusy: false\nLast Explorer paste: native file paste: copy filters are off\nLast notification: Saved Image.png";
        assert_eq!(parse_status(on), Some(Status { running: true, at_sign_in: true, paused: false, filters: false }));
        assert!(parse_status(on).unwrap().active());
        let filtered = "JustPaste Smart Paste: running\r\nStart at login: false\r\nCopy filters: on (JustPaste copies files)\r\nPaused: true\r\n";
        let s = parse_status(filtered).unwrap();
        assert!(s.filters && s.paused && !s.at_sign_in && !s.active());
        assert_eq!(describe(&s), "paused");
        assert_eq!(describe(&Status { paused: false, ..s }), "on · copy filters");
        assert_eq!(describe(&parse_status(on).unwrap()), "on");
        assert_eq!(describe(&Status::default()), "off");
        assert!(file_note(&s).contains("JustPaste copies") && file_note(&Status::default()).contains("normal Windows way"));
        assert_eq!(parse_status("just 2.8.0"), None);
        assert_eq!(parse_status(""), None);
    }

    /// Switches this machine's real Smart Paste off and back on (leaving it on): `cargo test -p bro-core
    /// justpaste -- --ignored`.
    #[test]
    #[ignore = "switches the real Smart Paste of the JustTools installed here"]
    fn switches_the_installed_smart_paste_off_and_on() {
        let jt = crate::justtools::installed().expect("JustTools is installed");
        assert!(!set(&jt, false).unwrap().running);
        let on = set(&jt, true).unwrap();
        assert!(on.running && on.at_sign_in, "{on:?}");
    }

    #[test]
    fn a_justtools_without_justpaste_has_no_status() {
        let dir = tempfile::tempdir().unwrap();
        let jt = Installed { bin: dir.path().to_path_buf(), version: Some("2.7.0".into()) };
        assert!(!available(&jt));
        assert_eq!(status(&jt).unwrap(), None);
        assert!(set(&jt, true).unwrap_err().to_string().contains("update JustTools"));
    }
}
