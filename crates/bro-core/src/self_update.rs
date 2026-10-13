//! Updating bro itself from its GitHub releases: the newest version, and replacing the running
//! binaries with it in place, the way `install.ps1` / `install.sh` install them: the archive for
//! this platform, checked against the release's `SHA256SUMS.txt`, unpacked with `tar`, copied in
//! beside the running `bro`. A running executable can't be overwritten on Windows, but it can be
//! renamed, so the old one is moved aside (`bro.exe.old`) and removed the next time bro starts.
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const REPO: &str = "JustSuperHuman/bro-cli";

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// `[updates]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for a new release when bro opens and every few hours after.
    pub check: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check: true }
    }
}

/// How often a running bro looks again.
pub const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// How far an update has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// the release archive
    Download,
    /// against SHA256SUMS.txt
    Verify,
    /// unpacking and swapping the binaries
    Install,
}

impl Step {
    pub const ALL: [Step; 3] = [Step::Download, Step::Verify, Step::Install];

    pub fn label(self) -> &'static str {
        match self {
            Step::Download => "downloading the release",
            Step::Verify => "checking its SHA-256",
            Step::Install => "replacing bro",
        }
    }
}

/// The release target this build was made for; None where there's no prebuilt binary.
pub fn target() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else {
        None
    }
}

/// The newest release on GitHub, e.g. "2.0.14". Blocking; needs the network.
pub fn latest_version() -> anyhow::Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = crate::http::get_json(&url, &[("user-agent", "bro")], Duration::from_secs(15))?;
    if !resp.ok() {
        bail!("GitHub answered HTTP {}", resp.status);
    }
    let tag = resp.body.get("tag_name").and_then(|t| t.as_str()).context("the release has no tag")?;
    Ok(tag.trim_start_matches('v').to_string())
}

/// `latest` is a later release than this build.
pub fn is_newer(latest: &str) -> bool {
    crate::justtools::is_newer(latest, CURRENT)
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// The SHA-256 `SHA256SUMS.txt` lists for `file` (`<hex>  <file>` or `<hex> *<file>`).
fn sum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let (hash, name) = (parts.next()?, parts.next()?);
        (name.trim_start_matches('*') == file).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher)?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The folder the running bro lives in.
pub fn install_dir() -> anyhow::Result<PathBuf> {
    let me = std::env::current_exe().context("finding the running bro")?;
    let me = std::fs::canonicalize(&me).unwrap_or(me);
    me.parent().map(Path::to_path_buf).context("the running bro has no folder")
}

/// Puts `new` where `dest` is, even while `dest` is running: the old file is renamed aside first.
fn swap_in(new: &Path, dest: &Path) -> anyhow::Result<()> {
    let name = dest.file_name().and_then(|n| n.to_str()).context("no file name")?;
    let staged = dest.with_file_name(format!(".{name}.new"));
    std::fs::copy(new, &staged).with_context(|| format!("writing {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    if dest.exists() && cfg!(windows) {
        // a previous update's leftover may still be running: pick a name that's free
        let mut old = dest.with_file_name(format!("{name}.old"));
        if old.exists() && std::fs::remove_file(&old).is_err() {
            old = dest.with_file_name(format!("{name}.{}.old", std::process::id()));
        }
        std::fs::rename(dest, &old).with_context(|| format!("moving the running {name} aside"))?;
    }
    std::fs::rename(&staged, dest).with_context(|| format!("installing {}", dest.display()))
}

/// Removes the binaries an earlier update moved aside (best effort; one still running stays).
pub fn clean_up_old() {
    let Ok(dir) = install_dir() else { return };
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("bro") && name.ends_with(".old") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Windows' own bsdtar, which reads .zip (a GNU tar from Git or MSYS earlier on PATH can't).
fn tar() -> PathBuf {
    match std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("tar.exe")) {
        Some(t) if cfg!(windows) && t.is_file() => t,
        _ => PathBuf::from("tar"),
    }
}

