//! Skill repos: git repositories bro watches for agent skills (`[skills] repos` in `~/.bro/v2.toml`, the
//! Skills section of settings, `bro skills`).
//!
//! Each repo is a shallow clone in `~/.bro/skill-repos/<owner>-<repo>` (override `BRO_SKILL_REPOS_DIR`),
//! pulled again on every sync. A skill is a folder holding a `SKILL.md`: the repo's root itself, a folder at
//! its root, or a folder under its `skills/`. Only those folders are checked out when git can (a partial,
//! sparse clone), so a big repo with one skill in it stays small.
//!
//! Every skill found is copied into each home agents read skills from ([`crate::justimagine::skill_homes`])
//! with bro's marker naming the repo, so the next sync refreshes it and it is taken back out when it leaves
//! the repo or the repo is dropped. A skill of the same name bro didn't write, or one another repo already
//! brought, is left alone.
use crate::quick_install::{find, search_dirs};
use crate::util::env_path;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Marks a skill directory bro wrote (the same marker as [`crate::managed_skill`]).
const MARKER: &str = ".bro-managed";
const MARKER_PREFIX: &str = "written by bro from ";

/// How often watched repos are pulled while bro runs with auto-sync on.
pub const SYNC_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A clone or pull that takes longer than this is given up on.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// A repo as typed ("https://github.com/t3dotgg/tstack", "github.com/t3dotgg/tstack/tree/main/skills",
/// "t3dotgg/tstack", "git@github.com:t3dotgg/tstack.git") in the form bro keeps: "owner/repo" for GitHub,
/// otherwise the URL without a trailing `.git` or `/`. None when it isn't a repo.
pub fn normalize(input: &str) -> Option<String> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let github = s
        .strip_prefix("git@github.com:")
        .or_else(|| s.strip_prefix("ssh://git@github.com/"))
        .or_else(|| ["https://", "http://", ""].iter().find_map(|scheme| s.strip_prefix(scheme).and_then(|r| r.strip_prefix("github.com/").or_else(|| r.strip_prefix("www.github.com/")))));
    let name_ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if let Some(path) = github {
        let mut parts = path.split('/');
        let (owner, repo) = (parts.next()?, parts.next()?.trim_end_matches(".git"));
        return (name_ok(owner) && name_ok(repo)).then(|| format!("{owner}/{repo}"));
    }
    if s.contains("://") || s.starts_with("git@") {
        return (!s.contains(char::is_whitespace)).then(|| s.to_string());
    }
    // owner/repo shorthand
    let mut parts = s.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && name_ok(owner) && name_ok(repo)).then(|| format!("{owner}/{repo}"))
}

/// The repo is on GitHub ("owner/repo").
pub fn is_github(repo: &str) -> bool {
    !repo.contains(':') && repo.matches('/').count() == 1
}

/// What `git clone` takes.
pub fn clone_url(repo: &str) -> String {
    if is_github(repo) { format!("https://github.com/{repo}.git") } else { repo.to_string() }
}

/// Where people look at it ("https://github.com/owner/repo").
pub fn web_url(repo: &str) -> String {
    if is_github(repo) { format!("https://github.com/{repo}") } else { repo.to_string() }
}

/// `~/.bro/skill-repos` (override `BRO_SKILL_REPOS_DIR`).
pub fn root_dir() -> PathBuf {
    env_path("BRO_SKILL_REPOS_DIR").unwrap_or_else(|| crate::paths::bro_dir().join("skill-repos"))
}

/// Where `repo`'s clone lives: `~/.bro/skill-repos/<owner>-<repo>`.
pub fn cache_dir(repo: &str) -> PathBuf {
    let tail = repo.split("://").last().unwrap_or(repo);
    let name: String = tail.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' }).collect();
    root_dir().join(name.trim_matches('-'))
}

/// A skill found in a repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// the folder it gets under `skills/`
    pub name: String,
    /// the folder that holds its SKILL.md, inside the clone
    pub dir: PathBuf,
}

