//! Resolving file references against a session's workspace and reading
//! bounded previews (`/api/sessions/{id}/file`, `links/resolve`, `open`). The
//! TUI shares [`resolve_path`] and [`read_preview`] for its own preview.
//!
//! Everything is canonicalised and must stay inside one of the session's
//! roots (cwd, project root), so symlinks and `..` cannot escape.

use base64::Engine;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Lines returned around a requested line when the client gives no `limit`.
pub const DEFAULT_LIMIT: usize = 300;
pub const MAX_LIMIT: usize = 5000;
/// Lines shown above the requested line in a centred window.
const CONTEXT_ABOVE: usize = 20;

pub const NOT_FOUND: &str = "File not found or unavailable.";
pub const OUTSIDE: &str = "This file is outside the session's workspace.";

/// Where a session's paths may resolve: its cwd and (optionally) its project root.
#[derive(Debug, Clone, Default)]
pub struct ResolveRoots {
    pub cwd: PathBuf,
    /// `SessionMeta.project`; when `None` the git root above `cwd` is used.
    pub project: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Dir => "dir",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Canonical absolute path without the Windows `\\?\` prefix.
    pub path: PathBuf,
    /// Forward-slashed, relative to the root it was found in (cwd preferred).
    pub relative: String,
    pub kind: Kind,
}

/// What part of a text file to return.
#[derive(Debug, Clone, Copy, Default)]
pub struct PreviewRequest {
    /// Line to centre on and highlight (1-based; 0/1 mean the top).
    pub line: usize,
    /// First line to return (1-based). Overrides centring.
    pub start: Option<usize>,
    /// Lines to return: default [`DEFAULT_LIMIT`], at most [`MAX_LIMIT`].
    pub limit: Option<usize>,
}

/// `\\?\C:\x` → `C:\x` (editors and humans don't want verbatim paths).
fn simplify(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path,
    }
}

fn canonical_dir(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().filter(|path| path.is_dir())
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Canonical roots, cwd first, deduplicated.
fn canonical_roots(roots: &ResolveRoots) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let cwd = (!roots.cwd.as_os_str().is_empty())
        .then(|| canonical_dir(&roots.cwd))
        .flatten();
    let project = roots
        .project
        .as_deref()
        .filter(|project| !project.as_os_str().is_empty())
        .and_then(canonical_dir)
        .or_else(|| {
            cwd.as_deref()
                .and_then(git_root)
                .and_then(|root| canonical_dir(&root))
        });
    for root in [cwd, project].into_iter().flatten() {
        if !found.contains(&root) {
            found.push(root);
        }
    }
    found
}

fn relative_text(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

/// Accepts `candidate` if it exists and canonicalises inside a root.
/// `Err(true)` when it exists but escapes every root.
fn accept(candidate: &Path, roots: &[PathBuf]) -> Result<Resolved, bool> {
    let canonical = candidate.canonicalize().map_err(|_| false)?;
    let root = roots
        .iter()
        .find(|root| canonical.starts_with(root))
        .ok_or(true)?;
    let kind = if canonical.is_dir() {
        Kind::Dir
    } else {
        Kind::File
    };
    let relative = relative_text(&canonical, root);
    Ok(Resolved {
        path: simplify(canonical),
        relative,
        kind,
    })
}

/// Normalises the spellings agents print: `file://` URLs, `~/`, MSYS `/c/x`.
fn normalize(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.chars().any(char::is_control)
        || raw.starts_with("\\\\")
        || (cfg!(windows) && raw.starts_with("//"))
    {
        return Err("Invalid file path.".into());
    }
    let mut text = if raw
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        let path = crate::file_links::file_url_path(raw).ok_or("Invalid file path.")?;
        // A fragment is a location, not part of the file name.
        path.split('#').next().unwrap_or_default().to_string()
    } else if raw.contains("://") {
        return Err("Invalid file path.".into());
    } else {
        raw.to_string()
    };
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return Ok(bro_core::util::home_dir().join(rest));
    }
    if cfg!(windows) {
        let bytes = text.as_bytes();
        if bytes.len() >= 2
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && (bytes.len() == 2 || bytes[2] == b'/')
        {
            text = format!(
                "{}:/{}",
                bytes[1].to_ascii_uppercase() as char,
                text.get(3..).unwrap_or_default()
            );
        }
    }
    Ok(PathBuf::from(text))
}

