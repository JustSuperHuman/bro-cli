//! Colour themes (ported from z4-oriel). A theme sets a handful of colours; background and body text come from
//! the terminal itself (`Color::Reset`) so bro looks native anywhere.
//!
//! * built-in palettes (oriel's, `ultra` — the animated default — included)
//! * `terminal`: ANSI colours only, so it follows the terminal's own theme
//! * your own: `~/.bro/themes/<name>.toml`, live-reloaded when the file changes:
//!
//! ```toml
//! base = "ultra"      # start from any theme; only list what you change
//! accent = "#ff79c6"
//! rainbow = false
//! ```

use ratatui::style::Color;
use std::path::PathBuf;

/// The colours every surface draws with.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    /// Reset = the terminal's own background
    pub bg: Color,
    /// body text
    pub fg: Color,
    /// focused frames, titles, highlights
    pub accent: Color,
    /// secondary highlight
    pub shine: Color,
    /// unfocused borders
    pub frame: Color,
    /// hints, metadata
    pub muted: Color,
    /// selection background tint
    pub user: Color,
    /// warm highlight (warnings, inline values)
    pub inline: Color,
    pub danger: Color,
    pub good: Color,
    /// rainbow logo + title (ultra)
    pub animated: bool,
}

// name, accent, shine, frame, muted, user, inline, good, danger
/// (name, accent, shine, frame, muted, user, inline, good, danger)
type Palette = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str);

const PALETTES: &[Palette] = &[
    ("ultra", "#b48cff", "#8be9fd", "#3d3852", "#7d7896", "#5c5480", "#ff9ad5", "#7dffc8", "#ff6b9d"),
    ("oriel", "#d4884a", "#ffd2a8", "#3c3c3c", "#6e6e6e", "#555555", "#e6b673", "#9cc46a", "#e0694a"),
    ("ember", "#ff5f3a", "#ffc7a8", "#4a2a24", "#7a5c55", "#6a3a30", "#ff9b6b", "#c2cc5a", "#ff4a3a"),
    ("ocean", "#4aa8d4", "#b8e6ff", "#24384a", "#5c6e7a", "#35556a", "#7fd0ff", "#4fd6b0", "#ff7a8a"),
    ("forest", "#6fbf73", "#c8f5c0", "#2a3d2b", "#5e7360", "#3f5a41", "#a6e3a1", "#8fe07a", "#e08a5a"),
    ("sakura", "#f28fb5", "#ffd6e7", "#4a2d3a", "#86697a", "#6a4256", "#ffb3d0", "#8fe0bc", "#ff5f8f"),
    ("synthwave", "#ff4fd8", "#7df9ff", "#3a2360", "#7a6a9a", "#5a3a90", "#7df9ff", "#5af2c0", "#ff4f7b"),
    ("matrix", "#39ff6a", "#c8ffd5", "#12361d", "#3f7a52", "#1f5a30", "#7dff9e", "#39ff6a", "#ff5a4a"),
    ("amber", "#ffb000", "#ffe0a0", "#4a3500", "#8a6a2a", "#6a4c00", "#ffcc55", "#c8d65a", "#ff6a3a"),
    ("dracula", "#bd93f9", "#ffb86c", "#44475a", "#6272a4", "#5a5e7a", "#50fa7b", "#50fa7b", "#ff5555"),
    ("mono", "#e0e0e0", "#ffffff", "#444444", "#777777", "#5a5a5a", "#cfcfcf", "#d8d8d8", "#8c8c8c"),
];

/// The default theme name.
pub const DEFAULT: &str = "ultra";

/// Every theme you can pick: built-ins, `terminal`, then your own files.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = builtin_names();
    for c in custom_names() {
        if !v.contains(&c) {
            v.push(c);
        }
    }
    v
}

/// The themes that come with bro.
pub fn builtin_names() -> Vec<String> {
    let mut v: Vec<String> = PALETTES.iter().map(|p| p.0.to_string()).collect();
    v.push("terminal".into());
    v
}

/// `#rrggbb` → Color.
pub fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#').trim_start_matches("0x");
    let s = s.get(..6)?;
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Linear blend of two RGB colours (`t` = 0 → a, 1 → b). Non-RGB colours return `a`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => a,
    }
}