/// The folders under the repo root that are skills, as git paths ("", "council", "skills/council"), given
/// every file path in the repo. The repo itself is one when its root holds a SKILL.md.
fn skill_paths<'a>(files: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = files
        .into_iter()
        .filter_map(|f| {
            let dir = f.strip_suffix("SKILL.md")?.trim_end_matches('/');
            let depth = if dir.is_empty() { 0 } else { dir.split('/').count() };
            let ok = depth == 0 || (depth == 1 && !dir.starts_with('.')) || (depth == 2 && dir.starts_with("skills/"));
            (ok && (dir.is_empty() || f.ends_with("/SKILL.md"))).then(|| dir.to_string())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every file path under `root`, relative and with `/` (no `.git`).
fn files_under(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>, depth: usize) {
        if depth > 3 {
            return;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            match e.file_type() {
                Ok(t) if t.is_dir() && name != ".git" => walk(&e.path(), &path, out, depth + 1),
                Ok(t) if t.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    let mut out = vec![];
    walk(root, "", &mut out, 0);
    out
}

/// The skills in a checkout: its root, each folder at its root and each folder under `skills/` that holds a
/// SKILL.md. A root skill is named by its SKILL.md's `name:`, else after the repo.
pub fn discover(root: &Path, repo: &str) -> Vec<Found> {
    let files = files_under(root);
    skill_paths(files.iter().map(String::as_str))
        .into_iter()
        .filter_map(|p| {
            let dir = if p.is_empty() { root.to_path_buf() } else { root.join(&p) };
            let name = match p.rsplit('/').next().filter(|n| !n.is_empty()) {
                Some(n) => n.to_string(),
                None => frontmatter_name(&dir.join("SKILL.md")).unwrap_or_else(|| repo.rsplit('/').next().unwrap_or(repo).to_string()),
            };
            let safe = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !name.starts_with('.');
            safe.then_some(Found { name, dir })
        })
        .collect()
}

/// A SKILL.md's `name:` from its frontmatter.
fn frontmatter_name(skill_md: &Path) -> Option<String> {
    let body = std::fs::read_to_string(skill_md).ok()?;
    let front = body.strip_prefix("---")?.split("\n---").next()?;
    front.lines().find_map(|l| l.trim().strip_prefix("name:")).map(|n| n.trim().trim_matches(['"', '\'']).to_string()).filter(|n| !n.is_empty())
}

/// A SKILL.md's `description:`, for the settings view.
pub fn description(skill_md: &Path) -> Option<String> {
    let body = std::fs::read_to_string(skill_md).ok()?;
    let front = body.strip_prefix("---")?.split("\n---").next()?;
    front.lines().find_map(|l| l.trim().strip_prefix("description:")).map(|d| d.trim().trim_matches(['"', '\'']).to_string()).filter(|d| !d.is_empty())
}

// ---- git ----

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Runs git (never prompting for a login) and returns its stdout, or its last error line.
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let exe = find("git", &search_dirs()).ok_or("watching skill repos needs Git: `bro install git`")?;
    let mut cmd = Command::new(exe);
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run git: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} took over {} s", args[0], GIT_TIMEOUT.as_secs()));
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        return Ok(out);
    }
    let why = err.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").trim_start_matches("fatal: ").to_string();
    Err(if why.is_empty() { format!("git {} failed ({status})", args[0]) } else { why })
}

/// Checks out only `paths` (git paths of skill folders; "" is the whole repo) when git can.
fn sparse(dir: &Path, paths: &[String]) -> Result<(), String> {
    if paths.iter().any(String::is_empty) {
        return git(Some(dir), &["sparse-checkout", "disable"]).map(|_| ());
    }
    let patterns: Vec<String> = paths.iter().map(|p| format!("/{p}/")).collect();
    let mut args = vec!["sparse-checkout", "set", "--no-cone"];
    args.extend(patterns.iter().map(String::as_str));
    git(Some(dir), &args).map(|_| ())
}

/// The skill folders in `rev` of the clone, from its file list (no file contents needed).
fn skills_at(dir: &Path, rev: &str) -> Result<Vec<String>, String> {
    Ok(skill_paths(git(Some(dir), &["ls-tree", "-r", "--name-only", rev])?.lines()))
}

/// A fresh clone of `repo` into `dir`, keeping only its skill folders when the server and git allow it.
fn clone(repo: &str, dir: &Path) -> Result<(), String> {
    let parent = dir.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".{}.{}.tmp", dir.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let url = clone_url(repo);
    let t = tmp.to_string_lossy().into_owned();
    let partial = git(None, &["clone", "--depth", "1", "--filter=blob:none", "--no-checkout", "-q", &url, &t]).and_then(|_| {
        let paths = skills_at(&tmp, "HEAD")?;
        sparse(&tmp, &paths)?;
        git(Some(&tmp), &["checkout", "-q"]).map(|_| ())
    });
    if let Err(e) = partial {
        // an old git or a server without partial clones: the whole tip
        let _ = std::fs::remove_dir_all(&tmp);
        git(None, &["clone", "--depth", "1", "-q", &url, &t]).map_err(|full| if full.is_empty() { e } else { full })?;
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir).map_err(|e| format!("moving the clone into {}: {e}", dir.display()))
}

/// Brings an existing clone up to its remote's tip (and the skill folders in it).
fn pull(dir: &Path) -> Result<(), String> {
    let partial = git(Some(dir), &["config", "--get", "remote.origin.promisor"]).is_ok_and(|v| v.trim() == "true");
    let mut fetch = vec!["fetch", "--depth", "1", "-q"];
    if partial {
        fetch.push("--filter=blob:none");
    }
    fetch.extend(["origin", "HEAD"]);
    git(Some(dir), &fetch)?;
    if git(Some(dir), &["sparse-checkout", "list"]).is_ok_and(|l| !l.trim().is_empty()) {
        sparse(dir, &skills_at(dir, "FETCH_HEAD")?)?;
    }
    git(Some(dir), &["reset", "--hard", "-q", "FETCH_HEAD"]).map(|_| ())
}

// ---- installing ----

/// The repo whose skill bro wrote into `dir`: Some("") for a bro skill that came from no repo
/// (JustTools, JustWatch, JustImagine), None for one bro didn't write.
fn owner(dir: &Path) -> Option<String> {
    let marker = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let first = marker.lines().next().unwrap_or("");
    Some(first.strip_prefix(MARKER_PREFIX).map(|r| r.split(';').next().unwrap_or("").trim().to_string()).unwrap_or_default())
}

/// Copies every file of `src` into `dst` (only what changed) and removes what `src` no longer has.
/// Links and `.git` are skipped.
fn mirror(src: &Path, dst: &Path, top: bool) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    let mut keep = vec![];
    for e in std::fs::read_dir(src)?.flatten() {
        let name = e.file_name();
        if name == ".git" || (top && name == MARKER) {
            continue;
        }
        let (from, to) = (e.path(), dst.join(&name));
        match e.file_type()? {
            t if t.is_dir() => mirror(&from, &to, false)?,
            t if t.is_file() => {
                let body = std::fs::read(&from)?;
                if std::fs::read(&to).ok().as_deref() != Some(&body[..]) {
                    if to.is_dir() {
                        std::fs::remove_dir_all(&to)?;
                    }
                    crate::util::atomic_write(&to, &body)?;
                }
            }
            _ => continue,
        }
        keep.push(name);
    }
    for e in std::fs::read_dir(dst)?.flatten() {
        let name = e.file_name();
        if keep.contains(&name) || (top && name == MARKER) {
            continue;
        }
        let path = e.path();
        let _ = if path.is_dir() && !e.file_type().is_ok_and(|t| t.is_symlink()) { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
    }
    Ok(())
}

/// What writing a repo's skills into one home did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    pub written: Vec<String>,
    /// held by a skill of yours, or one another repo brought
    pub taken: Vec<String>,
}