/// Section 4 of `docs/file-links.md`: absolute paths inside a root; relative
/// paths against cwd, the project root, then without a git diff `a/`/`b/`
/// prefix; finally a unique suffix match in the project's file index.
/// Blocking: may build the file index on first use.
pub fn resolve_path(roots: &ResolveRoots, raw: &str) -> Result<Resolved, String> {
    let path = normalize(raw)?;
    let roots = canonical_roots(roots);
    if roots.is_empty() {
        return Err("The session directory is unavailable.".into());
    }
    let mut outside = false;
    let try_candidate = |candidate: &Path, outside: &mut bool| match accept(candidate, &roots) {
        Ok(resolved) => Some(resolved),
        Err(escaped) => {
            *outside |= escaped;
            None
        }
    };
    if path.is_absolute() || path.has_root() {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            roots[0].join(&path)
        };
        return try_candidate(&absolute, &mut outside)
            .ok_or_else(|| if outside { OUTSIDE } else { NOT_FOUND }.to_string());
    }
    let text = path.to_string_lossy().replace('\\', "/");
    let stripped = text.strip_prefix("a/").or_else(|| text.strip_prefix("b/"));
    for relative in std::iter::once(path.as_path()).chain(stripped.map(Path::new)) {
        for root in &roots {
            if let Some(found) = try_candidate(&root.join(relative), &mut outside) {
                return Ok(found);
            }
        }
    }
    if outside {
        return Err(OUTSIDE.into());
    }
    let climbs = path
        .components()
        .any(|part| matches!(part, Component::ParentDir));
    if !climbs {
        let index_root = roots.last().expect("non-empty");
        let hits = crate::file_search::suffix_matches(&index_root.to_string_lossy(), &text, 2);
        if let [hit] = hits.as_slice()
            && let Some(found) = try_candidate(&index_root.join(&hit.path), &mut outside)
        {
            return Ok(found);
        }
    }
    Err(NOT_FOUND.into())
}

/// Lowercase language id from the file name (see `docs/file-links.md`).
pub fn language_for(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" | "containerfile" => return "dockerfile",
        "makefile" | "gnumakefile" => return "makefile",
        "cargo.lock" => return "toml",
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return "shell",
        ".gitignore" | ".dockerignore" | ".env" => return "ini",
        _ => {}
    }
    if name.starts_with("dockerfile.") {
        return "dockerfile";
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or_default();
    match extension {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "json" | "jsonc" | "json5" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" | "mdx" => "markdown",
        "py" | "pyi" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "css" => "css",
        "scss" | "sass" => "scss",
        "html" | "htm" => "html",
        "xml" | "xaml" | "csproj" | "props" | "targets" | "plist" | "svg" => "xml",
        "sql" => "sql",
        "sh" | "bash" | "zsh" | "fish" => "shell",
        "ps1" | "psm1" | "psd1" => "powershell",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "diff" | "patch" => "diff",
        "ini" | "cfg" | "conf" | "properties" | "env" => "ini",
        _ => "plaintext",
    }
}

/// Video/audio types: previewed by URL (`/raw`), never inlined, no size cap.
pub fn media_mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        _ => return None,
    })
}

