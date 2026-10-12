//! JustTools: what's installed, the newest release on GitHub, and the install / update / skill jobs. Everything
//! slow runs on a background thread and publishes into [`JustToolsState`].

use super::{Services, guard, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::justtools::{self, Installed, Step};
use std::time::Instant;

/// What a running job is for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JobKind {
    Install,
    Update,
    /// downloading the skill on its own
    Skill,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub kind: JobKind,
    pub step: Step,
    pub started: Instant,
}

/// What bro knows about JustTools.
#[derive(Clone, Debug, Default)]
pub struct JustToolsState {
    /// false until the first look
    pub checked: bool,
    pub installed: Option<Installed>,
    /// a look at the machine and GitHub is under way
    pub checking: bool,
    /// newest release on GitHub, once a check got through
    pub latest: Option<String>,
    /// the last check couldn't reach GitHub
    pub offline: bool,
    /// `~/.claude` has a `justtools` skill bro didn't write
    pub own_skill: bool,
    pub job: Option<Job>,
    /// what the last job did ("updated 2.8.0 → 2.9.0")
    pub done: Option<String>,
    /// why the last job didn't work
    pub error: Option<String>,
}

impl JustToolsState {
    /// The release to update to, when GitHub has a newer one than the installed copy.
    pub fn update(&self) -> Option<&str> {
        let current = self.installed.as_ref()?.version.as_deref()?;
        self.latest.as_deref().filter(|l| justtools::is_newer(l, current))
    }
}

impl Services {
    /// Take the job slot (one JustTools job at a time). False when one is running, or in the demo.
    fn begin_justtools(&self, kind: JobKind, step: Step) -> bool {
        if self.is_demo() {
            return false;
        }
        let mut st = self.inner.state.write();
        if st.justtools.job.is_some() {
            return false;
        }
        st.justtools.job = Some(Job { kind, step, started: Instant::now() });
        st.justtools.done = None;
        st.justtools.error = None;
        drop(st);
        self.wake();
        true
    }

    fn end_justtools(&self, result: Result<String, String>) {
        let found = guard("justtools::installed", justtools::installed);
        let (kind, text) = match &result {
            Ok(done) => (Kind::Info, format!("JustTools {done}")),
            Err(e) => (Kind::Error, format!("JustTools: {e}")),
        };
        self.update(|st| {
            let jt = &mut st.justtools;
            if let Ok(found) = found {
                jt.installed = found;
            }
            jt.job = None;
            match result {
                Ok(done) => jt.done = Some(done),
                Err(e) => jt.error = Some(e),
            }
        });
        self.send(Event::Toast(kind, text));
    }

    fn set_justtools_skill(&self, on: bool) {
        let mut s = self.settings();
        if s.justtools.skill != on {
            s.justtools.skill = on;
            self.save_settings(s);
        }
    }

    /// Look at what's installed and ask GitHub for the newest release; with auto-update on, install it when it
    /// is newer. Runs when bro opens and when the settings view does.
    pub fn check_justtools(&self) {
        if self.is_demo() {
            self.update(|st| st.justtools.checked = true);
            return;
        }
        self.update(|st| st.justtools.checking = true);
        let svc = self.clone();
        std::thread::spawn(move || {
            let found = guard("justtools::installed", justtools::installed).ok().flatten();
            let own_skill = guard("justtools::has_own_skill", || justtools::has_own_skill(&bro_core::paths::claude_local_dir())).unwrap_or(false);
            // nothing to compare a release with until it's installed
            let latest = found.as_ref().map(|_| guard_res("justtools::latest_version", justtools::latest_version));
            svc.update(|st| {
                let jt = &mut st.justtools;
                (jt.checked, jt.checking, jt.installed, jt.own_skill) = (true, false, found, own_skill);
                match latest {
                    Some(Ok(v)) => (jt.latest, jt.offline) = (Some(v), false),
                    Some(Err(_)) => jt.offline = true,
                    None => {}
                }
            });
            let wanted = svc.settings().justtools.auto_update && svc.state().justtools.update().is_some();
            if wanted && svc.begin_justtools(JobKind::Update, Step::Installer) {
                svc.run_justtools_install(JobKind::Update);
            }
        });
    }

    /// Install JustTools, or update it to the newest release.
    pub fn install_justtools(&self) {
        let kind = if self.state().justtools.installed.is_some() { JobKind::Update } else { JobKind::Install };
        if !self.begin_justtools(kind, Step::Installer) {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || svc.run_justtools_install(kind));
    }

    /// The job itself (blocking). A first install turns the skill on; an update refreshes it when it's on.
    fn run_justtools_install(&self, kind: JobKind) {
        let before = self.state().justtools.installed.as_ref().and_then(|i| i.version.clone());
        let step = |step: Step| {
            self.update(|st| {
                if let Some(job) = &mut st.justtools.job {
                    job.step = step;
                }
            })
        };
        let found = match guard_res("justtools::install", || justtools::install(&step)) {
            Ok(found) => found,
            Err(e) => return self.end_justtools(Err(e)),
        };
        let version = found.version.clone().unwrap_or_else(|| "the newest release".into());
        let mut done = match (kind, before) {
            (JobKind::Update, Some(old)) if old != version => format!("updated {old} → {version}"),
            (JobKind::Update, _) => format!("{version} is the newest release"),
            _ => format!("{version} installed"),
        };
        if kind == JobKind::Install || self.settings().justtools.skill {
            step(Step::Skill);
            match guard_res("justtools::fetch_skill", || justtools::fetch_skill(found.version.as_deref())) {
                Ok(()) if kind == JobKind::Install => {
                    self.set_justtools_skill(true);
                    done.push_str(" · new sessions get its skill");
                }
                Ok(()) => {}
                Err(e) => self.send(Event::Toast(Kind::Error, format!("couldn't download the JustTools skill: {e}"))),
            }
        }
        if !found.on_path() {
            done.push_str(" · restart bro so sessions find it on PATH");
        }
        self.end_justtools(Ok(done));
    }

    /// Give new Claude and Codex sessions the JustTools skill, or take bro's copies back out.
    pub fn set_justtools_skill_on(&self, on: bool) {
        if self.is_demo() {
            return self.set_justtools_skill(on);
        }
        if !on {
            self.set_justtools_skill(false);
            let svc = self.clone();
            std::thread::spawn(move || {
                let _ = guard("justtools::remove_skill_everywhere", justtools::remove_skill_everywhere);
                svc.wake();
            });
            return;
        }
        if !self.begin_justtools(JobKind::Skill, Step::Skill) {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let version = svc.state().justtools.installed.as_ref().and_then(|i| i.version.clone());
            let result = guard_res("justtools::fetch_skill", || justtools::fetch_skill(version.as_deref())).map(|()| {
                svc.set_justtools_skill(true);
                "skill on · new Claude and Codex sessions get it".to_string()
            });
            svc.end_justtools(result);
        });
    }
}
