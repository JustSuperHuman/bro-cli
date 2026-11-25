//! A project's icon: the picture the window's sidebar and project tabs show for it.
//!
//! What you choose lives in the project itself, in its `.bro` folder, so it outlasts closing the project and
//! travels with the repository:
//! - `.bro/icon.<svg|png|ico|webp|jpg|jpeg|gif>`: its picture (choosing one copies the file there);
//! - `.bro/project.json` `{"icon": "letter"}`: the letter tile, whatever pictures the project has.
//!
//! Without either, [`resolve`] looks for one, and ranks what it finds:
//! 1. an icon in the nearest `.bro` folder further in (an app inside a monorepo can carry its own);
//! 2. a picture named for the project (`justgains-app-icon.png`, `cpfloans-icon-light.png`, `cpf-logo.svg`);
//! 3. the platforms' names for a site's or an app's icon: an SVG favicon, `apple-touch-icon`, `icon`,
//!    `android-chrome-512`, then a raster favicon;
//! 4. a plain logo;
//!
//! then the letter tile. Nearer beats deeper, an SVG or PNG beats an ICO, and a copy in build output or a picture
//! in docs, examples or tests ranks after the same picture elsewhere. Social networks' icons, layers, splashes and
//! wide lockups never count. The look runs on ripgrep's walker: in parallel, past whatever the project's `.gitignore`
//! leaves out, a few folders in, and never through a whole drive or the home folder. It reads the disk: call it off
//! the UI thread.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The picture formats an icon can be, best first.
pub const EXTENSIONS: [&str; 7] = ["svg", "png", "webp", "ico", "jpg", "jpeg", "gif"];

/// The folder inside a project that holds what bro keeps for it.
pub const DIR: &str = ".bro";

/// How far into a project the look goes: pictures in folders this many levels in (the project's own is 0).
const MAX_DEPTH: usize = 4;
/// The most entries one look reads, and the longest it takes: a huge folder gives up rather than holds things up.
const MAX_ENTRIES: usize = 80_000;
const MAX_TIME: Duration = Duration::from_millis(2500);
/// A favicon this big is something else.
const MAX_FOUND_BYTES: u64 = 4 << 20;
/// The most a chosen picture may weigh (it's copied into the project).
const MAX_SET_BYTES: u64 = 8 << 20;

/// Folders never looked in: dependencies, caches, other programs' data. Hidden folders (but `.bro`) are skipped too,
/// and whatever the project's `.gitignore` leaves out.
const SKIP: [&str; 20] = [
    "AppData",
    "Application Data",
    "node_modules",
    "bower_components",
    "jspm_packages",
    "vendor",
    "target",
    "__pycache__",
    "venv",
    "env",
    "site-packages",
    "Pods",
    "DerivedData",
    "obj",
    "coverage",
    "tmp",
    "temp",
    "logs",
    "Library",
    "Carthage",
];

/// What a project's icon is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    /// the picture you chose: `.bro/icon.*` in the project's own folder
    Set(PathBuf),
    /// a picture found further in: a nested `.bro` folder's, or a favicon, app icon or logo
    Found(PathBuf),
    /// one drawn for it, as nothing was found ([`crate::icon_maker`]; never from [`resolve`], which leaves the
    /// choice of showing it to the caller)
    Made(PathBuf),
    /// the letter tile: you asked for it (`asked`), or the project has no picture
    Letter { asked: bool },
}

impl Icon {
    /// The picture to show, if it's one.
    pub fn picture(&self) -> Option<&Path> {
        match self {
            Icon::Set(p) | Icon::Found(p) | Icon::Made(p) => Some(p),
            Icon::Letter { .. } => None,
        }
    }
}

/// The project at `root`'s icon, as the sidebar shows it.
pub fn resolve(root: &Path) -> Icon {
    if asks_letter(root) {
        return Icon::Letter { asked: true };
    }
    if let Some(p) = chosen(root) {
        return Icon::Set(p);
    }
    match scan(root).into_iter().next() {
        Some(c) => Icon::Found(c.path),
        None => Icon::Letter { asked: false },
    }
}

