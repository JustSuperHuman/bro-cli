//! Every agent skill on this machine, as the Skills view lists them: what each agent home holds (`~/.claude`,
//! `~/.codex`, each pooled Claude account and Codex profile, `~/.agents`, Pi's and omp's) and what each project's
//! own `.claude/skills`, `.codex/skills` and `.agents/skills` hold, each told apart by where it came from: bro (for
//! a feature of its own), a repo bro watches ([`crate::skill_repos`]), Codex (`skills/.system`), or you.
//!
//! Copies of one skill in several homes (bro writes a watched repo's into each) are one skill with several places.
//! The skills that are disabled ([`crate::disabled_skills`]) come with them.
//! Read-only and blocking (every SKILL.md's frontmatter is read): call it off the UI thread.

use crate::{disabled_skills, paths, skill_repos};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Who reads a skills folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Agent {
    Claude,
    Codex,
    /// `~/.agents/skills` (a project's `.agents/skills`): the folder agents share
    Shared,
    Pi,
    Omp,
}

impl Agent {
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
            Agent::Shared => "shared",
            Agent::Pi => "Pi",
            Agent::Omp => "omp",
        }
    }

    /// The harness it is; None for the shared folder.
    pub fn harness(self) -> Option<crate::Harness> {
        match self {
            Agent::Claude => Some(crate::Harness::Claude),
            Agent::Codex => Some(crate::Harness::Codex),
            Agent::Pi => Some(crate::Harness::Pi),
            Agent::Omp => Some(crate::Harness::Omp),
            Agent::Shared => None,
        }
    }
}

/// Where a skill came from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// bro writes it for a feature of its own: bro-open, JustTools', JustWatch's, JustImagine's, the orchestrator's,
    /// the browser's
    Bro,
    /// a repo bro watches brought it: the spec as `[skills] repos` keeps it ("owner/repo", "owner/repo#name")
    Repo(String),
    /// Codex ships it (`skills/.system`) and keeps it up to date
    Codex,
    /// you put it there, by hand or with another tool: bro never changes it
    Yours,
}

/// One copy of a skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub agent: Agent,
    /// the folder holding its SKILL.md
    pub dir: PathBuf,
}

/// A skill: one per name, origin and project, with each copy of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// its SKILL.md's `description:`
    pub description: Option<String>,
    pub origin: Origin,
    /// the project whose own folder holds it; None for every project (an agent home's)
    pub project: Option<PathBuf>,
    /// every copy, Claude's first (never empty)
    pub places: Vec<Place>,
}

impl Skill {
    /// The agents that get it, each once, Claude first.
    pub fn agents(&self) -> Vec<Agent> {
        let mut a: Vec<Agent> = self.places.iter().map(|p| p.agent).collect();
        a.sort();
        a.dedup();
        a
    }

    /// The folder of its first copy (Claude's, when Claude has it).
    pub fn dir(&self) -> &Path {
        &self.places[0].dir
    }

    /// What it does, on one line (a description can run over several).
    pub fn summary(&self) -> Option<String> {
        self.description.as_deref().map(one_line).filter(|s| !s.is_empty())
    }

    /// It can be disabled ([`crate::disabled_skills`]): a repo's or yours. bro's own are switched off instead, and
    /// Codex puts its own back.
    pub fn can_disable(&self) -> bool {
        matches!(self.origin, Origin::Repo(_) | Origin::Yours)
    }
}

/// `text` on one line: every run of spaces and line breaks one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` as prose to wrap: the lines a SKILL.md breaks by hand run on, while paragraphs (a blank line between) and
/// list items ("- x", "1. x") stay apart.
pub fn reflow(text: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            gap = !out.is_empty();
            continue;
        }
        let item = line.starts_with(['-', '*', '•']) || line.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if !out.is_empty() {
            out.push_str(if gap { "\n\n" } else if item { "\n" } else { " " });
        }
        out.push_str(&one_line(line));
        gap = false;
    }
    out
}

/// Every skill found: the homes' first (bro's, the repos', Codex's, yours), then each project's; and the disabled ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub skills: Vec<Skill>,
    /// the skills that are disabled, by name
    pub disabled: Vec<disabled_skills::Disabled>,
}

impl Inventory {
    /// The skills every project gets (an agent home's).
    pub fn everywhere(&self) -> impl Iterator<Item = &Skill> {
        self.skills.iter().filter(|s| s.project.is_none())
    }

    /// The skills `root`'s own folders hold.
    pub fn of_project<'a>(&'a self, root: &'a Path) -> impl Iterator<Item = &'a Skill> {
        self.skills.iter().filter(move |s| s.project.as_deref().is_some_and(|p| crate::util::same_path(p, root)))
    }
}

/// The names bro writes skills under without its marker: bro-open, the orchestrator's, the browser's.
const BRO_NAMES: [&str; 3] = [crate::skills::BRO_OPEN_SKILL_NAME, crate::orchestrator::SKILL_NAME, "bro-browser"];

