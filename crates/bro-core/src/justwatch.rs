//! JustWatch (<https://github.com/JustSuperHuman/JustWatch>): the Windows tray app that starts,
//! watches and restarts local dev servers, with a control API on `http://localhost:6767`. bro
//! finds an installed copy, installs it with its own `install.ps1`, and hands its `justwatch`
//! skill (checking, restarting and tailing dev services through that API) to the harness homes
//! it launches, like the JustTools skill.
use crate::util::atomic_write;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

const REPO: &str = "JustSuperHuman/JustWatch";

pub const SKILL_NAME: &str = "justwatch";

/// The skill's files, relative to `skills/justwatch` in the repository.
const SKILL_FILES: [&str; 1] = ["SKILL.md"];

/// The control API JustWatch serves (its port is `web.port` in its config; 6767 by default).
pub const API: &str = "http://localhost:6767";

/// `[justwatch]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JustWatchSettings {
    /// New Claude and Codex sessions get the JustWatch skill.
    pub skill: bool,
}

/// JustWatch is a Windows app.
pub fn supported() -> bool {
    cfg!(windows)
}

/// The PowerShell one-liner that installs (or upgrades) it: the latest release, for this user,
/// with a Start Menu shortcut, starting at sign-in.
pub fn installer_command() -> String {
    format!("irm https://raw.githubusercontent.com/{REPO}/main/install.ps1 | iex")
}

/// An installed JustWatch.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// `…\JustWatch\app`, holding `JustWatch.exe` and its copy of the skill. None for a copy
    /// found only by its API answering (one run from somewhere bro can't see).
    pub app: Option<PathBuf>,
    /// From its Apps & features entry or its API; None when neither says.
    pub version: Option<String>,
}

/// Where its installer puts it: `JUSTWATCH_INSTALL_DIR`, else `%LOCALAPPDATA%\Programs\JustWatch`.
fn install_dir() -> Option<PathBuf> {
    std::env::var_os("JUSTWATCH_INSTALL_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("JustWatch")))
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\JustWatch";

/// The executable in a `Run` entry: `"F:\JustWatch\app\JustWatch.exe" --minimized`.
fn command_exe(command: &str) -> PathBuf {
    let c = command.trim();
    let exe = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => c.split(" --").next().unwrap_or_default(),
    };
    PathBuf::from(exe.trim())
}

/// Where its app may be: its installer's folder, then wherever its sign-in entry starts it from.
fn app_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = install_dir().map(|d| d.join("app")).into_iter().collect();
    if let Some(run) = reg_query(RUN_KEY, "JustWatch") {
        dirs.extend(command_exe(&run).parent().map(Path::to_path_buf));
    }
    dirs
}

/// Some(version) when JustWatch's API answers on this machine.
fn api_version() -> Option<Option<String>> {
    let resp = crate::http::get_json(&format!("{API}/api"), &[], Duration::from_secs(2)).ok()?;
    (resp.ok() && resp.body.get("name").and_then(|n| n.as_str()) == Some("JustWatch"))
        .then(|| resp.body.get("version").and_then(|v| v.as_str()).map(str::to_string))
}

/// Blocking (reads the registry, may ask the local API): call off the UI thread.
pub fn installed() -> Option<Installed> {
    if !supported() {
        return None;
    }
    match app_dirs().into_iter().find(|d| d.join("JustWatch.exe").is_file()) {
        Some(app) => {
            let version = reg_query(UNINSTALL_KEY, "DisplayVersion").or_else(|| api_version().flatten());
            Some(Installed { app: Some(app), version })
        }
        // a copy run from somewhere else still counts while it's running
        None => api_version().map(|version| Installed { app: None, version }),
    }
}

#[cfg(windows)]
fn reg_query(key: &str, value: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args(["query", key, "/v", value])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| crate::quick_install::reg_value(&String::from_utf8_lossy(&out.stdout))).flatten().filter(|v| !v.is_empty())
}

#[cfg(not(windows))]
fn reg_query(_key: &str, _value: &str) -> Option<String> {
    None
}

