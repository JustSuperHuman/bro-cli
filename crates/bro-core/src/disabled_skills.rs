//! Disabled skills: removing a skill in the Skills view (or `bro skills disable`) moves every copy of it out of the
//! agents' reach into `~/.bro/disabled-skills/<name>`, noting where each copy was, and enabling it puts each copy
//! back. Nothing is lost until it's deleted from there.
//!
//! An entry is a skill folder like any other, its SKILL.md at the top (it reads and opens as it is), plus
//! `.bro-disabled.json`: the skill's name, where it came from, its project, and each place it was in with the copy
//! that goes back there. A copy that differs from the first is kept under `.copies/<n>`; a place that was a link
//! (`npx skills` links one copy into each agent's folder) is noted as the link and made again.
//!
//! bro's own skills are switched off instead (bro writes them again while they're on) and Codex's come back with
//! Codex, so neither is disabled here. Blocking (it copies files): call it off the UI thread.

use crate::skill_inventory::{Agent, Origin, Place, Skill};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

const MANIFEST: &str = ".bro-disabled.json";
const COPIES: &str = ".copies";

/// `~/.bro/disabled-skills`.
pub fn root() -> PathBuf {
    crate::paths::bro_dir().join("disabled-skills")
}

/// A disabled skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disabled {
    /// its folder under [`root`]
    pub id: String,
    pub name: String,
    /// its SKILL.md's `description:`
    pub description: Option<String>,
    pub origin: Origin,
    /// the project whose own folder it was in; None for every project (an agent home's)
    pub project: Option<PathBuf>,
    /// where it was, Claude's first
    pub places: Vec<Place>,
    /// when it was disabled (seconds since 1970)
    pub at: u64,
}

impl Disabled {
    /// Its folder: the skill as it was, its SKILL.md at the top.
    pub fn dir(&self) -> PathBuf {
        root().join(&self.id)
    }

    /// Its SKILL.md: the kept copy's, or (a skill that was only links) the one they pointed at.
    pub fn skill_md(&self) -> PathBuf {
        let kept = self.dir().join("SKILL.md");
        if kept.is_file() {
            return kept;
        }
        manifest(&self.dir()).ok().and_then(|m| m.places.into_iter().find_map(|s| s.link.map(|l| target_of(&s.dir, &l).join("SKILL.md")))).unwrap_or(kept)
    }

    /// The agents that had it, each once, Claude first.
    pub fn agents(&self) -> Vec<Agent> {
        let mut a: Vec<Agent> = self.places.iter().map(|p| p.agent).collect();
        a.sort();
        a.dedup();
        a
    }
}

/// What an entry keeps besides the skill's own files.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    name: String,
    /// "yours" or "repo:<spec>"
    origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    places: Vec<Spot>,
    #[serde(default)]
    at: u64,
}

/// A place the skill was in.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Spot {
    agent: Agent,
    dir: PathBuf,
    /// the copy that goes back: 0 the entry's own files, n `.copies/<n>`
    #[serde(default)]
    copy: usize,
    /// it was a link to this (as it read), not a folder of its own
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<PathBuf>,
}

fn origin_key(o: &Origin) -> String {
    match o {
        Origin::Repo(spec) => format!("repo:{spec}"),
        Origin::Bro => "bro".into(),
        Origin::Codex => "codex".into(),
        Origin::Yours => "yours".into(),
    }
}

fn origin_of(key: &str) -> Origin {
    match key {
        "bro" => Origin::Bro,
        "codex" => Origin::Codex,
        _ => key.strip_prefix("repo:").map_or(Origin::Yours, |spec| Origin::Repo(spec.to_string())),
    }
}

