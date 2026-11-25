//! Drawing an icon for a project that has none of its own ([`crate::project_icon`] found no picture): a signed-in
//! Claude Code or Codex takes a quick read-only look at the project and draws one symbol for it.
//!
//! Every drawn icon shares one style because bro draws most of it. The agent only draws a white line symbol on a
//! 24 x 24 grid (in the manner of Lucide icons). bro checks the symbol: [`read_reply`] keeps only plain shapes and
//! gives every one the same white 2.5-wide round stroke. [`compose`] sets it in the middle of a clear 64 x 64
//! picture. The window and the phone draw the tile behind it in the project's own colour, the colour of its name
//! and its letter tile.
//!
//! A drive or the home folder is too broad to look into, so it gets a symbol of its own (a drive, a house) without
//! an agent.
//!
//! Drawn icons are kept in `~/.bro/project-icons/made/<hash>.svg`, not in the project, so drawing one leaves
//! the repository alone. `<hash>.failed` records a failed draw, which isn't tried again for [`RETRY_AFTER`].
//! `<hash>.lock` keeps two windows from drawing the same project at once.
//!
//! [`draw`] runs the agent and blocks for up to [`TIMEOUT`], so call it off the UI thread.

use crate::Harness;
use crate::util::{atomic_write, path_key, short_hash};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// How long the agent may take over one icon.
pub const TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A project whose drawing failed isn't tried again for this long (unless you ask).
pub const RETRY_AFTER: Duration = Duration::from_secs(3 * 24 * 3600);
/// A lock older than this was left by a window that went away.
const LOCK_STALE: Duration = Duration::from_secs(10 * 60);

/// The symbol's stroke, in its 24-unit grid.
const STROKE: f32 = 2.5;
/// The most shapes a symbol may have, and the longest its markup may be: a simple symbol is far smaller.
const MAX_SHAPES: usize = 60;
const MAX_MARKUP: usize = 12_000;

/// Who draws: a signed-in Claude or Codex login, run the way the watchdog runs its reviewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artist {
    pub harness: Harness,
    /// "claude" or "codex" (looked up on PATH)
    pub program: String,
    /// the login's own environment (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) and what to drop from bro's
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub model: Option<String>,
    /// "Claude · work"
    pub label: String,
}

/// Where drawn icons are kept: `~/.bro/project-icons/made`.
pub fn dir() -> PathBuf {
    crate::paths::bro_dir().join("project-icons").join("made")
}

/// The name a project's files go by in [`dir`].
fn stem(root: &Path) -> String {
    short_hash(&path_key(root), 16)
}

/// The icon drawn for the project at `root`, if it has one. A drive or the home folder has its own symbol, kept the
/// first time it's asked for. One an older bro drew on a coloured tile of its own is brought up to date: the tile
/// behind it comes from the project's colour now.
pub fn made(root: &Path) -> Option<PathBuf> {
    let p = dir().join(format!("{}.svg", stem(root)));
    if !p.is_file() {
        return crate::project_icon::too_broad(root).then(|| store(root, &compose(&broad(root))).ok()).flatten();
    }
    if let Some(d) = std::fs::read_to_string(&p).ok().as_deref().and_then(on_old_tile) {
        let _ = atomic_write(&p, compose(&d).as_bytes());
    }
    Some(p)
}

/// The symbol for a folder too broad to be one project: a drive for a drive, a house for the home folder (or the
/// folder it's in).
fn broad(root: &Path) -> Drawing {
    let drive = root.parent().is_none_or(|p| p.as_os_str().is_empty());
    let shapes = if drive {
        r#"<path d="M2.5 13.5 5.4 6.2A2 2 0 0 1 7.3 5h9.4a2 2 0 0 1 1.9 1.2l2.9 7.3"/><rect x="2.5" y="13.5" width="19" height="6" rx="2"/><path d="M7 16.5h.01M10.5 16.5h.01"/>"#
    } else {
        r#"<path d="M3.5 11 12 4l8.5 7"/><path d="M6 9.5V19a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 18 19V9.5"/><path d="M10 20.5V15h4v5.5"/>"#
    };
    Drawing { shapes: shapes.into() }
}