/// Look a theme up by name (unknown names fall back to the default palette).
pub fn get(name: &str) -> Theme {
    get_depth(name, 0)
}

fn get_depth(name: &str, depth: usize) -> Theme {
    if !builtin_names().iter().any(|b| b == name)
        && let Some(t) = custom(name, depth) {
            return t;
        }
    builtin(name)
}

fn builtin(name: &str) -> Theme {
    if name == "terminal" {
        return terminal();
    }
    let p = PALETTES.iter().find(|p| p.0 == name).unwrap_or(&PALETTES[0]);
    let c = |s| hex(s).unwrap_or(Color::Reset);
    // borders and hints lean grey so the theme colour is saved for what matters
    let calm = |s, grey: Color, a: f32| mix(c(s), grey, a);
    Theme {
        name: p.0.into(),
        bg: Color::Reset,
        fg: Color::Reset,
        accent: c(p.1),
        shine: c(p.2),
        frame: calm(p.3, Color::Rgb(58, 58, 62), 0.55),
        muted: calm(p.4, Color::Rgb(128, 128, 134), 0.5),
        user: calm(p.5, Color::Rgb(92, 92, 98), 0.45),
        inline: c(p.6),
        good: c(p.7),
        danger: c(p.8),
        animated: p.0 == "ultra",
    }
}

/// ANSI-only: every colour is one of the terminal's 16.
fn terminal() -> Theme {
    Theme {
        name: "terminal".into(),
        bg: Color::Reset,
        fg: Color::Reset,
        accent: Color::Blue,
        shine: Color::LightCyan,
        frame: Color::DarkGray,
        muted: Color::DarkGray,
        user: Color::DarkGray,
        inline: Color::Yellow,
        danger: Color::Red,
        good: Color::Green,
        animated: false,
    }
}

/// Rainbow colour for character i at time t (seconds) — the ultra theme's drifting title.
pub fn rainbow(i: usize, t: f64) -> Color {
    rainbow_at(i as f64 * 0.07 - t * 0.6, 0.55)
}

/// A point on the rainbow (0..1 wraps) at saturation `s`.
pub fn rainbow_at(pos: f64, s: f64) -> Color {
    let h = pos.rem_euclid(1.0) * 6.0;
    let c = s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 1.0 - c;
    let f = |u: f64| ((u + m) * 255.0) as u8;
    Color::Rgb(f(r), f(g), f(b))
}

// ------------------------------------------------------------------ your own themes

#[cfg(test)]
thread_local! {
    /// Tests keep their theme files in a folder of their own.
    pub static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Where your themes live: `~/.bro/themes`.
pub fn themes_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(d) = TEST_DIR.with(|d| d.borrow().clone()) {
        return d;
    }
    crate::util::bro_dir().join("themes")
}

fn file_of(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.toml"))
}

/// Names of the theme files in the themes dir, sorted.
pub fn custom_names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(themes_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "toml").then(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))?
        })
        .collect();
    v.sort();
    v
}

/// True for a theme that comes from a file.
pub fn is_custom(name: &str) -> bool {
    !builtin_names().iter().any(|b| b == name) && file_of(name).is_file()
}

const KEYS: &[&str] = &["accent", "shine", "good", "danger", "inline", "muted", "frame", "user", "text", "fg", "background", "bg"];

fn set_field(t: &mut Theme, key: &str, c: Color) {
    match key {
        "accent" => t.accent = c,
        "shine" => t.shine = c,
        "good" => t.good = c,
        "danger" => t.danger = c,
        "inline" => t.inline = c,
        "muted" => t.muted = c,
        "frame" => t.frame = c,
        "user" => t.user = c,
        "text" | "fg" => t.fg = c,
        "background" | "bg" => t.bg = c,
        _ => {}
    }
}