fn manifest(entry: &Path) -> io::Result<Manifest> {
    let body = std::fs::read_to_string(entry.join(MANIFEST))?;
    serde_json::from_str(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn from_manifest(id: String, m: Manifest) -> Disabled {
    let mut places: Vec<Place> = m.places.iter().map(|s| Place { agent: s.agent, dir: s.dir.clone() }).collect();
    places.sort_by_key(|p| p.agent);
    Disabled { id, name: m.name, description: m.description, origin: origin_of(&m.origin), project: m.project, places, at: m.at }
}

/// Every disabled skill, by name.
pub fn list() -> Vec<Disabled> {
    let mut out: Vec<Disabled> = std::fs::read_dir(root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| Some(from_manifest(e.file_name().to_string_lossy().into_owned(), manifest(&e.path()).ok()?)))
        .collect();
    out.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
    out
}

/// Disables `skill`: every copy of it moves into a new entry under [`root`]. Err: it's bro's own or Codex's (those
/// aren't disabled here), or a copy wouldn't move (then none does: the ones that went are put back).
pub fn disable(skill: &Skill) -> Result<Disabled, String> {
    match &skill.origin {
        Origin::Bro => return Err(format!("{} is bro's own: switch it off on the bro page", skill.name)),
        Origin::Codex => return Err(format!("{} comes with Codex, which puts it back", skill.name)),
        Origin::Repo(_) | Origin::Yours => {}
    }
    let root = root();
    std::fs::create_dir_all(&root).map_err(|e| format!("creating {}: {e}", root.display()))?;
    let id = free_id(&root, &skill.name);
    let entry = root.join(&id);
    // the entry is put together beside the others and renamed into place once it's whole
    let tmp = root.join(format!(".{id}.{}.tmp", std::process::id()));
    let _ = remove_tree(&tmp);
    let places = match keep(skill, &tmp) {
        Ok(p) if !p.is_empty() => p,
        kept => {
            let _ = remove_tree(&tmp);
            return Err(match kept {
                Err(e) => format!("copying {}: {e}", skill.name),
                Ok(_) => format!("{} isn't there anymore", skill.name),
            });
        }
    };
    let m = Manifest { name: skill.name.clone(), origin: origin_key(&skill.origin), project: skill.project.clone(), description: skill.description.clone(), places, at: now() };
    let body = serde_json::to_string_pretty(&m).unwrap_or_default();
    if let Err(e) = std::fs::create_dir_all(&tmp).and_then(|_| std::fs::write(tmp.join(MANIFEST), body)).and_then(|_| std::fs::rename(&tmp, &entry)) {
        let _ = remove_tree(&tmp);
        return Err(format!("keeping {} in {}: {e}", skill.name, root.display()));
    }
    // only now does each copy go; one that won't brings back the ones that went
    let mut gone: Vec<&Spot> = vec![];
    for s in &m.places {
        if let Err(e) = take_out(&s.dir) {
            for g in folders_first(&gone) {
                let _ = put_back(&entry, g);
            }
            let _ = remove_tree(&entry);
            return Err(format!("couldn't move {} out of {}: {e}", skill.name, s.dir.display()));
        }
        gone.push(s);
    }
    Ok(from_manifest(id, m))
}

/// Enables `d`: each copy goes back where it was, and its entry goes. A place whose agent home (or project) is gone
/// since is skipped. Err: something of its name is in one of its places again (then nothing goes back), or a place
/// couldn't be written (what went back is taken out again).
pub fn enable(d: &Disabled) -> Result<(), String> {
    let entry = d.dir();
    let m = manifest(&entry).map_err(|e| format!("reading {}: {e}", entry.join(MANIFEST).display()))?;
    let here = |s: &&Spot| match &m.project {
        Some(root) => root.is_dir(),
        None => s.dir.parent().and_then(Path::parent).is_some_and(Path::is_dir),
    };
    let spots: Vec<&Spot> = m.places.iter().filter(here).collect();
    if spots.is_empty() {
        return Err(format!("none of the places {} was in are there anymore", m.name));
    }
    if let Some(s) = spots.iter().find(|s| std::fs::symlink_metadata(&s.dir).is_ok()) {
        let folder = s.dir.parent().unwrap_or(&s.dir);
        return Err(format!("{} has a {} again: disable or delete that one first", folder.display(), m.name));
    }
    let mut back: Vec<&Spot> = vec![];
    for s in folders_first(&spots) {
        if let Err(e) = put_back(&entry, s) {
            for b in back.iter().rev() {
                let _ = take_out(&b.dir);
            }
            return Err(format!("couldn't put {} back in {}: {e}", m.name, s.dir.display()));
        }
        back.push(s);
    }
    let _ = remove_tree(&entry);
    Ok(())
}

/// Deletes `d` for good.
pub fn delete(d: &Disabled) -> Result<(), String> {
    remove_tree(&d.dir()).map_err(|e| format!("deleting {}: {e}", d.dir().display()))
}

/// The spots, folders before links (a link may point at one of the folders).
fn folders_first<'a>(spots: &[&'a Spot]) -> Vec<&'a Spot> {
    spots.iter().filter(|s| s.link.is_none()).chain(spots.iter().filter(|s| s.link.is_some())).copied().collect()
}

