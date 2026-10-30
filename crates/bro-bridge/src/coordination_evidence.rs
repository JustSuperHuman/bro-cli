//! Bounded host observations for coordination. Call on a blocking worker without authority locks.
//! Capture fails when a tree cannot be completely identified; check execution never invokes a shell.
use bro_core::coordination::{Artifact, ContentEntry, digest_bytes, digest_manifest};
use bro_core::proc_tree::ProcessTree;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant};

const FILE_BYTES: u64 = 256 * 1024 * 1024;
const TREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TREE_FILES: usize = 50_000;
const CAPTURE_TIME: Duration = Duration::from_secs(120);
const OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const LIST_BYTES: usize = 16 * 1024 * 1024;
const SOURCE_LIMIT: usize = 128;

#[derive(Debug)]
pub(crate) struct Captured {
    pub digest: String,
    pub artifacts: Vec<Artifact>,
    pub sources: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct CheckOutput {
    pub passed: bool,
    pub exit_code: i32,
    /// Deterministic stdout/stderr framing; the digest covers these exact reviewable bytes.
    pub output: Vec<u8>,
    pub digest: String,
}

fn error(context: &str, path: &Path, error: impl std::fmt::Display) -> String {
    format!("{context} {}: {error}", path.display())
}
fn timely(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("Content capture exceeded 120 seconds; no incomplete digest was accepted.".into())
    } else {
        Ok(())
    }
}
fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn stable(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len()
        && a.modified()
            .ok()
            .zip(b.modified().ok())
            .is_some_and(|(a, b)| a == b)
        && a.created().ok() == b.created().ok()
}

fn checked_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "Content path must be a relative path without parent traversal: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Reject interior links/junctions, including links whose current target appears safe: their target
/// can change independently of a worker's workspace reservation. Capture their isolated workspace instead.
fn checked_path(root: &Path, relative: &Path, allow_missing: bool) -> Result<PathBuf, String> {
    checked_relative(relative)?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(format!(
                    "Content capture refuses a symlink or reparse point: {}. Use an isolated regular-file workspace.",
                    path.display()
                ));
            }
            Ok(_) => {}
            Err(e) if allow_missing && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(root.join(relative));
            }
            Err(e) => return Err(error("Inspect content", &path, e)),
        }
    }
    Ok(path)
}

fn artifact_relative(workspace: &Path, requested: &Path) -> Result<PathBuf, String> {
    if requested
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Artifact paths cannot contain parent traversal.".into());
    }
    if !requested.is_absolute() {
        checked_relative(requested)?;
        return Ok(requested.to_path_buf());
    }
    let workspace = bro_core::util::normalize_path(workspace);
    let requested = bro_core::util::normalize_path(requested);
    let base: Vec<_> = workspace.components().collect();
    let all: Vec<_> = requested.components().collect();
    let matches = all.len() > base.len()
        && base.iter().zip(&all).all(|(a, b)| {
            #[cfg(windows)]
            {
                a.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
            }
            #[cfg(not(windows))]
            {
                a == b
            }
        });
    if !matches {
        return Err(format!(
            "Artifact lies outside its workspace: {}",
            requested.display()
        ));
    }
    let relative: PathBuf = all[base.len()..].iter().map(|c| c.as_os_str()).collect();
    checked_relative(&relative)?;
    Ok(relative)
}

fn hash_regular(path: &Path, deadline: Instant) -> Result<(String, u64), String> {
    timely(deadline)?;
    let before = fs::symlink_metadata(path).map_err(|e| error("Inspect file", path, e))?;
    if is_link(&before) || !before.is_file() {
        return Err(format!("A regular file is required: {}", path.display()));
    }
    if before.len() > FILE_BYTES {
        return Err(format!(
            "File exceeds the 256 MiB capture limit: {}",
            path.display()
        ));
    }
    let mut file = File::open(path).map_err(|e| error("Open file", path, e))?;
    let opened = file
        .metadata()
        .map_err(|e| error("Inspect open file", path, e))?;
    if !stable(&before, &opened) {
        return Err(format!(
            "File changed while capture opened it: {}",
            path.display()
        ));
    }
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        timely(deadline)?;
        let read = file
            .read(&mut chunk)
            .map_err(|e| error("Read file", path, e))?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or("file size overflow")?;
        if total > FILE_BYTES {
            return Err(format!(
                "File grew beyond the 256 MiB capture limit: {}",
                path.display()
            ));
        }
        hash.update(&chunk[..read]);
    }
    let after = fs::symlink_metadata(path).map_err(|e| error("Reinspect file", path, e))?;
    if is_link(&after) || !stable(&before, &after) || before.len() != total {
        return Err(format!(
            "File changed while it was being hashed: {}",
            path.display()
        ));
    }
    Ok((format!("{:x}", hash.finalize()), total))
}