/// The symbol in an icon an older bro composed on a coloured tile of its own (None for one composed now).
fn on_old_tile(svg: &str) -> Option<Drawing> {
    if !svg.contains(r#"fill="url(#tile)""#) {
        return None;
    }
    let group = svg.find(r#"<g transform="translate(12 12)"#)?;
    let start = group + svg[group..].find('>')? + 1;
    let end = svg.rfind("</g>").filter(|e| *e >= start)?;
    Some(Drawing { shapes: svg[start..end].to_string() })
}

/// Drop what was drawn for the project at `root` (and any failure noted), so it's drawn afresh.
pub fn forget(root: &Path) {
    for ext in ["svg", "failed"] {
        let _ = std::fs::remove_file(dir().join(format!("{}.{ext}", stem(root))));
    }
}

/// Drawing the project at `root` failed within [`RETRY_AFTER`].
pub fn failed_recently(root: &Path) -> bool {
    let marker = dir().join(format!("{}.failed", stem(root)));
    std::fs::metadata(marker).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < RETRY_AFTER)
}

/// Note that drawing the project at `root` failed, and why (kept for a look by hand).
pub fn note_failure(root: &Path, why: &str) {
    let text = format!("{}\n{why}\n", root.display());
    let _ = atomic_write(&dir().join(format!("{}.failed", stem(root))), text.as_bytes());
}

/// Keep `svg` as the project's drawn icon. Returns where it went.
pub fn store(root: &Path, svg: &str) -> io::Result<PathBuf> {
    let path = dir().join(format!("{}.svg", stem(root)));
    atomic_write(&path, svg.as_bytes())?;
    let _ = std::fs::remove_file(dir().join(format!("{}.failed", stem(root))));
    Ok(path)
}

/// One window drawing a project: the lock file goes when this does.
struct Lock(PathBuf);

