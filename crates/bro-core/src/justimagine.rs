//! JustImagine (<https://github.com/JustGains/JustImagine>): the self-hosted image, video, song and
//! voice gallery, run as a background service. bro keeps a checkout of its `main` branch, installs
//! its npm dependencies and registers it with JustImagine's own `service install` (a Task Scheduler
//! task on Windows, launchd on macOS, systemd or an XDG autostart entry on Linux).
//!
//! It has no releases, so an update is newer commits on `main`: GitHub's compare API says how far
//! the checkout is behind, and an update is `git pull --ff-only`, `npm install` and a service restart.
use crate::quick_install::{Step, find, search_dirs};
use crate::util::home_dir;
use anyhow::{Context, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const REPO: &str = "JustGains/JustImagine";

pub const CLONE_URL: &str = "https://github.com/JustGains/JustImagine.git";

/// The port its service listens on unless it recorded another.
pub const DEFAULT_PORT: u16 = 8793;

/// Where bro keeps its checkout: `JUSTIMAGINE_INSTALL_DIR`, else `%LOCALAPPDATA%\Programs\JustImagine`
/// on Windows and `$XDG_DATA_HOME/justimagine` (`~/.local/share/justimagine`) elsewhere.
pub fn install_dir() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("JUSTIMAGINE_INSTALL_DIR") {
        return dir;
    }
    if cfg!(windows) {
        let local = var("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local"));
        return local.join("Programs").join("JustImagine");
    }
    var("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local").join("share")).join("justimagine")
}

