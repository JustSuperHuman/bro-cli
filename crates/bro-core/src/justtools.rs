//! JustTools (<https://github.com/JustGains/JustTools>): finding an installed copy, installing
//! it with its own bootstrap, and handing its agent skill to the harness homes bro launches.
//!
//! The skill is downloaded once into `~/.bro/justtools-skill` and copied from there at
//! launch, beside `bro-open`. A `justtools` skill bro did not write is never touched.
use crate::util::{atomic_write, home_dir};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const RAW: &str = "https://raw.githubusercontent.com/JustGains/JustTools";
const LATEST_RELEASE: &str = "https://api.github.com/repos/JustGains/JustTools/releases/latest";

pub const SKILL_NAME: &str = "justtools";

/// The skill's files, relative to `skills/justtools` in the repository.
const SKILL_FILES: [&str; 3] = ["SKILL.md", "references/commands.md", "agents/openai.yaml"];

/// `[justtools]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JustToolsSettings {
    /// New Claude and Codex sessions get the JustTools skill.
    pub skill: bool,
    /// When bro opens and GitHub has a newer release, install it.
    pub auto_update: bool,
}

impl Default for JustToolsSettings {
    fn default() -> Self {
        Self { skill: false, auto_update: true }
    }
}

/// How far an install has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// fetching the bootstrap script
    Installer,
    /// downloading the release archive
    Download,
    /// checking the archive against its SHA-256
    Verify,
    /// `just install`: the binary and its aliases
    Install,
    /// downloading the agent skill
    Skill,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Installer, Step::Download, Step::Verify, Step::Install, Step::Skill];
}

/// An installed JustTools.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub bin: PathBuf,
    /// None when `just --version` couldn't be read.
    pub version: Option<String>,
}

impl Installed {
    /// The directory is on this process's PATH, so sessions bro launches can run the tools.
    pub fn on_path(&self) -> bool {
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| crate::util::same_path(&d, &self.bin)))
    }
}

pub(crate) fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Where JustTools may live: PATH, then the bootstrap's own choices (a fresh install isn't
/// on a running process's PATH yet).
fn candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(std::env::var_os("JUSTTOOLS_BIN").map(PathBuf::from));
    if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\cmd\bin"));
        dirs.extend(std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("JustTools").join("bin")));
    } else {
        dirs.push(home_dir().join(".local").join("bin"));
    }
    dirs
}

/// The first directory holding JustTools. `justready` is the tell: plenty of machines have
/// another `just` (the command runner).
fn find_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find(|d| !d.as_os_str().is_empty() && d.join(exe("justready")).is_file() && d.join(exe("just")).is_file()).cloned()
}

pub(crate) fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null())
}

/// "2.8.0" from `just --version`'s "just 2.8.0".
fn parse_version(out: &str) -> Option<String> {
    let v = out.lines().next()?.trim().strip_prefix("just ")?.trim();
    (!v.is_empty() && v.starts_with(|c: char| c.is_ascii_digit())).then(|| v.to_string())
}

/// Runs `just --version` (blocking: call off the UI thread).
pub fn installed() -> Option<Installed> {
    let bin = find_in(&candidates())?;
    let out = quiet(&mut Command::new(bin.join(exe("just")))).arg("--version").output().ok();
    let version = out.filter(|o| o.status.success()).and_then(|o| parse_version(&String::from_utf8_lossy(&o.stdout)));
    Some(Installed { bin, version })
}

fn get_text(url: &str, timeout: Duration) -> anyhow::Result<String> {
    let (status, body) = crate::http::get_text(url, timeout)?;
    if !(200..300).contains(&status) {
        bail!("GET {url}: HTTP {status}");
    }
    Ok(body)
}

/// The newest release on GitHub, e.g. "2.8.0". Blocking; needs the network.
pub fn latest_version() -> anyhow::Result<String> {
    let resp = crate::http::get_json(LATEST_RELEASE, &[("user-agent", "bro")], Duration::from_secs(15))?;
    if !resp.ok() {
        bail!("GitHub answered HTTP {}", resp.status);
    }
    let tag = resp.body.get("tag_name").and_then(|t| t.as_str()).context("the release has no tag")?;
    Ok(tag.trim_start_matches('v').to_string())
}