impl Lock {
    fn take(root: &Path) -> Option<Lock> {
        let path = dir().join(format!("{}.lock", stem(root)));
        std::fs::create_dir_all(dir()).ok()?;
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Some(Lock(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_none_or(|age| age > LOCK_STALE);
                    if !stale {
                        return None;
                    }
                    let _ = std::fs::remove_file(&path);
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What the agent is asked, for the project called `name`.
pub fn prompt(name: &str) -> String {
    format!(
        "Draw the icon for the software project in this folder, \"{name}\".\n\
         \n\
         1. Take a quick look to learn what it is: its README, its package or app manifest, the names of its main folders. Read a handful of files at most, and change nothing.\n\
         2. Pick one simple, recognizable symbol for what it does or what it is about (a dumbbell for a fitness app, a speech bubble for a chat app, a leaf for a garden planner). Not a letter, not text, not a brand logo you remember.\n\
         3. Draw it as a line icon on a 24 by 24 grid, in the style of Lucide icons:\n\
         \x20  - strokes {STROKE} wide with round caps and joins, and no fill, except for small solid dots;\n\
         \x20  - 1 to 5 shapes, bold and simple enough to read at 16 pixels, centered, about 2 units in from every edge;\n\
         \x20  - only <path>, <circle>, <ellipse>, <rect>, <line>, <polyline> and <polygon>.\n\
         \x20  It's shown in white on a tile in the project's own color, so leave color out.\n\
         \n\
         Reply with the picture alone, on one line, and nothing else:\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\">your shapes</svg>\n"
    )
}

/// A symbol the agent drew, checked: its shapes (markup for a 24 x 24 grid).
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub shapes: String,
}

/// Read the agent's reply: its symbol, keeping only plain shapes in white strokes (see the module docs). Anything
/// else it says (a colour it picked anyway, a sentence) is left out.
pub fn read_reply(text: &str) -> Result<Drawing, String> {
    let lower = text.to_ascii_lowercase();
    let (first, last_open, last_close) = (lower.find("<svg"), lower.rfind("<svg"), lower.rfind("</svg>"));
    let Some(close) = last_close else {
        return Err("the agent's reply had no picture in it".into());
    };
    let end = close + "</svg>".len();
    // the whole of it (a symbol may nest an <svg>), else the last one when it gave several
    let tries = [first, last_open].into_iter().flatten().filter(|s| *s < close);
    let mut why = String::from("the agent's picture couldn't be read");
    for start in tries {
        match shapes_of(&text[start..end]) {
            Ok(shapes) => return Ok(Drawing { shapes }),
            Err(e) => why = e,
        }
    }
    Err(why)
}

/// The symbol's shapes, refit to a 24-unit grid and in white strokes.
fn shapes_of(svg: &str) -> Result<String, String> {
    let doc = roxmltree::Document::parse(svg).map_err(|e| format!("the agent's picture isn't valid SVG: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err("the agent's picture isn't an SVG".into());
    }
    let mut out = String::new();
    let mut count = 0;
    for child in root.children().filter(roxmltree::Node::is_element) {
        write_shape(child, &mut out, &mut count);
    }
    if count == 0 {
        return Err("the agent's picture had no shapes bro can use".into());
    }
    if out.len() > MAX_MARKUP || count > MAX_SHAPES {
        return Err("the agent's picture is too detailed for an icon".into());
    }
    // a grid other than 24 x 24: fitted into it, centred, keeping the stroke 2.5 wide on the 24 grid
    let view = root.attribute("viewBox").map(numbers).filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0).or_else(|| {
        let (w, h) = (root.attribute("width").map(numbers)?, root.attribute("height").map(numbers)?);
        Some(vec![0.0, 0.0, *w.first()?, *h.first()?])
    });
    match view {
        Some(v) if (v[0], v[1], v[2], v[3]) != (0.0, 0.0, 24.0, 24.0) => {
            let s = 24.0 / v[2].max(v[3]);
            let (dx, dy) = ((24.0 - v[2] * s) / 2.0 - v[0] * s, (24.0 - v[3] * s) / 2.0 - v[1] * s);
            Ok(format!("<g transform=\"translate({} {}) scale({})\" stroke-width=\"{}\">{out}</g>", num(dx), num(dy), num(s), num(STROKE / s)))
        }
        _ => Ok(out),
    }
}

/// The attributes a shape keeps (besides fill, stroke and transform).
fn geometry(tag: &str) -> Option<&'static [&'static str]> {
    Some(match tag {
        "g" => &[],
        "path" => &["d"],
        "circle" => &["cx", "cy", "r"],
        "ellipse" => &["cx", "cy", "rx", "ry"],
        "rect" => &["x", "y", "width", "height", "rx", "ry"],
        "line" => &["x1", "y1", "x2", "y2"],
        "polyline" | "polygon" => &["points"],
        _ => return None,
    })
}

/// Write `node` (a shape, or a group of them) as bro keeps it; anything else, and what's in it, is left out.
fn write_shape(node: roxmltree::Node, out: &mut String, count: &mut usize) {
    let tag = node.tag_name().name();
    let Some(keep) = geometry(tag) else { return };
    if *count > MAX_SHAPES {
        return;
    }
    *count += usize::from(tag != "g");
    out.push('<');
    out.push_str(tag);
    for name in keep.iter().chain(&["transform", "fill-rule", "clip-rule"]) {
        if let Some(v) = node.attribute(*name).filter(|v| plain(v)) {
            out.push_str(&format!(" {name}=\"{}\"", v.trim()));
        }
    }
    // fill and stroke: either none or bro's white (from the style attribute too)
    let style = node.attribute("style").unwrap_or("");
    let styled = |key: &str| style.split(';').find_map(|d| d.split_once(':').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string()));
    let fill = node.attribute("fill").map(str::to_string).or_else(|| styled("fill"));
    let stroke = node.attribute("stroke").map(str::to_string).or_else(|| styled("stroke"));
    match fill.as_deref().map(str::trim) {
        None => {}
        Some("none" | "transparent") => out.push_str(" fill=\"none\""),
        Some(_) => out.push_str(" fill=\"#fff\""),
    }
    if matches!(stroke.as_deref().map(str::trim), Some("none" | "transparent")) {
        out.push_str(" stroke=\"none\"");
    }
    if tag == "g" {
        out.push('>');
        for child in node.children().filter(roxmltree::Node::is_element) {
            write_shape(child, out, count);
        }
        out.push_str("</g>");
    } else {
        out.push_str("/>");
    }
}

/// A geometry value made of numbers, path commands and transform functions only.
fn plain(v: &str) -> bool {
    !v.trim().is_empty() && v.len() <= 8000 && v.chars().all(|c| c.is_ascii_alphanumeric() || " ,.-+()\t\r\n".contains(c))
}

/// The numbers in an attribute ("0 0 24 24", "48px").
fn numbers(v: &str) -> Vec<f32> {
    v.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).filter_map(|s| s.trim_end_matches("px").parse().ok()).collect()
}