/// Installs release `version` over the running bro (and `bro-app` on Windows when it's there).
/// `on_step` hears how far it has got. Blocking; needs the network. bro must be restarted to
/// run the new version.
pub fn install(version: &str, on_step: &dyn Fn(Step)) -> anyhow::Result<PathBuf> {
    let target = target().context("there's no prebuilt bro for this platform: build it from source")?;
    let dir = install_dir()?;
    let tag = format!("v{}", version.trim_start_matches('v'));
    let name = format!("bro-{tag}-{target}");
    let file = if cfg!(windows) { format!("{name}.zip") } else { format!("{name}.tar.gz") };
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let tmp = std::env::temp_dir().join(format!("bro-update-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let result = (|| {
        on_step(Step::Download);
        let archive = tmp.join(&file);
        crate::http::download(&format!("{base}/{file}"), &archive, Duration::from_secs(300))?;
        on_step(Step::Verify);
        let sums_file = tmp.join("SHA256SUMS.txt");
        crate::http::download(&format!("{base}/SHA256SUMS.txt"), &sums_file, Duration::from_secs(30))?;
        let want = sum_for(&std::fs::read_to_string(&sums_file)?, &file).with_context(|| format!("SHA256SUMS.txt has no {file}"))?;
        if sha256_file(&archive)? != want {
            bail!("{file} doesn't match its SHA-256; nothing was changed");
        }
        on_step(Step::Install);
        // bsdtar (Windows 10+, macOS) and GNU tar unpack both .zip and .tar.gz
        let status = Command::new(tar()).arg("-xf").arg(&archive).arg("-C").arg(&tmp).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().context("running tar")?;
        if !status.status.success() {
            bail!("unpacking {file} failed: {}", String::from_utf8_lossy(&status.stderr).trim());
        }
        let unpacked = tmp.join(&name);
        let bro = unpacked.join(exe("bro"));
        if !bro.is_file() {
            bail!("{file} has no {}", exe("bro"));
        }
        swap_in(&bro, &dir.join(exe("bro")))?;
        let app = unpacked.join(exe("bro-app"));
        if app.is_file() {
            swap_in(&app, &dir.join(exe("bro-app")))?;
        }
        Ok(dir.clone())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_parse_and_targets_and_versions() {
        let sums = "ABC123  bro-v2.0.14-x86_64-pc-windows-msvc.zip\ndef456 *bro-v2.0.14-x86_64-unknown-linux-gnu.tar.gz\n";
        assert_eq!(sum_for(sums, "bro-v2.0.14-x86_64-pc-windows-msvc.zip").as_deref(), Some("abc123"));
        assert_eq!(sum_for(sums, "bro-v2.0.14-x86_64-unknown-linux-gnu.tar.gz").as_deref(), Some("def456"));
        assert_eq!(sum_for(sums, "bro-v2.0.14-aarch64-apple-darwin.tar.gz"), None);
        assert!(is_newer("999.0.0") && !is_newer(CURRENT) && !is_newer("0.1.0"));
        assert!(UpdateSettings::default().check);
        if cfg!(all(windows, target_arch = "x86_64")) {
            assert_eq!(target(), Some("x86_64-pc-windows-msvc"));
        }
    }

    #[test]
    fn swaps_a_file_in_while_the_old_one_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let (new, dest) = (dir.path().join("new"), dir.path().join(exe("bro")));
        std::fs::write(&new, "v2").unwrap();
        std::fs::write(&dest, "v1").unwrap();
        swap_in(&new, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "v2");
        // and again, with the first leftover still there
        std::fs::write(&new, "v3").unwrap();
        swap_in(&new, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "v3");
        assert!(!dir.path().join(format!(".{}.new", exe("bro"))).exists());
        let sha = sha256_file(&dest).unwrap();
        assert_eq!(sha.len(), 64);
    }

    #[test]
    #[ignore = "network; writes bro beside the test binary"]
    fn installs_a_published_release_beside_the_running_binary() {
        let dir = install(CURRENT, &|_| {}).unwrap();
        let bro = dir.join(exe("bro"));
        let out = Command::new(&bro).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("bro {CURRENT}"));
        // installing again moves the first copy aside instead of failing
        install(CURRENT, &|_| {}).unwrap();
        for f in [exe("bro"), exe("bro-app")] {
            let _ = std::fs::remove_file(dir.join(f));
        }
        clean_up_old();
    }

    #[test]
    #[ignore = "network"]
    fn github_names_the_latest_bro_release() {
        let v = latest_version().unwrap();
        assert!(v.starts_with(|c: char| c.is_ascii_digit()), "{v}");
    }
}