fn version_parts(v: &str) -> Vec<u64> {
    let core = v.trim_start_matches('v').split(['-', '+']).next().unwrap_or_default();
    core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// `latest` is a later release than `current` ("2.10.0" is newer than "2.9.3").
pub fn is_newer(latest: &str, current: &str) -> bool {
    version_parts(latest) > version_parts(current)
}

/// What a line the bootstrap printed says about its progress.
fn step_for(line: &str) -> Option<Step> {
    let line = line.trim();
    if line.starts_with("Downloading") {
        return Some(if line.ends_with(".sha256") { Step::Verify } else { Step::Download });
    }
    line.starts_with("Verified").then_some(Step::Install)
}

/// The last lines an installer printed, for an error message.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" · ")
}

/// Installs (or updates to) the latest release with JustTools' own bootstrap, which
/// verifies the download's SHA-256. JustReady is not opened. `on_step` hears how far it
/// has got. Blocking; needs the network.
pub fn install(on_step: &dyn Fn(Step)) -> anyhow::Result<Installed> {
    on_step(Step::Installer);
    let name = if cfg!(windows) { "ready.ps1" } else { "ready.sh" };
    let script = get_text(&format!("{RAW}/main/{name}"), Duration::from_secs(30))?;
    let file = std::env::temp_dir().join(format!("bro-justtools-{}-{name}", std::process::id()));
    std::fs::write(&file, script).with_context(|| format!("writing {}", file.display()))?;
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]);
        c
    } else {
        Command::new("sh")
    };
    let child = quiet(cmd.arg(&file).env("JUSTREADY_NO_RUN", "1")).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&file);
            return Err(e).context("starting the JustTools installer");
        }
    };
    // stderr is drained on its own thread, so a chatty installer can't fill the pipe and stall
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut e) = stderr {
            let mut bytes = vec![];
            let _ = e.read_to_end(&mut bytes);
            text = String::from_utf8_lossy(&bytes).into_owned();
        }
        text
    });
    let mut printed = String::new();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line);
            if let Some(step) = step_for(&line) {
                on_step(step);
            }
            printed.push_str(&line);
            printed.push('\n');
        }
    }
    let status = child.wait();
    let _ = std::fs::remove_file(&file);
    printed.push_str(&errors.join().unwrap_or_default());
    if !status.context("waiting for the JustTools installer")?.success() {
        bail!("the JustTools installer failed: {}", tail(&printed));
    }
    installed().context("the installer finished but JustTools wasn't found")
}

/// `~/.bro/justtools-skill`: the downloaded skill, copied into harness homes at launch.
pub fn skill_cache_dir() -> PathBuf {
    crate::managed_skill::cache_dir(SKILL_NAME)
}

pub fn skill_cached() -> bool {
    skill_cache_dir().join(SKILL_FILES[0]).is_file()
}

fn fetch_skill_at(reference: &str) -> anyhow::Result<Vec<String>> {
    let files: Vec<String> = SKILL_FILES
        .iter()
        .map(|f| get_text(&format!("{RAW}/{reference}/skills/{SKILL_NAME}/{f}"), Duration::from_secs(20)))
        .collect::<anyhow::Result<_>>()?;
    if !files[0].starts_with("---") || !files[0].contains("name: justtools") {
        bail!("{reference} has no JustTools skill");
    }
    Ok(files)
}

