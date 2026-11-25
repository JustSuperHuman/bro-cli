//! Firewall readiness for phone connections (see `bro_core::firewall`), and on a Mac the privacy permissions
//! remote desktop needs.
//!
//! bro reads the rules on its own (no rights needed) and asks for administrator rights (Windows' UAC prompt,
//! macOS's password dialog) only when you turn phone access or remote desktop on, or press Allow: moments
//! you're at the computer. Turning phone access on writes the rules before the bridge first listens, so the
//! system never shows its own "Allow access?" prompt. Linux firewalls never prompt; bro says what to run.

use std::time::{Duration, Instant};

use bro_core::firewall::{self, Outcome, Readiness, Report, Target};

use super::{Event, Services};
use crate::alerts::Kind;

/// How long a reading stays fresh while a view shows it.
const FRESH: Duration = Duration::from_secs(15);

/// Under bro's data folder: the programs bro already asked about when a window opened.
const ASKED_FILE: &str = "firewall-asked.json";

/// A program as asked about: its path and when it last changed (macOS's firewall treats an updated program as a
/// new one, so an updated voxl is asked about again).
fn key(target: &Target) -> String {
    let changed = std::fs::metadata(&target.program).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("{:?}:{}:{changed}", target.app, target.program.to_string_lossy().replace('/', "\\").to_lowercase())
}

/// Whether every one of these programs was asked about before.
fn asked_before(file: &std::path::Path, targets: &[Target]) -> bool {
    let asked: Vec<String> = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    targets.iter().all(|t| asked.contains(&key(t)))
}

fn remember_asked(file: &std::path::Path, targets: &[Target]) {
    let mut asked: Vec<String> = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for target in targets {
        let k = key(target);
        if !asked.contains(&k) {
            asked.push(k);
        }
    }
    let Ok(text) = serde_json::to_string_pretty(&asked) else { return };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let temp = file.with_extension(format!("json.{}-{nanos:x}.tmp", std::process::id()));
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&temp, text).is_ok() && std::fs::rename(&temp, file).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

#[derive(Clone, Debug, Default)]
pub struct FirewallState {
    /// The last reading (none yet).
    pub report: Option<Report>,
    pub checked_at: Option<Instant>,
    pub checking: bool,
    /// The UAC prompt is up, or the rules are being written.
    pub asking: bool,
    /// Why the last Allow didn't finish, for the settings row.
    pub problem: Option<String>,
    /// macOS: Screen Recording and Accessibility as a fresh process sees them (None elsewhere, or not read yet).
    pub mac: Option<bro_core::rmm::MacPermissions>,
    pub mac_checked_at: Option<Instant>,
    pub mac_asking: bool,
}

/// What brought on the reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A view showing the state wants it current (skipped while the last reading is fresh).
    Refresh,
    /// After something changed: read now.
    Now,
}

impl Services {
    /// What has to be reachable now. `bridge_on` / `remote_on` override the saved switches for a change in flight.
    pub fn firewall_targets_for(&self, bridge_on: bool, remote_on: bool) -> Vec<Target> {
        let s = self.settings();
        let bridge = (bridge_on && firewall::reachable_bind(&s.bridge.bind)).then(|| std::env::current_exe().ok()).flatten();
        let remote = (remote_on && bro_core::rmm::supported()).then(|| firewall::remote_desktop_program(&crate::util::bro_dir()));
        firewall::targets(bridge.as_deref(), remote.as_deref())
    }

    pub fn firewall_targets(&self) -> Vec<Target> {
        let s = self.settings();
        self.firewall_targets_for(s.bridge.enabled, s.mobile_companion.enabled)
    }

    /// Whether bro should show the firewall at all: something a phone connects to is on.
    pub fn firewall_applies(&self) -> bool {
        !self.is_demo() && !self.firewall_targets().is_empty()
    }