fn num(v: f32) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The icon: `d`'s symbol in white, in the middle of a clear 64 x 64 picture. The tile behind it is drawn where it
/// shows, in the project's colour.
pub fn compose(d: &Drawing) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\" width=\"64\" height=\"64\">\
         <g transform=\"translate(12 12) scale({})\" fill=\"none\" stroke=\"#fff\" stroke-width=\"{STROKE}\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{}</g>\
         </svg>\n",
        num(40.0 / 24.0),
        d.shapes
    )
}

/// The project at `root`'s name, as the agent hears it.
fn name_of(root: &Path) -> String {
    root.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| root.display().to_string())
}

/// Have `artist` draw the project at `root` an icon and keep it ([`made`] finds it from then on): where it went,
/// or None when another bro window is drawing it. Blocks for as long as the agent takes ([`TIMEOUT`] at most).
/// Errors say why, in words for a toast.
pub fn draw(artist: &Artist, root: &Path) -> Result<Option<PathBuf>, String> {
    if !root.is_dir() {
        return Err(format!("{} isn't a folder bro can see", root.display()));
    }
    // a drive or the home folder: its own symbol, no agent needed
    if crate::project_icon::too_broad(root) {
        return store(root, &compose(&broad(root))).map(Some).map_err(|e| format!("couldn't keep the icon: {e}"));
    }
    let Some(_lock) = Lock::take(root) else {
        return Ok(None);
    };
    let name = name_of(root);
    let reply = run(artist, root, &prompt(&name))?;
    let drawing = read_reply(&reply)?;
    store(root, &compose(&drawing)).map(Some).map_err(|e| format!("couldn't keep the icon: {e}"))
}

/// Variables a session sets for its own children: a drawing agent started from inside one mustn't think it's
/// part of it.
const SESSION_VARS: [&str; 12] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
];

/// The agent's arguments: read-only, no saved session, none of your hooks or tool servers, the prompt on stdin.
/// Codex writes its answer to `answer`.
pub fn args(artist: &Artist, root: &Path, quiet_settings: &Path, answer: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![];
    match artist.harness {
        Harness::Codex => {
            a.extend(["exec", "--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--ignore-rules", "--color", "never", "-s", "read-only"].map(String::from));
            a.extend(["-C".into(), root.to_string_lossy().into_owned(), "-o".into(), answer.to_string_lossy().into_owned()]);
            a.extend(["-c".into(), "model_reasoning_effort=medium".into()]);
            if let Some(m) = artist.model.as_deref().filter(|m| !m.trim().is_empty()) {
                a.extend(["-m".into(), m.to_string()]);
            }
            a.push("-".into());
        }
        _ => {
            a.extend(["-p", "--output-format", "json", "--no-session-persistence", "--strict-mcp-config", "--permission-mode", "dontAsk", "--tools", "Read,Glob,Grep"].map(String::from));
            a.extend(["--settings".into(), quiet_settings.to_string_lossy().into_owned()]);
            if let Some(m) = artist.model.as_deref().filter(|m| !m.trim().is_empty()) {
                a.extend(["--model".into(), m.to_string()]);
            }
            // variadic: last, so it swallows nothing (the prompt comes on stdin)
            a.extend(["--allowedTools", "Read", "Glob", "Grep"].map(String::from));
        }
    }
    a
}