/// Copies `skill`'s folders into `entry`, each different copy once, and notes every place with the copy that goes
/// back there, or the link it is.
fn keep(skill: &Skill, entry: &Path) -> io::Result<Vec<Spot>> {
    let mut spots = vec![];
    let mut copies: Vec<u64> = vec![];
    for p in &skill.places {
        let Ok(meta) = std::fs::symlink_metadata(&p.dir) else { continue };
        if meta.file_type().is_symlink() {
            spots.push(Spot { agent: p.agent, dir: p.dir.clone(), copy: 0, link: Some(std::fs::read_link(&p.dir)?) });
            continue;
        }
        let sig = signature(&p.dir);
        let copy = match copies.iter().position(|s| *s == sig) {
            Some(n) => n,
            None => {
                let n = copies.len();
                copy_tree(&p.dir, &if n == 0 { entry.to_path_buf() } else { entry.join(COPIES).join(n.to_string()) }, &[])?;
                copies.push(sig);
                n
            }
        };
        spots.push(Spot { agent: p.agent, dir: p.dir.clone(), copy, link: None });
    }
    Ok(spots)
}

/// Puts a place back from `entry`: its copy (through a folder beside it, renamed into place once whole), or the
/// link it was (a copy of what that points at where this system won't make the link).
fn put_back(entry: &Path, s: &Spot) -> io::Result<()> {
    let parent = s.dir.parent().ok_or_else(|| io::Error::other("it has no folder"))?;
    std::fs::create_dir_all(parent)?;
    if let Some(link) = &s.link {
        if make_link(link, &s.dir).is_ok() {
            return Ok(());
        }
        return copy_tree(&target_of(&s.dir, link), &s.dir, &[]);
    }
    let (src, skip) = match s.copy {
        0 => (entry.to_path_buf(), &[MANIFEST, COPIES][..]),
        n => (entry.join(COPIES).join(n.to_string()), &[][..]),
    };
    let name = s.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = parent.join(format!(".{name}.bro-enabling"));
    let _ = remove_tree(&tmp);
    let copied = copy_tree(&src, &tmp, skip).and_then(|_| std::fs::rename(&tmp, &s.dir));
    if copied.is_err() {
        let _ = remove_tree(&tmp);
    }
    copied
}

/// Takes a place out: a link goes (what it points at stays), a folder goes with everything in it.
fn take_out(dir: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(dir)?.file_type().is_symlink() {
        // a link to a folder (a junction too) is removed as a folder on Windows, as a file elsewhere
        return std::fs::remove_dir(dir).or_else(|_| std::fs::remove_file(dir));
    }
    remove_tree(dir)
}

/// Where a link at `place` points: `link` itself, or read from the folder it's in.
fn target_of(place: &Path, link: &Path) -> PathBuf {
    match place.parent() {
        Some(p) if link.is_relative() => p.join(link),
        _ => link.to_path_buf(),
    }
}

#[cfg(unix)]
fn make_link(target: &Path, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

/// (Windows makes a link only with developer mode on, or as an administrator.)
#[cfg(windows)]
fn make_link(target: &Path, at: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, at)
}

/// Copies the folders and files of `src` into `dst`, except the top-level names in `skip`. Links are left out, as
/// bro's own copies leave them.
fn copy_tree(src: &Path, dst: &Path, skip: &[&str]) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        if skip.iter().any(|s| e.file_name() == *s) {
            continue;
        }
        let (from, to) = (e.path(), dst.join(e.file_name()));
        match e.file_type()? {
            t if t.is_dir() => copy_tree(&from, &to, &[])?,
            t if t.is_file() => {
                std::fs::copy(&from, &to)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Removes a folder and everything in it, read-only files too (git keeps its packs read-only on Windows).
fn remove_tree(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            writable(dir);
            std::fs::remove_dir_all(dir)
        }
        done => done,
    }
}