pub fn skill_cache_dir() -> PathBuf {
    crate::managed_skill::cache_dir(SKILL_NAME)
}

pub fn skill_cached() -> bool {
    skill_cache_dir().join(SKILL_FILES[0]).is_file()
}

fn is_skill(body: &str) -> bool {
    body.starts_with("---") && body.contains("name: justwatch")
}

fn get_text(url: &str, timeout: Duration) -> anyhow::Result<String> {
    let (status, body) = crate::http::get_text(url, timeout)?;
    if !(200..300).contains(&status) {
        bail!("GET {url}: HTTP {status}");
    }
    Ok(body)
}

/// The skill from the first place that has it: the installed app (it ships a copy beside
/// `JustWatch.exe`), the running app's API, then GitHub.
fn find_skill(app: Option<&Path>) -> anyhow::Result<String> {
    if let Some(body) = app.and_then(|a| std::fs::read_to_string(a.join("skills").join(SKILL_NAME).join("SKILL.md")).ok()).filter(|b| is_skill(b)) {
        return Ok(body);
    }
    if let Some(body) = get_text(&format!("{API}/SKILL.md"), Duration::from_secs(3)).ok().filter(|b| is_skill(b)) {
        return Ok(body);
    }
    let url = format!("https://raw.githubusercontent.com/{REPO}/main/skills/{SKILL_NAME}/SKILL.md");
    let body = get_text(&url, Duration::from_secs(20))?;
    if !is_skill(&body) {
        bail!("{url} isn't the JustWatch skill");
    }
    Ok(body)
}

/// Puts the skill in the cache. Blocking; may need the network.
pub fn fetch_skill() -> anyhow::Result<()> {
    let found = installed();
    let body = find_skill(found.as_ref().and_then(|f| f.app.as_deref()))?;
    let path = skill_cache_dir().join(SKILL_FILES[0]);
    atomic_write(&path, body.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

/// `home` holds a `justwatch` skill bro didn't write.
pub fn has_own_skill(home: &Path) -> bool {
    crate::managed_skill::has_own(home, SKILL_NAME)
}

/// Copies the cached skill under `home` (a no-op when unchanged). False when nothing is
/// cached yet, or `home` has a `justwatch` skill of its own, which is left as it is.
pub fn write_skill(home: &Path) -> std::io::Result<bool> {
    crate::managed_skill::write(home, SKILL_NAME, &SKILL_FILES)
}

/// Takes the skill back out of every Claude and Codex home bro launches.
pub fn remove_skill_everywhere() {
    crate::managed_skill::remove_everywhere(SKILL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn the_installed_apps_skill_is_cached_then_copied_and_removed() {
        let sb = sandbox();
        let app = sb.home().join("JustWatch").join("app");
        let shipped = app.join("skills").join(SKILL_NAME).join("SKILL.md");
        atomic_write(&shipped, b"---\nname: justwatch\n---\nbody").unwrap();
        assert!(find_skill(Some(&app)).unwrap().contains("body"));
        atomic_write(&skill_cache_dir().join("SKILL.md"), find_skill(Some(&app)).unwrap().as_bytes()).unwrap();
        let home = sb.home().join(".claude");
        assert!(skill_cached() && write_skill(&home).unwrap());
        let dir = home.join("skills").join(SKILL_NAME);
        assert!(dir.join("SKILL.md").is_file() && !has_own_skill(&home));
        remove_skill_everywhere();
        assert!(!dir.exists());
        assert!(!is_skill("<html>not found</html>"));
        assert!(installer_command().contains("JustSuperHuman/JustWatch/main/install.ps1"));
        assert_eq!(command_exe(r#""F:\JustWatch\app\JustWatch.exe" --minimized"#), PathBuf::from(r"F:\JustWatch\app\JustWatch.exe"));
        assert_eq!(command_exe(r"C:\x\JustWatch.exe --minimized"), PathBuf::from(r"C:\x\JustWatch.exe"));
    }
}
