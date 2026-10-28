//! Agent skills: the watched repos (see [`bro_core::skill_repos`]), what each holds, pulling them on a background
//! thread and publishing into [`SkillsState`], adding and dropping them, the auto-sync loop, the `bro-open` switch
//! and JustImagine's skills; installing a repo's skills everywhere or into a project, disabling, enabling and
//! deleting skills ([`bro_core::disabled_skills`]); and every skill installed, as the Skills view lists them
//! ([`bro_core::skill_inventory`]), looked at again whenever one of those changes them.

use super::{Services, guard};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::disabled_skills::{self, Disabled};
use bro_core::skill_inventory::{self, Inventory, Skill};
use bro_core::skill_repos::{self, RepoSkills, Target};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// JustImagine's skills are all in `~/.claude/skills` (None until looked)
    pub imagine: Option<bool>,
    /// every skill installed, in the agent homes and the sidebar's projects (None until the Skills view looks)
    pub inventory: Option<Arc<Inventory>>,
    /// a look at them is under way
    pub scanning: bool,
    /// something changed them while it looked: look once more
    pub scan_again: bool,
    /// the skills being installed, disabled, enabled or deleted right now, by name, since when
    pub pending: Vec<(String, Instant)>,
}

impl SkillsState {
    /// A pull, an install or another change is under way (the view keeps its spinners moving).
    pub fn busy(&self) -> bool {
        self.repos.values().any(|r| r.syncing.is_some()) || !self.pending.is_empty()
    }

    /// Since when skill `name` is being changed, if it is.
    pub fn pending(&self, name: &str) -> Option<Instant> {
        self.pending.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, at)| *at)
    }
}

/// A skill bro brings for a feature of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bundled {
    /// `bro-open`: agents put a file in front of you
    Open,
    /// JustTools': when to use the just* commands
    JustTools,
    /// JustWatch's: your dev servers
    JustWatch,
    /// JustImagine's generate-images, -video and -voice
    JustImagine,
    /// the orchestrator's: only its own session gets it
    Orchestrator,
    /// the browser's: sessions that drive your browser get it
    Browser,
}

impl Bundled {
    /// The ones this machine has, in the order they're listed.
    pub fn here() -> Vec<Bundled> {
        use Bundled::*;
        [Open, JustTools, JustWatch, JustImagine, Orchestrator, Browser].into_iter().filter(|b| *b != JustWatch || bro_core::justwatch::supported()).collect()
    }

    pub fn title(self) -> &'static str {
        match self {
            Bundled::Open => "bro-open",
            Bundled::JustTools => "JustTools",
            Bundled::JustWatch => "JustWatch",
            Bundled::JustImagine => "JustImagine",
            Bundled::Orchestrator => "Orchestrator",
            Bundled::Browser => "Browser",
        }
    }

    /// The skill folders it writes.
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Bundled::Open => &[bro_core::skills::BRO_OPEN_SKILL_NAME],
            Bundled::JustTools => &[bro_core::justtools::SKILL_NAME],
            Bundled::JustWatch => &[bro_core::justwatch::SKILL_NAME],
            Bundled::JustImagine => &["generate-images", "generate-video", "generate-voice"],
            Bundled::Orchestrator => &[bro_core::orchestrator::SKILL_NAME],
            Bundled::Browser => &["bro-browser"],
        }
    }

    /// What it lets agents do.
    pub fn about(self) -> &'static str {
        match self {
            Bundled::Open => "lets agents put a file in front of you, at a line, in bro's preview on your desktop, browser and phone (bro open file:line)",
            Bundled::JustTools => "tells new Claude and Codex sessions when to use the just* commands",
            Bundled::JustWatch => "lets new Claude and Codex sessions check, restart and tail your dev servers through JustWatch",
            Bundled::JustImagine => "generate-images, -video and -voice: agents make pictures, clips and voice-overs through JustImagine",
            Bundled::Orchestrator => "how bro's orchestrator drives your other sessions: its own session gets it, no other",
            Bundled::Browser => "how a session drives your signed-in browser through JustBrowse: sessions with the browser on get it",
        }
    }
}

