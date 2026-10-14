//! bro's own updates: the newest release on GitHub (when bro opens and every
//! [`self_update::CHECK_EVERY`], unless switched off), and installing it over the running bro.
//! The same looks ask whether JustImagine's checkout is behind GitHub. Everything slow runs on a background thread and publishes into [`UpdateState`].

use super::{Services, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::self_update::{self, Step};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Job {
    pub step: Step,
    pub started: Instant,
}

/// What bro knows about its own releases.
#[derive(Clone, Debug, Default)]
pub struct UpdateState {
    /// a look at GitHub is under way
    pub checking: bool,
    /// when the last look finished
    pub checked_at: Option<Instant>,
    /// newest release on GitHub, once a look got through
    pub latest: Option<String>,
    /// the last look couldn't reach GitHub
    pub offline: bool,
    /// the release a toast already announced (each new one is announced once)
    pub announced: Option<String>,
    pub job: Option<Job>,
    /// the version installed over this bro; it runs after a restart
    pub installed: Option<String>,
    /// why the last update didn't work
    pub error: Option<String>,
}

impl UpdateState {
    /// The release to update to, when GitHub has a newer one than this build (and it isn't installed yet).
    pub fn available(&self) -> Option<&str> {
        self.latest.as_deref().filter(|l| self_update::is_newer(l) && self.installed.as_deref() != Some(l))
    }
}

impl Services {
    /// Look on GitHub for a newer bro. `quiet`: a background look, which only speaks up (once per
    /// release) when there is one; otherwise the answer is always toasted.
    pub fn check_bro_update(&self, quiet: bool) {
        if self.is_demo() {
            return self.update(|st| st.updates.checked_at = Some(Instant::now()));
        }
        {
            let mut st = self.inner.state.write();
            if st.updates.checking {
                return;
            }
            st.updates.checking = true;
        }
        self.wake();
        let svc = self.clone();
        std::thread::spawn(move || {
            let latest = guard_res("self_update::latest_version", self_update::latest_version);
            let mut toast = None;
            svc.update(|st| {
                let u = &mut st.updates;
                u.checking = false;
                u.checked_at = Some(Instant::now());
                match &latest {
                    Ok(v) => (u.latest, u.offline) = (Some(v.clone()), false),
                    Err(_) => u.offline = true,
                }
                toast = match (&latest, u.available().map(str::to_string)) {
                    (_, Some(new)) if !quiet || u.announced.as_deref() != Some(&new) => {
                        u.announced = Some(new.clone());
                        Some((Kind::Info, format!("bro {new} is out (you have {}) · settings → updates installs it", self_update::CURRENT)))
                    }
                    // a background look already announced this one
                    (_, Some(_)) => None,
                    _ if quiet => None,
                    (Ok(_), None) => Some((Kind::Info, format!("bro {} is the newest release", self_update::CURRENT))),
                    (Err(e), None) => Some((Kind::Error, format!("couldn't look for a bro update: {e}"))),
                };
            });
            if let Some((kind, text)) = toast {
                svc.send(Event::Toast(kind, text));
            }
        });
    }

    /// Looks when bro opens, then every [`self_update::CHECK_EVERY`] while "Check for updates" is on.
    pub(super) fn spawn_update_checks(&self) {
        if self.is_demo() {
            return;
        }
        let _ = std::thread::Builder::new().name("bro-update-check".into()).spawn({
            let svc = self.clone();
            move || {
                self_update::clean_up_old();
                loop {
                    if svc.settings().updates.check {
                        svc.check_bro_update(true);
                        svc.check_justimagine(true);
                    }
                    std::thread::sleep(self_update::CHECK_EVERY);
                }
            }
        });
    }

    /// Install the newest release over this bro; it runs from the next start.
    pub fn install_bro_update(&self) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't install anything".into()));
        }
        let version = {
            let mut st = self.inner.state.write();
            let Some(v) = st.updates.available().map(str::to_string) else { return };
            if st.updates.job.is_some() {
                return;
            }
            st.updates.job = Some(Job { step: Step::Download, started: Instant::now() });
            st.updates.error = None;
            v
        };
        self.wake();
        let svc = self.clone();
        std::thread::spawn(move || {
            let step = |step: Step| {
                svc.update(|st| {
                    if let Some(job) = &mut st.updates.job {
                        job.step = step;
                    }
                })
            };
            let result = guard_res("self_update::install", || self_update::install(&version, &step));
            let (kind, text) = match &result {
                Ok(dir) => (Kind::Info, format!("bro {version} installed in {} · restart bro to use it", dir.display())),
                Err(e) => (Kind::Error, format!("bro update: {e}")),
            };
            svc.update(|st| {
                st.updates.job = None;
                match result {
                    Ok(_) => st.updates.installed = Some(version.clone()),
                    Err(e) => st.updates.error = Some(e),
                }
            });
            svc.send(Event::Toast(kind, text));
        });
    }
}
