//! The throwaway folders automation tools make in the temp folder for each run, and delete when the run ends
//! normally: Puppeteer's and Playwright's browser profiles (Remotion's Chrome runs on Puppeteer's), Playwright's
//! artifacts folders, Remotion's bundles and render folders. A run that's cut short leaves its folder behind, and
//! they add up: a profile is a few MB, a Remotion bundle tens to hundreds.
//!
//! A folder is cleared only when nothing can still be using it:
//!
//! - a browser profile: no browser on the machine was started with it, Chrome's `lockfile` isn't in it, and it's
//!   older than [`PROFILE_AGE`] (a tool makes the folder a moment before it starts the browser), unless it's the
//!   profile of a browser the collector just closed;
//! - a Remotion folder: untouched for [`SCRATCH_AGE`], and since before anything of Remotion's now running on the
//!   machine started (a process can't have made a folder before it existed, so none of them is using it);
//! - a Playwright artifacts folder: empty, and untouched for [`SCRATCH_AGE`].
//!
//! Then it's renamed aside (`<name>.bro-slop`) before it's deleted. Windows refuses that rename while anything has a
//! file in the folder open or works inside it, so a folder in use is never touched, and nothing can open the folder
//! under its old name while it's being deleted. A folder only partly deleted keeps its new name and goes next time.

use super::TempFiles;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// A browser profile younger than this may belong to a browser that's only starting.
pub(crate) const PROFILE_AGE: Duration = Duration::from_secs(10 * 60);
/// Remotion's folders and Playwright's artifacts: a day untouched.
pub(crate) const SCRATCH_AGE: Duration = Duration::from_secs(24 * 3600);
/// How often it lists the temp folder (sooner when asked, or when a browser it closed let go of its profile).
const LIST_EVERY_MS: u64 = 10 * 60_000;
/// How long one look may spend deleting, on its own and when asked.
pub(crate) const BUDGET: Duration = Duration::from_secs(3);
pub(crate) const BY_HAND: Duration = Duration::from_secs(10);
/// What a folder is renamed to before it's deleted.
const CLAIMED: &str = ".bro-slop";

/// What the machine says about who might be using a temp folder.
#[derive(Default, Debug)]
pub(crate) struct InUse {
    /// The profile folders running browsers were started with (folder names, lower case).
    pub profiles: HashSet<String>,
    /// When the oldest thing of Remotion's still running (a render, Studio) started (None: nothing is).
    pub remotion: Option<SystemTime>,
    /// This platform can't tell: clear nothing.
    pub unknown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rule {
    Profile,
    Remotion,
    Artifacts,
}

/// Whose a temp folder is, and how it's judged.
fn rule(name: &str) -> Option<(Rule, &'static str)> {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| name.starts_with(p));
    Some(match () {
        _ if starts(&["puppeteer_dev_chrome_profile-", "puppeteer_dev_firefox_profile-"]) => (Rule::Profile, "Puppeteer"),
        _ if starts(&["playwright_chromiumdev_profile-", "playwright_firefoxdev_profile-", "playwright_webkitdev_profile-"]) => (Rule::Profile, "Playwright"),
        _ if starts(&["playwright-artifacts-"]) => (Rule::Artifacts, "Playwright"),
        _ if starts(&["remotion-webpack-bundle-", "react-motion-render"]) || (name.starts_with("remotion-v") && name.contains("-assets")) => (Rule::Remotion, "Remotion"),
        _ => return None,
    })
}

/// The profile folder a browser was started with (`--user-data-dir`, Firefox's `-profile`): its name, lower case.
pub(crate) fn profile_of(cmd: &str) -> Option<String> {
    let lower = cmd.to_ascii_lowercase();
    let (at, flag) = match (lower.find("--user-data-dir="), lower.find(" -profile ")) {
        (Some(at), _) => (at, "--user-data-dir=".len()),
        (None, Some(at)) => (at, " -profile ".len()),
        _ => return None,
    };
    let value = cmd[at + flag..].trim_start();
    // "--user-data-dir=C:\a b\c" (the whole argument quoted), --user-data-dir="C:\a b\c", or bare
    let whole_quoted = at > 0 && cmd.as_bytes()[at - 1] == b'"';
    let path = match value.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None if whole_quoted => value.split('"').next()?,
        None => value.split_whitespace().next()?,
    };
    let name = path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// A folder to clear.
#[derive(Debug)]
struct Junk {
    path: PathBuf,
    bytes: u64,
    tool: &'static str,
    /// Already renamed aside (a delete that didn't finish).
    claimed: bool,
}

/// Lists and clears one temp folder's leftovers.
pub(crate) struct Sweeper {
    dir: PathBuf,
    next_list_ms: u64,
    /// What the last listing found and isn't cleared yet, smallest first.
    found: Vec<Junk>,
    /// Sizes by folder, measured once.
    sizes: HashMap<PathBuf, u64>,
    /// Whose folders it cleared since [`Sweeper::take_tools`], and how much.
    tools: HashMap<&'static str, u64>,
}

impl Sweeper {
    pub fn new(dir: PathBuf) -> Sweeper {
        Sweeper { dir, next_list_ms: 0, found: vec![], sizes: HashMap::new(), tools: HashMap::new() }
    }