/// Pictures in the project that would make a good icon, best first (a nested `.bro` folder's, then favicons and
/// app icons, then logos; nearest first): what the sidebar offers when you choose one. At most `limit`.
pub fn candidates(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = chosen(root).into_iter().collect();
    for c in scan(root) {
        if v.len() >= limit {
            break;
        }
        if !v.contains(&c.path) {
            v.push(c.path);
        }
    }
    v.truncate(limit);
    v
}

/// The picture chosen for the project at `root`: its `.bro/icon.*`.
pub fn chosen(root: &Path) -> Option<PathBuf> {
    let dir = root.join(DIR);
    if is_bro_home(&dir) {
        return None;
    }
    picture_in(&dir, &["icon"])
}

/// Use `picture` as the project's icon: it's copied to `<root>/.bro/icon.<ext>` (replacing the one chosen before),
/// and the letter tile is no longer asked for. Returns where it went.
pub fn set(root: &Path, picture: &Path) -> io::Result<PathBuf> {
    let ext = ext_of(picture).filter(|e| EXTENSIONS.contains(&e.as_str())).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "bro shows SVG, PNG, ICO, WebP, JPEG and GIF pictures as icons")
    })?;
    let meta = std::fs::metadata(picture)?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "that's a folder, not a picture"));
    }
    if meta.len() > MAX_SET_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "that picture is over 8 MB: pick a smaller one"));
    }
    let dir = root.join(DIR);
    if is_bro_home(&dir) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "this folder is bro's own home: its icon can't be kept here"));
    }
    let bytes = std::fs::read(picture)?;
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("icon.{ext}"));
    crate::util::atomic_write(&dest, &bytes)?;
    for other in EXTENSIONS.iter().filter(|e| **e != ext) {
        let _ = std::fs::remove_file(dir.join(format!("icon.{other}")));
    }
    write_pref(root, None)?;
    Ok(dest)
}

/// Show the letter tile for the project at `root`, whatever pictures it has (a chosen one stays in `.bro`, unused).
pub fn use_letter(root: &Path) -> io::Result<()> {
    write_pref(root, Some("letter"))
}

/// Back to finding the project's icon by itself: the chosen picture and the letter preference go (and `.bro`
/// with them when nothing else is in it).
pub fn reset(root: &Path) -> io::Result<()> {
    let dir = root.join(DIR);
    if is_bro_home(&dir) {
        return Ok(());
    }
    for ext in EXTENSIONS {
        let p = dir.join(format!("icon.{ext}"));
        if p.is_file() {
            std::fs::remove_file(&p)?;
        }
    }
    write_pref(root, None)?;
    // only removes it when empty
    let _ = std::fs::remove_dir(&dir);
    Ok(())
}

/// `<root>/.bro/project.json` asks for the letter tile.
pub fn asks_letter(root: &Path) -> bool {
    read_config(root).get("icon").and_then(Value::as_str) == Some("letter")
}

fn config_path(root: &Path) -> PathBuf {
    root.join(DIR).join("project.json")
}

fn read_config(root: &Path) -> Map<String, Value> {
    std::fs::read_to_string(config_path(root)).ok().and_then(|s| serde_json::from_str::<Map<String, Value>>(&s).ok()).unwrap_or_default()
}

/// Set (or with None drop) `icon` in `.bro/project.json`, keeping whatever else is there. A file left empty goes.
fn write_pref(root: &Path, icon: Option<&str>) -> io::Result<()> {
    let path = config_path(root);
    if is_bro_home(&root.join(DIR)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "this folder is bro's own home: its icon can't be kept here"));
    }
    let mut config = read_config(root);
    match icon {
        Some(v) => {
            config.insert("icon".into(), Value::String(v.into()));
        }
        None => {
            if config.remove("icon").is_none() {
                return Ok(());
            }
        }
    }
    if config.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(root.join(DIR))?;
    let text = serde_json::to_string_pretty(&Value::Object(config)).map_err(io::Error::other)?;
    crate::util::atomic_write(&path, format!("{text}\n").as_bytes())
}

/// Where the window keeps each project's icon for its next start, inside bro's home: one small PNG per project next
/// to an index keyed by the project's key ([`crate::projects::ProjectKey::key`]). The bridge reads the same index to
/// show the phone what the window shows.
pub const KEPT_DIR: &str = "project-icons";