/// Run the agent on `prompt` in `root`: its last message.
fn run(artist: &Artist, root: &Path, prompt: &str) -> Result<String, String> {
    let stem = stem(root);
    std::fs::create_dir_all(dir()).map_err(|e| format!("couldn't make {}: {e}", dir().display()))?;
    let answer = dir().join(format!("{stem}.answer"));
    let _ = std::fs::remove_file(&answer);
    // Claude Code's settings for the run: none of your hooks (a sound, a notification) for a quiet background job
    let quiet = dir().join("quiet-claude.json");
    if !quiet.is_file() {
        atomic_write(&quiet, b"{\"disableAllHooks\": true}\n").map_err(|e| format!("couldn't write {}: {e}", quiet.display()))?;
    }
    let (program, args) = crate::launch::program_and_args(&artist.program, args(artist, root, &quiet, &answer));
    let mut cmd = Command::new(&program);
    cmd.args(&args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k == "BRO" || k.starts_with("BRO_") || SESSION_VARS.contains(&k.as_str()) {
            cmd.env_remove(&k);
        }
    }
    for k in &artist.env_remove {
        cmd.env_remove(k);
    }
    cmd.envs(artist.env.iter().map(|(k, v)| (k, v)));
    let (mut child, mut tree) = crate::proc_tree::ProcessTree::spawn(&mut cmd).map_err(|e| format!("couldn't start {}: {e}", artist.label))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let pipe = |p: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = p {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let out = pipe(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            _ => {
                tree.terminate();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    tree.terminate();
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    let Some(status) = status else {
        return Err(format!("{} took longer than {} minutes", artist.label, TIMEOUT.as_secs() / 60));
    };
    let reply = match artist.harness {
        Harness::Codex => {
            let text = std::fs::read_to_string(&answer).unwrap_or_default();
            let _ = std::fs::remove_file(&answer);
            Some(if text.trim().is_empty() { stdout.clone() } else { text })
        }
        _ => claude_result(&stdout)?,
    };
    match reply.filter(|r| !r.trim().is_empty()) {
        Some(r) => Ok(r),
        None => {
            let why = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer");
            Err(format!("{} stopped{}: {}", artist.label, status.code().map(|c| format!(" (exit code {c})")).unwrap_or_default(), clip(why, 240)))
        }
    }
}

/// Claude Code's `--output-format json` answer: its result, or what went wrong.
fn claude_result(stdout: &str) -> Result<Option<String>, String> {
    let v = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()
        .or_else(|| stdout.lines().rev().find_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok().filter(serde_json::Value::is_object)));
    let Some(v) = v else { return Ok(None) };
    let result = v.get("result").and_then(serde_json::Value::as_str).map(str::to_string);
    if v.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) {
        return Err(format!("Claude couldn't draw it: {}", clip(result.as_deref().unwrap_or("an error"), 240)));
    }
    Ok(result)
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn a_reply_becomes_white_strokes_on_a_clear_picture() {
        let reply = "Here you go:\n\ncolor: Teal\n```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\">\
            <title>dumbbell</title><path d=\"M6 5v14M18 5v14\" stroke=\"#000\" stroke-width=\"2\"/><circle cx=\"12\" cy=\"12\" r=\"1\" fill=\"currentColor\" style=\"stroke:none\"/>\
            <text x=\"2\" y=\"20\">J</text><image href=\"file:///c:/secret.png\"/><g transform=\"rotate(45 12 12)\"><line x1=\"4\" y1=\"12\" x2=\"20\" y2=\"12\" onclick=\"x()\"/></g></svg>\n```";
        let d = read_reply(reply).unwrap();
        assert_eq!(
            d.shapes,
            "<path d=\"M6 5v14M18 5v14\"/><circle cx=\"12\" cy=\"12\" r=\"1\" fill=\"#fff\" stroke=\"none\"/><g transform=\"rotate(45 12 12)\"><line x1=\"4\" y1=\"12\" x2=\"20\" y2=\"12\"/></g>",
            "shapes only: no text, pictures, titles or handlers, and bro's own stroke"
        );
        let svg = compose(&d);
        assert!(roxmltree::Document::parse(&svg).is_ok(), "{svg}");
        assert!(!svg.contains("<rect") && !svg.contains("Gradient"), "no tile of its own: the project's colour goes behind it");
        assert!(svg.contains("stroke=\"#fff\"") && svg.contains("stroke-width=\"2.5\""));
    }

    #[test]
    fn a_symbol_on_another_grid_is_fitted() {
        let d = read_reply("<svg viewBox=\"0 0 48 48\"><rect x=\"8\" y=\"8\" width=\"32\" height=\"32\" rx=\"6\"/></svg>").unwrap();
        assert!(d.shapes.starts_with("<g transform=\"translate(0 0) scale(0.5)\" stroke-width=\"5\">"), "{}", d.shapes);
        // a wide one sits centred
        let wide = read_reply("color: red\n<svg width=\"48\" height=\"24\"><path d=\"M0 0h48\"/></svg>").unwrap();
        assert!(wide.shapes.starts_with("<g transform=\"translate(0 6) scale(0.5)\""), "{}", wide.shapes);
    }

    #[test]
    fn a_reply_without_a_usable_symbol_is_an_error() {
        assert!(read_reply("I couldn't find anything to draw.").is_err());
        assert!(read_reply("<svg viewBox=\"0 0 24 24\"><text>J</text></svg>").is_err(), "letters aren't a symbol");
        assert!(read_reply("<svg><path d=\"M1 1\"></svg>").is_err(), "broken markup");
        let busy: String = (0..80).map(|i| format!("<circle cx=\"{i}\" cy=\"1\" r=\"1\"/>")).collect();
        assert!(read_reply(&format!("<svg viewBox=\"0 0 24 24\">{busy}</svg>")).is_err(), "too much for an icon");
        // geometry that isn't geometry is dropped
        let d = read_reply("<svg viewBox=\"0 0 24 24\"><path d=\"M1 1 url(javascript:x)\"/><path d=\"M2 2L4 4\"/></svg>").unwrap();
        assert_eq!(d.shapes, "<path/><path d=\"M2 2L4 4\"/>");
    }

    #[test]
    fn an_icon_drawn_on_its_own_tile_loses_the_tile() {
        let _sb = sandbox();
        let root = tempfile::tempdir().unwrap();
        // as bro 2.1.32 kept them: a teal tile under the symbol
        let old = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\" width=\"64\" height=\"64\"><defs><linearGradient id=\"tile\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#2CC9B5\"/><stop offset=\"1\" stop-color=\"#0E9384\"/></linearGradient></defs><rect width=\"64\" height=\"64\" rx=\"17\" fill=\"url(#tile)\"/><g transform=\"translate(12 12) scale(1.6667)\" fill=\"none\" stroke=\"#fff\" stroke-width=\"2.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><circle cx=\"12\" cy=\"12\" r=\"8\"/><g transform=\"rotate(45 12 12)\"><line x1=\"4\" y1=\"12\" x2=\"20\" y2=\"12\"/></g></g></svg>\n";
        let at = store(root.path(), old).unwrap();
        assert_eq!(made(root.path()), Some(at.clone()));
        let now = std::fs::read_to_string(&at).unwrap();
        let symbol = Drawing { shapes: "<circle cx=\"12\" cy=\"12\" r=\"8\"/><g transform=\"rotate(45 12 12)\"><line x1=\"4\" y1=\"12\" x2=\"20\" y2=\"12\"/></g>".into() };
        assert_eq!(now, compose(&symbol));
        // and it's left alone after that
        assert!(on_old_tile(&now).is_none());
    }

    #[test]
    fn a_drive_and_the_home_folder_get_their_own_symbols() {
        let _sb = sandbox();
        let drive = Path::new(if cfg!(windows) { "K:\\" } else { "/" });
        assert!(crate::project_icon::too_broad(drive));
        let at = made(drive).expect("drawn without an agent");
        let drive_svg = std::fs::read_to_string(&at).unwrap();
        assert!(drive_svg.contains("<rect x=\"2.5\" y=\"13.5\""), "a drive: {drive_svg}");
        let home = made(&crate::util::home_dir()).expect("the home folder too");
        assert!(std::fs::read_to_string(home).unwrap().contains("M3.5 11 12 4l8.5 7"), "a house");
        // a project of its own still waits for an agent
        let root = tempfile::tempdir().unwrap();
        assert!(made(root.path()).is_none());
        forget(drive);
    }

    #[test]
    fn drawn_icons_are_kept_outside_the_project_and_failures_wait() {
        let _sb = sandbox();
        let root = tempfile::tempdir().unwrap();
        assert!(made(root.path()).is_none());
        let svg = compose(&Drawing { shapes: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>".into() });
        let at = store(root.path(), &svg).unwrap();
        assert_eq!(made(root.path()), Some(at.clone()));
        assert!(at.starts_with(dir()), "kept in bro's folder");
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none(), "the project is left alone");
        // the same folder however it's written
        let slashed = PathBuf::from(format!("{}/", root.path().display()));
        assert_eq!(made(&slashed), Some(at));
        note_failure(root.path(), "no answer");
        assert!(failed_recently(root.path()));
        forget(root.path());
        assert!(made(root.path()).is_none() && !failed_recently(root.path()));
        // one window at a time
        let first = Lock::take(root.path()).expect("free");
        assert!(Lock::take(root.path()).is_none());
        drop(first);
        assert!(Lock::take(root.path()).is_some());
    }

    /// Draws real projects' icons with a real login, side by side (by hand, it spends usage):
    /// `BRO_DIR=<scratch> BRO_ICON_ARTIST=codex|claude BRO_ICON_PROJECTS="F:\a;F:\b" cargo test -p bro-core --lib
    /// icon_maker::tests::draws_real_projects -- --ignored --nocapture`. The machine's own login draws; the icons
    /// land in `<scratch>/project-icons/made`.
    #[test]
    #[ignore]
    fn draws_real_projects() {
        let bro = std::env::var("BRO_DIR").expect("point BRO_DIR at a scratch folder: this keeps what it draws");
        assert!(!crate::util::same_path(Path::new(&bro), &crate::util::home_dir().join(".bro")), "not your own ~/.bro");
        let codex = std::env::var("BRO_ICON_ARTIST").as_deref() == Ok("codex");
        let artist = if codex {
            Artist { harness: Harness::Codex, program: "codex".into(), env: vec![("CODEX_HOME".into(), crate::util::home_dir().join(".codex").to_string_lossy().into_owned())], env_remove: vec![], model: std::env::var("BRO_ICON_MODEL").ok(), label: "Codex".into() }
        } else {
            let gone = ["CLAUDE_CONFIG_DIR", "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL"];
            Artist { harness: Harness::Claude, program: "claude".into(), env: vec![], env_remove: gone.map(String::from).to_vec(), model: std::env::var("BRO_ICON_MODEL").ok(), label: "Claude".into() }
        };
        let projects = std::env::var("BRO_ICON_PROJECTS").expect("BRO_ICON_PROJECTS: folders, ; between");
        let runs: Vec<_> = projects
            .split(';')
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                let (artist, root) = (artist.clone(), PathBuf::from(p.trim()));
                std::thread::spawn(move || {
                    let t = Instant::now();
                    forget(&root);
                    let out = draw(&artist, &root);
                    (root, out, t.elapsed())
                })
            })
            .collect();
        for r in runs {
            let (root, out, took) = r.join().unwrap();
            println!("{} · {:.0}s · {:?}", root.display(), took.as_secs_f32(), out);
        }
    }

    #[test]
    fn the_agent_runs_read_only_and_quietly() {
        let root = Path::new("/p");
        let claude = Artist { harness: Harness::Claude, program: "claude".into(), env: vec![], env_remove: vec![], model: Some("sonnet".into()), label: "Claude".into() };
        let a = args(&claude, root, Path::new("q.json"), Path::new("a.txt")).join(" ");
        assert_eq!(a, "-p --output-format json --no-session-persistence --strict-mcp-config --permission-mode dontAsk --tools Read,Glob,Grep --settings q.json --model sonnet --allowedTools Read Glob Grep");
        let codex = Artist { harness: Harness::Codex, program: "codex".into(), model: None, label: "Codex".into(), ..claude };
        let a = args(&codex, root, Path::new("q.json"), Path::new("a.txt"));
        assert!(a.join(" ").starts_with("exec --skip-git-repo-check --ephemeral --ignore-user-config --ignore-rules --color never -s read-only -C"));
        assert_eq!(a.last().map(String::as_str), Some("-"), "the prompt on stdin");
        assert!(prompt("justgains").contains("\"justgains\""));
        assert_eq!(claude_result("{\"type\":\"result\",\"is_error\":false,\"result\":\"color: red\"}"), Ok(Some("color: red".into())));
        assert!(claude_result("{\"is_error\":true,\"result\":\"Credit balance is too low\"}").unwrap_err().contains("Credit balance"));
    }
}