/// A colour in a theme file: "#rrggbb", "#rgb", an ANSI name ("red", "bright-blue"), or "terminal".
pub fn color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if matches!(s.as_str(), "terminal" | "default" | "none" | "reset") {
        return Some(Color::Reset);
    }
    if let Some(h) = s.strip_prefix('#') {
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match h.len() {
            3 => hex(&h.chars().flat_map(|c| [c, c]).collect::<String>()),
            6 => hex(h),
            _ => None,
        };
    }
    Some(match s.replace(['-', '_', ' '], "").as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" | "gray" | "grey" => Color::Gray,
        "brightblack" | "darkgray" | "darkgrey" => Color::DarkGray,
        "brightred" => Color::LightRed,
        "brightgreen" => Color::LightGreen,
        "brightyellow" => Color::LightYellow,
        "brightblue" => Color::LightBlue,
        "brightmagenta" => Color::LightMagenta,
        "brightcyan" => Color::LightCyan,
        "brightwhite" => Color::White,
        _ => return None,
    })
}

/// The last version of each theme file that parsed, so a typo mid-edit doesn't flash another theme.
static LAST_GOOD: parking_lot::Mutex<Vec<(String, Theme)>> = parking_lot::Mutex::new(Vec::new());

fn custom(name: &str, depth: usize) -> Option<Theme> {
    let text = std::fs::read_to_string(file_of(name)).ok()?;
    let Ok(tbl) = text.parse::<toml::Table>() else {
        let last = LAST_GOOD.lock().iter().find(|(n, _)| n == name).map(|(_, t)| t.clone());
        return Some(last.unwrap_or_else(|| Theme { name: name.to_string(), ..builtin(DEFAULT) }));
    };
    let base = tbl.get("base").and_then(|v| v.as_str()).unwrap_or(DEFAULT);
    let mut t = if base == name || depth >= 4 { builtin(base) } else { get_depth(base, depth + 1) };
    t.name = name.to_string();
    for (k, v) in &tbl {
        if let Some(c) = v.as_str().and_then(color) {
            set_field(&mut t, k, c);
        }
    }
    if let Some(b) = tbl.get("rainbow").and_then(|v| v.as_bool()) {
        t.animated = b;
    }
    let mut last = LAST_GOOD.lock();
    last.retain(|(n, _)| n != name);
    last.push((name.to_string(), t.clone()));
    Some(t)
}

/// Anything wrong with a theme file, in words ("mine.toml: 'acent' isn't a setting").
pub fn problems(name: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file_of(name)) else { return vec![] };
    let file = format!("{name}.toml");
    match text.parse::<toml::Table>() {
        Err(e) => vec![format!("{file}: {}", e.to_string().lines().next().unwrap_or("can't read it"))],
        Ok(tbl) => tbl
            .iter()
            .filter_map(|(k, v)| match k.as_str() {
                "base" => v.as_str().filter(|b| !names().iter().any(|n| n == b)).map(|b| format!("{file}: there's no theme called '{b}'")),
                "rainbow" => (!v.is_bool()).then(|| format!("{file}: rainbow is true or false")),
                k if KEYS.contains(&k) => v.as_str().and_then(color).is_none().then(|| format!("{file}: '{k}' isn't a colour")),
                _ => Some(format!("{file}: '{k}' isn't a setting")),
            })
            .collect(),
    }
}

/// Watch the themes dir; `on_change` runs (on a watcher thread) when a file is written.
pub fn watch(on_change: impl Fn() + Send + 'static) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    if cfg!(test) {
        return None;
    }
    let dir = themes_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| e.kind.is_modify() || e.kind.is_create()) {
            on_change();
        }
    })
    .ok()?;
    w.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_and_custom_files() {
        let dir = tempfile::tempdir().unwrap();
        TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
        assert!(get("ultra").animated);
        assert_eq!(get("nope").name, "ultra");
        assert!(names().contains(&"terminal".to_string()));
        std::fs::write(dir.path().join("mine.toml"), "base = \"ocean\"\naccent = \"#ff0000\"\nrainbow = true\nacent = \"x\"\n").unwrap();
        let t = get("mine");
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert!(t.animated);
        assert!(is_custom("mine"));
        assert_eq!(problems("mine").len(), 1);
        assert_eq!(color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
        TEST_DIR.with(|d| *d.borrow_mut() = None);
    }
}