pub fn image_mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Reads a bounded preview of a resolved file: `kind: "text"` with a line
/// window, or `kind: "image"` with base64 data. Errors are user-facing.
pub fn read_preview(
    roots: &ResolveRoots,
    target: &str,
    request: PreviewRequest,
) -> Result<Value, String> {
    let resolved = resolve_path(roots, target)?;
    if resolved.kind != Kind::File {
        return Err("This path is not a file.".into());
    }
    let file = File::open(&resolved.path).map_err(|_| "File could not be opened.")?;
    let metadata = file
        .metadata()
        .map_err(|_| "File could not be inspected.")?;
    let display_path = resolved.path.to_string_lossy().into_owned();
    let extension = resolved
        .path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if let Some(mime) = media_mime(&extension) {
        // The route adds `url` (`/raw`); the bytes are streamed, not inlined.
        let kind = if mime.starts_with("video/") {
            "video"
        } else {
            "audio"
        };
        return Ok(json!({
            "path": display_path, "relativePath": resolved.relative, "kind": kind,
            "mimeType": mime, "bytes": metadata.len(),
        }));
    }
    if metadata.len() > MAX_BYTES {
        return Err("This file is too large to preview (2 MB limit).".into());
    }
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|_| "File could not be read.")?;
    if data.len() as u64 > MAX_BYTES {
        return Err("This file is too large to preview (2 MB limit).".into());
    }
    if let Some(mime) = image_mime(&extension) {
        return Ok(json!({
            "path": display_path, "relativePath": resolved.relative, "kind": "image", "mimeType": mime,
            "bytes": data.len(), "data": base64::engine::general_purpose::STANDARD.encode(&data),
        }));
    }
    if data.contains(&0) {
        return Err("This binary file cannot be previewed.".into());
    }
    let bytes = data.len();
    let text = String::from_utf8(data)
        .map_err(|_| "This file is not UTF-8 text and cannot be previewed.")?;
    let mut lines: Vec<_> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop(); // the final newline does not start another line
    }
    let language = language_for(&resolved.path);
    let line = request.line.max(1).min(lines.len());
    let limit = request
        .limit
        .unwrap_or(if language == "markdown" && request.start.is_none() {
            MAX_LIMIT
        } else {
            DEFAULT_LIMIT
        })
        .clamp(1, MAX_LIMIT);
    let start = match request.start {
        Some(start) => start.max(1).min(lines.len()) - 1,
        None if language == "markdown" => 0,
        None => line.saturating_sub(CONTEXT_ABOVE + 1),
    };
    let end = (start + limit).min(lines.len());
    let selected = lines[start..end].join("\n");
    let max_chars = (limit * 400).max(120_000);
    let content: String = selected.chars().take(max_chars).collect();
    let returned = content.split('\n').count();
    Ok(json!({
        "path": display_path, "relativePath": resolved.relative, "kind": "text", "language": language,
        "line": line, "startLine": start + 1, "endLine": start + returned, "totalLines": lines.len(),
        "truncated": start > 0 || end < lines.len() || content.len() < selected.len(),
        "bytes": bytes, "content": content,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(cwd: &Path) -> ResolveRoots {
        ResolveRoots {
            cwd: cwd.to_path_buf(),
            project: None,
        }
    }

    fn at(line: usize) -> PreviewRequest {
        PreviewRequest {
            line,
            ..Default::default()
        }
    }

    #[test]
    fn preview_resolves_paths_and_centres_line_references() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file with spaces.rs");
        std::fs::write(
            &file,
            (1..=500)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let preview = read_preview(&roots(root.path()), "file with spaces.rs", at(100)).unwrap();
        assert_eq!(preview["line"], 100);
        assert_eq!(preview["startLine"], 80);
        assert_eq!(preview["endLine"], 379);
        assert_eq!(preview["totalLines"], 500);
        assert_eq!(preview["truncated"], true);
        assert_eq!(preview["language"], "rust");
        assert_eq!(preview["relativePath"], "file with spaces.rs");
        assert!(preview["content"].as_str().unwrap().contains("line 100"));
        assert!(!preview["path"].as_str().unwrap().starts_with(r"\\?\"));
        assert!(read_preview(&roots(root.path()), file.to_str().unwrap(), at(1)).is_ok());
        assert!(
            read_preview(&roots(root.path()), ".", at(1))
                .unwrap_err()
                .contains("not a file")
        );
        assert!(read_preview(&roots(root.path()), "missing", at(1)).is_err());
    }

    #[test]
    fn paging_and_markdown_windows() {
        let root = tempfile::tempdir().unwrap();
        let body = (1..=6000)
            .map(|n| format!("l{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(root.path().join("a.txt"), &body).unwrap();
        std::fs::write(root.path().join("notes.md"), &body).unwrap();
        let page = read_preview(
            &roots(root.path()),
            "a.txt",
            PreviewRequest {
                line: 1,
                start: Some(301),
                limit: Some(10),
            },
        )
        .unwrap();
        assert_eq!(
            (page["startLine"].as_u64(), page["endLine"].as_u64()),
            (Some(301), Some(310))
        );
        assert_eq!(
            page["content"].as_str().unwrap().lines().next(),
            Some("l301")
        );
        let capped = read_preview(
            &roots(root.path()),
            "a.txt",
            PreviewRequest {
                line: 1,
                start: Some(1),
                limit: Some(99_999),
            },
        )
        .unwrap();
        assert_eq!(capped["endLine"], 5000);
        let markdown = read_preview(&roots(root.path()), "notes.md", at(4000)).unwrap();
        assert_eq!(
            (
                markdown["startLine"].as_u64(),
                markdown["endLine"].as_u64(),
                markdown["line"].as_u64()
            ),
            (Some(1), Some(5000), Some(4000))
        );
        assert_eq!(markdown["language"], "markdown");
    }

    #[test]
    fn media_is_described_not_inlined_and_has_no_size_cap() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("clip.MOV"),
            vec![0u8; MAX_BYTES as usize + 10],
        )
        .unwrap();
        std::fs::write(root.path().join("song.flac"), [1u8, 2]).unwrap();
        let video = read_preview(&roots(root.path()), "clip.MOV", at(1)).unwrap();
        assert_eq!(
            (video["kind"].as_str(), video["mimeType"].as_str()),
            (Some("video"), Some("video/quicktime"))
        );
        assert_eq!(video["bytes"], MAX_BYTES + 10);
        assert!(video.get("data").is_none() && video.get("content").is_none());
        let audio = read_preview(&roots(root.path()), "song.flac", at(1)).unwrap();
        assert_eq!(
            (audio["kind"].as_str(), audio["mimeType"].as_str()),
            (Some("audio"), Some("audio/flac"))
        );
    }

    #[test]
    fn images_include_svg_and_text_types_are_detected() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("logo.svg"),
            "<svg xmlns='http://www.w3.org/2000/svg'/>",
        )
        .unwrap();
        let image = read_preview(&roots(root.path()), "logo.svg", at(1)).unwrap();
        assert_eq!(
            (image["kind"].as_str(), image["mimeType"].as_str()),
            (Some("image"), Some("image/svg+xml"))
        );
        assert_eq!(language_for(Path::new("Dockerfile")), "dockerfile");
        assert_eq!(language_for(Path::new("x/App.TSX")), "tsx");
        assert_eq!(language_for(Path::new("run.ps1")), "powershell");
        assert_eq!(language_for(Path::new("LICENSE")), "plaintext");
    }

    #[test]
    fn preview_rejects_files_outside_workspace_and_binary_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let r = roots(root.path());
        assert!(
            read_preview(&r, outside.path().to_str().unwrap(), at(1))
                .unwrap_err()
                .contains("outside")
        );
        assert!(read_preview(&r, "../x", at(1)).is_err());
        std::fs::write(root.path().join("binary"), [0u8, 1, 2]).unwrap();
        assert!(
            read_preview(&r, "binary", at(1))
                .unwrap_err()
                .contains("binary")
        );
        std::fs::write(
            root.path().join("large"),
            vec![b'a'; MAX_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            read_preview(&r, "large", at(1))
                .unwrap_err()
                .contains("too large")
        );
        for bad in [
            "",
            "a\u{7}b",
            r"\\server\share\x",
            "https://example.com/a.rs",
        ] {
            assert!(resolve_path(&r, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn resolution_order_project_root_diff_prefix_urls_and_suffix() {
        let project = tempfile::tempdir().unwrap();
        let cwd = project.path().join("crates/app");
        std::fs::create_dir_all(cwd.join("src")).unwrap();
        std::fs::write(cwd.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(project.path().join("README.md"), "# hi").unwrap();
        std::fs::create_dir_all(project.path().join("docs/deep")).unwrap();
        std::fs::write(project.path().join("docs/deep/unique_name.txt"), "x").unwrap();
        std::fs::write(project.path().join("docs/a.txt"), "x").unwrap();
        std::fs::write(cwd.join("a.txt"), "x").unwrap();
        let r = ResolveRoots {
            cwd: cwd.clone(),
            project: Some(project.path().to_path_buf()),
        };

        let found = resolve_path(&r, "src/main.rs").unwrap();
        assert_eq!(
            (found.relative.as_str(), found.kind),
            ("src/main.rs", Kind::File)
        );
        assert_eq!(resolve_path(&r, "README.md").unwrap().relative, "README.md");
        assert_eq!(
            resolve_path(&r, "b/src/main.rs").unwrap().relative,
            "src/main.rs"
        );
        assert_eq!(
            resolve_path(&r, "./src/main.rs").unwrap().relative,
            "src/main.rs"
        );
        assert_eq!(resolve_path(&r, "src").unwrap().kind, Kind::Dir);
        let url =
            url::Url::from_file_path(simplify(cwd.join("src/main.rs").canonicalize().unwrap()))
                .unwrap();
        assert_eq!(
            resolve_path(&r, &format!("{url}#L3")).unwrap().relative,
            "src/main.rs"
        );
        // Unique suffix match through the (walked, non-git) project index.
        assert_eq!(
            resolve_path(&r, "deep/unique_name.txt").unwrap().relative,
            "docs/deep/unique_name.txt"
        );
        assert_eq!(
            resolve_path(&r, "unique_name.txt").unwrap().relative,
            "docs/deep/unique_name.txt"
        );
        // cwd wins over the project root for the same relative path.
        assert_eq!(
            resolve_path(&r, "a.txt").unwrap().path,
            simplify(cwd.join("a.txt").canonicalize().unwrap())
        );
        assert_eq!(
            resolve_path(&r, "nope/unique_name.txt").unwrap_err(),
            NOT_FOUND
        );
    }

    #[test]
    fn ambiguous_suffixes_and_git_root_fallback() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        std::fs::create_dir_all(project.path().join("one")).unwrap();
        std::fs::create_dir_all(project.path().join("two")).unwrap();
        std::fs::create_dir_all(project.path().join("sub")).unwrap();
        std::fs::write(project.path().join("one/mod.rs"), "").unwrap();
        std::fs::write(project.path().join("two/mod.rs"), "").unwrap();
        std::fs::write(project.path().join("top.txt"), "").unwrap();
        // No project given: the git root above cwd is the second root.
        let r = roots(&project.path().join("sub"));
        assert_eq!(resolve_path(&r, "top.txt").unwrap().relative, "top.txt");
        assert_eq!(
            resolve_path(&r, "mod.rs").unwrap_err(),
            NOT_FOUND,
            "ambiguous"
        );
        assert_eq!(
            resolve_path(&r, "two/mod.rs").unwrap().relative,
            "two/mod.rs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("link"))
            .unwrap();
        assert_eq!(
            resolve_path(&roots(root.path()), "link").unwrap_err(),
            OUTSIDE
        );
    }
}
