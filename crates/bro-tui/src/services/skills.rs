//! Watched skill repos (see [`bro_core::skill_repos`]): what each holds, pulling them on a background thread
//! and publishing into [`SkillsState`], adding and dropping them, the auto-sync loop, and the `bro-open` switch.

use super::{Services, guard};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::skill_repos::{self, RepoSkills};
use std::collections::HashMap;
use std::time::Instant;

/// One watched repo as bro last saw it.
#[derive(Clone, Debug, Default)]
pub struct RepoState {
    /// what its clone holds (from the last sync, or the clone on disk)
    pub skills: Option<RepoSkills>,
    /// a pull is under way, since then
    pub syncing: Option<Instant>,
    /// why the last pull didn't work
    pub error: Option<String>,
    /// when the last pull got through (this run)
    pub synced: Option<Instant>,
}

/// What bro knows about the watched repos, by repo.
#[derive(Clone, Debug, Default)]
pub struct SkillsState {
    /// false until the clones on disk were looked at
    pub loaded: bool,
    pub repos: HashMap<String, RepoState>,
    /// skills in `~/.claude/skills` bro didn't write
    pub own: Vec<String>,
}

impl SkillsState {
    /// A pull is under way for some repo (the view keeps its spinner moving).
    pub fn busy(&self) -> bool {
        self.repos.values().any(|r| r.syncing.is_some())
    }
}

impl Services {
    /// Read what the clones on disk hold (no network), once per view.
    pub fn load_skill_repos(&self) {
        if self.is_demo() {
            return self.update(|st| st.skills.loaded = true);
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let repos = svc.settings().skills.repos;
            let found: Vec<(String, Option<RepoSkills>)> = repos.into_iter().map(|r| {
                let cached = guard("skill_repos::cached", || skill_repos::cached(&r)).ok().flatten();
                (r, cached)
            }).collect();
            let own = guard("skill_repos::own_skills", skill_repos::own_skills).unwrap_or_default();
            svc.update(|st| {
                for (repo, cached) in found {
                    let r = st.skills.repos.entry(repo).or_default();
                    if r.skills.is_none() {
                        r.skills = cached;
                    }
                }
                st.skills.own = own;
                st.skills.loaded = true;
            });
        });
    }

    /// Pull `repo` and refresh its skills in every home. `announce`: say how it went in a toast.
    pub fn sync_skill_repo(&self, repo: &str, announce: bool) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't pull anything".into()));
        }
        {
            let mut st = self.inner.state.write();
            let r = st.skills.repos.entry(repo.to_string()).or_default();
            if r.syncing.is_some() {
                return;
            }
            r.syncing = Some(Instant::now());
        }
        self.wake();
        let (svc, repo) = (self.clone(), repo.to_string());
        std::thread::spawn(move || {
            let result = guard("skill_repos::sync", || skill_repos::sync(&repo)).unwrap_or_else(Err);
            let own = guard("skill_repos::own_skills", skill_repos::own_skills).unwrap_or_default();
            // dropped while it was pulling: take back out what the pull just wrote
            let dropped = !svc.settings().skills.repos.contains(&repo);
            if dropped {
                let _ = guard("skill_repos::forget", || skill_repos::forget(&repo));
            }
            let toast = match (&result, dropped) {
                (_, true) => None,
                (Ok(s), _) if announce && s.skills.is_empty() => Some((Kind::Info, format!("{repo}: no skills found (a folder with a SKILL.md at its root or under skills/)"))),
                (Ok(s), _) if announce => Some((Kind::Info, format!("{repo}: {} · new sessions get {}", plural(s.skills.len(), "skill"), if s.skills.len() == 1 { "it" } else { "them" }))),
                (Ok(_), _) => None,
                (Err(e), _) => Some((Kind::Error, format!("couldn't pull {repo}: {e}"))),
            };
            svc.update(|st| {
                st.skills.own = own;
                if dropped {
                    st.skills.repos.remove(&repo);
                    return;
                }
                let r = st.skills.repos.entry(repo.clone()).or_default();
                r.syncing = None;
                match result {
                    Ok(s) => (r.skills, r.error, r.synced) = (Some(s), None, Some(Instant::now())),
                    Err(e) => r.error = Some(e),
                }
            });
            if let Some((kind, text)) = toast {
                svc.send(Event::Toast(kind, text));
            }
        });
    }

    /// Pull every watched repo (quietly: only failures speak up).
    pub fn sync_skill_repos(&self) {
        for repo in self.settings().skills.repos {
            self.sync_skill_repo(&repo, false);
        }
    }

    /// Watch the repo `input` names ("https://github.com/owner/repo", "owner/repo", a git URL) and pull it now.
    /// Err: it isn't a repo, or it's watched already.
    pub fn add_skill_repo(&self, input: &str) -> Result<String, String> {
        let repo = skill_repos::normalize(input).ok_or_else(|| format!("'{}' isn't a repo: paste owner/repo, a GitHub link or a git URL", input.trim()))?;
        let mut s = self.settings();
        if s.skills.repos.contains(&repo) {
            return Err(format!("already watching {repo}"));
        }
        s.skills.repos.push(repo.clone());
        self.save_settings(s);
        self.sync_skill_repo(&repo, true);
        Ok(repo)
    }

    /// Stop watching `repo`: its skills come back out of every home and its clone goes.
    pub fn remove_skill_repo(&self, repo: &str) {
        let mut s = self.settings();
        s.skills.repos.retain(|r| r != repo);
        self.save_settings(s);
        let busy = {
            let mut st = self.inner.state.write();
            let busy = st.skills.repos.get(repo).is_some_and(|r| r.syncing.is_some());
            if !busy {
                st.skills.repos.remove(repo);
            }
            busy
        };
        self.wake();
        // a pull under way cleans up after itself when it finishes
        if busy || self.is_demo() {
            return;
        }
        let (svc, repo) = (self.clone(), repo.to_string());
        std::thread::spawn(move || {
            let _ = guard("skill_repos::forget", || skill_repos::forget(&repo));
            let own = guard("skill_repos::own_skills", skill_repos::own_skills).unwrap_or_default();
            svc.update(|st| st.skills.own = own);
        });
    }

    /// Switch the `bro-open` skill: off takes bro's copies back out of every home.
    pub fn set_bro_open_skill(&self, on: bool) {
        let mut s = self.settings();
        s.skills.bro_open = on;
        self.save_settings(s);
        if on || self.is_demo() {
            return;
        }
        std::thread::spawn(|| {
            let _ = guard("skills::remove_bro_open_skill", || {
                for home in bro_core::managed_skill::homes() {
                    bro_core::skills::remove_bro_open_skill(&home);
                }
            });
        });
    }

    /// Pulls the watched repos when bro opens, then every [`skill_repos::SYNC_EVERY`] while auto-sync is on.
    pub(super) fn spawn_skill_syncs(&self) {
        if self.is_demo() {
            return;
        }
        let _ = std::thread::Builder::new().name("bro-skill-sync".into()).spawn({
            let svc = self.clone();
            move || loop {
                if svc.settings().skills.auto_sync {
                    svc.sync_skill_repos();
                }
                std::thread::sleep(skill_repos::SYNC_EVERY);
            }
        });
    }
}

/// "1 skill" / "9 skills".
pub fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