pub(crate) fn hash_file(path: &Path) -> Result<(String, u64), String> {
    hash_regular(path, Instant::now() + CAPTURE_TIME)
}

fn utf8_path(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or_else(|| {
        format!(
            "Non-UTF-8 paths require a separate workspace capture: {}",
            path.display()
        )
    })?;
    #[cfg(windows)]
    {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(format!("//{}", rest.replace('\\', "/")));
        }
        Ok(text
            .strip_prefix(r"\\?\")
            .unwrap_or(text)
            .replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        Ok(text.into())
    }
}

fn git_marker(workspace: &Path) -> Result<Option<PathBuf>, String> {
    for dir in workspace.ancestors() {
        match fs::symlink_metadata(dir.join(".git")) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(format!(
                    "Git metadata is a symlink or reparse point: {}",
                    dir.display()
                ));
            }
            Ok(_) => return Ok(Some(dir.to_path_buf())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error("Inspect repository", dir, e)),
        }
    }
    Ok(None)
}

fn git(workspace: &Path, args: &[&str], deadline: Instant) -> Result<Vec<u8>, String> {
    timely(deadline)?;
    let executable = resolve_program(workspace, "git")?;
    let mut command = Command::new(executable);
    command
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(workspace);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(key);
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    let output = execute(
        command,
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(30)),
        LIST_BYTES,
    )?;
    if !output.status.success() {
        return Err(format!(
            "Cannot enumerate the complete Git tree: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

/// A bounded direct Git operation used by the optional managed-worktree adapter.
pub(crate) fn git_output(
    workspace: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    git(workspace, args, Instant::now() + timeout)
}

fn git_paths(root: &Path, deadline: Instant) -> Result<BTreeSet<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    for args in [
        &["ls-files", "-z", "--cached"][..],
        &["ls-files", "-z", "--others", "--exclude-standard"][..],
    ] {
        let output = git(root, args, deadline)?;
        if !output.is_empty() && output.last() != Some(&0) {
            return Err("Git path list was incomplete; capture refused.".into());
        }
        for path in output.split(|&b| b == 0).filter(|s| !s.is_empty()) {
            let path = std::str::from_utf8(path)
                .map_err(|_| "Git returned a non-UTF-8 path; capture refused.")?;
            let path = PathBuf::from(path);
            checked_relative(&path)?;
            paths.insert(path);
            if paths.len() > TREE_FILES {
                return Err(
                    "Git tree exceeds the 50,000-file capture limit; isolate a smaller workspace."
                        .into(),
                );
            }
        }
    }
    Ok(paths)
}

fn ordinary_paths(root: &Path, deadline: Instant) -> Result<BTreeSet<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    let mut directories = vec![root.to_path_buf()];
    let mut entries = 0usize;
    while let Some(dir) = directories.pop() {
        timely(deadline)?;
        let relative = dir.strip_prefix(root).map_err(|e| e.to_string())?;
        if relative.components().count() > 128 {
            return Err("Non-Git tree exceeds the 128-level directory limit.".into());
        }
        for entry in fs::read_dir(&dir).map_err(|e| error("List directory", &dir, e))? {
            timely(deadline)?;
            let entry = entry.map_err(|e| error("Read directory entry", &dir, e))?;
            entries += 1;
            if entries > TREE_FILES {
                return Err("Non-Git tree exceeds the 50,000-entry capture limit; nothing was silently excluded.".into());
            }
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|e| error("Inspect tree entry", &path, e))?;
            if is_link(&metadata) {
                return Err(format!(
                    "Non-Git capture refuses a symlink or reparse point: {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() {
                paths.insert(
                    path.strip_prefix(root)
                        .map_err(|e| e.to_string())?
                        .to_path_buf(),
                );
                if paths.len() > TREE_FILES {
                    return Err("Non-Git tree exceeds the 50,000-file capture limit; nothing was silently excluded.".into());
                }
            } else {
                return Err(format!(
                    "Special filesystem entry cannot be captured: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(paths)
}

fn policy_paths(
    root: &Path,
    files: &BTreeSet<PathBuf>,
    artifacts: &[PathBuf],
) -> Result<BTreeSet<PathBuf>, String> {
    let mut directories: BTreeSet<PathBuf> = root.ancestors().map(Path::to_path_buf).collect();
    for file in files
        .iter()
        .map(|p| root.join(p))
        .chain(artifacts.iter().cloned())
    {
        for dir in file.parent().into_iter().flat_map(Path::ancestors) {
            directories.insert(dir.to_path_buf());
            if dir == root {
                break;
            }
        }
    }
    let mut policies = BTreeSet::new();
    for dir in directories {
        for name in [
            "AGENTS.md",
            "CLAUDE.md",
            ".claude/CLAUDE.md",
            ".codex/AGENTS.md",
            ".agents/AGENTS.md",
        ] {
            let path = checked_path(&dir, Path::new(name), true)?;
            match fs::symlink_metadata(&path) {
                Ok(metadata) if is_link(&metadata) => {
                    return Err(format!(
                        "Applicable instruction file is a symlink or reparse point: {}",
                        path.display()
                    ));
                }
                Ok(metadata) if metadata.is_file() => {
                    policies.insert(path);
                }
                Ok(_) => {
                    return Err(format!(
                        "Applicable instruction path is not a regular file: {}",
                        path.display()
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(error("Inspect applicable instructions", &path, e)),
            }
        }
    }
    Ok(policies)
}

pub(crate) fn capture(workspace: &Path, artifacts: &[Artifact]) -> Result<Captured, String> {
    let deadline = Instant::now() + CAPTURE_TIME;
    let workspace =
        fs::canonicalize(workspace).map_err(|e| error("Resolve workspace", workspace, e))?;
    if !workspace.is_dir() {
        return Err("Content workspace must be a directory.".into());
    }
    if artifacts.len() > 128 {
        return Err("A result exceeds the 128-artifact capture limit.".into());
    }
    let repository = git_marker(&workspace)?;
    let root = if let Some(root) = repository {
        let actual = git(&workspace, &["rev-parse", "--show-toplevel"], deadline)?;
        let actual = std::str::from_utf8(&actual)
            .map_err(|_| "Git repository root is not UTF-8")?
            .trim_end_matches(['\r', '\n']);
        let actual = fs::canonicalize(actual).map_err(|e| error("Resolve Git root", &root, e))?;
        if !workspace.starts_with(&actual) {
            return Err("Git root does not contain the requested workspace.".into());
        }
        actual
    } else {
        workspace.clone()
    };
    let is_git = git_marker(&root)?.is_some();
    let files = if is_git {
        git_paths(&root, deadline)?
    } else {
        ordinary_paths(&root, deadline)?
    };
    let mut manifest = Vec::new();
    let mut all_sources = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut total = 0u64;
    let mut observed: Vec<(PathBuf, Option<Metadata>)> = Vec::new();
    for relative in &files {
        timely(deadline)?;
        let path = checked_path(&root, relative, is_git)?;
        let name = utf8_path(relative)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let (sha256, bytes) = hash_regular(&path, deadline)?;
                total = total.checked_add(bytes).ok_or("tree byte total overflow")?;
                if total > TREE_BYTES {
                    return Err(
                        "Tree exceeds the 2 GiB capture limit; isolate a smaller workspace.".into(),
                    );
                }
                manifest.push(ContentEntry {
                    path: format!("file:{name}"),
                    sha256: sha256.clone(),
                    bytes,
                    deleted: false,
                });
                // Executable mode changes matter on Unix even when file bytes stay equal.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = metadata.permissions().mode() & 0o111;
                    manifest.push(ContentEntry {
                        path: format!("mode:{name}"),
                        sha256: digest_bytes(mode.to_string().as_bytes()),
                        bytes: 0,
                        deleted: false,
                    });
                }
                all_sources.insert(utf8_path(&path)?, sha256);
                let after = fs::symlink_metadata(&path)
                    .map_err(|e| error("Reinspect captured file", &path, e))?;
                if !stable(&metadata, &after) {
                    return Err(format!(
                        "File changed while capture identified it: {}",
                        path.display()
                    ));
                }
                observed.push((path, Some(metadata)));
            }
            Ok(_) => {
                return Err(format!(
                    "Git tree entry is not a regular file (submodules and directory links require their own capture): {}",
                    path.display()
                ));
            }
            Err(e) if is_git && e.kind() == std::io::ErrorKind::NotFound => {
                manifest.push(ContentEntry {
                    path: format!("file:{name}"),
                    sha256: String::new(),
                    bytes: 0,
                    deleted: true,
                });
                observed.push((path, None));
            }
            Err(e) => return Err(error("Inspect captured file", &path, e)),
        }
    }
    let mut artifact_paths = Vec::new();
    let mut captured_artifacts = Vec::new();
    let mut artifact_ids = BTreeSet::new();
    for artifact in artifacts {
        if artifact.id.is_empty() || artifact.id.len() > 200 || !artifact_ids.insert(&artifact.id) {
            return Err("Artifact IDs must be nonempty, bounded and unique.".into());
        }
        let relative = artifact_relative(&workspace, Path::new(&artifact.path))?;
        let path = checked_path(&workspace, &relative, false)?;
        let metadata =
            fs::symlink_metadata(&path).map_err(|e| error("Inspect artifact", &path, e))?;
        let (sha256, bytes) = hash_regular(&path, deadline)?;
        total = total
            .checked_add(bytes)
            .ok_or("capture byte total overflow")?;
        if total > TREE_BYTES {
            return Err(
                "Tree and artifact reads exceed the 2 GiB capture limit; split the task.".into(),
            );
        }
        if !artifact.sha256.is_empty() && !artifact.sha256.eq_ignore_ascii_case(&sha256) {
            return Err(format!(
                "Artifact {} hash differs from the actual file bytes.",
                artifact.id
            ));
        }
        if artifact.bytes != 0 && artifact.bytes != bytes {
            return Err(format!(
                "Artifact {} byte length differs from the actual file.",
                artifact.id
            ));
        }
        manifest.push(ContentEntry {
            path: format!("artifact:{}:{}", artifact.id, utf8_path(&path)?),
            sha256: sha256.clone(),
            bytes,
            deleted: false,
        });
        sources.insert(utf8_path(&path)?, sha256.clone());
        artifact_paths.push(path.clone());
        observed.push((path.clone(), Some(metadata)));
        let mut artifact = artifact.clone();
        artifact.path = utf8_path(&path)?;
        artifact.sha256 = sha256;
        artifact.bytes = bytes;
        captured_artifacts.push(artifact);
    }
    let policies = policy_paths(&root, &files, &artifact_paths)?;
    for path in &policies {
        timely(deadline)?;
        let metadata =
            fs::symlink_metadata(path).map_err(|e| error("Inspect instructions", path, e))?;
        let (sha256, bytes) = hash_regular(&path, deadline)?;
        total = total
            .checked_add(bytes)
            .ok_or("capture byte total overflow")?;
        if total > TREE_BYTES {
            return Err(
                "Tree and instruction reads exceed the 2 GiB capture limit; split the task.".into(),
            );
        }
        let name = utf8_path(&path)?;
        manifest.push(ContentEntry {
            path: format!("policy:{name}"),
            sha256: sha256.clone(),
            bytes,
            deleted: false,
        });
        sources.insert(name, sha256);
        observed.push((path.clone(), Some(metadata)));
    }
    let after = if is_git {
        git_paths(&root, deadline)?
    } else {
        ordinary_paths(&root, deadline)?
    };
    if files != after {
        return Err(
            "Workspace file membership changed during capture; retry after the writer stops."
                .into(),
        );
    }
    if policies != policy_paths(&root, &after, &artifact_paths)? {
        return Err(
            "Applicable instruction files changed during capture; retry after policy edits finish."
                .into(),
        );
    }
    for (path, before) in observed {
        timely(deadline)?;
        match (before, fs::symlink_metadata(&path)) {
            (None, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            (Some(before), Ok(after))
                if !is_link(&after) && after.is_file() && stable(&before, &after) =>
            {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if before.permissions().mode() != after.permissions().mode() {
                        return Err(format!(
                            "File mode changed during capture: {}",
                            path.display()
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "Captured file changed before the complete tree was identified: {}",
                    path.display()
                ));
            }
        }
    }
    let digest = digest_manifest(&manifest).map_err(|e| e.to_string())?;
    for artifact in &mut captured_artifacts {
        artifact.source_digest = digest.clone();
    }
    sources.insert(format!("workspace:{}", utf8_path(&root)?), digest.clone());
    if sources.len() > SOURCE_LIMIT {
        return Err("Applicable policy and artifact sources exceed the 128-source contract limit; split the task.".into());
    }
    if sources.len() + all_sources.len() <= SOURCE_LIMIT {
        sources.extend(all_sources);
    }
    Ok(Captured {
        digest,
        artifacts: captured_artifacts,
        sources,
    })
}

/// Deliberately small argv grammar: quotes and literal escapes, no expansion or shell operators.
/// Windows backslashes are preserved except the documented escaped quote/backslash forms.
fn parse_command(input: &str) -> Result<Vec<String>, String> {
    if input.is_empty() || input.len() > 8192 || input.contains(['\0', '\n', '\r']) {
        return Err("Check command must be one bounded explicit command line.".into());
    }
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => { if c == '\'' { quote = None; } else { word.push(c); } }
            Some('"') => {
                if c == '"' { quote = None; }
                else if c == '\\' {
                    let mut count = 1;
                    while chars.peek() == Some(&'\\') { chars.next(); count += 1; }
                    if chars.peek() == Some(&'"') {
                        chars.next(); word.extend(std::iter::repeat_n('\\', count / 2));
                        if count % 2 == 1 { word.push('"'); } else { quote = None; }
                    } else { word.extend(std::iter::repeat_n('\\', count)); }
                }
                else { word.push(c); }
            }
            _ => match c {
                '"' | '\'' => { quote = Some(c); started = true; }
                ' ' | '\t' => { if started { args.push(std::mem::take(&mut word)); started = false; } }
                '|' | '&' | ';' | '<' | '>' | '`' | '$' => return Err("Shell operators, substitutions and chaining are not accepted in check commands. Use one executable with explicit arguments.".into()),
                '\\' if chars.peek().is_some_and(|c| matches!(c, '"' | '\'' | ' ' | '\t')) => { word.push(chars.next().unwrap()); started = true; }
                _ => { word.push(c); started = true; }
            },
        }
    }
    if quote.is_some() {
        return Err("Check command has an unterminated quote.".into());
    }
    if started {
        args.push(word);
    }
    if args.is_empty() || args[0].is_empty() || args.len() > 256 {
        return Err("Check command requires one executable and at most 255 arguments.".into());
    }
    Ok(args)
}

fn resolve_program(workspace: &Path, program: &str) -> Result<PathBuf, String> {
    let path = Path::new(program);
    let explicit = path.is_absolute() || program.contains(['/', '\\']);
    let candidates: Vec<PathBuf> = if explicit {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace.join(path)
        };
        if path.is_file() {
            vec![path]
        } else if cfg!(windows)
            && path.extension().is_none()
            && path.with_extension("exe").is_file()
        {
            vec![path.with_extension("exe")]
        } else {
            vec![]
        }
    } else {
        // Ignore empty/relative PATH entries: they otherwise resolve against mutable task content.
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .filter(|dir| dir.is_absolute())
            .filter_map(|dir| {
                let path = dir.join(program);
                if path.is_file() && (!cfg!(windows) || path.extension().is_some()) {
                    Some(path)
                } else if cfg!(windows)
                    && path.extension().is_none()
                    && path.with_extension("exe").is_file()
                {
                    Some(path.with_extension("exe"))
                } else {
                    None
                }
            })
            .collect()
    };
    let mut inaccessible = None;
    for candidate in candidates {
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(e) if !explicit => {
                inaccessible = Some(error("Skip inaccessible executable alias", &candidate, e));
                continue;
            }
            Err(e) => return Err(error("Resolve explicit executable", &candidate, e)),
        };
        let name = canonical
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let ext = canonical
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "cmd" | "bat" | "ps1" | "sh")
            || matches!(
                name.as_str(),
                "cmd" | "sh" | "bash" | "zsh" | "dash" | "fish" | "wscript" | "cscript"
            )
        {
            return Err("Implicit shell commands and shims are not accepted. Name the native checker or use an explicit script-file interpreter (PowerShell -File is supported).".into());
        }
        #[cfg(windows)]
        if !matches!(ext.as_str(), "exe" | "com") {
            return Err("Windows checks require a native .exe or .com executable; shell shims are not accepted.".into());
        }
        // Keep the resolved invocation name: rustup/npm-style native launchers may select their
        // tool from argv[0]. The canonical target above was used to detect interpreter aliases.
        return Ok(candidate);
    }
    Err(inaccessible.unwrap_or_else(|| format!("Executable was not found on the absolute PATH: {program}. Use a native executable or explicit interpreter with script path.")))
}

fn explicit_powershell_script(
    workspace: &Path,
    executable: &Path,
    args: &[String],
) -> Result<Option<PathBuf>, String> {
    let name = executable
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if !matches!(name.as_str(), "powershell" | "pwsh") {
        return Ok(None);
    }
    let mut index = 0;
    while index < args.len() {
        match args[index].to_ascii_lowercase().as_str() {
            "-noprofile" | "-noninteractive" | "-nologo" => index += 1,
            "-executionpolicy" => {
                let value = args.get(index + 1).ok_or("PowerShell execution policy needs its explicit value")?.to_ascii_lowercase();
                if !matches!(value.as_str(), "allsigned" | "bypass" | "default" | "remotesigned" | "restricted" | "undefined" | "unrestricted") { return Err("Unknown explicit PowerShell execution policy.".into()); }
                index += 2;
            }
            "-file" => {
                let script = args.get(index + 1).ok_or("PowerShell -File requires its explicit script path")?;
                let relative = artifact_relative(workspace, Path::new(script))?;
                let path = checked_path(workspace, &relative, false)?;
                if !path.extension().is_some_and(|s| s.eq_ignore_ascii_case("ps1")) { return Err("PowerShell -File check requires a .ps1 script in its workspace.".into()); }
                return Ok(Some(path));
            }
            _ => return Err("PowerShell checks require explicit -File with optional -NoProfile/-NonInteractive/-NoLogo/-ExecutionPolicy. Inline or encoded command execution is not accepted.".into()),
        }
    }
    Err("PowerShell checks require an explicit -File script path.".into())
}

struct Executed {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
enum Pipe {
    Chunk(bool, Vec<u8>),
    End,
    Error(String),
}
fn reader(mut pipe: impl Read + Send + 'static, stdout: bool, tx: SyncSender<Pipe>) {
    let mut bytes = [0u8; 16384];
    loop {
        match pipe.read(&mut bytes) {
            Ok(0) => {
                let _ = tx.send(Pipe::End);
                return;
            }
            Ok(n) => {
                if tx.send(Pipe::Chunk(stdout, bytes[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                let _ = tx.send(Pipe::Error(e.to_string()));
                return;
            }
        }
    }
}

fn execute(
    mut command: Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<Executed, String> {
    if timeout.is_zero() || timeout > Duration::from_secs(3600) {
        return Err("Check timeout must be greater than zero and at most one hour.".into());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut tree) =
        ProcessTree::spawn(&mut command).map_err(|e| format!("Start direct check: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("check stdout pipe was not created")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("check stderr pipe was not created")?;
    let (tx, rx) = mpsc::sync_channel(4);
    let out = std::thread::Builder::new()
        .name("bro-check-stdout".into())
        .spawn({
            let tx = tx.clone();
            move || reader(stdout, true, tx)
        })
        .map_err(|e| e.to_string())?;
    let err = std::thread::Builder::new()
        .name("bro-check-stderr".into())
        .spawn(move || reader(stderr, false, tx))
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut status = None;
    let mut ended = 0;
    let mut output = Executed {
        status: dummy_status(),
        stdout: vec![],
        stderr: vec![],
    };
    let mut failure = None;
    while ended < 2 || status.is_none() {
        if started.elapsed() >= timeout {
            failure = Some(format!(
                "Check timed out after {} ms; its owned process tree was terminated.",
                timeout.as_millis()
            ));
            break;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    tree.terminate();
                }
                Ok(None) => {}
                Err(e) => {
                    failure = Some(format!("Observe check process: {e}"));
                    break;
                }
            }
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Pipe::Chunk(stdout, bytes)) => {
                if output.stdout.len() + output.stderr.len() + bytes.len() > output_limit {
                    failure = Some(format!(
                        "Check output exceeded the {}-byte limit; no truncated passing result was accepted.",
                        output_limit
                    ));
                    break;
                }
                if stdout {
                    output.stdout.extend(bytes);
                } else {
                    output.stderr.extend(bytes);
                }
            }
            Ok(Pipe::End) => ended += 1,
            Ok(Pipe::Error(e)) => {
                failure = Some(format!("Read check output: {e}"));
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) if ended < 2 => {
                failure = Some("Check output readers ended without complete output.".into());
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {}
        }
    }
    tree.terminate();
    if status.is_none() {
        let _ = child.kill();
        let reap_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < reap_deadline {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    failure.get_or_insert_with(|| format!("Reap check process: {error}"));
                    break;
                }
            }
        }
        if status.is_none() {
            failure = Some(
                "Check process did not confirm exit after termination; no result was accepted."
                    .into(),
            );
        }
    }
    drop(rx); // Wake bounded-channel readers before joining after a failure.
    let cleanup = Instant::now() + Duration::from_secs(2);
    while (!out.is_finished() || !err.is_finished()) && Instant::now() < cleanup {
        std::thread::sleep(Duration::from_millis(5));
    }
    if !out.is_finished() || !err.is_finished() {
        return Err("Check descendants kept an output pipe open after process-tree cleanup; output is incomplete.".into());
    }
    if out.join().is_err() || err.join().is_err() {
        return Err("Check output reader failed.".into());
    }
    if let Some(failure) = failure {
        return Err(failure);
    }
    output.status = status.ok_or("check had no observed process exit status")?;
    Ok(output)
}

fn dummy_status() -> ExitStatus {
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
}

pub(crate) fn run_check(
    workspace: &Path,
    command: &str,
    timeout: Duration,
) -> Result<CheckOutput, String> {
    let workspace =
        fs::canonicalize(workspace).map_err(|e| error("Resolve check workspace", workspace, e))?;
    if !workspace.is_dir() {
        return Err("Check workspace is not a directory.".into());
    }
    let args = parse_command(command)?;
    let executable = resolve_program(&workspace, &args[0])?;
    let script = explicit_powershell_script(&workspace, &executable, &args[1..])?;
    let script_before = script.as_ref().map(|p| hash_file(p)).transpose()?;
    let mut child = Command::new(executable);
    child.args(&args[1..]).current_dir(&workspace);
    let result = execute(child, timeout, OUTPUT_BYTES)?;
    if let Some(script) = &script {
        if script_before.as_ref() != Some(&hash_file(script)?) {
            return Err(
                "Verifier script changed during its execution; no result was accepted.".into(),
            );
        }
    }
    let exit_code = result.status.code().unwrap_or(-1);
    let mut output = Vec::with_capacity(result.stdout.len() + result.stderr.len() + 32);
    if let Some((digest, _)) = script_before {
        output.extend_from_slice(format!("scriptSha256:{digest}\n").as_bytes());
    }
    output.extend_from_slice(b"stdout:\n");
    output.extend(result.stdout);
    output.extend_from_slice(b"\nstderr:\n");
    output.extend(result.stderr);
    let digest = digest_bytes(&output);
    Ok(CheckOutput {
        passed: result.status.success() && exit_code == 0,
        exit_code,
        output,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn git_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }
    fn add(dir: &Path) {
        assert!(
            Command::new("git")
                .args(["add", "--all"])
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        );
    }
    fn artifact(path: &str) -> Artifact {
        Artifact {
            id: "output".into(),
            kind: "research".into(),
            path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn git_identity_covers_dirty_untracked_deleted_and_ignored_policy() {
        let dir = git_fixture();
        fs::write(dir.path().join("tracked.txt"), "original").unwrap();
        fs::write(dir.path().join("deleted.txt"), "retained until deleted").unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\nAGENTS.md\n").unwrap();
        add(dir.path());
        let clean = capture(dir.path(), &[]).unwrap();
        fs::write(dir.path().join("tracked.txt"), "modified").unwrap();
        let dirty = capture(dir.path(), &[]).unwrap();
        assert_ne!(clean.digest, dirty.digest);
        fs::write(dir.path().join("untracked.txt"), "untracked result").unwrap();
        let untracked = capture(dir.path(), &[]).unwrap();
        assert_ne!(dirty.digest, untracked.digest);
        fs::remove_file(dir.path().join("deleted.txt")).unwrap();
        let deleted = capture(dir.path(), &[]).unwrap();
        assert_ne!(untracked.digest, deleted.digest);
        fs::write(dir.path().join("ignored.txt"), "generated output").unwrap();
        assert_eq!(
            deleted.digest,
            capture(dir.path(), &[]).unwrap().digest,
            "ordinary ignored bytes are outside the Git tree contract"
        );
        fs::write(
            dir.path().join("AGENTS.md"),
            "Required repository instructions",
        )
        .unwrap();
        let policy = capture(dir.path(), &[]).unwrap();
        assert_ne!(deleted.digest, policy.digest);
        assert!(policy.sources.keys().any(|p| p.ends_with("/AGENTS.md")));
    }

    #[test]
    fn artifacts_are_hashed_from_actual_bytes_and_mismatches_refused() {
        let dir = git_fixture();
        fs::write(dir.path().join("report.txt"), "source-backed findings").unwrap();
        let captured = capture(dir.path(), &[artifact("report.txt")]).unwrap();
        let observed = &captured.artifacts[0];
        assert_eq!(observed.sha256, digest_bytes(b"source-backed findings"));
        assert_eq!(observed.bytes, 22);
        assert_eq!(observed.source_digest, captured.digest);
        assert_eq!(
            captured.digest,
            capture(dir.path(), &captured.artifacts).unwrap().digest
        );
        fs::write(dir.path().join("report.txt"), "revised findings").unwrap();
        assert!(
            capture(dir.path(), &captured.artifacts)
                .unwrap_err()
                .contains("hash differs")
        );
    }

    #[test]
    fn artifact_paths_cannot_escape_and_empty_non_git_workspace_is_identifiable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            capture(dir.path(), &[]).unwrap().digest,
            capture(dir.path(), &[]).unwrap().digest
        );
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(capture(dir.path(), &[artifact(&outside.path().to_string_lossy())]).is_err());
        assert!(capture(dir.path(), &[artifact("../escaped.txt")]).is_err());
        fs::write(dir.path().join("findings.txt"), "research").unwrap();
        let absolute = artifact(&dir.path().join("findings.txt").to_string_lossy());
        assert!(
            capture(dir.path(), &[absolute]).is_ok(),
            "normal absolute Windows paths match a canonical workspace"
        );
    }

    #[test]
    fn symlink_or_reparse_capture_fails_instead_of_following_outside() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(outside.path(), dir.path().join("escape"));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(outside.path(), dir.path().join("escape"));
        if linked.is_err() {
            return;
        } // Windows developer-mode privilege may be unavailable.
        assert!(
            capture(dir.path(), &[])
                .unwrap_err()
                .contains("symlink or reparse")
        );
    }

    #[test]
    fn parser_preserves_quoted_windows_paths_and_rejects_chaining() {
        assert_eq!(parse_command(r#""C:\Program Files\checker.exe" --name "one two" "C:\folder\\" "literal & argument""#).unwrap(),
            vec!["C:\\Program Files\\checker.exe", "--name", "one two", "C:\\folder\\", "literal & argument"]);
        for command in [
            "checker && other",
            "checker | other",
            "checker;other",
            "checker > output",
            "checker $(other)",
            "checker\nother",
            "\"unterminated",
        ] {
            assert!(parse_command(command).is_err(), "{command}");
        }
    }

    const WORKER: &str = "coordination_evidence::tests::check_worker";
    fn worker_command() -> String {
        format!(
            "\"{}\" --exact {WORKER} --ignored --nocapture",
            std::env::current_exe().unwrap().display()
        )
    }

    #[test]
    #[ignore = "test process used by the direct verifier tests"]
    fn check_worker() {
        let cwd = std::env::current_dir().unwrap();
        if std::env::var_os("BRO_COORDINATION_EVIDENCE_CHILD").is_some() {
            for i in 0..10000 {
                fs::write(cwd.join("heartbeat"), i.to_string()).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
            return;
        }
        let mode = fs::read_to_string(cwd.join("check-mode")).unwrap();
        match mode.as_str() {
            "pass" => {
                println!("real verification passed");
                eprintln!("stderr is also captured");
            }
            "fail" => panic!("real verification failure"),
            "sleep" => std::thread::sleep(Duration::from_secs(120)),
            "flood" => {
                let bytes = vec![b'x'; 64 * 1024];
                for _ in 0..128 {
                    std::io::stdout().write_all(&bytes).unwrap();
                }
            }
            "tree" => {
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--exact", WORKER, "--ignored", "--nocapture"])
                    .env("BRO_COORDINATION_EVIDENCE_CHILD", "1");
                // The descendant inherits the pipes; cleanup must not wait forever for their EOF.
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x0800_0000);
                }
                let mut child = command.spawn().unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cwd.join("heartbeat").exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let _ = child.wait();
            }
            _ => panic!("unknown test mode"),
        }
    }

    #[test]
    fn direct_checks_record_real_pass_failure_exit_and_complete_output() {
        let dir = tempfile::tempdir().unwrap();
        let command = worker_command();
        fs::write(dir.path().join("check-mode"), "pass").unwrap();
        let result = run_check(dir.path(), &command, Duration::from_secs(10)).unwrap();
        assert!(result.passed);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.digest, digest_bytes(&result.output));
        let output = String::from_utf8_lossy(&result.output);
        assert!(
            output.contains("real verification passed")
                && output.contains("stderr is also captured")
        );
        fs::write(dir.path().join("check-mode"), "fail").unwrap();
        let result = run_check(dir.path(), &command, Duration::from_secs(10)).unwrap();
        assert!(!result.passed);
        assert_ne!(result.exit_code, 0);
        assert!(
            run_check(
                dir.path(),
                &format!("{command} && echo forged"),
                Duration::from_secs(1)
            )
            .is_err()
        );
        let shell = if cfg!(windows) {
            "cmd.exe /c echo forged"
        } else {
            "/bin/sh -c 'echo forged'"
        };
        assert!(run_check(dir.path(), shell, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn explicit_powershell_script_is_bound_to_actual_script_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let Some(powershell) = resolve_program(dir.path(), "pwsh")
            .or_else(|_| resolve_program(dir.path(), "powershell"))
            .ok()
        else {
            return;
        };
        let script = dir.path().join("verify.ps1");
        let contents = b"Write-Output 'verified by explicit script'; exit 0";
        fs::write(&script, contents).unwrap();
        let command = format!(
            "\"{}\" -NoProfile -NonInteractive -File \"{}\"",
            powershell.display(),
            script.display()
        );
        let result = run_check(dir.path(), &command, Duration::from_secs(15)).unwrap();
        assert!(result.passed);
        assert!(
            String::from_utf8_lossy(&result.output)
                .contains(&format!("scriptSha256:{}", digest_bytes(contents)))
        );
        let inline = format!(
            "\"{}\" -NoProfile -Command \"exit 0\"",
            powershell.display()
        );
        assert!(run_check(dir.path(), &inline, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn timeout_output_cap_and_owned_descendants_are_failures_without_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let command = worker_command();
        fs::write(dir.path().join("check-mode"), "sleep").unwrap();
        let start = Instant::now();
        assert!(
            run_check(dir.path(), &command, Duration::from_millis(150))
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        fs::write(dir.path().join("check-mode"), "flood").unwrap();
        assert!(
            run_check(dir.path(), &command, Duration::from_secs(10))
                .unwrap_err()
                .contains("output exceeded")
        );
        fs::write(dir.path().join("check-mode"), "tree").unwrap();
        assert!(run_check(dir.path(), &command, Duration::from_secs(1)).is_err());
        assert!(
            dir.path().join("heartbeat").exists(),
            "descendant ran before the timeout"
        );
        let before = fs::read(dir.path().join("heartbeat")).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            before,
            fs::read(dir.path().join("heartbeat")).unwrap(),
            "owned descendant must stop writing after cleanup"
        );
    }
}