impl Services {
    /// Look at every installed skill again, in the agent homes and the sidebar's projects, on a thread (no network).
    pub fn scan_skills(&self) {
        if self.is_demo() {
            return self.update(|st| st.skills.inventory = Some(Arc::new(super::demo::skill_inventory())));
        }
        {
            let mut st = self.inner.state.write();
            if st.skills.scanning {
                st.skills.scan_again = true;
                return;
            }
            st.skills.scanning = true;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            loop {
                let roots: Vec<std::path::PathBuf> = svc.projects().into_iter().map(|p| p.root).collect();
                let found = guard("skill_inventory::scan", || skill_inventory::scan(&roots)).unwrap_or_default();
                let again = {
                    let mut st = svc.inner.state.write();
                    st.skills.inventory = Some(Arc::new(found));
                    let again = std::mem::take(&mut st.skills.scan_again);
                    st.skills.scanning = again;
                    again
                };
                svc.wake();
                if !again {
                    break;
                }
            }
        });
    }

    /// Something wrote or removed skills: look again, when anything has looked yet.
    pub(crate) fn rescan_skills(&self) {
        let looked = {
            let st = self.state();
            st.skills.inventory.is_some() || st.skills.scanning
        };
        if looked {
            self.scan_skills();
        }
    }

    /// Write JustImagine's skills into every agent home (again), on a thread; say how it went.
    pub fn install_imagine_skills(&self) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't install anything".into()));
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let lines = guard("justimagine::install_skills", bro_core::justimagine::install_skills).unwrap_or_default();
            let failed = lines.iter().filter(|l| l.contains("couldn't write")).count();
            let text = match failed {
                0 => format!("JustImagine's skills are in {}", plural(lines.len(), "agent home")),
                n => format!("couldn't write JustImagine's skills into {}", plural(n, "agent home")),
            };
            svc.send(Event::Toast(if failed == 0 { Kind::Info } else { Kind::Error }, text));
            svc.load_skill_repos();
            svc.rescan_skills();
        });
    }

    /// Bundled `b` is switched on. None: it has no switch (JustImagine's are written when you ask, the orchestrator's
    /// and the browser's whenever they're needed).
    pub fn bundled_on(&self, b: Bundled) -> Option<bool> {
        let st = self.state();
        match b {
            Bundled::Open => Some(st.settings.skills.bro_open),
            Bundled::JustTools => Some(st.settings.justtools.skill),
            Bundled::JustWatch => Some(st.settings.justwatch.skill),
            Bundled::JustImagine | Bundled::Orchestrator | Bundled::Browser => None,
        }
    }

    /// Switch bundled `b` on or off (JustTools' needs JustTools; off takes bro's copies back out of every home).
    /// JustImagine's are written again when it's switched on.
    pub fn set_bundled(&self, b: Bundled, on: bool) {
        let toast = |text: &str| self.send(Event::Toast(Kind::Info, text.into()));
        match b {
            Bundled::Open => {
                self.set_bro_open_skill(on);
                if !on {
                    toast("bro-open off · bro took its copies back out");
                }
            }
            Bundled::JustTools if !on => {
                self.set_justtools_skill_on(false);
                toast("JustTools skill off · bro took its copies back out");
            }
            Bundled::JustTools if self.state().justtools.installed.is_none() && !self.is_demo() => toast("install JustTools first: its skill needs the commands"),
            Bundled::JustTools => self.set_justtools_skill_on(true),
            Bundled::JustWatch => {
                self.set_justwatch_skill_on(on);
                if !on {
                    toast("JustWatch skill off · bro took its copies back out");
                }
            }
            Bundled::JustImagine if on => self.install_imagine_skills(),
            Bundled::JustImagine | Bundled::Orchestrator | Bundled::Browser => {}
        }
    }

    /// Read what the clones on disk hold (no network), once per view.
    pub fn load_skill_repos(&self) {
        if self.is_demo() {
            return self.update(|st| (st.skills.loaded, st.skills.imagine) = (true, Some(false)));
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let repos = svc.settings().skills.repos;
            let found: Vec<(String, Option<RepoSkills>)> = repos.into_iter().map(|r| {
                let cached = guard("skill_repos::cached", || skill_repos::cached(&r)).ok().flatten();
                (r, cached)
            }).collect();
            let own = guard("skill_repos::own_skills", skill_repos::own_skills).unwrap_or_default();
            let skills = bro_core::paths::claude_local_dir().join("skills");
            let imagine = bro_core::justimagine::SKILLS.iter().all(|(name, _)| skills.join(name).join("SKILL.md").is_file());
            svc.update(|st| {
                st.skills.imagine = Some(imagine);
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

    /// The sidebar's projects: a sync brings up to date what's installed in their own folders too.
    fn project_roots(&self) -> Vec<PathBuf> {
        self.projects().into_iter().map(|p| p.root).collect()
    }

    /// A project's name as the sidebar shows it, else its folder's.
    pub fn project_label(&self, root: &Path) -> String {
        self.projects().into_iter().find(|p| bro_core::util::same_path(&p.root, root)).map(|p| p.name).unwrap_or_else(|| root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned()))
    }

    /// Pull `repo` and bring the skills installed from it up to date. `announce`: say how it went in a toast
    /// (`fresh`: it was just watched).
    pub fn sync_skill_repo(&self, repo: &str, announce: bool) {
        self.pull_skill_repo(repo, announce, false);
    }

    fn pull_skill_repo(&self, repo: &str, announce: bool, fresh: bool) {
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
        let (svc, repo, roots) = (self.clone(), repo.to_string(), self.project_roots());
        std::thread::spawn(move || {
            let result = guard("skill_repos::sync", || skill_repos::sync(&repo, &roots)).unwrap_or_else(Err);
            let own = guard("skill_repos::own_skills", skill_repos::own_skills).unwrap_or_default();
            // dropped while it was pulling: its clone goes
            let dropped = !svc.settings().skills.repos.contains(&repo);
            if dropped {
                let _ = guard("skill_repos::forget", || skill_repos::forget(&repo));
            }
            let name = skill_repos::label(&repo);
            let toast = match (&result, dropped) {
                (_, true) => None,
                (Ok(s), _) if announce && s.skills.is_empty() => Some((Kind::Info, format!("{name}: no skills found (a folder with a SKILL.md at its root, one folder down, or under skills/, .claude/skills/ or plugins/*/skills/)"))),
                (Ok(s), _) if announce && fresh => Some((Kind::Info, format!("watching {name} · {} to pick from on its page: install the ones you want", plural(s.skills.len(), "skill")))),
                (Ok(s), _) if announce => Some((Kind::Info, format!("{name} is up to date · {} · the ones you installed follow it", plural(s.skills.len(), "skill")))),
                (Ok(_), _) => None,
                (Err(e), _) => Some((Kind::Error, format!("couldn't pull {name}: {e}"))),
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
            svc.rescan_skills();
        });
    }

    /// Pull every watched repo (quietly: only failures speak up).
    pub fn sync_skill_repos(&self) {
        for repo in self.settings().skills.repos {
            self.sync_skill_repo(&repo, false);
        }
    }

    /// Watch the repo `input` names ("https://github.com/owner/repo", "owner/repo", a git URL) and pull it now: its
    /// skills are listed on its page, none installed. Err: it isn't a repo, or it's watched already.
    pub fn add_skill_repo(&self, input: &str) -> Result<String, String> {
        let repo = skill_repos::normalize(input).ok_or_else(|| format!("'{}' isn't a repo: paste owner/repo, a GitHub link or a git URL", input.trim()))?;
        let mut s = self.settings();
        if s.skills.repos.iter().any(|r| skill_repos::same(r, &repo)) {
            return Err(format!("already watching {repo}"));
        }
        s.skills.repos.push(repo.clone());
        self.save_settings(s);
        self.pull_skill_repo(&repo, true, true);
        Ok(repo)
    }

    /// Stop watching `repo`: its clone goes. The skills installed from it stay, no longer kept in step; `disable`:
    /// they're disabled too (moved out of reach, as removing one does).
    pub fn remove_skill_repo(&self, repo: &str, disable: bool) {
        let mut s = self.settings();
        s.skills.repos.retain(|r| !skill_repos::same(r, repo));
        self.save_settings(s);
        let (busy, installed) = {
            let mut st = self.inner.state.write();
            let busy = st.skills.repos.get(repo).is_some_and(|r| r.syncing.is_some());
            if !busy {
                st.skills.repos.remove(repo);
            }
            let from = |s: &&Skill| matches!(&s.origin, skill_inventory::Origin::Repo(r) if skill_repos::same(r, repo));
            let installed: Vec<Skill> = st.skills.inventory.as_ref().map(|i| i.skills.iter().filter(from).cloned().collect()).unwrap_or_default();
            (busy, installed)
        };
        self.wake();
        if self.is_demo() {
            return;
        }
        let label = skill_repos::label(repo);
        if disable && !installed.is_empty() {
            self.disable_skills(installed.clone(), Some(format!("stopped watching {label} · its {} under Disabled", if installed.len() == 1 { "skill is".to_string() } else { format!("{} skills are", installed.len()) })));
        } else {
            let kept = match installed.len() {
                0 => String::new(),
                1 => " · the skill you installed from it stays, no longer updated".into(),
                n => format!(" · the {n} skills you installed from it stay, no longer updated"),
            };
            self.send(Event::Toast(Kind::Info, format!("stopped watching {label}{kept}")));
        }
        // a pull under way cleans up after itself when it finishes
        if busy {
            return;
        }
        let (svc, repo) = (self.clone(), repo.to_string());
        std::thread::spawn(move || {
            let _ = guard("skill_repos::forget", || skill_repos::forget(&repo));
            svc.rescan_skills();
        });
    }

    /// Mark `names` as being changed (their rows spin) until `done` runs on a thread, then look at the skills again.
    fn changing(&self, names: Vec<String>, done: impl FnOnce(&Services) + Send + 'static) {
        let at = Instant::now();
        self.update(|st| st.skills.pending.extend(names.iter().map(|n| (n.clone(), at))));
        let svc = self.clone();
        std::thread::spawn(move || {
            done(&svc);
            let own = guard("skill_repos::own_skills", skill_repos::own_skills).unwrap_or_default();
            svc.update(|st| {
                st.skills.own = own;
                st.skills.pending.retain(|(n, when)| !(*when == at && names.contains(n)));
            });
            svc.scan_skills();
        });
    }

    /// Install `names` of `spec` (every skill it watches, when empty) into `target`, on a thread. A spec that isn't
    /// watched yet is watched first: a skill installed from Skill Issue is watched on its own, to stay in step.
    pub fn install_skills(&self, spec: &str, names: Vec<String>, target: Target) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't install anything".into()));
        }
        // one pull or install of a repo at a time: they share its clone
        {
            let mut st = self.inner.state.write();
            let r = st.skills.repos.entry(spec.to_string()).or_default();
            if r.syncing.is_some() {
                drop(st);
                return self.send(Event::Toast(Kind::Info, format!("{} is busy pulling or installing · try again in a moment", skill_repos::label(spec))));
            }
            r.syncing = Some(Instant::now());
        }
        let mut s = self.settings();
        if !s.skills.repos.iter().any(|r| skill_repos::same(r, spec)) {
            s.skills.repos.push(spec.to_string());
            self.save_settings(s);
        }
        let where_to = match &target {
            Target::Everywhere => "everywhere · new Claude and Codex sessions get".to_string(),
            Target::Project(root) => format!("into {} · agents working there get", self.project_label(root)),
        };
        // a pick's one skill spins under its own name
        let shown = if names.is_empty() { vec![skill_repos::pick(spec).unwrap_or(spec).rsplit('/').next().unwrap_or(spec).to_string()] } else { names.clone() };
        let spec = spec.to_string();
        self.changing(shown, move |svc| {
            let result = guard("skill_repos::install", || skill_repos::install(&spec, &names, &target)).unwrap_or_else(Err);
            let listed = guard("skill_repos::cached", || skill_repos::cached(&spec)).ok().flatten();
            let toast = match &result {
                Ok(w) if w.written.is_empty() => (Kind::Info, format!("left out {}: you have a skill of that name already", w.taken.join(", "))),
                Ok(w) => {
                    let them = if w.written.len() == 1 { "it" } else { "them" };
                    let left = if w.taken.is_empty() { String::new() } else { format!(" · left out {}: you have a skill of that name", w.taken.join(", ")) };
                    (Kind::Info, format!("installed {} {where_to} {them}{left}", and_list(&w.written)))
                }
                Err(e) => (Kind::Error, format!("couldn't install from {}: {e}", skill_repos::label(&spec))),
            };
            svc.update(|st| {
                let r = st.skills.repos.entry(spec.clone()).or_default();
                r.syncing = None;
                match &result {
                    Ok(_) => (r.error, r.synced) = (None, Some(Instant::now())),
                    Err(e) => r.error = Some(e.clone()),
                }
                if listed.is_some() {
                    r.skills = listed;
                }
            });
            svc.send(Event::Toast(toast.0, toast.1));
        });
    }

    /// Disable `skill` (every copy moves out of the agents' reach, kept under Disabled), on a thread.
    pub fn disable_skill(&self, skill: Skill) {
        let what = format!("disabled {} · it waits under Disabled until you enable it", skill.name);
        self.disable_skills(vec![skill], Some(what));
    }

    /// Disable each of `skills`, on one thread; `done`: what the toast says when they all went.
    fn disable_skills(&self, skills: Vec<Skill>, done: Option<String>) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't change your skills".into()));
        }
        let names = skills.iter().map(|s| s.name.clone()).collect();
        self.changing(names, move |svc| {
            let failed: Vec<String> = skills.iter().filter_map(|s| guard("disabled_skills::disable", || disabled_skills::disable(s)).unwrap_or_else(Err).err()).collect();
            match failed.first() {
                None => svc.send(Event::Toast(Kind::Info, done.unwrap_or_else(|| format!("disabled {}", plural(skills.len(), "skill"))))),
                Some(e) => svc.send(Event::Toast(Kind::Error, if failed.len() == 1 { format!("couldn't disable it: {e}") } else { format!("couldn't disable {} of them: {e}", failed.len()) })),
            }
        });
    }

    /// Enable `d` again: each copy goes back where it was, on a thread.
    pub fn enable_skill(&self, d: Disabled) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't change your skills".into()));
        }
        self.changing(vec![d.name.clone()], move |svc| match guard("disabled_skills::enable", || disabled_skills::enable(&d)).unwrap_or_else(Err) {
            Ok(()) => svc.send(Event::Toast(Kind::Info, format!("enabled {} · new sessions get it again", d.name))),
            Err(e) => svc.send(Event::Toast(Kind::Error, format!("couldn't enable {}: {e}", d.name))),
        });
    }

    /// Delete disabled `d` for good, on a thread.
    pub fn delete_disabled(&self, d: Disabled) {
        if self.is_demo() {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't change your skills".into()));
        }
        self.changing(vec![d.name.clone()], move |svc| match guard("disabled_skills::delete", || disabled_skills::delete(&d)).unwrap_or_else(Err) {
            Ok(()) => svc.send(Event::Toast(Kind::Info, format!("deleted {} for good", d.name))),
            Err(e) => svc.send(Event::Toast(Kind::Error, format!("couldn't delete {}: {e}", d.name))),
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
        let svc = self.clone();
        std::thread::spawn(move || {
            let _ = guard("skills::remove_bro_open_skill", || {
                for home in bro_core::managed_skill::homes() {
                    bro_core::skills::remove_bro_open_skill(&home);
                }
            });
            svc.rescan_skills();
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

/// "a", "a and b", "a, b and c".
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