#[allow(clippy::permissions_set_readonly_false)]
fn writable(dir: &Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => writable(&path),
            Ok(t) if t.is_file() => {
                if let Ok(meta) = std::fs::metadata(&path)
                    && meta.permissions().readonly()
                {
                    let mut p = meta.permissions();
                    p.set_readonly(false);
                    let _ = std::fs::set_permissions(&path, p);
                }
            }
            _ => {}
        }
    }
}

/// What a folder holds, as a number: the same files with the same contents give the same one.
fn signature(dir: &Path) -> u64 {
    fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(root.join(rel)).into_iter().flatten().flatten() {
            let r = rel.join(e.file_name());
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(root, &r, out),
                Ok(t) if t.is_file() => out.push(r),
                _ => {}
            }
        }
    }
    let mut files = vec![];
    walk(dir, Path::new(""), &mut files);
    files.sort();
    let mut h = std::hash::DefaultHasher::new();
    for rel in files {
        rel.hash(&mut h);
        std::fs::read(dir.join(&rel)).unwrap_or_default().hash(&mut h);
    }
    h.finish()
}

/// A folder name under `root` for a skill named `name`: the name, else the name and a number.
fn free_id(root: &Path, name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    let safe = match safe.trim_matches('.') {
        "" => "skill".to_string(),
        s => s.to_string(),
    };
    (1..).map(|n| if n == 1 { safe.clone() } else { format!("{safe}-{n}") }).find(|id| std::fs::symlink_metadata(root.join(id)).is_err()).unwrap_or(safe)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_inventory::scan;
    use crate::util::test_env::sandbox;

    fn skill(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    fn found(name: &str, project: Option<&Path>) -> Skill {
        let roots: Vec<PathBuf> = project.into_iter().map(Path::to_path_buf).collect();
        scan(&roots).skills.into_iter().find(|s| s.name == name && s.project.as_deref() == project).unwrap_or_else(|| panic!("{name} isn't installed"))
    }

    #[test]
    fn a_skill_moves_out_of_every_home_and_back() {
        let sb = sandbox();
        let home = sb.home();
        let body = "---\nname: notes\ndescription: Take notes\n---\nkeep it short\n";
        let (claude, codex) = (home.join(".claude").join("skills").join("notes"), home.join(".codex").join("skills").join("notes"));
        skill(&claude, body);
        std::fs::create_dir_all(claude.join("scripts")).unwrap();
        std::fs::write(claude.join("scripts").join("run"), [0u8, 1, 2]).unwrap();
        skill(&codex, body);
        std::fs::create_dir_all(codex.join("scripts")).unwrap();
        std::fs::write(codex.join("scripts").join("run"), [0u8, 1, 2]).unwrap();

        let d = disable(&found("notes", None)).unwrap();
        assert!(!claude.exists() && !codex.exists(), "both copies went");
        assert_eq!((d.name.as_str(), d.origin.clone(), d.agents()), ("notes", Origin::Yours, vec![Agent::Claude, Agent::Codex]));
        assert_eq!(d.description.as_deref(), Some("Take notes"));
        // the same files in both: kept once, readable where it is
        assert!(d.skill_md().is_file() && !d.dir().join(COPIES).exists());
        assert_eq!(list(), std::slice::from_ref(&d));
        assert!(scan(&[]).skills.iter().all(|s| s.name != "notes"));
        assert_eq!(scan(&[]).disabled.len(), 1);

        enable(&d).unwrap();
        for dir in [&claude, &codex] {
            assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), body);
            assert_eq!(std::fs::read(dir.join("scripts").join("run")).unwrap(), [0u8, 1, 2]);
            assert!(!dir.join(MANIFEST).exists());
        }
        assert!(list().is_empty() && !d.dir().exists());
    }

    #[test]
    fn copies_that_differ_each_go_back_as_they_were() {
        let sb = sandbox();
        let home = sb.home();
        let (claude, codex) = (home.join(".claude").join("skills").join("tidy"), home.join(".codex").join("skills").join("tidy"));
        skill(&claude, "---\nname: tidy\n---\nfor Claude\n");
        skill(&codex, "---\nname: tidy\n---\nfor Codex\n");
        let d = disable(&found("tidy", None)).unwrap();
        assert!(d.dir().join(COPIES).join("1").join("SKILL.md").is_file());
        enable(&d).unwrap();
        assert!(std::fs::read_to_string(claude.join("SKILL.md")).unwrap().contains("for Claude"));
        assert!(std::fs::read_to_string(codex.join("SKILL.md")).unwrap().contains("for Codex"));
    }

    #[test]
    fn a_repo_skill_keeps_its_repo_and_a_project_skill_its_project() {
        let sb = sandbox();
        let home = sb.home();
        let council = home.join(".claude").join("skills").join("council");
        skill(&council, "---\nname: council\n---\n");
        std::fs::write(council.join(".bro-managed"), "written by bro from t3dotgg/tstack; …\n").unwrap();
        let project = home.join("code").join("app");
        let release = project.join(".claude").join("skills").join("release");
        skill(&release, "---\nname: release\n---\n");

        let d = disable(&found("council", None)).unwrap();
        assert_eq!(d.origin, Origin::Repo("t3dotgg/tstack".into()));
        enable(&d).unwrap();
        assert_eq!(crate::skill_repos::owner(&council).as_deref(), Some("t3dotgg/tstack"));

        let d = disable(&found("release", Some(&project))).unwrap();
        assert_eq!(d.project.as_deref(), Some(project.as_path()));
        assert!(!release.exists());
        // another skill named the same: a second entry beside it
        skill(&home.join(".codex").join("skills").join("release"), "---\nname: release\n---\nmine\n");
        let other = disable(&found("release", None)).unwrap();
        assert_eq!((d.id.as_str(), other.id.as_str()), ("release", "release-2"));
        enable(&d).unwrap();
        assert!(release.join("SKILL.md").is_file());
        delete(&other).unwrap();
        assert!(list().is_empty());
    }

    #[test]
    fn nothing_goes_back_over_a_skill_of_the_same_name() {
        let sb = sandbox();
        let home = sb.home();
        let (claude, codex) = (home.join(".claude").join("skills").join("draft"), home.join(".codex").join("skills").join("draft"));
        skill(&claude, "---\nname: draft\n---\nold\n");
        skill(&codex, "---\nname: draft\n---\nold\n");
        let d = disable(&found("draft", None)).unwrap();
        skill(&codex, "---\nname: draft\n---\nnew\n");
        let e = enable(&d).unwrap_err();
        assert!(e.contains("has a draft again"), "{e}");
        assert!(!claude.exists(), "nothing went back");
        assert!(std::fs::read_to_string(codex.join("SKILL.md")).unwrap().contains("new"));
        assert_eq!(list().len(), 1);
    }

    #[test]
    fn bros_own_and_codexs_arent_disabled_here() {
        let sb = sandbox();
        let home = sb.home();
        skill(&home.join(".claude").join("skills").join("bro-open"), crate::skills::BRO_OPEN_SKILL);
        skill(&home.join(".codex").join("skills").join(".system").join("imagegen"), "---\nname: imagegen\n---\n");
        assert!(disable(&found("bro-open", None)).unwrap_err().contains("switch it off"));
        assert!(disable(&found("imagegen", None)).unwrap_err().contains("Codex"));
        assert!(list().is_empty());
    }

    #[test]
    fn a_link_goes_and_comes_back_as_a_link() {
        let sb = sandbox();
        let home = sb.home();
        // npx skills: the skill in ~/.agents/skills, linked into ~/.claude/skills
        let real = home.join(".agents").join("skills").join("pdf");
        skill(&real, "---\nname: pdf\n---\n");
        let link = home.join(".claude").join("skills").join("pdf");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        if make_link(&real, &link).is_err() {
            // this system won't make links (Windows without developer mode)
            return;
        }
        let d = disable(&found("pdf", None)).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err() && !real.exists());
        assert_eq!(d.agents(), [Agent::Claude, Agent::Shared]);
        enable(&d).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(link.join("SKILL.md").is_file() && real.join("SKILL.md").is_file());
    }
}