/// JustImagine's own home (keys, characters, service files): `JUSTIMAGINE_HOME`, else `~/.justimagine`.
pub fn app_home() -> PathBuf {
    std::env::var_os("JUSTIMAGINE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".justimagine"))
}

/// What its service recorded in `<home>/service/service.json`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
struct ServiceRecord {
    root: Option<PathBuf>,
    port: Option<u16>,
}

fn service_record(home: &Path) -> Option<ServiceRecord> {
    let text = std::fs::read_to_string(home.join("service").join("service.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The gallery the service serves: the one it served before, else bro v1's `~/.bro/justimagine`
/// when that exists (v1 ran the same service on it), else `<home>/gallery`.
fn gallery_for(home: &Path, v1: &Path) -> PathBuf {
    if let Some(root) = service_record(home).and_then(|r| r.root).filter(|r| !r.as_os_str().is_empty()) {
        return root;
    }
    if v1.is_dir() {
        return v1.to_path_buf();
    }
    home.join("gallery")
}

pub fn gallery_root() -> PathBuf {
    gallery_for(&app_home(), &home_dir().join(".bro").join("justimagine"))
}

/// An installed JustImagine.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// bro's checkout. None for a copy found only by its service answering (run from somewhere
    /// bro doesn't manage, e.g. a development checkout).
    pub dir: Option<PathBuf>,
    /// `version` in its package.json
    pub version: Option<String>,
    /// the checked-out commit
    pub commit: Option<String>,
    /// where its service answers, when it's running
    pub url: Option<String>,
}

impl Installed {
    /// "0.1.0 · 0a50ae7", for lists.
    pub fn describe(&self) -> String {
        match (&self.version, &self.commit) {
            (Some(v), Some(c)) => format!("{v} · {}", short(c)),
            (None, Some(c)) => short(c).to_string(),
            (Some(v), None) => v.clone(),
            (None, None) if self.url.is_some() => "running".into(),
            (None, None) => "installed".into(),
        }
    }
}

/// A commit's first 7 characters.
pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

fn is_checkout(dir: &Path) -> bool {
    dir.join("bin").join("justimagine.js").is_file() && std::fs::read_to_string(dir.join("package.json")).is_ok_and(|p| p.contains("justimagine"))
}

fn package_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version").and_then(|v| v.as_str()).map(str::to_string)
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The commit `dir`'s HEAD points at, read from `.git` (no git needed): a loose ref, then packed-refs.
fn head_commit(dir: &Path) -> Option<String> {
    let git = dir.join(".git");
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let sha = match head.strip_prefix("ref:").map(str::trim) {
        None => head.to_string(),
        Some(name) => match std::fs::read_to_string(git.join(name)) {
            Ok(s) => s.trim().to_string(),
            Err(_) => std::fs::read_to_string(git.join("packed-refs")).ok()?.lines().find_map(|l| {
                let (sha, r) = l.split_once(' ')?;
                (r.trim() == name).then(|| sha.to_string())
            })?,
        },
    };
    is_sha(&sha).then_some(sha)
}

/// Some(url) when a JustImagine answers on `port`.
fn running(port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}");
    let resp = crate::http::get_json(&format!("{url}/health"), &[], Duration::from_secs(2)).ok()?;
    (resp.ok() && resp.body.get("service").and_then(|s| s.as_str()) == Some("justimagine")).then_some(url)
}

/// The port its service listens on: the one it recorded, else [`DEFAULT_PORT`].
pub fn port() -> u16 {
    service_record(&app_home()).and_then(|r| r.port).unwrap_or(DEFAULT_PORT)
}

/// Its API and sign-in token: beside its config (`JUSTIMAGINE_CONFIG_PATH`), else in its home.
fn token_file() -> PathBuf {
    match std::env::var_os("JUSTIMAGINE_CONFIG_PATH").filter(|v| !v.is_empty()) {
        Some(config) => Path::new(&config).parent().map_or_else(app_home, Path::to_path_buf).join("auth.token"),
        None => app_home().join("auth.token"),
    }
}

/// `token` for a query string.
fn encode(token: &str) -> String {
    token.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// The gallery's address, signed in: JustImagine trades `?token=` for a cookie and drops it from
/// the address bar. Without a token file, the plain address.
pub fn signed_in_url(port: u16, token: Option<&str>) -> String {
    let base = format!("http://127.0.0.1:{port}");
    match token.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{base}/?token={}", encode(t)),
        None => base,
    }
}

/// The running gallery's signed-in address, or None when nothing answers. Blocking.
pub fn open_url() -> Option<String> {
    let port = port();
    running(port)?;
    Some(signed_in_url(port, std::fs::read_to_string(token_file()).ok().as_deref()))
}

/// bro's checkout, or a service answering on its recorded port. Blocking (asks the local
/// service): call off the UI thread.
pub fn installed() -> Option<Installed> {
    let url = running(port());
    let dir = Some(install_dir()).filter(|d| is_checkout(d));
    if dir.is_none() && url.is_none() {
        return None;
    }
    Some(Installed { version: dir.as_deref().and_then(package_version), commit: dir.as_deref().and_then(head_commit), dir, url })
}

/// What `main` on GitHub has that a checkout doesn't.
#[derive(Debug, Clone, PartialEq)]
pub struct Behind {
    /// commits on `main` the checkout lacks; 0 = up to date
    pub commits: u32,
    /// `main`'s newest commit, when the checkout lacks any
    pub latest: Option<String>,
}

/// A compare answer for `{checkout}...main`: `ahead_by` is what `main` has on top of the checkout.
fn parse_compare(body: &serde_json::Value) -> anyhow::Result<Behind> {
    let commits = body.get("ahead_by").and_then(|n| n.as_u64()).context("GitHub's answer has no ahead_by")? as u32;
    let latest = body.get("commits").and_then(|c| c.as_array()).and_then(|c| c.last()).and_then(|c| c.get("sha")).and_then(|s| s.as_str()).map(str::to_string);
    Ok(Behind { commits, latest: latest.filter(|_| commits > 0) })
}

/// How far `commit` is behind `main` on GitHub. Blocking; needs the network.
pub fn behind(commit: &str) -> anyhow::Result<Behind> {
    let url = format!("https://api.github.com/repos/{REPO}/compare/{commit}...main");
    let resp = crate::http::get_json(&url, &[("user-agent", "bro")], Duration::from_secs(15))?;
    match resp.status {
        404 => bail!("GitHub doesn't know commit {} (a local commit?)", short(commit)),
        s if !(200..300).contains(&s) => bail!("GitHub answered HTTP {s}"),
        _ => parse_compare(&resp.body),
    }
}

fn step(program: &Path, args: &[&std::ffi::OsStr]) -> Step {
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let name = program.file_stem().map_or_else(|| program.display().to_string(), |s| s.to_string_lossy().into_owned());
    let quoted: Vec<String> = args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect();
    Step { program: program.display().to_string(), display: format!("{name} {}", quoted.join(" ")), args, ok_codes: vec![], or_else: None }
}

/// The commands that install JustImagine in `dir` (or update the checkout there) and (re)start its
/// service on the gallery `root`.
pub fn plan(dir: &Path, root: &Path, git: &Path, npm: &Path, node: &Path) -> Vec<Step> {
    let os = |s: &str| std::ffi::OsString::from(s);
    let (dir, root) = (dir.as_os_str().to_owned(), root.as_os_str().to_owned());
    let update = Path::new(&dir).join(".git").exists();
    let mut steps = vec![if update {
        step(git, &[&os("-C"), &dir, &os("pull"), &os("--ff-only")])
    } else {
        step(git, &[&os("clone"), &os("--depth"), &os("1"), &os(CLONE_URL), &dir])
    }];
    // no package-lock.json in the checkout: an untracked one could block the next pull
    steps.push(step(npm, &[&os("install"), &os("--prefix"), &dir, &os("--omit=dev"), &os("--no-package-lock"), &os("--no-audit"), &os("--no-fund")]));
    let entry = Path::new(&dir).join("bin").join("justimagine.js").into_os_string();
    // a running service keeps the old code until it stops; with none installed this fails, harmlessly
    let mut stop = step(node, &[&entry, &os("service"), &os("stop")]);
    stop.ok_codes = vec![1];
    steps.push(stop);
    // a first install opens the gallery, signed in (JustImagine adds its token); an update doesn't
    let mut install = vec![entry.clone(), os("service"), os("install"), os("--root"), root];
    if update {
        install.push(os("--no-open"));
    }
    steps.push(step(node, &install.iter().map(|a| a.as_os_str()).collect::<Vec<_>>()));
    steps
}

/// The service mechanism on this OS, for the Quick Install view.
pub fn mechanism() -> &'static str {
    if cfg!(windows) {
        "a Task Scheduler task"
    } else if cfg!(target_os = "macos") {
        "a launchd agent"
    } else {
        "a systemd user unit"
    }
}

/// One line on how it gets installed, for the Quick Install view.
pub fn how() -> String {
    format!("git clone {CLONE_URL} · npm install · its own `service install` ({}, port {DEFAULT_PORT})", mechanism())
}

/// Install (or update) JustImagine and its service in the current terminal. `say` gets progress lines.
pub fn install(mut say: impl FnMut(&str)) -> Result<(), String> {
    let dirs = search_dirs();
    let need = |exe: &str, name: &str, id: &str| find(exe, &dirs).ok_or_else(|| format!("JustImagine needs {name}: `bro install {id}` first"));
    let git = need("git", "Git", "git")?;
    let node = need("node", "Node.js", "node")?;
    let npm = need("npm", "npm", "node")?;
    let dir = install_dir();
    if !dir.join(".git").exists() && std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{} isn't empty and isn't a JustImagine checkout: move it aside, or set JUSTIMAGINE_INSTALL_DIR", dir.display()));
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let root = gallery_root();
    for s in plan(&dir, &root, &git, &npm, &node) {
        say(&format!("$ {}", s.display));
        crate::quick_install::run(&s)?;
    }
    say(&format!("gallery: {}", root.display()));
    Ok(())
}

/// Restart bro's JustImagine service so it reads the environment again (it takes keys from it
/// only when it starts). Runs in the current terminal. Err when bro's copy isn't installed.
pub fn restart_service() -> Result<(), String> {
    let dir = install_dir();
    if !is_checkout(&dir) {
        return Err("bro's JustImagine isn't installed".into());
    }
    let node = find("node", &search_dirs()).ok_or("Node.js isn't installed")?;
    let entry = dir.join("bin").join("justimagine.js").into_os_string();
    let os = |s: &str| std::ffi::OsString::from(s);
    crate::quick_install::run(&step(&node, &[&entry, &os("service"), &os("restart")]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::atomic_write;

    #[test]
    fn reads_the_checked_out_commit_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(head_commit(d), None);
        let sha = "0a50ae7c5f0e7c2b1d1e9f8a7b6c5d4e3f2a1b0c";
        atomic_write(&d.join(".git").join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        atomic_write(&d.join(".git").join("packed-refs"), format!("# pack-refs\n{sha} refs/heads/main\n").as_bytes()).unwrap();
        assert_eq!(head_commit(d).as_deref(), Some(sha));
        let loose = "1111111111111111111111111111111111111111";
        atomic_write(&d.join(".git").join("refs").join("heads").join("main"), format!("{loose}\n").as_bytes()).unwrap();
        assert_eq!(head_commit(d).as_deref(), Some(loose));
        atomic_write(&d.join(".git").join("HEAD"), b"not a sha").unwrap();
        assert_eq!(head_commit(d), None);

        assert!(!is_checkout(d));
        atomic_write(&d.join("package.json"), br#"{"name":"@justsuperhuman/justimagine","version":"0.1.0"}"#).unwrap();
        atomic_write(&d.join("bin").join("justimagine.js"), b"").unwrap();
        assert!(is_checkout(d));
        assert_eq!(package_version(d).as_deref(), Some("0.1.0"));
        let found = Installed { dir: Some(d.to_path_buf()), version: Some("0.1.0".into()), commit: Some(sha.into()), url: None };
        assert_eq!(found.describe(), "0.1.0 · 0a50ae7");
        assert_eq!(Installed { dir: None, version: None, commit: None, url: Some("http://127.0.0.1:8793".into()) }.describe(), "running");
    }

    #[test]
    fn compare_answers_count_what_main_has_on_top() {
        let behind = parse_compare(&serde_json::json!({"status": "ahead", "ahead_by": 3, "behind_by": 0, "commits": [{"sha": "a"}, {"sha": "b"}, {"sha": "c"}]})).unwrap();
        assert_eq!(behind, Behind { commits: 3, latest: Some("c".into()) });
        // a development checkout ahead of main has nothing to pull
        let ahead = parse_compare(&serde_json::json!({"status": "behind", "ahead_by": 0, "behind_by": 7, "commits": []})).unwrap();
        assert_eq!(ahead, Behind { commits: 0, latest: None });
        assert!(parse_compare(&serde_json::json!({"message": "Not Found"})).is_err());
    }

    #[test]
    fn the_gallery_follows_the_service_then_bro_v1() {
        let dir = tempfile::tempdir().unwrap();
        let (home, v1) = (dir.path().join(".justimagine"), dir.path().join(".bro").join("justimagine"));
        assert_eq!(gallery_for(&home, &v1), home.join("gallery"));
        std::fs::create_dir_all(&v1).unwrap();
        assert_eq!(gallery_for(&home, &v1), v1);
        let mine = dir.path().join("pictures");
        let record = serde_json::json!({"root": mine, "port": 8791}).to_string();
        atomic_write(&home.join("service").join("service.json"), record.as_bytes()).unwrap();
        assert_eq!(gallery_for(&home, &v1), mine);
        assert_eq!(service_record(&home).unwrap().port, Some(8791));
    }

    #[test]
    fn plans_a_clone_or_a_pull_then_npm_and_the_service() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("JustImagine");
        let (git, npm, node) = (PathBuf::from("git"), PathBuf::from("npm"), PathBuf::from("node"));
        let root = dir.path().join("gallery");
        let steps = plan(&app, &root, &git, &npm, &node);
        assert_eq!(steps.len(), 4);
        assert!(steps[0].display.starts_with("git clone --depth 1 https://github.com/JustGains/JustImagine.git"), "{}", steps[0].display);
        assert_eq!(steps[1].args[..3], ["install".to_string(), "--prefix".into(), app.display().to_string()]);
        assert!(steps[2].args.ends_with(&["service".into(), "stop".into()]) && steps[2].ok_codes == [1]);
        // a first install opens the gallery signed in
        assert_eq!(steps[3].args[1..], ["service".to_string(), "install".into(), "--root".into(), root.display().to_string()]);
        std::fs::create_dir_all(app.join(".git")).unwrap();
        let steps = plan(&app, &root, &git, &npm, &node);
        assert_eq!(steps[0].args, ["-C".to_string(), app.display().to_string(), "pull".into(), "--ff-only".into()]);
        assert_eq!(steps[3].args.last().map(String::as_str), Some("--no-open"), "an update doesn't open a browser");
    }

    #[test]
    fn the_gallery_url_carries_its_token() {
        assert_eq!(signed_in_url(8793, Some("ab12\n")), "http://127.0.0.1:8793/?token=ab12");
        assert_eq!(signed_in_url(8791, Some("a+b/c=")), "http://127.0.0.1:8791/?token=a%2Bb%2Fc%3D");
        assert_eq!(signed_in_url(8793, Some("  ")), "http://127.0.0.1:8793");
        assert_eq!(signed_in_url(8793, None), "http://127.0.0.1:8793");
    }

    /// Clones and installs dependencies for real, without touching the service.
    #[test]
    #[ignore = "network; needs git and npm"]
    fn clones_and_installs_dependencies_then_pulls() {
        let dirs = search_dirs();
        let (git, npm, node) = (find("git", &dirs).unwrap(), find("npm", &dirs).unwrap(), find("node", &dirs).unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("Just Imagine");
        let steps = plan(&app, &tmp.path().join("gallery"), &git, &npm, &node);
        for s in &steps[..2] {
            crate::quick_install::run(s).unwrap();
        }
        assert!(is_checkout(&app) && head_commit(&app).is_some());
        if std::fs::read_to_string(app.join("package.json")).unwrap().contains("\"dependencies\"") {
            assert!(app.join("node_modules").is_dir());
        }
        assert!(!app.join("package-lock.json").exists());
        let pull = &plan(&app, &tmp.path().join("gallery"), &git, &npm, &node)[0];
        assert_eq!(pull.args[2], "pull");
        crate::quick_install::run(pull).unwrap();
    }

    #[test]
    #[ignore = "network"]
    fn github_compares_a_commit_with_main() {
        let b = behind("0a50ae7").unwrap();
        assert!(b.commits == 0 || b.latest.is_some(), "{b:?}");
        assert!(behind("0000000000000000000000000000000000000000").is_err());
    }
}