/// Downloads the skill into the cache: the one tagged for the installed `version`, else the
/// newest. Blocking; needs the network.
pub fn fetch_skill(version: Option<&str>) -> anyhow::Result<()> {
    let tagged = version.and_then(|v| fetch_skill_at(&format!("v{v}")).ok());
    let files = match tagged {
        Some(f) => f,
        None => fetch_skill_at("main")?,
    };
    for (name, body) in SKILL_FILES.iter().zip(files) {
        let path = skill_cache_dir().join(name);
        atomic_write(&path, body.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
fn skill_dir(home: &Path) -> PathBuf {
    crate::managed_skill::skill_dir(home, SKILL_NAME)
}

/// `home` holds a `justtools` skill bro didn't write (yours, e.g. from `npx skills add`).
pub fn has_own_skill(home: &Path) -> bool {
    crate::managed_skill::has_own(home, SKILL_NAME)
}

/// Copies the cached skill under `home` (a no-op when unchanged). False when there is
/// nothing cached yet, or `home` has a `justtools` skill of its own, which is left as it is.
pub fn write_skill(home: &Path) -> std::io::Result<bool> {
    crate::managed_skill::write(home, SKILL_NAME, &SKILL_FILES)
}

/// Removes the copy bro wrote under `home`. A link is skipped (what it points at is
/// removed where it really lives); your own skill is never removed.
pub fn remove_skill(home: &Path) {
    crate::managed_skill::remove(home, SKILL_NAME)
}

/// Takes the skill back out of every Claude and Codex home bro launches.
pub fn remove_skill_everywhere() {
    crate::managed_skill::remove_everywhere(SKILL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn cache() {
        for name in SKILL_FILES {
            atomic_write(&skill_cache_dir().join(name), format!("{name} body").as_bytes()).unwrap();
        }
    }

    #[test]
    fn finds_justtools_by_justready_not_by_any_just() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, tools) = (dir.path().join("runner"), dir.path().join("tools"));
        for d in [&runner, &tools] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(exe("just")), "").unwrap();
        }
        std::fs::write(tools.join(exe("justready")), "").unwrap();
        assert_eq!(find_in(&[PathBuf::new(), runner.clone(), tools.clone()]), Some(tools));
        assert_eq!(find_in(&[runner]), None, "another `just` is not JustTools");
        assert_eq!(parse_version("just 2.8.0\n").as_deref(), Some("2.8.0"));
        assert_eq!(parse_version("just: unknown option"), None);
    }

    #[test]
    fn releases_compare_by_number_and_installer_lines_map_to_steps() {
        assert!(is_newer("2.10.0", "2.9.3") && is_newer("v3.0.0", "2.8.0") && is_newer("2.8.1", "2.8"));
        assert!(!is_newer("2.8.0", "2.8.0") && !is_newer("2.7.9", "2.8.0") && !is_newer("2.8.0-rc1", "2.8.0"));
        assert_eq!(step_for("Downloading https://github.com/x/releases/latest/download/justtools-windows-x64.zip"), Some(Step::Download));
        assert_eq!(step_for("Downloading https://github.com/x/releases/latest/download/justtools-windows-x64.zip.sha256\r"), Some(Step::Verify));
        assert_eq!(step_for("Verified justtools-windows-x64.zip"), Some(Step::Install));
        assert_eq!(step_for("Installed 31 commands"), None);
        assert_eq!(tail("a\n\n b \nc\nd\n"), "b · c · d");
        assert!(JustToolsSettings::default().auto_update && !JustToolsSettings::default().skill);
    }

    #[test]
    #[ignore = "network"]
    fn github_names_the_latest_release() {
        let v = latest_version().unwrap();
        assert!(v.starts_with(|c: char| c.is_ascii_digit()) && !is_newer("0.0.1", &v), "{v}");
    }

    #[test]
    fn skill_is_copied_from_the_cache_and_removed_again() {
        let sb = sandbox();
        let home = sb.home().join(".codex");
        assert!(!write_skill(&home).unwrap(), "nothing downloaded yet");
        assert!(!home.join("skills").exists());
        cache();
        assert!(skill_cached() && write_skill(&home).unwrap());
        let dir = skill_dir(&home);
        assert_eq!(std::fs::read_to_string(dir.join("references").join("commands.md")).unwrap(), "references/commands.md body");
        // a newer download replaces bro's copy
        atomic_write(&skill_cache_dir().join("SKILL.md"), b"newer").unwrap();
        assert!(write_skill(&home).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "newer");
        remove_skill_everywhere();
        assert!(!dir.exists());
    }

    #[test]
    #[ignore = "network"]
    fn the_skill_downloads_for_a_release_and_for_main() {
        let _sb = sandbox();
        fetch_skill(Some("2.8.0")).unwrap();
        let body = std::fs::read_to_string(skill_cache_dir().join("SKILL.md")).unwrap();
        assert!(body.contains("name: justtools"), "{body}");
        assert!(skill_cache_dir().join("references").join("commands.md").is_file());
        // a version with no tag falls back to the newest skill
        fetch_skill(Some("0.0.0-none")).unwrap();
    }

    #[test]
    fn your_own_skill_is_left_alone() {
        let sb = sandbox();
        cache();
        let home = sb.home().join(".claude");
        let mine = skill_dir(&home).join("SKILL.md");
        atomic_write(&mine, b"mine").unwrap();
        assert!(has_own_skill(&home));
        assert!(!write_skill(&home).unwrap());
        remove_skill_everywhere();
        assert_eq!(std::fs::read_to_string(&mine).unwrap(), "mine");
    }
}