    /// Time to list again.
    pub fn due(&self, now_ms: u64) -> bool {
        now_ms >= self.next_list_ms
    }

    /// List again at the next look.
    pub fn soon(&mut self) {
        self.next_list_ms = 0;
    }

    /// Off: start over next time.
    pub fn forget(&mut self) {
        self.found.clear();
        self.tools.clear();
        self.next_list_ms = 0;
    }

    /// Folders waiting to be cleared.
    pub fn pending(&self) -> usize {
        self.found.len()
    }

    /// Find what can go. `released`: profiles of browsers the collector just closed, which skip the age rule.
    pub fn list(&mut self, now: SystemTime, now_ms: u64, in_use: &InUse, released: &HashSet<String>) {
        self.next_list_ms = now_ms + LIST_EVERY_MS;
        self.found.clear();
        if in_use.unknown {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        let mut kept = HashSet::new();
        for e in entries.flatten() {
            let lower = e.file_name().to_string_lossy().to_ascii_lowercase();
            let claimed = lower.ends_with(CLAIMED);
            let name = lower.strip_suffix(CLAIMED).unwrap_or(&lower);
            let Some((rule, tool)) = rule(name) else { continue };
            // never a link out of the temp folder
            let Ok(kind) = e.file_type() else { continue };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let touched = [meta.created().ok(), meta.modified().ok()].into_iter().flatten().max().unwrap_or(now);
            let age = now.duration_since(touched).unwrap_or_default();
            let path = e.path();
            let gone = claimed
                || match rule {
                    Rule::Profile => (age >= PROFILE_AGE || released.contains(name)) && !in_use.profiles.contains(name) && !path.join("lockfile").exists(),
                    Rule::Remotion => age >= SCRATCH_AGE && in_use.remotion.is_none_or(|since| touched < since),
                    Rule::Artifacts => age >= SCRATCH_AGE && std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none()),
                };
            if !gone {
                continue;
            }
            kept.insert(path.clone());
            let bytes = *self.sizes.entry(path.clone()).or_insert_with(|| size(&path));
            self.found.push(Junk { path, bytes, tool, claimed });
        }
        self.sizes.retain(|p, _| kept.contains(p));
        self.found.sort_by_key(|j| j.bytes);
    }

    /// Clear what the last listing found, biggest first, for up to `budget`: (folders, bytes) cleared.
    pub fn clear(&mut self, budget: Duration) -> (u32, u64) {
        let start = Instant::now();
        let (mut folders, mut bytes) = (0, 0);
        while let Some(j) = self.found.pop() {
            self.sizes.remove(&j.path);
            let aside = if j.claimed {
                j.path.clone()
            } else {
                let mut name = j.path.file_name().unwrap_or_default().to_os_string();
                name.push(CLAIMED);
                j.path.with_file_name(name)
            };
            // in use: Windows won't rename it, and it stays
            if j.claimed || std::fs::rename(&j.path, &aside).is_ok() {
                let _ = std::fs::remove_dir_all(&aside);
                if !aside.exists() {
                    folders += 1;
                    bytes += j.bytes;
                    *self.tools.entry(j.tool).or_default() += j.bytes;
                }
            }
            if start.elapsed() >= budget {
                break;
            }
        }
        (folders, bytes)
    }

    /// What's waiting.
    pub fn summary(&self) -> TempFiles {
        let mut by: HashMap<&str, u64> = HashMap::new();
        for j in &self.found {
            *by.entry(j.tool).or_default() += j.bytes.max(1);
        }
        TempFiles { folders: self.found.len() as u32, bytes: self.found.iter().map(|j| j.bytes).sum(), tools: most_first(by), clearing: false }
    }