/// Every agent home that keeps skills (has a `skills` folder), with who reads it.
pub fn homes() -> Vec<(Agent, PathBuf)> {
    let subdirs = |dir: PathBuf| {
        let mut d: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        d.sort();
        d
    };
    let mut out = vec![(Agent::Claude, paths::claude_local_dir()), (Agent::Codex, paths::codex_local_dir())];
    out.extend(subdirs(paths::claude_accounts_dir()).into_iter().map(|d| (Agent::Claude, d)));
    out.extend(subdirs(paths::codex_profiles_dir()).into_iter().map(|d| (Agent::Codex, d)));
    out.extend([(Agent::Shared, crate::util::home_dir().join(".agents")), (Agent::Pi, paths::pi_agent_dir()), (Agent::Omp, paths::omp_agent_dir())]);
    out.retain(|(_, h)| h.join("skills").is_dir());
    out
}

/// A project's own skill folders, with who reads each.
pub fn project_folders(root: &Path) -> [(Agent, PathBuf); 3] {
    [(Agent::Claude, root.join(".claude").join("skills")), (Agent::Codex, root.join(".codex").join("skills")), (Agent::Shared, root.join(".agents").join("skills"))]
}

/// A skill folder found, before copies of one skill are put together.
struct Found {
    name: String,
    origin: Origin,
    project: Option<PathBuf>,
    place: Place,
}

/// The folders in `dir`, by name.
fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).filter(|(_, p)| p.is_dir()).collect();
    out.sort();
    out
}