    /// Read the rules again off the UI thread (`Refresh` skips it while the last reading is fresh).
    pub fn check_firewall(&self, why: Why) {
        if self.is_demo() {
            return;
        }
        {
            let st = self.state();
            let fw = &st.firewall;
            if fw.checking || fw.asking {
                return;
            }
            if why == Why::Refresh && fw.checked_at.is_some_and(|at| at.elapsed() < FRESH) {
                return;
            }
        }
        let targets = self.firewall_targets();
        self.update(|st| st.firewall.checking = true);
        let svc = self.clone();
        std::thread::spawn(move || {
            let report = firewall::check(&targets);
            svc.update(|st| {
                st.firewall.checking = false;
                st.firewall.checked_at = Some(Instant::now());
                st.firewall.report = Some(report);
            });
        });
    }

    /// A window opened (the desktop app or the terminal UI, never the background bro). With phone access already
    /// on and a rule missing, bro asks once for these programs, so the rules exist before a phone connects.
    /// A declined prompt isn't repeated on later starts; the Allow button in Settings stays.
    pub fn firewall_on_open(&self) {
        let remote_session = std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some();
        if self.is_demo() || remote_session {
            return;
        }
        let targets = self.firewall_targets();
        if targets.is_empty() {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let report = firewall::check(&targets);
            let needs = report.readiness == Readiness::NeedsRules;
            svc.update(|st| {
                st.firewall.checked_at = Some(Instant::now());
                st.firewall.report = Some(report);
            });
            if !needs {
                return;
            }
            let memory = crate::util::bro_dir().join(ASKED_FILE);
            if asked_before(&memory, &targets) {
                svc.send(Event::Toast(
                    Kind::NeedsYou,
                    format!("{} would stop your phone's first connection with a prompt on this computer. Allow bro in Settings → Remote Control.", firewall::name()),
                ));
                return;
            }
            remember_asked(&memory, &targets);
            svc.allow_firewall_then(targets, || {});
        });
    }

    /// macOS: read Screen Recording and Accessibility again (a fresh process answers), off the UI thread.
    pub fn check_mac_permissions(&self, why: Why) {
        if !cfg!(target_os = "macos") || self.is_demo() {
            return;
        }
        {
            let st = self.state();
            let fw = &st.firewall;
            if fw.mac_asking || (why == Why::Refresh && fw.mac_checked_at.is_some_and(|at| at.elapsed() < FRESH)) {
                return;
            }
        }
        self.update(|st| st.firewall.mac_checked_at = Some(Instant::now()));
        let svc = self.clone();
        std::thread::spawn(move || {
            let read = bro_core::rmm::mac_permissions();
            svc.update(|st| st.firewall.mac = read);
        });
    }

    /// macOS: ask for Screen Recording and Accessibility while you're at the Mac (its own prompts, then the System
    /// Settings page for whatever's still off), so a phone never meets a prompt nobody can answer.
    pub fn request_mac_permissions(&self) {
        if !cfg!(target_os = "macos") || self.is_demo() || self.state().firewall.mac_asking {
            return;
        }
        self.update(|st| st.firewall.mac_asking = true);
        let svc = self.clone();
        std::thread::spawn(move || {
            let got = bro_core::rmm::request_mac_permissions();
            svc.update(|st| {
                st.firewall.mac_asking = false;
                st.firewall.mac = got;
                st.firewall.mac_checked_at = Some(Instant::now());
            });
            if got.is_some_and(|p| !p.screen || !p.accessibility) {
                svc.send(Event::Toast(
                    Kind::NeedsYou,
                    "Turn Bro on in System Settings → Privacy & Security for Screen Recording and Accessibility, then come back. You may need to quit and reopen Bro.".into(),
                ));
            }
        });
    }

    /// Make sure phones get through, asking for administrator rights once if rules are missing. Off the UI thread.
    pub fn allow_firewall(&self) {
        let targets = self.firewall_targets();
        self.allow_firewall_then(targets, || {});
    }

    /// Turn phone access on with the rules written first: the bridge's first listen then raises no
    /// system prompt. The switch shows on at once; the bridge starts once that's answered.
    pub fn turn_on_mobile(&self) {
        if self.is_demo() {
            return self.set_bridge_enabled(true);
        }
        let targets = self.firewall_targets_for(true, self.settings().mobile_companion.enabled);
        let mut s = self.settings();
        s.bridge.enabled = true;
        self.save_settings(s);
        self.update(|st| st.bridge.enabled = true);
        let svc = self.clone();
        self.allow_firewall_then(targets, move || svc.set_bridge_enabled(true));
    }

    fn allow_firewall_then(&self, targets: Vec<Target>, then: impl FnOnce() + Send + 'static) {
        if self.is_demo() || targets.is_empty() || self.state().firewall.asking {
            return then();
        }
        self.update(|st| {
            st.firewall.asking = true;
            st.firewall.problem = None;
        });
        let svc = self.clone();
        std::thread::spawn(move || {
            let before = firewall::check(&targets);
            let outcome = if before.readiness == Readiness::NeedsRules {
                match std::env::current_exe() {
                    Ok(exe) => firewall::request(&exe, &targets),
                    Err(error) => Outcome::Failed(format!("bro couldn't find its own program: {error}")),
                }
            } else {
                Outcome::Done(before.clone())
            };
            let asked = before.readiness == Readiness::NeedsRules;
            let (report, problem, toast) = match outcome {
                Outcome::Done(report) => {
                    let toast = asked.then(|| match &report.readiness {
                        Readiness::Ready => (Kind::Info, format!("{} lets your phone in now, on any network, with no prompt on this computer.", firewall::name())),
                        Readiness::BlocksAll(profiles) => (Kind::NeedsYou, firewall::blocks_all_fix(*profiles)),
                        _ => (Kind::NeedsYou, format!("{} still doesn't let bro in. See Settings → Remote Control.", firewall::name())),
                    });
                    (report, None, toast)
                }
                Outcome::Declined => (
                    before,
                    Some("Permission wasn't given, so nothing changed.".to_string()),
                    Some((Kind::NeedsYou, "Without the firewall rules, your phone's first connection waits on an Allow prompt on this PC. Choose Allow in Settings → Remote Control when you're ready.".to_string())),
                ),
                Outcome::Failed(error) => (
                    firewall::check(&targets),
                    Some(error.clone()),
                    Some((Kind::Error, format!("couldn't add the firewall rules: {error}"))),
                ),
            };
            svc.update(|st| {
                st.firewall.asking = false;
                st.firewall.checked_at = Some(Instant::now());
                st.firewall.report = Some(report);
                st.firewall.problem = problem;
            });
            if let Some((kind, text)) = toast {
                svc.send(Event::Toast(kind, text));
            }
            then();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::firewall::App;

    #[test]
    fn a_program_asked_about_once_isnt_asked_about_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(ASKED_FILE);
        let bridge = Target::new(App::Bridge, r"C:\Users\me\AppData\Local\Programs\bro\bro.exe");
        let remote = Target::new(App::RemoteDesktop, r"C:\Users\me\.bro\rmm\packages\protocol-v1\voxl.exe");
        assert!(!asked_before(&file, std::slice::from_ref(&bridge)));
        remember_asked(&file, std::slice::from_ref(&bridge));
        assert!(asked_before(&file, std::slice::from_ref(&bridge)));
        // the same program spelled another way
        let shouted = Target::new(App::Bridge, r"c:/USERS/me/appdata/local/programs/bro/BRO.EXE");
        assert!(asked_before(&file, &[shouted]));
        // a program not asked about yet still gets its prompt
        assert!(!asked_before(&file, &[bridge.clone(), remote.clone()]));
        remember_asked(&file, &[bridge.clone(), remote.clone()]);
        assert!(asked_before(&file, &[bridge, remote]));
        let saved: Vec<String> = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp files left behind");
    }
}