/// One project's icon as kept in [`KEPT_DIR`]'s index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// "set", "found", "made", "letter" or "asked" (the letter you chose)
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    /// the source's modification time when it was kept
    #[serde(default)]
    pub stamp: u64,
    /// the picture, next to the index
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// what the picture needs behind it: "opaque" (it fills its tile), "dark" or "light" (marks on clear), "mid", or
    /// "symbol" (a drawn white symbol: a tile in the project's colour)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    /// how the window fitted the picture (it trims margins, say): a picture fitted by an older window is fitted again
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fit: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Kept {
    /// The kept picture's file name, when the project shows a picture (not its letter).
    pub fn picture(&self) -> Option<&str> {
        if !matches!(self.kind.as_str(), "set" | "found" | "made") {
            return None;
        }
        self.file.as_deref().filter(|f| is_kept_name(f))
    }
}

/// The index of what's kept in `dir` ([`KEPT_DIR`]).
pub fn kept_index(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

/// What's kept in `dir`: nothing when the index is missing or unreadable.
pub fn read_kept(dir: &Path) -> HashMap<String, Kept> {
    std::fs::read(kept_index(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// A kept picture's name: 16 hex digits and `.png`, so a name read from the index can't point anywhere else.
pub fn is_kept_name(name: &str) -> bool {
    name.strip_suffix(".png").is_some_and(|stem| stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `dir` is bro's own home (`~/.bro`): a project at the home folder doesn't get its icon from there.
fn is_bro_home(dir: &Path) -> bool {
    crate::util::same_path(dir, &crate::paths::bro_dir())
}

fn ext_of(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// The first picture in `dir` named one of `stems` (in that order), any case, best format first.
fn picture_in(dir: &Path, stems: &[&str]) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut found: Vec<(usize, usize, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_string_lossy().to_ascii_lowercase();
            let s = stems.iter().position(|w| *w == stem)?;
            let f = EXTENSIONS.iter().position(|x| Some(*x) == ext_of(&p).as_deref())?;
            Some((s, f, p))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(.., p)| p)
}

/// A picture the look found, and how good an icon it makes (lower is better).
#[derive(Debug)]
struct Candidate {
    score: usize,
    path: PathBuf,
}

/// Words that keep a picture's other words from mattering: the size (`32x32`, `512`, `2x`).
fn is_size(w: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(w) || w.strip_suffix('x').is_some_and(digits) || w.split_once('x').is_some_and(|(a, b)| digits(a) && digits(b))
}

/// Versions of the same mark: `-dark`, `-ink`, `-round`, `-512`…
const VARIANTS: [&str; 34] = [
    "dark", "light", "ink", "paper", "mono", "monochrome", "color", "colour", "colored", "round", "rounded", "circle", "square", "maskable", "white", "black", "app", "main", "default", "primary",
    "filled", "fill", "outline", "transparent", "small", "large", "big", "hd", "new", "alt", "inverse", "inverted", "gradient", "solid",
];

/// Words that name a mark.
const MARKS: [&str; 9] = ["icon", "favicon", "logo", "logomark", "mark", "symbol", "glyph", "emblem", "brand"];

/// Words that say a picture isn't the project's mark: someone else's (a social network, a store), a piece of one (a
/// layer, a splash, a sprite sheet), or too wide for a square (a lockup, a wordmark, a banner).
const NOT_A_MARK: [&str; 38] = [
    "lockup", "wordmark", "banner", "text", "horizontal", "wide", "og", "social", "cover", "hero", "splash", "screenshot", "preview", "background", "bg", "foreground", "sheet", "sprite",
    "sprites", "notification", "badge", "store", "placeholder", "loading", "spinner", "facebook", "twitter", "x", "instagram", "linkedin", "youtube", "tiktok", "google", "github",
    "discord", "slack", "whatsapp", "telegram",
];

/// How good a picture named `stem` is as the icon of a project named `name` (its letters and digits, lower case):
/// lower is better, None when it isn't one. A picture named for the project (`justgains-app-icon`,
/// `cpfloans-icon-light`, `cpf-logo`) first, then the platforms' names for a site's or an app's icon (an SVG
/// favicon, `apple-touch-icon`, `icon`, `android-chrome-512`, a raster favicon), then a plain logo.
fn name_score(stem: &str, name: &str, svg: bool) -> Option<usize> {
    let s = stem.to_ascii_lowercase();
    let words: Vec<&str> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // the plain mark before its versions ("cpfloans-icon" before "cpfloans-icon-light")
    let versions = words.iter().filter(|w| VARIANTS.contains(w) && !matches!(**w, "app" | "main" | "default" | "primary")).count();
    raw_score(&words, name, svg).map(|n| n + versions)
}

fn raw_score(words: &[&str], name: &str, svg: bool) -> Option<usize> {
    let first = *words.first()?;
    let plain = |from: usize| words[from..].iter().all(|w| is_size(w) || VARIANTS.contains(w));
    if words.starts_with(&["apple", "touch", "icon"]) && words[3..].iter().all(|w| is_size(w) || *w == "precomposed") {
        return Some(22);
    }
    if words.starts_with(&["android", "chrome"]) && plain(2) {
        return Some(25);
    }
    if words.iter().any(|w| NOT_A_MARK.contains(w)) {
        return None;
    }
    if first == "favicon" && plain(1) {
        return Some(if svg { 20 } else { 26 });
    }
    if (first == "icon" || first == "appicon") && plain(1) || words.starts_with(&["app", "icon"]) && plain(2) {
        return Some(23);
    }
    // named for the project: its name (or the start of it, three letters at least) and then only marks and versions
    if name.len() >= 3 {
        let tail = |i: usize| words[i..].iter().all(|w| is_size(w) || VARIANTS.contains(w) || MARKS.contains(w));
        if let Some(i) = (1..=words.len()).find(|&i| tail(i)) {
            let lead = words[..i].concat();
            let marks: Vec<&str> = words[i..].iter().copied().filter(|w| MARKS.contains(w)).collect();
            // the name alone counts only as the whole name ("worldswire", not "claudia-01-color")
            if lead.len() >= 3 && name.starts_with(&lead) && (!marks.is_empty() || lead == name && i == words.len()) {
                let iconic = marks.iter().any(|m| matches!(*m, "icon" | "favicon")) || words[i..].contains(&"app");
                return Some(if iconic { 10 } else if marks.is_empty() { 12 } else { 14 } + if lead == name { 0 } else { 2 });
            }
        }
    }
    if MARKS[2..].contains(&first) && words[1..].iter().all(|w| is_size(w) || VARIANTS.contains(w) || *w == "icon") {
        return Some(if words.contains(&"icon") { 36 } else { 40 });
    }
    None
}

/// Folders whose pictures are copies (build output) or asides (docs, examples, tests): what's in them ranks after
/// the same picture elsewhere.
const OUTPUT: [&str; 10] = ["dist", "build", "out", "output", ".output", "www", "_site", "release", "artifacts", "storybook-static"];
const ASIDE: [&str; 17] = ["docs", "doc", "example", "examples", "test", "tests", "__tests__", "fixtures", "fixture", "samples", "sample", "demo", "demos", "templates", "template", "third_party", "stories"];

/// Folders of pictures that are never a project's mark: social cards, screenshots, textures, video frames.
const NOT_MARKS: [&str; 14] = ["og", "social", "screenshots", "screenshot", "previews", "banners", "covers", "thumbnails", "thumbs", "textures", "frames", "sprites", "emoji", "flags"];

/// The picture at `path` (`depth` entries below `root`) as a candidate for the icon of a project named `name`.
fn candidate(root: &Path, path: &Path, depth: usize, name: &str) -> Option<Candidate> {
    let ext = ext_of(path)?;
    let format = EXTENSIONS.iter().position(|x| *x == ext)?;
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let in_bro = path.parent().and_then(Path::file_name).is_some_and(|n| n == DIR);
    // folders between the project and the picture
    let level = depth.saturating_sub(1);
    let score = if in_bro {
        // the project's own `.bro` is what you chose ([`chosen`]); one further in comes before anything else
        if level <= 1 || !matches!(stem.to_ascii_lowercase().as_str(), "icon" | "favicon" | "logo") || is_bro_home(path.parent()?) {
            return None;
        }
        level
    } else {
        let base = name_score(&stem, name, ext == "svg")?;
        let mut aside = 0;
        if let Some(dirs) = path.strip_prefix(root).ok().and_then(Path::parent) {
            for d in dirs.components() {
                let d = d.as_os_str().to_string_lossy().to_ascii_lowercase();
                if NOT_MARKS.contains(&d.as_str()) {
                    return None;
                }
                aside += if OUTPUT.contains(&d.as_str()) {
                    15
                } else if ASIDE.contains(&d.as_str()) || d.starts_with('_') {
                    8
                } else {
                    0
                };
            }
        }
        base + level * 4 + format + aside
    };
    let size = std::fs::metadata(path).ok()?.len();
    (size > 0 && size <= MAX_FOUND_BYTES).then(|| Candidate { score, path: path.to_path_buf() })
}

/// A project's name as pictures are named for it: its letters and digits, lower case ("bro-cli-v2": "brocliv2").
fn name_of(root: &Path) -> String {
    root.file_name().map(|n| n.to_string_lossy().chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()).unwrap_or_default()
}

/// Folders too broad to have an icon of their own: a drive, the home folder or one above it. A look through one
/// would take long and come back with someone else's.
pub fn too_broad(root: &Path) -> bool {
    let home = crate::util::home_dir();
    root.parent().is_none_or(|p| p.as_os_str().is_empty()) || crate::util::same_path(root, &home) || home.starts_with(root)
}

/// Folders the look goes into.
fn enters(name: &str) -> bool {
    name == DIR || !(name.starts_with('.') || SKIP.contains(&name) || name.starts_with("target-") || name.starts_with("target_"))
}

/// Every picture in the project that would make an icon, best first. ripgrep's walker reads the folders in
/// parallel and leaves out what the project's `.gitignore` does (build output, mostly), [`MAX_DEPTH`] folders in at
/// most, for [`MAX_TIME`] at most.
fn scan(root: &Path) -> Vec<Candidate> {
    if too_broad(root) || !root.is_dir() {
        return vec![];
    }
    let name = name_of(root);
    let found = parking_lot::Mutex::new(Vec::new());
    let seen = AtomicUsize::new(0);
    let start = Instant::now();
    let mut walk = ignore::WalkBuilder::new(root);
    walk.max_depth(Some(MAX_DEPTH + 1)).hidden(false).require_git(false).follow_links(false).threads(4).filter_entry(|e| {
        e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) || enters(&e.file_name().to_string_lossy())
    });
    walk.build_parallel().run(|| {
        let (found, seen, name) = (&found, &seen, &name);
        Box::new(move |entry| {
            if seen.fetch_add(1, Ordering::Relaxed) > MAX_ENTRIES || start.elapsed() > MAX_TIME {
                return ignore::WalkState::Quit;
            }
            if let Ok(e) = entry
                && e.file_type().is_some_and(|t| t.is_file())
                && let Some(c) = candidate(root, e.path(), e.depth(), name)
            {
                found.lock().push(c);
            }
            ignore::WalkState::Continue
        })
    });
    let mut out = found.into_inner();
    out.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.path.cmp(&b.path)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"picture").unwrap();
    }

    #[test]
    fn only_a_kept_picture_with_a_plain_name_is_shown() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(KEPT_DIR);
        // nothing kept yet, or an index that won't parse: nothing shown
        assert!(read_kept(&dir).is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(kept_index(&dir), b"{ not json").unwrap();
        assert!(read_kept(&dir).is_empty());
        let index = r#"{
            "f:/site": { "kind": "found", "source": "F:\\site\\favicon.svg", "stamp": 7, "file": "00112233445566ff.png", "tone": "dark" },
            "f:/plain": { "kind": "letter" },
            "f:/asked": { "kind": "asked", "file": "00112233445566aa.png" },
            "f:/odd": { "kind": "set", "file": "..\\..\\secret.png" },
            "f:/old": { "kind": "made", "file": "0011223344556677.png" }
        }"#;
        std::fs::write(kept_index(&dir), index).unwrap();
        let kept = read_kept(&dir);
        assert_eq!(kept["f:/site"].picture(), Some("00112233445566ff.png"));
        assert_eq!(kept["f:/site"].tone.as_deref(), Some("dark"));
        assert_eq!(kept["f:/plain"].picture(), None);
        assert_eq!(kept["f:/asked"].picture(), None, "the letter you chose wins over a picture");
        assert_eq!(kept["f:/odd"].picture(), None, "a name that isn't one of bro's own is never followed");
        // kept before tones were: still shown
        assert_eq!(kept["f:/old"].picture(), Some("0011223344556677.png"));
        assert_eq!(kept["f:/old"].tone, None);
    }

    #[test]
    fn names_that_make_an_icon() {
        let score = |s: &str| name_score(s, "justgains", false);
        for s in ["favicon", "favicon-32x32", "apple-touch-icon", "apple-touch-icon-precomposed", "android-chrome-192x192", "icon", "icon-512", "icon@2x", "icon-dark", "app-icon", "AppIcon"] {
            assert!(score(s).is_some_and(|n| (20..=26).contains(&n)), "{s}: {:?}", score(s));
        }
        assert!(name_score("favicon", "x", true) < name_score("favicon", "x", false), "an SVG favicon draws sharpest");
        // named for the project beats them all; its start does too ("cpf" for cpfloans)
        assert_eq!(score("justgains-app-icon"), Some(10));
        assert_eq!(score("JustGains_Icon_Ink"), Some(11), "a version of it, just after the plain one");
        assert_eq!(score("justgains-logo"), Some(14));
        assert_eq!(score("justgains"), Some(12));
        assert_eq!(name_score("cpf-logo-white", "cpfloans", false), Some(17));
        assert_eq!(name_score("claudia-01-color", "claudia", false), None, "the name with a number isn't a mark");
        assert_eq!(name_score("bro-icon", "brocliv2", false), Some(12));
        for s in ["logo", "logo-dark", "Logo_white", "logomark", "brand", "logo-icon"] {
            assert!(score(s).is_some_and(|n| n >= 36), "{s}");
        }
        // someone else's, a piece of one, too wide for a square, or not a mark at all
        for s in [
            "icons", "iconography", "logos", "hero", "screenshot", "icon-facebook", "icon-apple-health-clean", "Icon - Apple Health", "notification-icon", "splash-icon", "android-icon-foreground",
            "justimagine-icon", "justgains-lockup-ink", "logo-text-yellow", "logo_lockup", "forge-logo", "08-logo-badge", "app-store-badge", "ju-icon", "context-menu-icons",
        ] {
            assert_eq!(score(s), None, "{s}");
        }
    }

    #[test]
    fn the_projects_own_icon_wins_then_the_nearest_favicon_and_copies_come_last() {
        let _env = sandbox();
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("app");
        touch(&root.join("docs").join("logo.png"));
        touch(&root.join("apps").join("web").join("public").join("favicon.ico"));
        touch(&root.join("apps").join("web").join("public").join("favicon.svg"));
        touch(&root.join("node_modules").join("x").join("favicon.svg"));
        touch(&root.join(".next").join("favicon.svg"));
        touch(&root.join("target-gui").join("favicon.png"));
        touch(&root.join("dist").join("favicon.svg"));
        touch(&root.join("README.md"));
        let want = root.join("apps").join("web").join("public").join("favicon.svg");
        assert_eq!(resolve(&root), Icon::Found(want.clone()), "a favicon before a logo, an SVG before an ICO, the source before its built copy");
        let all = candidates(&root, 10);
        assert_eq!(all, vec![want.clone(), root.join("dist").join("favicon.svg"), root.join("apps").join("web").join("public").join("favicon.ico"), root.join("docs").join("logo.png")]);
        assert_eq!(candidates(&root, 1).len(), 1);
        // a picture named for the project beats a favicon, even a couple of folders further in
        let site = d.path().join("justsite");
        touch(&site.join("public").join("favicon.svg"));
        let named = site.join("marketing").join("brand").join("press").join("justsite-app-icon.png");
        touch(&named);
        assert_eq!(resolve(&site), Icon::Found(named), "{:?}", candidates(&site, 6));
        // the build output a .gitignore leaves out isn't looked at
        std::fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        assert!(!candidates(&root, 10).contains(&root.join("dist").join("favicon.svg")));
        // nothing at all: the letter
        let empty = d.path().join("empty");
        std::fs::create_dir_all(empty.join("src")).unwrap();
        assert_eq!(resolve(&empty), Icon::Letter { asked: false });
        // a social card named for the project isn't its icon
        touch(&site.join("public").join("og").join("justsite.png"));
        assert_ne!(resolve(&site), Icon::Found(site.join("public").join("og").join("justsite.png")));
        // empty files, other formats and social networks' icons aren't the project's
        std::fs::create_dir_all(empty.join("public")).unwrap();
        std::fs::write(empty.join("public").join("favicon.png"), b"").unwrap();
        touch(&empty.join("favicon.icns"));
        touch(&empty.join("shell").join("icon-facebook.svg"));
        assert_eq!(resolve(&empty), Icon::Letter { asked: false });
    }

    #[test]
    fn a_nested_bro_folder_comes_before_any_favicon() {
        let _env = sandbox();
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("mono");
        touch(&root.join("mono-icon.svg"));
        touch(&root.join("packages").join("site").join("public").join(".bro").join("Icon.PNG"));
        touch(&root.join("packages").join("site").join(".bro").join("notes.txt"));
        assert_eq!(resolve(&root), Icon::Found(root.join("packages").join("site").join("public").join(".bro").join("Icon.PNG")));
    }

    #[test]
    fn drives_and_the_home_folder_are_too_broad_to_look_through() {
        let env = sandbox();
        let home = env.dir.path();
        touch(&home.join("site").join("favicon.svg"));
        assert_eq!(resolve(home), Icon::Letter { asked: false }, "the home folder");
        assert_eq!(resolve(home.parent().unwrap()), Icon::Letter { asked: false }, "above it");
        assert_eq!(resolve(&home.join("site")), Icon::Found(home.join("site").join("favicon.svg")), "a project in it");
        assert!(too_broad(Path::new(if cfg!(windows) { "K:\\" } else { "/" })));
        if cfg!(windows) {
            assert!(too_broad(Path::new("K:")));
        }
    }

    #[test]
    fn choosing_an_icon_keeps_it_in_the_project_and_survives_until_reset() {
        let env = sandbox();
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("site");
        touch(&root.join("public").join("favicon.png"));
        let mine = d.path().join("brand.SVG");
        std::fs::write(&mine, b"<svg/>").unwrap();
        // an icon set before in another format goes
        touch(&root.join(DIR).join("icon.png"));
        let dest = set(&root, &mine).unwrap();
        assert_eq!(dest, root.join(DIR).join("icon.svg"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"<svg/>");
        assert!(!root.join(DIR).join("icon.png").exists());
        assert_eq!(resolve(&root), Icon::Set(dest.clone()));
        assert_eq!(candidates(&root, 5).first(), Some(&dest), "the chosen one leads what's offered");
        // the letter, even with a picture there; other settings in project.json are kept
        crate::util::atomic_write(&root.join(DIR).join("project.json"), br#"{"color": "teal"}"#).unwrap();
        use_letter(&root).unwrap();
        assert_eq!(resolve(&root), Icon::Letter { asked: true });
        assert!(dest.exists());
        // back to finding it: the favicon again, `.bro` keeps only the other setting
        reset(&root).unwrap();
        assert_eq!(resolve(&root), Icon::Found(root.join("public").join("favicon.png")));
        let left: Value = serde_json::from_str(&std::fs::read_to_string(root.join(DIR).join("project.json")).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({"color": "teal"}));
        assert!(!dest.exists());
        // with nothing else in it, `.bro` goes too
        std::fs::remove_file(root.join(DIR).join("project.json")).unwrap();
        set(&root, &mine).unwrap();
        reset(&root).unwrap();
        assert!(!root.join(DIR).exists());
        // not a picture, or a folder
        assert!(set(&root, &root.join("public")).is_err());
        let doc = d.path().join("notes.txt");
        std::fs::write(&doc, b"x").unwrap();
        assert!(set(&root, &doc).is_err());
        // a project at the home folder never uses bro's own `.bro`
        let home = env.dir.path();
        touch(&home.join(".bro").join("icon.png"));
        assert_eq!(chosen(home), None);
        assert!(set(home, &mine).is_err());
    }

}
