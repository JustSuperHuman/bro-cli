//! JustImagine: what's installed and how far its checkout is behind `main` on GitHub. The look runs
//! on a background thread and publishes into [`JustImagineState`]; updates run in a tab
//! (`bro install --wait justimagine --force`) so npm's and the service's output stay visible.

use super::{Services, guard, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::justimagine::{self, Behind, Installed};

/// What bro knows about JustImagine.
#[derive(Clone, Debug, Default)]
pub struct JustImagineState {
    /// false until the first look
    pub checked: bool,
    /// a look at the machine and GitHub is under way
    pub checking: bool,
    pub installed: Option<Installed>,
    /// how far bro's checkout is behind `main`, once a look got through
    pub behind: Option<Behind>,
    /// the commit [`Self::behind`] was worked out for
    pub compared: Option<String>,
    /// why the last look at GitHub didn't work
    pub error: Option<String>,
    /// the newest commit a toast already announced (each is announced once)
    pub announced: Option<String>,
}

impl JustImagineState {
    /// Commits on `main` the checkout lacks, when there are any.
    pub fn update(&self) -> Option<u32> {
        self.behind.as_ref().map(|b| b.commits).filter(|n| *n > 0)
    }
}

/// "1 new commit" / "3 new commits".
pub fn commits(n: u32) -> String {
    format!("{n} new commit{}", if n == 1 { "" } else { "s" })
}

impl Services {
    /// Look at what's installed and ask GitHub how far bro's checkout is behind `main`. `quiet`: a
    /// background look, which only speaks up (once per new commit) when there's an update.
    pub fn check_justimagine(&self, quiet: bool) {
        self.look_justimagine(quiet, false);
    }

    /// Look at the machine again, asking GitHub only when the checked-out commit changed since the
    /// last answer (cheap enough to repeat while an update runs in a tab).
    pub fn refresh_justimagine(&self) {
        self.look_justimagine(true, true);
    }

    fn look_justimagine(&self, quiet: bool, reuse: bool) {
        if self.is_demo() {
            return self.update(|st| st.justimagine.checked = true);
        }
        {
            let mut st = self.inner.state.write();
            if st.justimagine.checking {
                return;
            }
            st.justimagine.checking = true;
        }
        self.wake();
        let svc = self.clone();
        std::thread::spawn(move || {
            let found = guard("justimagine::installed", justimagine::installed).ok().flatten();
            // only bro's checkout is compared: a copy elsewhere is updated by whoever runs it
            let commit = found.as_ref().and_then(|f| f.dir.as_ref().and(f.commit.clone()));
            let known = {
                let ji = &svc.state().justimagine;
                ji.behind.clone().filter(|_| reuse && ji.compared.is_some() && ji.compared == commit)
            };
            let behind = match known {
                Some(b) => Some(Ok(b)),
                None => commit.as_ref().map(|c| guard_res("justimagine::behind", || justimagine::behind(c))),
            };
            let mut toast = None;
            svc.update(|st| {
                let ji = &mut st.justimagine;
                (ji.checked, ji.checking, ji.installed) = (true, false, found);
                match behind {
                    Some(Ok(b)) => (ji.behind, ji.compared, ji.error) = (Some(b), commit, None),
                    Some(Err(e)) => {
                        // an answer for another commit no longer says anything
                        if ji.compared != commit {
                            (ji.behind, ji.compared) = (None, None);
                        }
                        ji.error = Some(e);
                    }
                    None => (ji.behind, ji.compared, ji.error) = (None, None, None),
                }
                let latest = ji.behind.as_ref().and_then(|b| b.latest.clone());
                toast = match (ji.update(), &ji.error) {
                    (Some(n), _) if !quiet || ji.announced != latest => {
                        ji.announced = latest;
                        Some((Kind::Info, format!("JustImagine has {} on GitHub · settings → JustImagine updates it", commits(n))))
                    }
                    _ if quiet => None,
                    (Some(_), _) => None,
                    (None, Some(e)) => Some((Kind::Error, format!("couldn't look for a JustImagine update: {e}"))),
                    (None, None) if ji.installed.is_none() => Some((Kind::Info, "JustImagine isn't installed · Quick Install has it".into())),
                    (None, None) => Some((Kind::Info, "JustImagine is up to date with GitHub".into())),
                };
            });
            if let Some((kind, text)) = toast {
                svc.send(Event::Toast(kind, text));
            }
        });
    }
}