/// What `skills` (a skills folder `agent` reads) holds: each folder with a SKILL.md, and Codex's own under `.system`.
fn read(skills: &Path, agent: Agent, project: Option<&Path>, out: &mut Vec<Found>) {
    let found = |name: String, origin: Origin, dir: PathBuf| Found { name, origin, project: project.map(Path::to_path_buf), place: Place { agent, dir } };
    for (name, dir) in entries(skills) {
        if name == ".system" {
            out.extend(entries(&dir).into_iter().filter(|(n, d)| !n.starts_with('.') && d.join("SKILL.md").is_file()).map(|(n, d)| found(n, Origin::Codex, d)));
            continue;
        }
        if name.starts_with('.') || !dir.join("SKILL.md").is_file() {
            continue;
        }
        let origin = match skill_repos::owner(&dir) {
            Some(repo) if !repo.is_empty() => Origin::Repo(repo),
            Some(_) => Origin::Bro,
            None if project.is_none() && BRO_NAMES.contains(&name.as_str()) => Origin::Bro,
            None => Origin::Yours,
        };
        out.push(found(name, origin, dir));
    }
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Every skill in the agent homes ([`homes`]) and in `projects`' own folders ([`project_folders`]), and every disabled
/// one. A project folder that is a home's (a project at `~`) is the home's.
pub fn scan(projects: &[PathBuf]) -> Inventory {
    let mut found = vec![];
    let mut seen: Vec<PathBuf> = vec![];
    for (agent, home) in homes() {
        let skills = home.join("skills");
        seen.push(canonical(&skills));
        read(&skills, agent, None, &mut found);
    }
    let mut roots: Vec<&PathBuf> = vec![];
    for root in projects {
        if roots.iter().any(|r| crate::util::same_path(r, root)) {
            continue;
        }
        roots.push(root);
        for (agent, dir) in project_folders(root) {
            if dir.is_dir() && !seen.iter().any(|s| crate::util::same_path(s, &canonical(&dir))) {
                read(&dir, agent, Some(root), &mut found);
            }
        }
    }
    Inventory { disabled: disabled_skills::list(), ..merge(found) }
}

/// One skill per name, origin and project, with each copy; each described by its SKILL.md.
fn merge(found: Vec<Found>) -> Inventory {
    let mut skills: Vec<Skill> = vec![];
    let mut at: HashMap<(String, Origin, Option<PathBuf>), usize> = HashMap::new();
    for f in found {
        let key = (f.name.to_lowercase(), f.origin.clone(), f.project.clone());
        match at.get(&key) {
            Some(&i) => skills[i].places.push(f.place),
            None => {
                at.insert(key, skills.len());
                skills.push(Skill { name: f.name, description: None, origin: f.origin, project: f.project, places: vec![f.place] });
            }
        }
    }
    for s in &mut skills {
        s.places.sort_by_key(|p| p.agent);
        s.description = s.places.iter().find_map(|p| skill_repos::description(&p.dir.join("SKILL.md")));
    }
    skills.sort_by(|a, b| (a.project.is_some(), &a.project, &a.origin, a.name.to_lowercase()).cmp(&(b.project.is_some(), &b.project, &b.origin, b.name.to_lowercase())));
    Inventory { skills, disabled: vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn skill(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn every_home_and_project_with_where_each_came_from() {
        let sb = sandbox();
        let home = sb.home().to_path_buf();
        let tstack = |dir: &Path| std::fs::write(dir.join(".bro-managed"), "written by bro from t3dotgg/tstack; drop the repo under Skills in bro's settings to remove it\n").unwrap();
        let claude = home.join(".claude").join("skills");
        skill(&claude.join("bro-open"), crate::skills::BRO_OPEN_SKILL);
        skill(&claude.join("council"), "---\nname: council\ndescription: Ask the council\n---\n");
        tstack(&claude.join("council"));
        skill(&claude.join("justtools"), "---\nname: justtools\n---\n");
        std::fs::write(claude.join("justtools").join(".bro-managed"), "written by bro; turn it off in bro's settings\n").unwrap();
        skill(&claude.join("mine"), "---\nname: mine\ndescription: >-\n  mine,\n  folded\n---\n");
        // a folder that isn't a skill
        std::fs::create_dir_all(claude.join("synced").join("bucket")).unwrap();
        let codex = home.join(".codex").join("skills");
        skill(&codex.join("council"), "---\nname: council\n---\n");
        tstack(&codex.join("council"));
        skill(&codex.join(".system").join("imagegen"), "---\nname: imagegen\ndescription: Make images\n---\n");
        let account = home.join(".claude-max-pool").join("accounts").join("work").join("skills");
        skill(&account.join("council"), "---\nname: council\n---\n");
        tstack(&account.join("council"));
        skill(&home.join(".agents").join("skills").join("hypit"), "---\nname: hypit\n---\n");
        // a project's own, for Claude and in the shared folder
        let project = home.join("code").join("app");
        skill(&project.join(".claude").join("skills").join("release"), "---\nname: release\ndescription: Cut a release\n---\n");
        skill(&project.join(".agents").join("skills").join("release"), "---\nname: release\n---\n");

        let inv = scan(&[project.clone(), project.clone()]);
        let by = |n: &str, p: Option<&Path>| inv.skills.iter().find(|s| s.name == n && s.project.as_deref() == p).unwrap_or_else(|| panic!("{n}: {inv:#?}"));
        let council = by("council", None);
        assert_eq!(council.origin, Origin::Repo("t3dotgg/tstack".into()));
        assert_eq!((council.agents(), council.places.len()), (vec![Agent::Claude, Agent::Codex], 3));
        assert_eq!(council.description.as_deref(), Some("Ask the council"));
        assert_eq!(by("bro-open", None).origin, Origin::Bro);
        assert_eq!(by("justtools", None).origin, Origin::Bro);
        assert_eq!(by("mine", None).origin, Origin::Yours);
        assert_eq!(by("mine", None).description.as_deref(), Some("mine, folded"));
        assert_eq!((by("imagegen", None).origin.clone(), by("imagegen", None).agents()), (Origin::Codex, vec![Agent::Codex]));
        assert_eq!(by("hypit", None).agents(), [Agent::Shared]);
        let release = by("release", Some(&project));
        assert_eq!((release.origin.clone(), release.agents()), (Origin::Yours, vec![Agent::Claude, Agent::Shared]));
        assert!(release.dir().starts_with(project.join(".claude")));
        assert!(!inv.skills.iter().any(|s| s.name == "synced"));
        // bro's, then the repos', Codex's, yours; the projects' last
        let order: Vec<&str> = inv.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(order, ["bro-open", "justtools", "council", "imagegen", "hypit", "mine", "release"]);
        assert_eq!((inv.everywhere().count(), inv.of_project(&project).count()), (6, 1));
        // a project at home: its .claude/skills is the home's, counted once
        assert_eq!(scan(std::slice::from_ref(&home)).of_project(&home).count(), 0);
    }

    #[test]
    fn a_description_over_several_lines_sums_up_on_one() {
        let sb = sandbox();
        // a literal block keeps its line breaks (the details show them); a list's line has none
        skill(&sb.home().join(".claude").join("skills").join("higgsfield"), "---\nname: higgsfield\ndescription: |\n  Generate images via Higgsfield AI. Defaults:\n  GPT Image 2 for images,\n\n  Seedance 2.0 for video.\n---\n");
        let inv = scan(&[]);
        let s = &inv.skills[0];
        assert!(s.description.as_deref().unwrap().contains('\n'));
        assert_eq!(s.summary().as_deref(), Some("Generate images via Higgsfield AI. Defaults: GPT Image 2 for images, Seedance 2.0 for video."));
        assert!(s.can_disable());
        assert_eq!(one_line("  a\r\n\tb  "), "a b");
        // the details reflow it: hand-wrapped lines run on, paragraphs and list items stay apart
        assert_eq!(reflow(s.description.as_deref().unwrap()), "Generate images via Higgsfield AI. Defaults: GPT Image 2 for images,\n\nSeedance 2.0 for video.");
        assert_eq!(reflow("Use when:\n- one\n- two\n1. three\nmore"), "Use when:\n- one\n- two\n1. three more");
    }
}
