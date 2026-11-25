//! Cua Driver: install the upstream desktop driver, then let Bro own harness wiring.
use crate::util::{env_path, home_dir};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

pub const SERVER_NAME: &str = "cua-driver";
pub const SKILL: &str = "cua-driver";
pub const DOCS: &str = "https://cua.ai/docs/cua-driver/quickstart";

/// Local files only. Include the installer's location before a running Bro has refreshed PATH.
pub fn installed() -> Option<PathBuf> {
    if let Some(path) = env_path("BRO_CUA_DRIVER") {
        return path.is_file().then_some(path);
    }
    crate::launch::which("cua-driver").or_else(|| {
        let home = home_dir();
        let path = if cfg!(windows) {
            env_path("LOCALAPPDATA")
                .unwrap_or_else(|| home.join("AppData/Local"))
                .join("Programs/Cua/cua-driver/bin/cua-driver.exe")
        } else if cfg!(target_os = "macos") {
            home.join("Applications/CuaDriver.app/Contents/MacOS/cua-driver")
        } else {
            home.join(".local/bin/cua-driver")
        };
        path.is_file().then_some(path)
    })
}

pub fn supported() -> bool {
    cfg!(windows) || cfg!(target_os = "macos") || cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

/// Check driver prerequisites without launching or operating an application.
pub fn check_readiness() -> Result<String, String> {
    let binary = installed().ok_or("Cua Driver is no longer installed.")?;
    let mut command = Command::new(binary);
    command.args(["doctor", "--json"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| format!("Could not start Cua Driver: {e}"))?;
    diagnostic_summary(&output.stdout, output.status.success())
}

fn diagnostic_summary(stdout: &[u8], success: bool) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Report {
        ok: bool,
        probes: Vec<Probe>,
    }
    #[derive(serde::Deserialize)]
    struct Probe {
        label: String,
        status: String,
        message: String,
    }
    let help = "Run `cua-driver doctor` for details.";
    let report: Report =
        serde_json::from_slice(stdout).map_err(|_| format!("Bro could not read CUA's diagnostic report. {help}"))?;
    // Upstream deliberately exits zero for warnings. Preserve them instead of
    // presenting transport/system readiness as proven app automation.
    let issues: Vec<_> = report
        .probes
        .iter()
        .filter(|p| p.status != "ok")
        .take(3)
        .map(|p| format!("{}: {}", p.label, p.message))
        .collect();
    if !issues.is_empty() {
        return Err(format!("CUA needs attention. {}. {help}", issues.join("; ")));
    }
    if !success || !report.ok || report.probes.is_empty() {
        return Err(format!("CUA could not confirm its system checks. {help}"));
    }
    Ok("Cua Driver passed its system checks. App controls still need to be checked.".into())
}

pub fn install(mut say: impl FnMut(&str)) -> Result<(), String> {
    if !supported() {
        return Err("Cua Driver supports Windows, macOS and x86_64 Linux desktops.".into());
    }
    let suffix = if cfg!(windows) { "ps1" } else { "sh" };
    let script = std::env::temp_dir().join(format!("bro-cua-{}-{:x}.{suffix}", std::process::id(), rand::random::<u64>()));
    let result = (|| -> anyhow::Result<()> {
        say("Downloading Cua Driver's official installer…");
        crate::http::download(&format!("https://cua.ai/driver/install.{suffix}"), &script, Duration::from_secs(60))?;
        // Upstream verifies its release. Bro supplies session-scoped MCP configuration; the
        // installer must not register a second global server or perform desktop actions as a test.
        let mut command = if cfg!(windows) {
            let mut c = Command::new("powershell.exe");
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .args(["-NoConnect", "-NoTest"]);
            c
        } else {
            let mut c = Command::new("bash");
            c.arg(&script).args(["--no-connect", "--no-test"]);
            c
        };
        command.env("CUA_DRIVER_NO_CONNECT", "1").env("CUA_DRIVER_NO_TEST", "1");
        if !command.status()?.success() {
            anyhow::bail!("Cua Driver's installer failed; run `bro install cua --force` to retry");
        }
        let binary =
            installed().ok_or_else(|| anyhow::anyhow!("Cua Driver installed, but Bro could not find cua-driver; restart Bro and retry"))?;
        say("Installing Cua Driver's version-matched guides…");
        if !Command::new(&binary).args(["skills", "install"]).status()?.success() {
            anyhow::bail!("Cua Driver installed, but its guides need attention; run `bro install cua --force` to retry");
        }
        let output = Command::new(&binary).args(["skills", "path"]).output()?;
        if !output.status.success() {
            anyhow::bail!("Cua Driver could not locate its guides");
        }
        let source = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        crate::tool_skills::cache(SKILL, &source).map_err(anyhow::Error::msg)?;
        say("Bro will connect CUA and disable built-in browser and computer tools in new sessions.");
        Ok(())
    })();
    let _ = std::fs::remove_file(&script);
    result.map_err(|e| format!("{e:#}"))
}

pub fn after_install() -> String {
    let platform = if cfg!(target_os = "macos") {
        "Run `cua-driver permissions grant` to grant Accessibility and Screen Recording to CuaDriver."
    } else if cfg!(target_os = "linux") {
        "Run in a graphical X11 or XWayland session with AT-SPI 2."
    } else {
        "Keep a Windows desktop session signed in. Leave apps open behind your active window; minimizing can hide their controls. Elevated apps need matching driver privileges."
    };
    format!(
        "{platform} Bro connects Claude and Codex through MCP, and Pi and omp through the CUA CLI. New sessions use Bro's tools. Run `cua-driver doctor` to check readiness."
    )
}

#[cfg(test)]
mod tests {
    use super::diagnostic_summary;

    #[test]
    fn readiness_preserves_warnings_even_when_doctor_exits_zero() {
        let report = br#"{"ok":true,"probes":[{"label":"interactive session","status":"warn","message":"No attached desktop"}]}"#;
        let result = diagnostic_summary(report, true).unwrap_err();
        assert!(result.contains("interactive session: No attached desktop"));
    }

    #[test]
    fn readiness_requires_a_complete_successful_report() {
        for report in [
            &b"not json"[..],
            br#"{"ok":true,"probes":[]}"#,
            br#"{"ok":false,"probes":[{"label":"UI Automation","status":"ok","message":"Reachable"}]}"#,
            br#"{"ok":false,"probes":[{"label":"UI Automation","status":"err","message":"Unavailable"}]}"#,
            br#"{"ok":true,"probes":[{"label":"UI Automation","status":"unknown","message":"Unverified"}]}"#,
        ] {
            assert!(diagnostic_summary(report, true).is_err());
        }
        let report = br#"{"ok":true,"probes":[{"label":"UI Automation","status":"ok","message":"Reachable"}]}"#;
        assert!(diagnostic_summary(report, false).is_err());
        assert!(diagnostic_summary(report, true).is_ok());
    }
}
