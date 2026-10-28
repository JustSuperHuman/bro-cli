//! Skill repos: git repositories bro watches for agent skills (`[skills] repos` in `~/.bro/v2.toml`, the
//! Skills view, `bro skills`).
//!
//! A repo can also be watched for one skill in it (`owner/repo#name`, or `owner/repo#path/to/folder`): how
//! Skill Issue installs ([`crate::skill_market`]). A name is the folder of that name holding a SKILL.md anywhere
//! in the repo; a folder that isn't a skill itself brings the skills in it.
//!
//! Each repo is a shallow clone in `~/.bro/skill-repos/<owner>-<repo>` (override `BRO_SKILL_REPOS_DIR`),
//! pulled again on every sync. A skill is a folder holding a `SKILL.md`: the repo's root itself, a folder at
//! its root, or a folder under its `skills/`. Only those folders are checked out when git can (a partial,
//! sparse clone), so a big repo with one skill in it stays small.
//!
//! Watching a repo installs nothing: its skills are listed ([`cached`]) and you install the ones you want
//! ([`install`]), everywhere (every home agents read skills from, [`crate::justimagine::skill_homes`]) or into a
//! project's own folders ([`Target`]). Each copy carries bro's marker naming the repo, so every sync brings it up to
//! date, one installed everywhere reaches a home that's new since, and one that leaves the repo is taken back out.
//! A skill of the same name bro didn't write, or one another repo brought, is left alone.
use crate::quick_install::{find, search_dirs};
use crate::util::env_path;
use std::collections::BTreeSet;
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
/// otherwise the URL without a trailing `.git` or `/`; then `#` and the skill when it names one
/// ("owner/repo#council", "owner/repo#skills/council"). None when it isn't a repo.
pub fn normalize(input: &str) -> Option<String> {
    let input = input.trim();
    if let Some((repo, pick)) = input.rsplit_once('#') {
        let pick = pick.trim().trim_matches('/');
        let segment_ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        let repo = normalize(repo)?;
        return match pick {
            "" => Some(repo),
            _ => pick.split('/').all(segment_ok).then(|| format!("{repo}#{pick}")),
        };
    }
    let s = input.trim_end_matches('/');
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

/// The repo of a watched spec ("owner/repo#council" → "owner/repo").
pub fn base(spec: &str) -> &str {
    spec.split_once('#').map_or(spec, |(r, _)| r)
}

/// The one skill a spec watches ("owner/repo#council" → "council"); None for the whole repo.
pub fn pick(spec: &str) -> Option<&str> {
    spec.split_once('#').map(|(_, p)| p).filter(|p| !p.is_empty())
}

/// How a spec reads: "owner/repo", or "owner/repo › council".
pub fn label(spec: &str) -> String {
    match pick(spec) {
        Some(p) => format!("{} › {p}", base(spec)),
        None => spec.to_string(),
    }
}

/// The same spec: GitHub doesn't mind the case of an owner or repo ("Leonxlnx/taste-skill" is "leonxlnx/taste-skill").
pub fn same(a: &str, b: &str) -> bool {
    if is_github(a) && is_github(b) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// The repo is on GitHub ("owner/repo").
pub fn is_github(repo: &str) -> bool {
    let repo = base(repo);
    !repo.contains(':') && repo.matches('/').count() == 1
}

/// What `git clone` takes.
pub fn clone_url(repo: &str) -> String {
    let repo = base(repo);
    if is_github(repo) { format!("https://github.com/{repo}.git") } else { repo.to_string() }
}

/// Where people look at it ("https://github.com/owner/repo", "https://github.com/owner/repo/tree/HEAD/skills/x").
pub fn web_url(repo: &str) -> String {
    let url = if is_github(repo) { format!("https://github.com/{}", base(repo)) } else { base(repo).to_string() };
    match pick(repo) {
        Some(p) if is_github(repo) && p.contains('/') => format!("{url}/tree/HEAD/{p}"),
        _ => url,
    }
}

/// `~/.bro/skill-repos` (override `BRO_SKILL_REPOS_DIR`).
pub fn root_dir() -> PathBuf {
    env_path("BRO_SKILL_REPOS_DIR").unwrap_or_else(|| crate::paths::bro_dir().join("skill-repos"))
}

/// Where `repo`'s clone lives: `~/.bro/skill-repos/<owner>-<repo>`.
/// A picked skill's is `<owner>-<repo>-<its folder>-<hash>`: short, since its checkout nests the skill's whole path
/// and Windows' git gives up past 260 characters.
pub fn cache_dir(repo: &str) -> PathBuf {
    let base = base(repo);
    let tail = base.split("://").last().unwrap_or(base);
    let mut name: String = tail.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' }).collect();
    // a long URL (a folder on this machine) keeps its end and a hash of the rest, for the same reason
    if name.len() > 48 {
        let end: String = name.chars().skip(name.chars().count() - 40).collect();
        name = format!("{}-{}", end.trim_matches('-'), crate::util::short_hash(base, 6));
    }
    if let Some(p) = pick(repo) {
        let leaf: String = p.rsplit('/').next().unwrap_or(p).chars().take(32).collect();
        name = format!("{name}-{leaf}-{}", crate::util::short_hash(p, 6));
    }
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
/// every file path in the repo. The repo itself is one when its root holds a SKILL.md. A repo with none there
/// keeps them where agents' own tools look ([`deeper_paths`]).
fn skill_paths<'a>(files: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let dirs: Vec<&str> = files.into_iter().filter_map(|f| f.strip_suffix("SKILL.md").map(|d| d.trim_end_matches('/')).filter(|d| d.is_empty() || f.ends_with("/SKILL.md"))).collect();
    let mut out: Vec<String> = dirs
        .iter()
        .filter(|dir| {
            let depth = if dir.is_empty() { 0 } else { dir.split('/').count() };
            depth == 0 || (depth == 1 && !dir.starts_with('.')) || (depth == 2 && dir.starts_with("skills/"))
        })
        .map(|d| d.to_string())
        .collect();
    if out.is_empty() {
        out = deeper_paths(&dirs);
    }
    out.sort();
    out.dedup();
    out
}

/// Skill folders in the other places skill repos keep them: `.claude/skills/<x>`, `.agents/skills/<x>`,
/// `plugins/<p>/skills/<x>`, `<p>/skills/<x>`. One per name (a repo often ships the same skill for several
/// agents), `.claude`'s first.
fn deeper_paths(dirs: &[&str]) -> Vec<String> {
    let rank = |d: &str| -> Option<u8> {
        let parts: Vec<&str> = d.split('/').collect();
        match parts[..] {
            [".claude", "skills", _] => Some(0),
            [".agents" | ".agent", "skills", _] => Some(1),
            ["plugins", p, "skills", _] if !p.starts_with('.') => Some(2),
            [p, "skills", _] if !p.starts_with('.') => Some(3),
            [p, "skills", _] if p.starts_with('.') => Some(4),
            _ => None,
        }
    };
    let mut found: Vec<(u8, &str)> = dirs.iter().filter_map(|d| Some((rank(d)?, *d))).collect();
    found.sort();
    let mut names = std::collections::HashSet::new();
    found.into_iter().filter(|(_, d)| names.insert(d.rsplit('/').next().unwrap_or(d))).map(|(_, d)| d.to_string()).collect()
}

/// What names a skill folder in its SKILL.md (`name:`), when that can be read.
type Named<'n> = &'n dyn Fn(&str) -> Option<String>;

/// The folders of `files` that `spec` watches: its skill when it picks one ([`picked_paths`]), else every skill
/// folder bro looks in ([`skill_paths`]).
fn paths_for<'a>(spec: &str, files: impl IntoIterator<Item = &'a str>, named: Named) -> Vec<String> {
    match pick(spec) {
        Some(p) => picked_paths(p, files, named),
        None => skill_paths(files),
    }
}

/// The folders `pick` names in a repo of `files`: a path is that folder when it holds a SKILL.md, else the skills
/// under it (up to two folders down); a name is the shallowest folder of that name holding one, wherever it is,
/// else the one whose SKILL.md is `name:`d so (as skills.sh lists them), else the repo itself when its root holds
/// the only SKILL.md (the skill is the repo).
fn picked_paths<'a>(pick: &str, files: impl IntoIterator<Item = &'a str>, named: Named) -> Vec<String> {
    let dirs: Vec<&str> = files.into_iter().filter_map(|f| f.strip_suffix("SKILL.md").map(|d| d.trim_end_matches('/')).filter(|d| d.is_empty() || f.ends_with("/SKILL.md"))).collect();
    if dirs.contains(&pick) {
        return vec![pick.to_string()];
    }
    if pick.contains('/') {
        let under = format!("{pick}/");
        let mut out: Vec<String> = dirs.iter().filter(|d| d.strip_prefix(&under).is_some_and(|rest| rest.split('/').count() <= 2)).map(|d| d.to_string()).collect();
        out.sort();
        return out;
    }
    let shallowest = |d: &&&str| (d.split('/').count(), d.len());
    let by_folder = dirs.iter().filter(|d| d.rsplit('/').next() == Some(pick)).min_by_key(shallowest);
    let by_name = || dirs.iter().filter(|d| named(d).is_some_and(|n| n.eq_ignore_ascii_case(pick))).min_by_key(shallowest);
    match by_folder.or_else(by_name) {
        Some(d) => vec![d.to_string()],
        None if dirs == [""] => vec![String::new()],
        None => vec![],
    }
}