/// Writes `found` (the skills of `repo`) into `home/skills` and removes the ones `repo` brought earlier that
/// it no longer has.
pub fn write_into(home: &Path, repo: &str, found: &[Found]) -> io::Result<Written> {
    let skills = home.join("skills");
    let mut done = Written::default();
    for f in found {
        let dir = skills.join(&f.name);
        let linked = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink());
        if linked || (std::fs::symlink_metadata(&dir).is_ok() && owner(&dir).as_deref() != Some(repo)) {
            done.taken.push(f.name.clone());
            continue;
        }
        mirror(&f.dir, &dir, true)?;
        crate::browser::write_if_changed(&dir.join(MARKER), &format!("{MARKER_PREFIX}{repo}; drop the repo under Skills in bro's settings to remove it\n"))?;
        done.written.push(f.name.clone());
    }
    remove_from(home, repo, &found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>());
    Ok(done)
}

/// Removes the skills `repo` brought into `home`, except those named in `keep`.
fn remove_from(home: &Path, repo: &str, keep: &[&str]) {
    for e in std::fs::read_dir(home.join("skills")).into_iter().flatten().flatten() {
        let path = e.path();
        let linked = e.file_type().is_ok_and(|t| t.is_symlink());
        let name = e.file_name().to_string_lossy().into_owned();
        if !linked && !keep.contains(&name.as_str()) && owner(&path).as_deref() == Some(repo) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// What a repo holds as bro last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoSkills {
    /// the clone's commit (short)
    pub commit: Option<String>,
    /// the skills in it, by name
    pub skills: Vec<String>,
    /// those a skill of yours (or another repo's) already holds in `~/.claude/skills`, so bro left them out
    pub taken: Vec<String>,
}

/// Writes `repo`'s cloned skills into every skill home; what `~/.claude` (else the first home) ended up with.
fn install(repo: &str, dir: &Path) -> Result<RepoSkills, String> {
    let found = discover(dir, repo);
    let mut out = RepoSkills { commit: git(Some(dir), &["rev-parse", "--short", "HEAD"]).ok().map(|c| c.trim().to_string()), skills: found.iter().map(|f| f.name.clone()).collect(), taken: vec![] };
    let root = crate::paths::claude_local_dir();
    let mut first = None;
    for home in crate::justimagine::skill_homes() {
        let done = write_into(&home, repo, &found).map_err(|e| format!("writing into {}: {e}", home.join("skills").display()))?;
        if crate::util::same_path(&home, &root) || first.is_none() {
            first = Some(done.taken);
        }
    }
    out.taken = first.unwrap_or_default();
    Ok(out)
}

/// Clone or pull `repo`, then write its skills into every skill home and take out the ones that left it.
/// Blocking (network): call off the UI thread.
pub fn sync(repo: &str) -> Result<RepoSkills, String> {
    let dir = cache_dir(repo);
    let pulled = if dir.join(".git").exists() { pull(&dir) } else { Err(String::new()) };
    if pulled.is_err() {
        // no clone yet, or one git can't bring up to date: start over
        clone(repo, &dir)?;
    }
    install(repo, &dir)
}

/// What's already cloned for `repo` (no network, nothing written).
pub fn cached(repo: &str) -> Option<RepoSkills> {
    let dir = cache_dir(repo);
    if !dir.join(".git").exists() {
        return None;
    }
    let skills = discover(&dir, repo).into_iter().map(|f| f.name).collect();
    let head = std::fs::read_to_string(dir.join(".git").join("HEAD")).ok();
    // a detached HEAD is the commit itself; on a branch, the commit its ref file holds
    let commit = head.and_then(|h| match h.trim().strip_prefix("ref: ") {
        Some(r) => std::fs::read_to_string(dir.join(".git").join(r)).ok(),
        None => Some(h),
    });
    Some(RepoSkills { commit: commit.map(|c| c.trim().chars().take(7).collect()), skills, taken: vec![] })
}

/// Writes the cloned skills of `repos` into `home` (a launch's harness home, which a sync may not have seen
/// yet). No network; a repo not cloned yet is skipped.
pub fn write_cached(home: &Path, repos: &[String]) {
    for repo in repos {
        let dir = cache_dir(repo);
        if dir.join(".git").exists() {
            let _ = write_into(home, repo, &discover(&dir, repo));
        }
    }
}

/// Takes every skill `repo` brought back out of every skill home, and deletes its clone.
pub fn forget(repo: &str) {
    for home in crate::justimagine::skill_homes() {
        remove_from(&home, repo, &[]);
    }
    let _ = std::fs::remove_dir_all(cache_dir(repo));
}

/// Skills in `~/.claude/skills` bro didn't write (yours): their names, sorted.
pub fn own_skills() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(crate::paths::claude_local_dir().join("skills"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("SKILL.md").is_file() && owner(&e.path()).is_none())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != crate::skills::BRO_OPEN_SKILL_NAME && n != crate::orchestrator::SKILL_NAME && n != "bro-browser")
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
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
    fn normalizes_what_people_paste() {
        for s in ["https://github.com/t3dotgg/tstack", "https://github.com/t3dotgg/tstack/", "github.com/t3dotgg/tstack", "t3dotgg/tstack", "git@github.com:t3dotgg/tstack.git", "https://github.com/t3dotgg/tstack/tree/main/skills", "https://www.github.com/t3dotgg/tstack.git"] {
            assert_eq!(normalize(s).as_deref(), Some("t3dotgg/tstack"), "{s}");
        }
        assert_eq!(normalize("https://gitlab.com/me/skills.git").as_deref(), Some("https://gitlab.com/me/skills"));
        for bad in ["", "tstack", "a/b/c", "has space/repo", "https://github.com/onlyowner", "../x"] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
        assert!(is_github("t3dotgg/tstack") && !is_github("https://gitlab.com/me/skills"));
        assert_eq!(clone_url("t3dotgg/tstack"), "https://github.com/t3dotgg/tstack.git");
        assert!(cache_dir("t3dotgg/tstack").ends_with("t3dotgg-tstack"));
        assert!(cache_dir("https://gitlab.com/me/skills").ends_with("gitlab.com-me-skills"));
    }

    #[test]
    fn finds_skills_at_the_root_one_down_and_under_skills() {
        let files = ["README.md", "SKILL.md", "council/SKILL.md", "skills/babysit-pr/SKILL.md", "skills/babysit-pr/scripts/pr-wait", "skills/SKILL.md", "docs/deep/x/SKILL.md", ".github/SKILL.md", "notes/SKILL.md.bak"];
        assert_eq!(skill_paths(files), ["", "council", "skills", "skills/babysit-pr"]);
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        skill(&root.join("skills").join("full-send"), "---\nname: full-send\n---\n");
        skill(&root.join("council"), "---\nname: council\ndescription: \"Ask the council\"\n---\n");
        std::fs::create_dir_all(root.join("app")).unwrap();
        let found: Vec<String> = discover(root, "t3dotgg/tstack").into_iter().map(|f| f.name).collect();
        assert_eq!(found, ["council", "full-send"]);
        assert_eq!(description(&root.join("council").join("SKILL.md")).as_deref(), Some("Ask the council"));
        // the repo is the skill
        let one = tempfile::tempdir().unwrap();
        skill(one.path(), "---\nname: tidy\n---\n");
        assert_eq!(discover(one.path(), "me/whatever")[0].name, "tidy");
    }

    #[test]
    fn writes_refreshes_and_takes_back_its_own_skills_only() {
        let sb = sandbox();
        let clone = sb.home().join("clone");
        skill(&clone.join("skills").join("council"), "v1");
        std::fs::create_dir_all(clone.join("skills").join("council").join("scripts")).unwrap();
        std::fs::write(clone.join("skills").join("council").join("scripts").join("run"), [0u8, 159, 146, 150]).unwrap();
        skill(&clone.join("skills").join("mine"), "theirs");
        skill(&clone.join("skills").join("other"), "theirs");
        let home = sb.home().join(".claude");
        skill(&home.join("skills").join("mine"), "my own");
        skill(&home.join("skills").join("other"), "another repo's");
        std::fs::write(home.join("skills").join("other").join(MARKER), format!("{MARKER_PREFIX}someone/else; …\n")).unwrap();

        let found = discover(&clone, "t3dotgg/tstack");
        let done = write_into(&home, "t3dotgg/tstack", &found).unwrap();
        assert_eq!(done.written, ["council"]);
        assert_eq!(done.taken, ["mine", "other"]);
        let council = home.join("skills").join("council");
        assert_eq!(std::fs::read(council.join("scripts").join("run")).unwrap(), [0u8, 159, 146, 150]);
        assert_eq!(owner(&council).as_deref(), Some("t3dotgg/tstack"));
        assert_eq!(std::fs::read_to_string(home.join("skills").join("mine").join("SKILL.md")).unwrap(), "my own");

        // upstream changed and dropped a file: the copy follows
        std::fs::write(clone.join("skills").join("council").join("SKILL.md"), "v2").unwrap();
        std::fs::remove_dir_all(clone.join("skills").join("council").join("scripts")).unwrap();
        write_into(&home, "t3dotgg/tstack", &discover(&clone, "t3dotgg/tstack")).unwrap();
        assert_eq!(std::fs::read_to_string(council.join("SKILL.md")).unwrap(), "v2");
        assert!(!council.join("scripts").exists() && council.join(MARKER).is_file());

        // the skill left the repo: bro's copy goes, yours and the other repo's stay
        std::fs::remove_dir_all(clone.join("skills").join("council")).unwrap();
        write_into(&home, "t3dotgg/tstack", &discover(&clone, "t3dotgg/tstack")).unwrap();
        assert!(!council.exists());
        assert!(home.join("skills").join("mine").exists() && home.join("skills").join("other").exists());
        assert_eq!(own_skills(), ["mine"]);
    }

    #[test]
    fn syncs_a_local_repo_and_forgets_it() {
        let sb = sandbox();
        let Some(git_exe) = find("git", &search_dirs()) else { return };
        let src = sb.home().join("src");
        skill(&src.join("skills").join("council"), "---\nname: council\n---\nv1\n");
        skill(&src.join("big").join("nested").join("deep"), "not a skill: too deep");
        std::fs::write(src.join("README.md"), "hi").unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new(&git_exe).arg("-C").arg(&src).args(args).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
            assert!(ok.status.success(), "{args:?}: {}", String::from_utf8_lossy(&ok.stderr));
        };
        run(&["init", "-q"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "one"]);
        let repo = format!("file://{}", src.to_string_lossy().replace('\\', "/"));
        std::fs::create_dir_all(sb.home().join(".claude").join("skills")).unwrap();
        let first = sync(&repo).unwrap();
        assert_eq!(first.skills, ["council"]);
        let copy = sb.home().join(".claude").join("skills").join("council");
        assert!(copy.join("SKILL.md").is_file());
        assert_eq!(cached(&repo).unwrap().skills, ["council"]);

        std::fs::write(src.join("skills").join("council").join("SKILL.md"), "---\nname: council\n---\nv2\n").unwrap();
        run(&["commit", "-qam", "two"]);
        let second = sync(&repo).unwrap();
        assert_ne!(first.commit, second.commit);
        assert!(std::fs::read_to_string(copy.join("SKILL.md")).unwrap().contains("v2"));

        forget(&repo);
        assert!(!copy.exists() && !cache_dir(&repo).exists());
    }
}