    /// Whose folders it cleared since the last call, most first.
    pub fn take_tools(&mut self) -> Vec<String> {
        most_first(std::mem::take(&mut self.tools))
    }
}

fn most_first(by: HashMap<&str, u64>) -> Vec<String> {
    let mut v: Vec<(&str, u64)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter().map(|(t, _)| t.to_string()).collect()
}

/// A folder's size on disk, links not followed.
fn size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(root: &Path, name: &str, files: &[(&str, usize)]) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (f, n) in files {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![7u8; *n]).unwrap();
        }
        d
    }

    #[test]
    fn profiles_command_lines_name() {
        assert_eq!(profile_of(r"chrome.exe --headless --user-data-dir=C:\Temp\puppeteer_dev_chrome_profile-AbC123 about:blank").as_deref(), Some("puppeteer_dev_chrome_profile-abc123"));
        assert_eq!(profile_of(r#"chrome.exe "--user-data-dir=C:\Users\J Doe\Temp\playwright_chromiumdev_profile-x1" --remote-debugging-pipe"#).as_deref(), Some("playwright_chromiumdev_profile-x1"));
        assert_eq!(profile_of(r#"chrome.exe --user-data-dir="C:\a b\remotion-profile\" x"#).as_deref(), Some("remotion-profile"));
        assert_eq!(profile_of(r"firefox.exe -no-remote -profile C:\Temp\playwright_firefoxdev_profile-q -juggler-pipe").as_deref(), Some("playwright_firefoxdev_profile-q"));
        assert_eq!(profile_of("chrome.exe https://example.com"), None);
    }

    #[test]
    fn clears_what_the_tools_left_and_nothing_in_use() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dead = folder(root, "puppeteer_dev_chrome_profile-Dead01", &[("Default/Cookies", 4096), ("DevToolsActivePort", 10)]);
        let running = folder(root, "puppeteer_dev_chrome_profile-Live02", &[("lockfile", 0), ("Default/Cookies", 4096)]);
        let named = folder(root, "playwright_chromiumdev_profile-Used03", &[("Default/History", 2048)]);
        let bundle = folder(root, "remotion-webpack-bundle-Bnd04", &[("bundle.js", 100_000), ("index.html", 500)]);
        let assets = folder(root, "remotion-v4.0.528-assetsxyz", &[("remotion-assets-dir/a.png", 1000)]);
        let empty = folder(root, "playwright-artifacts-Emp05", &[]);
        let artifacts = folder(root, "playwright-artifacts-Full06", &[("video.webm", 10)]);
        let yours = folder(root, "my-project-build", &[("a", 10)]);
        std::fs::write(root.join("puppeteer_dev_chrome_profile-file"), b"not a folder").unwrap();

        let in_use = InUse { profiles: ["playwright_chromiumdev_profile-used03".to_string()].into(), ..InUse::default() };
        let mut s = Sweeper::new(root.to_path_buf());
        let none = HashSet::new();

        // just made: a browser may still be starting on it
        s.list(SystemTime::now(), 0, &in_use, &none);
        assert_eq!(s.pending(), 0);

        // a browser the collector just closed: its profile goes now
        let released: HashSet<String> = ["puppeteer_dev_chrome_profile-dead01".to_string()].into();
        s.list(SystemTime::now(), 0, &in_use, &released);
        assert_eq!(s.summary().folders, 1);

        // a day on: everything nobody holds
        let later = SystemTime::now() + SCRATCH_AGE + Duration::from_secs(60);
        s.list(later, 0, &in_use, &none);
        let sum = s.summary();
        assert_eq!(sum.folders, 4, "{sum:?}");
        assert_eq!(sum.tools, ["Remotion", "Puppeteer", "Playwright"]);
        assert!(sum.bytes >= 105_000);
        assert!(!s.due(1) && s.due(LIST_EVERY_MS));

        // ...but not Remotion's while a render that may have made them runs
        let rendering = InUse { remotion: Some(SystemTime::now() - Duration::from_secs(3600)), ..InUse::default() };
        s.list(later, 0, &rendering, &none);
        assert_eq!(s.summary().tools, ["Puppeteer", "Playwright"]);
        // a Studio started after them can't be using them
        let studio = InUse { remotion: Some(SystemTime::now() + Duration::from_secs(3600)), ..InUse::default() };
        s.list(later, 0, &studio, &none);
        assert_eq!(s.summary().tools, ["Remotion", "Puppeteer", "Playwright"]);

        s.list(later, 0, &in_use, &none);
        assert_eq!(s.clear(BUDGET), (4, sum.bytes));
        for gone in [&dead, &bundle, &assets, &empty] {
            assert!(!gone.exists(), "{gone:?}");
        }
        for kept in [&running, &named, &artifacts, &yours] {
            assert!(kept.exists(), "{kept:?}");
        }
        assert_eq!(s.take_tools(), ["Remotion", "Puppeteer", "Playwright"]);
        assert!(root.join("puppeteer_dev_chrome_profile-file").exists());
    }

    /// The safety that matters: a folder something has open is never touched.
    #[cfg(windows)]
    #[test]
    fn a_folder_with_a_file_open_stays_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let busy = folder(tmp.path(), "puppeteer_dev_chrome_profile-Open01", &[("Default/Cookies", 100), ("Default/Prefs", 100)]);
        let held = std::fs::File::open(busy.join("Default/Cookies")).unwrap();
        let mut s = Sweeper::new(tmp.path().to_path_buf());
        s.list(SystemTime::now() + PROFILE_AGE * 2, 0, &InUse::default(), &HashSet::new());
        assert_eq!(s.pending(), 1);
        assert_eq!(s.clear(BUDGET), (0, 0));
        assert!(busy.join("Default/Prefs").exists(), "not even partly deleted");
        drop(held);
        // a delete that stopped partway: the claimed folder goes next time
        let claimed = folder(tmp.path(), "remotion-webpack-bundle-Half02.bro-slop", &[("x.js", 50)]);
        s.list(SystemTime::now(), 0, &InUse::default(), &HashSet::new());
        assert_eq!(s.pending(), 1, "the profile is young again for this clock, the claimed folder isn't");
        assert_eq!(s.clear(BUDGET), (1, 50));
        assert!(!claimed.exists());
    }
}