/// Every file path under `root`, relative and with `/` (no `.git`), `max` folders down at most.
fn files_under(root: &Path, max: usize) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>, depth: usize, max: usize) {
        if depth > max {
            return;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            match e.file_type() {
                Ok(t) if t.is_dir() && name != ".git" && name != "node_modules" => walk(&e.path(), &path, out, depth + 1, max),
                Ok(t) if t.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    let mut out = vec![];
    walk(root, "", &mut out, 0, max);
    out
}

/// The skills in a checkout: its root, each folder at its root and each folder under `skills/` that holds a
/// SKILL.md; or the one `repo` picks ([`picked_paths`]). A root skill is named by its SKILL.md's `name:`, else after
/// the repo.
pub fn discover(root: &Path, repo: &str) -> Vec<Found> {
    // a pick may be anywhere in the repo (its checkout holds little else)
    let files = files_under(root, if pick(repo).is_some() { 12 } else { 4 });
    let named = |dir: &str| frontmatter_name(&root.join(dir).join("SKILL.md"));
    paths_for(repo, files.iter().map(String::as_str), &named)
        .into_iter()
        .filter_map(|p| {
            let dir = if p.is_empty() { root.to_path_buf() } else { root.join(&p) };
            let name = match p.rsplit('/').next().filter(|n| !n.is_empty()) {
                Some(n) => n.to_string(),
                None => frontmatter_name(&dir.join("SKILL.md")).unwrap_or_else(|| base(repo).rsplit('/').next().unwrap_or(repo).to_string()),
            };
            let safe = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !name.starts_with('.');
            safe.then_some(Found { name, dir })
        })
        .collect()
}

/// A SKILL.md's `name:` from its frontmatter.
fn frontmatter_name(skill_md: &Path) -> Option<String> {
    front(&std::fs::read_to_string(skill_md).ok()?, "name")
}

/// A SKILL.md's `description:`, for the Skills view.
pub fn description(skill_md: &Path) -> Option<String> {
    front(&std::fs::read_to_string(skill_md).ok()?, "description")
}

/// The value of `key` in a SKILL.md's frontmatter: a plain or quoted line (and the indented lines that carry it
/// on), or a folded (`>`, `>-`) or literal (`|`) block, as skills often wrap a long description.
pub fn front(body: &str, key: &str) -> Option<String> {
    let body = body.trim_start_matches('\u{feff}');
    let front = body.strip_prefix("---")?.split("\n---").next()?;
    let lines: Vec<&str> = front.lines().map(|l| l.trim_end_matches('\r')).collect();
    let at = lines.iter().position(|l| !l.starts_with([' ', '\t']) && l.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with(':')))?;
    let first = lines[at][key.len()..].trim_start()[1..].trim();
    let more: Vec<&str> = lines[at + 1..].iter().take_while(|l| l.is_empty() || l.starts_with([' ', '\t'])).map(|l| l.trim()).collect();
    let text = match first.chars().next() {
        Some('|') => more.join("\n"),
        Some('>') | None => fold(&more),
        _ => unquote(&fold(&[&[first][..], &more].concat())),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Lines as YAML folds them: one line, a blank line for a break.
fn fold(lines: &[&str]) -> String {
    let mut out = String::new();
    for l in lines {
        if l.is_empty() {
            out.push('\n');
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(l);
        }
    }
    out
}

/// A YAML scalar without its quotes ("…", '…').
fn unquote(s: &str) -> String {
    let s = s.trim();
    match (s.chars().next(), s.chars().last()) {
        (Some('"'), Some('"')) if s.len() >= 2 => s[1..s.len() - 1].replace("\\\"", "\""),
        (Some('\''), Some('\'')) if s.len() >= 2 => s[1..s.len() - 1].replace("''", "'"),
        _ => s.to_string(),
    }
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
    // a skill's folder can sit deep in a repo, under a long home
    #[cfg(windows)]
    cmd.args(["-c", "core.longpaths=true"]);
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
    let why = err.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").trim_start_matches("fatal: ").to_string();
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

/// The skill folders `repo` watches in `rev` of the clone, from its file list (no file contents needed).
fn skills_at(dir: &Path, rev: &str, repo: &str) -> Result<Vec<String>, String> {
    Ok(paths_for(repo, git(Some(dir), &["ls-tree", "-r", "--name-only", rev])?.lines(), &|_| None))
}

/// Why a picked skill didn't come: its repo has no such folder.
fn missing(repo: &str, pick: &str) -> String {
    format!("{} has no skill {pick} (a folder with a SKILL.md)", base(repo))
}

/// A fresh clone of `repo` into `dir`, keeping only its skill folders when the server and git allow it.
fn clone(repo: &str, dir: &Path) -> Result<(), String> {
    let parent = dir.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".{}.{}.tmp", dir.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let url = clone_url(repo);
    let t = tmp.to_string_lossy().into_owned();
    let listed = git(None, &["clone", "--depth", "1", "--filter=blob:none", "--no-checkout", "-q", &url, &t]).and_then(|_| skills_at(&tmp, "HEAD", repo)).and_then(|paths| match pick(repo) {
        // a name no folder has: fetch just the SKILL.md files and read their names
        Some(p) if paths.is_empty() && !p.contains('/') => {
            git(Some(&tmp), &["sparse-checkout", "set", "--no-cone", "SKILL.md"])?;
            git(Some(&tmp), &["checkout", "-q"])?;
            Ok(discover(&tmp, repo).into_iter().filter_map(|f| f.dir.strip_prefix(&tmp).ok().map(|d| d.to_string_lossy().replace('\\', "/"))).collect())
        }
        _ => Ok(paths),
    });
    // the repo came and the skill picked from it isn't there: the whole tip wouldn't have it either
    if let (Ok(paths), Some(p)) = (&listed, pick(repo))
        && paths.is_empty()
    {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(missing(repo, p));
    }
    let partial = listed.and_then(|paths| {
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

/// Brings an existing clone up to its remote's tip (and the skill folders `repo` watches in it).
fn pull(dir: &Path, repo: &str) -> Result<(), String> {
    let partial = git(Some(dir), &["config", "--get", "remote.origin.promisor"]).is_ok_and(|v| v.trim() == "true");
    let mut fetch = vec!["fetch", "--depth", "1", "-q"];
    if partial {
        fetch.push("--filter=blob:none");
    }
    fetch.extend(["origin", "HEAD"]);
    git(Some(dir), &fetch)?;
    if git(Some(dir), &["sparse-checkout", "list"]).is_ok_and(|l| !l.trim().is_empty()) {
        let paths = skills_at(dir, "FETCH_HEAD", repo)?;
        // a pick known only by its SKILL.md's name keeps the folder it has
        if !(paths.is_empty() && pick(repo).is_some()) {
            sparse(dir, &paths)?;
        }
    }
    git(Some(dir), &["reset", "--hard", "-q", "FETCH_HEAD"]).map(|_| ())
}

// ---- installing ----

/// The repo whose skill bro wrote into `dir`: Some("") for a bro skill that came from no repo
/// (JustTools, JustWatch, JustImagine), None for one bro didn't write.
pub(crate) fn owner(dir: &Path) -> Option<String> {
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
                // a skill's scripts stay runnable
                #[cfg(unix)]
                std::fs::set_permissions(&to, std::fs::metadata(&from)?.permissions())?;
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

/// Where installing a skill puts it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// every home agents read skills from ([`crate::justimagine::skill_homes`]): every project gets it
    Everywhere,
    /// a project's own folders: `.claude/skills` (Claude) and `.agents/skills` (Codex and the agents that share it)
    Project(PathBuf),
}

impl Target {
    /// The skills folders it writes into.
    pub fn folders(&self) -> Vec<PathBuf> {
        match self {
            Target::Everywhere => crate::justimagine::skill_homes().into_iter().map(|h| h.join("skills")).collect(),
            Target::Project(root) => vec![root.join(".claude").join("skills"), root.join(".agents").join("skills")],
        }
    }
}

/// What writing a repo's skills did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    pub written: Vec<String>,
    /// held by a skill of yours, or one another repo brought
    pub taken: Vec<String>,
}

/// Writes `found` (skills of `repo`) into the skills folder `skills`, each over its own earlier copy but never over
/// a skill of yours, another repo's or a link. `prune`: and takes out what `repo` brought there that `found` hasn't.
fn write_skills(skills: &Path, repo: &str, found: &[Found], prune: bool) -> io::Result<Written> {
    let mut done = Written::default();
    for f in found {
        let dir = skills.join(&f.name);
        let linked = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink());
        if linked || (std::fs::symlink_metadata(&dir).is_ok() && !owner(&dir).is_some_and(|o| same(&o, repo))) {
            done.taken.push(f.name.clone());
            continue;
        }
        mirror(&f.dir, &dir, true)?;
        crate::browser::write_if_changed(&dir.join(MARKER), &format!("{MARKER_PREFIX}{repo}; bro keeps it in step with the repo · disable it in bro's Skills view to take it out\n"))?;
        done.written.push(f.name.clone());
    }
    if prune {
        remove_from(skills, repo, &found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>());
    }
    Ok(done)
}

/// Writes `found` (the skills of `repo`) into `home/skills` and removes the ones `repo` brought earlier that
/// it no longer has.
pub fn write_into(home: &Path, repo: &str, found: &[Found]) -> io::Result<Written> {
    write_skills(&home.join("skills"), repo, found, true)
}

/// Removes the skills `repo` brought into the skills folder `skills`, except those named in `keep`.
fn remove_from(skills: &Path, repo: &str, keep: &[&str]) {
    for e in std::fs::read_dir(skills).into_iter().flatten().flatten() {
        let path = e.path();
        let linked = e.file_type().is_ok_and(|t| t.is_symlink());
        let name = e.file_name().to_string_lossy().into_owned();
        if !linked && !keep.contains(&name.as_str()) && owner(&path).is_some_and(|o| same(&o, repo)) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// The skills `repo` brought into `folders` (skills folders), by their folder's name.
pub fn installed_names(repo: &str, folders: &[PathBuf]) -> BTreeSet<String> {
    folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten())
        .filter(|e| !e.file_type().is_ok_and(|t| t.is_symlink()) && owner(&e.path()).is_some_and(|o| same(&o, repo)))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// The skills folder of every agent home.
fn home_folders() -> Vec<PathBuf> {
    crate::justimagine::skill_homes().into_iter().map(|h| h.join("skills")).collect()
}

/// One skill a watched repo holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSkill {
    pub name: String,
    /// its SKILL.md's `description:`
    pub description: Option<String>,
    /// the folder holding its SKILL.md, in the clone
    pub dir: PathBuf,
}

/// What a repo holds as bro last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoSkills {
    /// the clone's commit (short)
    pub commit: Option<String>,
    /// the skills in it, by name
    pub skills: Vec<String>,
    /// each of them, with what it does and where it is in the clone
    pub found: Vec<RepoSkill>,
    /// those a skill of yours (or another repo's) holds in `~/.claude/skills`: installed everywhere, they stay out
    pub taken: Vec<String>,
}

/// What the clone of `repo` in `dir` holds.
fn listing(repo: &str, dir: &Path) -> RepoSkills {
    let found = discover(dir, repo);
    let claude = crate::paths::claude_local_dir().join("skills");
    let taken = found
        .iter()
        .filter(|f| {
            let d = claude.join(&f.name);
            std::fs::symlink_metadata(&d).is_ok() && !owner(&d).is_some_and(|o| same(&o, repo))
        })
        .map(|f| f.name.clone())
        .collect();
    RepoSkills {
        commit: head_commit(dir),
        skills: found.iter().map(|f| f.name.clone()).collect(),
        found: found.into_iter().map(|f| RepoSkill { description: description(&f.dir.join("SKILL.md")), name: f.name, dir: f.dir }).collect(),
        taken,
    }
}

/// The clone's commit, short, read from its files (no git needed).
fn head_commit(dir: &Path) -> Option<String> {
    let git = dir.join(".git");
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    // a detached HEAD is the commit itself; on a branch, the commit its ref holds (packed, in a fresh clone)
    let commit = match head.trim().strip_prefix("ref: ") {
        Some(r) => std::fs::read_to_string(git.join(r))
            .ok()
            .or_else(|| std::fs::read_to_string(git.join("packed-refs")).ok()?.lines().find_map(|l| l.strip_suffix(r).map(str::to_string)))?,
        None => head,
    };
    let short: String = commit.trim().chars().take(7).collect();
    (!short.is_empty()).then_some(short)
}

/// Brings every installed copy of `repo`'s skills up to date with its clone in `dir`: in every agent home the ones
/// installed everywhere (a home that's new since gets them too), in `projects`' own folders the ones installed
/// there. One that left the repo is taken back out; nothing else is installed.
fn refresh(repo: &str, dir: &Path, projects: &[PathBuf]) -> Result<(), String> {
    let found = discover(dir, repo);
    let write = |folder: &Path, names: &BTreeSet<String>| {
        let chosen: Vec<Found> = found.iter().filter(|f| names.contains(&f.name)).cloned().collect();
        write_skills(folder, repo, &chosen, true).map(|_| ()).map_err(|e| format!("writing into {}: {e}", folder.display()))
    };
    let homes = home_folders();
    let everywhere = installed_names(repo, &homes);
    if !everywhere.is_empty() {
        for folder in &homes {
            write(folder, &everywhere)?;
        }
    }
    for root in projects {
        for (_, folder) in crate::skill_inventory::project_folders(root) {
            let here = installed_names(repo, std::slice::from_ref(&folder));
            if !here.is_empty() {
                write(&folder, &here)?;
            }
        }
    }
    Ok(())
}

/// Clone or pull `repo`, then bring the skills installed from it up to date (in every agent home, and in
/// `projects`' own folders). Installs nothing new. Blocking (network): call off the UI thread.
pub fn sync(repo: &str, projects: &[PathBuf]) -> Result<RepoSkills, String> {
    let dir = cache_dir(repo);
    let pulled = if dir.join(".git").exists() { pull(&dir, repo) } else { Err(String::new()) };
    if pulled.is_err() {
        // no clone yet, or one git can't bring up to date: start over
        clone(repo, &dir)?;
    }
    let out = listing(repo, &dir);
    if let Some(p) = pick(repo).filter(|_| out.skills.is_empty()) {
        return Err(missing(repo, p));
    }
    refresh(repo, &dir, projects)?;
    Ok(out)
}

/// Installs the skills of `repo` named in `names` (every one when it's empty) into `target`, cloning the repo first
/// when bro hasn't yet. A skill of yours (or another repo's) of the same name stays: it comes back in `taken`.
/// Blocking (network on a first clone): call off the UI thread.
pub fn install(repo: &str, names: &[String], target: &Target) -> Result<Written, String> {
    let dir = cache_dir(repo);
    if !dir.join(".git").exists() {
        clone(repo, &dir)?;
    }
    let found: Vec<Found> = discover(&dir, repo).into_iter().filter(|f| names.is_empty() || names.iter().any(|n| n.eq_ignore_ascii_case(&f.name))).collect();
    if found.is_empty() {
        return Err(match (names, pick(repo)) {
            ([], Some(p)) => missing(repo, p),
            ([], None) => format!("{} has no skills (a folder with a SKILL.md)", label(repo)),
            _ => format!("{} has no skill {}", label(repo), names.join(", ")),
        });
    }
    let folders = target.folders();
    if folders.is_empty() {
        return Err("no agent home to install into yet: open Claude Code or Codex once first".into());
    }
    let mut done = Written::default();
    for folder in folders {
        let w = write_skills(&folder, repo, &found, false).map_err(|e| format!("writing into {}: {e}", folder.display()))?;
        for (all, now) in [(&mut done.written, w.written), (&mut done.taken, w.taken)] {
            for n in now {
                if !all.contains(&n) {
                    all.push(n);
                }
            }
        }
    }
    Ok(done)
}

/// What's already cloned for `repo` (no network, nothing written).
pub fn cached(repo: &str) -> Option<RepoSkills> {
    let dir = cache_dir(repo);
    dir.join(".git").exists().then(|| listing(repo, &dir))
}

/// Writes the skills of `repos` installed everywhere into `home` (a launch's harness home, which a sync may not
/// have reached yet). No network; a repo not cloned yet is skipped.
pub fn write_cached(home: &Path, repos: &[String]) {
    let homes = home_folders();
    for repo in repos {
        let dir = cache_dir(repo);
        if !dir.join(".git").exists() {
            continue;
        }
        let everywhere = installed_names(repo, &homes);
        if !everywhere.is_empty() {
            let found: Vec<Found> = discover(&dir, repo).into_iter().filter(|f| everywhere.contains(&f.name)).collect();
            let _ = write_skills(&home.join("skills"), repo, &found, false);
        }
    }
}

/// Deletes `repo`'s clone. The skills installed from it stay where they are, no longer kept in step.
pub fn forget(repo: &str) {
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
        // a long one (a repo on this machine) stays short: Windows' git gives up past 260 characters
        let local = "file:///C:/Users/someone/AppData/Local/Temp/claude/a-very-long-folder-name/scratchpad/e2e/src";
        let name = cache_dir(local).file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.len() <= 48 && name.contains("scratchpad-e2e-src") && name != cache_dir(&local.replace("e2e", "e3e")).file_name().unwrap().to_string_lossy(), "{name}");
    }

    #[test]
    fn reads_frontmatter_as_skills_write_it() {
        // a folded description (tstack's modernize), and a key under another one isn't it
        let md = "---
name: modernize
description: >-
  For getting old prs up to date. Use when asked to rebase a pr or
  \"modernize\".
metadata:
  name: not this
---
# Modernize
";
        assert_eq!(front(md, "name").as_deref(), Some("modernize"));
        assert_eq!(front(md, "description").as_deref(), Some("For getting old prs up to date. Use when asked to rebase a pr or \"modernize\"."));
        assert_eq!(front(md, "license"), None);
        // quoted, on Windows line ends
        let quoted = "---
name: \"council\"
description: 'It''s the council'
---
";
        assert_eq!(front(quoted, "name").as_deref(), Some("council"));
        assert_eq!(front(quoted, "description").as_deref(), Some("It's the council"));
        // literal, and a plain value carried on
        assert_eq!(front("---
description: |
  one
  two
---
", "description").as_deref(), Some("one
two"));
        assert_eq!(front("---
description: a long
  line here
name: x
---
", "description").as_deref(), Some("a long line here"));
        // no frontmatter, an empty value, a key that only starts the same
        assert_eq!(front("# just markdown
name: x
", "name"), None);
        assert_eq!(front("---
name:
---
", "name"), None);
        assert_eq!(front("---
names: a
---
", "name"), None);
    }

    #[test]
    fn a_spec_can_pick_one_skill() {
        assert_eq!(normalize("https://github.com/wshobson/agents#react-state-management").as_deref(), Some("wshobson/agents#react-state-management"));
        assert_eq!(normalize("wshobson/agents#/skills/x/").as_deref(), Some("wshobson/agents#skills/x"));
        assert_eq!(normalize("wshobson/agents#").as_deref(), Some("wshobson/agents"));
        assert_eq!(normalize("https://gitlab.com/me/skills.git#council").as_deref(), Some("https://gitlab.com/me/skills#council"));
        for bad in ["wshobson/agents#../x", "wshobson/agents#a b", "nope#x"] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
        let spec = "wshobson/agents#react-state-management";
        assert_eq!((base(spec), pick(spec)), ("wshobson/agents", Some("react-state-management")));
        assert_eq!(label(spec), "wshobson/agents › react-state-management");
        assert_eq!(clone_url(spec), "https://github.com/wshobson/agents.git");
        assert_eq!(web_url("o/r#skills/x"), "https://github.com/o/r/tree/HEAD/skills/x");
        assert_ne!(cache_dir(spec), cache_dir("wshobson/agents"));

        let none = |_: &str| None;
        let files = ["README.md", "plugins/frontend/skills/react-state-management/SKILL.md", "plugins/frontend/skills/react-state-management/ref.md", "plugins/other/skills/x/SKILL.md", "skills/godot/SKILL.md", "skills/unity/SKILL.md"];
        // a name, wherever it is
        assert_eq!(picked_paths("react-state-management", files, &none), ["plugins/frontend/skills/react-state-management"]);
        // a path to the skill, or to a folder of them
        assert_eq!(picked_paths("skills/godot", files, &none), ["skills/godot"]);
        assert_eq!(picked_paths("skills", files, &none), Vec::<String>::new());
        assert_eq!(picked_paths("plugins/frontend", files, &none), ["plugins/frontend/skills/react-state-management"]);
        assert_eq!(picked_paths("missing", files, &none), Vec::<String>::new());
        // a name only a SKILL.md gives (skills.sh lists them so)
        let named = |d: &str| (d == "skills/godot").then(|| "godot-gdscript".to_string());
        assert_eq!(picked_paths("godot-gdscript", files, &named), ["skills/godot"]);
        // the repo is the skill
        assert_eq!(picked_paths("tidy", ["SKILL.md", "README.md"], &none), [""]);
        // the whole repo still only looks where it always did
        assert_eq!(paths_for("o/r", files, &none), ["skills/godot", "skills/unity"]);
    }

    #[test]
    fn a_repo_with_none_in_the_usual_places_is_looked_into_deeper() {
        // one skill shipped for several agents comes once, .claude's copy
        let agents = [".agents/skills/impeccable/SKILL.md", ".claude/skills/impeccable/SKILL.md", ".cursor/skills/impeccable/SKILL.md", ".claude/skills/brand/SKILL.md"];
        assert_eq!(skill_paths(agents), [".claude/skills/brand", ".claude/skills/impeccable"]);
        let plugins = ["plugins/expo/skills/eas-hosting/SKILL.md", "plugins/expo/skills/eas-hosting/ref.md", "laravel/skills/starter-kit/SKILL.md", "archive/plugins/x/skills/old/SKILL.md"];
        assert_eq!(skill_paths(plugins), ["laravel/skills/starter-kit", "plugins/expo/skills/eas-hosting"]);
        // the usual places win: a repo's own dev skills stay out
        assert_eq!(skill_paths(["skills/council/SKILL.md", ".claude/skills/release/SKILL.md"]), ["skills/council"]);
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

    /// How much of Skill Issue's catalogue bro can install: each entry's spec checked against its repo's file
    /// list (one blob-less clone per repo). Prints what doesn't resolve.
    #[test]
    #[ignore = "network: clones every repo the marketplace lists"]
    fn the_marketplace_resolves() {
        let sb = sandbox();
        // SAFETY: the sandbox holds the env lock
        unsafe {
            std::env::set_var("BRO_SKILL_MARKET_URL", crate::skill_market::SITE);
            std::env::set_var("PATH", std::env::var_os("BRO_TEST_PATH").unwrap_or_default());
        }
        let cat = crate::skill_market::load(true).unwrap();
        let mut files: std::collections::HashMap<String, Result<Vec<String>, String>> = Default::default();
        let (mut ok, mut bad) = (0, vec![]);
        for (n, s) in cat.skills.iter().enumerate() {
            let Some(spec) = crate::skill_market::source(s) else {
                bad.push(format!("{}: no source ({})", s.id, s.link));
                continue;
            };
            let list = files.entry(base(&spec).to_lowercase()).or_insert_with(|| {
                let dir = sb.home().join(format!("r{n}"));
                let t = dir.to_string_lossy().into_owned();
                git(None, &["clone", "--depth", "1", "--filter=blob:none", "--no-checkout", "-q", &clone_url(&spec), &t])
                    .and_then(|_| git(Some(&dir), &["ls-tree", "-r", "--name-only", "HEAD"]))
                    .map(|l| l.lines().map(str::to_string).collect())
            });
            match list {
                // a name that's only in a SKILL.md resolves at install (it needs the files)
                Ok(f) if !paths_for(&spec, f.iter().map(String::as_str), &|_| None).is_empty() => ok += 1,
                Ok(_) => bad.push(format!("{}: {spec} has no such skill", s.id)),
                Err(e) => bad.push(format!("{}: {spec}: {e}", s.id)),
            }
        }
        println!("{ok} of {} resolve", cat.skills.len());
        for b in &bad {
            println!("  {b}");
        }
    }

    #[test]
    fn watching_installs_nothing_until_asked_then_keeps_what_was_installed_in_step() {
        let sb = sandbox();
        let Some(git_exe) = find("git", &search_dirs()) else { return };
        let home = sb.home().to_path_buf();
        let src = home.join("src");
        skill(&src.join("skills").join("council"), "---\nname: council\ndescription: Ask the council\n---\nv1\n");
        skill(&src.join("skills").join("full-send"), "---\nname: full-send\n---\nv1\n");
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
        let (claude, codex) = (home.join(".claude").join("skills"), home.join(".codex").join("skills"));
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        let project = home.join("code").join("app");
        std::fs::create_dir_all(&project).unwrap();
        let projects = [project.clone()];

        // watching lists its skills and installs none of them
        let first = sync(&repo, &projects).unwrap();
        assert_eq!(first.skills, ["council", "full-send"]);
        assert_eq!(first.found[0].description.as_deref(), Some("Ask the council"));
        assert!(first.commit.as_deref().is_some_and(|c| c.len() == 7), "{first:?}");
        assert!(!claude.join("council").exists() && !codex.join("council").exists());
        assert_eq!(cached(&repo).unwrap().skills, ["council", "full-send"]);

        // installed everywhere: every agent home; into a project: its .claude and .agents folders only
        let done = install(&repo, &["council".into()], &Target::Everywhere).unwrap();
        assert_eq!((done.written, done.taken), (vec!["council".to_string()], vec![]));
        assert!(claude.join("council").join("SKILL.md").is_file() && codex.join("council").join("SKILL.md").is_file());
        assert_eq!(owner(&claude.join("council")).as_deref(), Some(repo.as_str()));
        install(&repo, &["full-send".into()], &Target::Project(project.clone())).unwrap();
        let in_project = |agent: &str| project.join(agent).join("skills").join("full-send");
        assert!(in_project(".claude").join("SKILL.md").is_file() && in_project(".agents").join("SKILL.md").is_file());
        assert!(!claude.join("full-send").exists());
        assert!(install(&repo, &["nope".into()], &Target::Everywhere).unwrap_err().contains("no skill nope"));

        // upstream moves on: each installed copy follows it, and a new skill waits to be picked
        std::fs::write(src.join("skills").join("council").join("SKILL.md"), "---\nname: council\n---\nv2\n").unwrap();
        std::fs::write(src.join("skills").join("full-send").join("SKILL.md"), "---\nname: full-send\n---\nv2\n").unwrap();
        skill(&src.join("skills").join("sol-loop"), "---\nname: sol-loop\n---\n");
        run(&["add", "-A"]);
        run(&["commit", "-qm", "two"]);
        let second = sync(&repo, &projects).unwrap();
        assert_ne!(first.commit, second.commit);
        assert!(std::fs::read_to_string(claude.join("council").join("SKILL.md")).unwrap().contains("v2"));
        assert!(std::fs::read_to_string(in_project(".agents").join("SKILL.md")).unwrap().contains("v2"));
        assert!(second.skills.contains(&"sol-loop".to_string()) && !claude.join("sol-loop").exists());

        // a home that's new since gets what's installed everywhere when it launches
        let account = home.join(".claude-max-pool").join("accounts").join("work");
        std::fs::create_dir_all(&account).unwrap();
        write_cached(&account, std::slice::from_ref(&repo));
        assert!(account.join("skills").join("council").join("SKILL.md").is_file() && !account.join("skills").join("full-send").exists());

        // a skill that leaves the repo leaves every home it was installed in
        std::fs::remove_dir_all(src.join("skills").join("council")).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "three"]);
        sync(&repo, &projects).unwrap();
        assert!(!claude.join("council").exists() && !account.join("skills").join("council").exists());

        // stop watching: the clone goes, what's installed stays
        forget(&repo);
        assert!(!cache_dir(&repo).exists() && in_project(".claude").join("SKILL.md").is_file());

        // one skill picked out of a deeper layout: watching it installs nothing, installing it brings just it
        skill(&src.join("plugins").join("web").join("skills").join("react-state"), "---\nname: react-state\n---\n");
        run(&["add", "-A"]);
        run(&["commit", "-qm", "four"]);
        let pick = format!("{repo}#react-state");
        assert_eq!(sync(&pick, &[]).unwrap().skills, ["react-state"]);
        let picked = claude.join("react-state");
        assert!(!picked.exists());
        install(&pick, &[], &Target::Everywhere).unwrap();
        assert!(picked.join("SKILL.md").is_file() && !claude.join("sol-loop").exists());
        assert_eq!(owner(&picked).as_deref(), Some(pick.as_str()));
        assert!(sync(&format!("{repo}#nope"), &[]).unwrap_err().contains("no skill nope"));
    }
}
