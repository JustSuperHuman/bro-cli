//! Claude Code in bro's colours: bro writes its theme as a Claude Code custom theme,
//! `<claude config>/themes/bro.json`, and its Claude launches select it (`"theme": "custom:bro"` in
//! the `--settings` file, so your own theme setting is untouched outside bro). Claude Code watches its
//! themes folder, so a theme change in bro (a preview too) recolours every running session.
//!
//! The file also carries a `bro` block Claude Code ignores: the theme's id, label and marker, for the
//! bro mod (see [`crate::claude_mod`]), which gives the spinner and the turn line the theme's voice.
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The theme's file name in Claude Code's themes folder, and the `theme` setting that picks it.
pub const SLUG: &str = "bro";
pub const SETTING: &str = "custom:bro";

/// A colour as bro's theme holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Rgb(u8, u8, u8),
    /// One of the terminal's 16 colours: 0 black .. 7 white, 8 bright black .. 15 bright white.
    Ansi(u8),
    /// The terminal's own colour: bro leaves it alone, and so does the Claude theme.
    Terminal,
}

/// bro's theme, flattened for Claude Code (bro-tui builds it from its `Theme`).
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// bro's theme id (`tokyo-night`, or a theme file's name).
    pub id: String,
    /// The built-in theme it derives from: `id` itself for a built-in.
    pub base: String,
    pub light: bool,
    pub rainbow: bool,
    /// The marker bro draws before the focused session's title.
    pub marker: String,
    pub bg: Ink,
    pub fg: Ink,
    pub accent: Ink,
    pub shine: Ink,
    pub frame: Ink,
    pub muted: Ink,
    pub user: Ink,
    /// The warm highlight: warnings, inline values.
    pub inline: Ink,
    pub good: Ink,
    pub danger: Ink,
    /// The sessions' 16 colours, on a theme that paints them.
    pub ansi: Option<[Ink; 16]>,
    /// The desktop app's window around the sessions.
    pub panel: Option<Ink>,
}

type Rgb = [u8; 3];

const WHITE: Rgb = [255, 255, 255];
/// What bro assumes behind a theme that keeps the terminal's background (`Theme::deep`).
const DARK_BG: Rgb = [18, 18, 22];
const LIGHT_BG: Rgb = [255, 255, 255];
const DARK_FG: Rgb = [228, 230, 235];
const LIGHT_FG: Rgb = [31, 35, 40];

/// Ink's and Claude Code's names for the 16 terminal colours.
const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "blackBright",
    "redBright",
    "greenBright",
    "yellowBright",
    "blueBright",
    "magentaBright",
    "cyanBright",
    "whiteBright",
];

impl Palette {
    fn rgb(&self, ink: Ink) -> Option<Rgb> {
        match ink {
            Ink::Rgb(r, g, b) => Some([r, g, b]),
            Ink::Ansi(i) => match self.ansi.as_ref().and_then(|a| a.get(usize::from(i))) {
                Some(&Ink::Rgb(r, g, b)) => Some([r, g, b]),
                _ => None,
            },
            Ink::Terminal => None,
        }
    }

    fn bg_or_assumed(&self) -> Rgb {
        self.rgb(self.bg).unwrap_or(if self.light { LIGHT_BG } else { DARK_BG })
    }

    fn fg_or_assumed(&self) -> Rgb {
        self.rgb(self.fg).unwrap_or(if self.light { LIGHT_FG } else { DARK_FG })
    }

    /// One of the theme's own 16 colours, when it paints them.
    fn ansi_rgb(&self, i: usize) -> Option<Rgb> {
        match self.ansi.as_ref()?.get(i)? {
            &Ink::Rgb(r, g, b) => Some([r, g, b]),
            _ => None,
        }
    }

    /// Paints its own background.
    fn painted(&self) -> bool {
        self.bg != Ink::Terminal
    }
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// A colour as Ink (the mod's drawings) takes it.
fn ink_color(p: &Palette, ink: Ink) -> Option<String> {
    match ink {
        Ink::Rgb(r, g, b) => Some(hex([r, g, b])),
        Ink::Ansi(i) => p.rgb(ink).map(hex).or_else(|| ANSI_NAMES.get(usize::from(i)).map(|n| (*n).to_string())),
        Ink::Terminal => None,
    }
}

/// `tokyo-night` → `Tokyo Night`, with the names their authors spell their own way.
pub fn label(id: &str) -> String {
    let fixed = [
        ("github-dark", "GitHub Dark"),
        ("github-dimmed", "GitHub Dimmed"),
        ("github-light", "GitHub Light"),
        ("synthwave-84", "SynthWave '84"),
        ("rose-pine", "Rosé Pine"),
        ("rose-pine-dawn", "Rosé Pine Dawn"),
        ("catppuccin-frappe", "Catppuccin Frappé"),
        ("cobalt2", "Cobalt2"),
        ("dark-modern", "Dark Modern"),
    ];
    if let Some((_, name)) = fixed.iter().find(|(i, _)| *i == id) {
        return (*name).to_string();
    }
    id.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The Claude Code theme for `p`: `{ name, base, overrides, bro }`.
pub fn theme_json(p: &Palette) -> Value {
    // a theme of terminal colours stays one: Claude's ANSI bases follow the terminal's palette, with
    // Claude's own picks for its colours (bro has no RGB to measure their contrast with)
    let ansi_only = [p.accent, p.shine, p.good, p.danger, p.inline].iter().all(|i| matches!(i, Ink::Ansi(_)) && p.rgb(*i).is_none());
    let base = match (p.light, ansi_only) {
        (false, false) => "dark",
        (true, false) => "light",
        (false, true) => "dark-ansi",
        (true, true) => "light-ansi",
    };
    let label = label(&p.id);
    let mut palette = Map::new();
    let slots = [("accent", p.accent), ("shine", p.shine), ("good", p.good), ("warn", p.inline), ("danger", p.danger), ("muted", p.muted), ("frame", p.frame), ("fg", p.fg), ("bg", p.bg)];
    for (key, ink) in slots {
        if let Some(c) = ink_color(p, ink) {
            palette.insert(key.into(), Value::String(c));
        }
    }
    json!({
        "name": format!("bro · {label}"),
        "base": base,
        "overrides": if ansi_only { Map::new() } else { overrides(p) },
        "bro": {
            "v": 1,
            "id": p.id,
            "label": label,
            "base": p.base,
            "light": p.light,
            "rainbow": p.rainbow,
            "marker": p.marker,
            "palette": palette,
        },
    })
}

/// Claude Code's colours for a theme. Claude's own keep their parts (the orange that is Claude, the blue of its
/// questions, the violet and teal of its modes, green, red and amber for how things went), each in the theme's
/// version of that hue and each made to stand out on the theme's background: never just the theme's accent, so
/// the mascot and the spinner still pop. Text, hints, frames and surfaces are the theme's own, kept legible.
fn overrides(p: &Palette) -> Map<String, Value> {
    let mut o = Map::new();
    let mut put = |key: &str, c: Rgb| {
        o.insert(key.to_string(), Value::String(hex(c)));
    };
    let bg = p.bg_or_assumed();
    let fg = p.fg_or_assumed();
    let t = Tones::of(p);
    // the brighter twin of the shimmer that sweeps across animated text
    let lift = |c: Rgb| mix(c, WHITE, 0.3);

    // a vivid orange on white reads below 4.5:1; past 3:1 it stands out as Claude's own light theme does,
    // where darker would turn it brown
    let brand = if p.light { vivid(readable(t.orange, bg, LIGHT_BRAND_CONTRAST), bg) } else { readable(t.orange, bg, BRAND_CONTRAST) };
    for key in ["claude", "clawd_body", "briefLabelClaude", "fastMode"] {
        put(key, brand);
    }
    put("claudeShimmer", lift(brand));
    put("fastModeShimmer", lift(brand));
    let blue = readable(t.blue, bg, BRAND_CONTRAST);
    for key in ["permission", "suggestion", "remember", "claudeBlue_FOR_SYSTEM_SPINNER", "briefLabelYou", "ide", "rate_limit_fill"] {
        put(key, blue);
    }
    put("permissionShimmer", lift(blue));
    put("claudeBlueShimmer_FOR_SYSTEM_SPINNER", lift(blue));
    let violet = readable(t.violet, bg, BRAND_CONTRAST);
    for key in ["autoAccept", "skill", "merged", "effortUltra"] {
        put(key, violet);
    }
    put("autoAcceptShimmer", lift(violet));
    let teal = readable(t.cyan, bg, BRAND_CONTRAST);
    put("planMode", teal);
    put("background", teal);
    put("bashBorder", readable(t.pink, bg, ACCENT_CONTRAST));
    // how things went
    put("success", readable(t.green, bg, BRAND_CONTRAST));
    put("error", readable(t.red, bg, BRAND_CONTRAST));
    let warn = readable(t.yellow, bg, BRAND_CONTRAST);
    put("warning", warn);
    put("warningShimmer", lift(warn));
    // text, hints and frames
    if p.painted() {
        put("text", fg);
        put("inverseText", bg);
        put("clawd_background", bg);
    }
    let muted = p.rgb(p.muted).unwrap_or_else(|| mix(bg, fg, 0.45));
    let inactive = readable(muted, bg, HINT_CONTRAST);
    put("inactive", inactive);
    put("inactiveShimmer", mix(inactive, fg, 0.35));
    let frame = p.rgb(p.frame).unwrap_or_else(|| mix(bg, fg, 0.25));
    put("subtle", readable(frame, bg, 2.2));
    put("rate_limit_empty", readable(frame, bg, 1.6));
    let border = readable(muted, bg, 2.5);
    put("promptBorder", border);
    put("promptBorderShimmer", lift(border));
    // diffs: the theme's green and red, washed into its background
    let (green, red) = (p.rgb(p.good).unwrap_or(t.green), p.rgb(p.danger).unwrap_or(t.red));
    let (line, dimmed, word) = if p.light { (0.28, 0.14, 0.5) } else { (0.3, 0.16, 0.55) };
    put("diffAdded", mix(bg, green, line));
    put("diffRemoved", mix(bg, red, line));
    put("diffAddedDimmed", mix(bg, green, dimmed));
    put("diffRemovedDimmed", mix(bg, red, dimmed));
    put("diffAddedWord", mix(bg, green, word));
    put("diffRemovedWord", mix(bg, red, word));
    // surfaces: your prompt sits on the theme's selection tint
    if let Some(user) = p.rgb(p.user) {
        let tint = if p.painted() { user } else { mix(bg, user, 0.6) };
        put("userMessageBackground", tint);
        put("userMessageBackgroundHover", mix(tint, fg, 0.06));
        put("selectionBg", if p.painted() { user } else { mix(bg, user, 0.8) });
    }
    put("composerSidebarBackground", p.panel.and_then(|ink| p.rgb(ink)).unwrap_or_else(|| mix(bg, fg, 0.04)));
    put("bashMessageBackgroundColor", mix(bg, t.yellow, 0.12));
    put("memoryBackgroundColor", mix(bg, t.blue, 0.12));
    // subagents and `ultrathink`'s rainbow: Claude's hues, in the theme's versions
    let tone = |c: Rgb| readable(c, bg, ACCENT_CONTRAST);
    let agents = [
        ("red_FOR_SUBAGENTS_ONLY", t.red),
        ("blue_FOR_SUBAGENTS_ONLY", t.blue),
        ("green_FOR_SUBAGENTS_ONLY", t.green),
        ("yellow_FOR_SUBAGENTS_ONLY", t.yellow),
        ("purple_FOR_SUBAGENTS_ONLY", t.violet),
        ("orange_FOR_SUBAGENTS_ONLY", t.orange),
        ("pink_FOR_SUBAGENTS_ONLY", t.pink),
        ("cyan_FOR_SUBAGENTS_ONLY", t.cyan),
    ];
    for (key, c) in agents {
        put(key, tone(c));
    }
    let rainbow = [
        ("red", t.red),
        ("orange", t.orange),
        ("yellow", t.yellow),
        ("green", t.green),
        ("blue", t.blue),
        ("indigo", mix(t.blue, t.violet, 0.5)),
        ("violet", t.violet),
    ];
    for (name, c) in rainbow {
        put(&format!("rainbow_{name}"), tone(c));
        put(&format!("rainbow_{name}_shimmer"), lift(tone(c)));
    }
    o
}

/// What Claude's colours must reach against the background (WCAG's mark for text), what its minor marks and
/// tints must reach (a border, a subagent's name), and its hints (dim, but still read).
const BRAND_CONTRAST: f32 = 4.5;
const ACCENT_CONTRAST: f32 = 3.0;
const LIGHT_BRAND_CONTRAST: f32 = 3.0;
const HINT_CONTRAST: f32 = 4.0;

/// The hues Claude Code's colours stand for, each found in the theme's own palette (its colours, then its 16
/// terminal colours), or made in the theme's manner where it has none.
struct Tones {
    red: Rgb,
    orange: Rgb,
    yellow: Rgb,
    green: Rgb,
    cyan: Rgb,
    blue: Rgb,
    violet: Rgb,
    pink: Rgb,
}

impl Tones {
    fn of(p: &Palette) -> Tones {
        let r = |ink: Ink| p.rgb(ink);
        let a = |i: usize| p.ansi_rgb(i);
        // the theme's manner: how saturated and how light its colours run
        let own: Vec<(f32, f32, f32)> =
            [p.accent, p.shine, p.inline, p.good, p.danger].into_iter().filter_map(r).chain((0..16).filter_map(a)).map(hsl).filter(|&(_, s, _)| s >= 0.25).collect();
        let mean = |part: fn(&(f32, f32, f32)) -> f32, or: f32| if own.is_empty() { or } else { own.iter().map(part).sum::<f32>() / own.len() as f32 };
        let vivid = mean(|c| c.1, 0.6).clamp(0.45, 0.85);
        let light = mean(|c| c.2, if p.light { 0.42 } else { 0.66 }).clamp(0.3, 0.75);
        // a colour of the theme's near `hue`, else one made at that hue in its manner
        let find = |hue: f32, window: f32, pool: &[Option<Rgb>], avoid: &[Rgb]| {
            pool.iter()
                .flatten()
                .copied()
                .find(|&c| {
                    let (h, s, l) = hsl(c);
                    s >= 0.25 && (0.12..=0.92).contains(&l) && hue_gap(h, hue) <= window && !avoid.iter().any(|&x| alike(c, x))
                })
                .unwrap_or_else(|| from_hsl(hue, vivid, light))
        };
        let red = find(0.0, 25.0, &[r(p.danger), a(1), a(9)], &[]);
        let yellow = find(48.0, 20.0, &[r(p.inline), a(3), a(11)], &[]);
        // Claude's orange: the theme's own orange where it has one (never its accent, so the mascot stands apart
        // from bro's own chrome), else the theme's red warmed toward its yellow
        let accent: Vec<Rgb> = r(p.accent).into_iter().collect();
        let orange = [r(p.inline), r(p.danger), a(3), a(11), a(1), a(9)]
            .into_iter()
            .flatten()
            .find(|&c| {
                let (h, s, l) = hsl(c);
                s >= 0.35 && (0.25..=0.85).contains(&l) && (10.0..=32.0).contains(&h) && !accent.iter().any(|&x| alike(c, x))
            })
            .unwrap_or_else(|| mix(red, yellow, 0.5));
        // an amber that is the orange would say "warning" in the mascot's colour
        let yellow = if alike(yellow, orange) { find(48.0, 20.0, &[a(3), a(11)], &[orange]) } else { yellow };
        let blue = find(222.0, 30.0, &[a(4), a(12), r(p.shine), r(p.accent)], &[]);
        let violet = find(275.0, 30.0, &[a(5), a(13), r(p.accent), r(p.shine)], &[blue]);
        Tones {
            red,
            orange,
            yellow,
            green: find(130.0, 45.0, &[r(p.good), a(2), a(10)], &[]),
            cyan: find(185.0, 25.0, &[a(6), a(14), r(p.shine), r(p.accent)], &[]),
            blue,
            violet,
            pink: find(330.0, 25.0, &[a(13), a(5), r(p.accent), r(p.shine), r(p.danger)], &[violet, red]),
        }
    }
}

// ------------------------------------------------------------------ colour maths

/// Hue in degrees, saturation and lightness 0 to 1 (HSL).
fn hsl(c: Rgb) -> (f32, f32, f32) {
    let [r, g, b] = c.map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h.rem_euclid(360.0), s.clamp(0.0, 1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let v = |n: f32| ((n + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [v(r), v(g), v(b)]
}

/// WCAG relative luminance.
fn luminance(c: Rgb) -> f32 {
    let channel = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
}

/// The WCAG contrast ratio of two colours, 1 to 21.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A colour's hue, in degrees.
pub fn hue(c: [u8; 3]) -> f32 {
    hsl(c).0
}

/// `#rrggbb` as RGB.
pub fn rgb_of(hex: &str) -> Option<[u8; 3]> {
    let h = hex.strip_prefix('#').filter(|h| h.len() == 6)?;
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// What the theme's colours are read against: its background, or the dark (or white) bro assumes behind a
/// theme that keeps the terminal's.
pub fn backdrop(p: &Palette) -> [u8; 3] {
    p.bg_or_assumed()
}

fn hue_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Two colours a glance can't tell apart.
fn alike(a: Rgb, b: Rgb) -> bool {
    let d = |i: usize| (i32::from(a[i]) - i32::from(b[i])).abs();
    d(0) + d(1) + d(2) < 36
}

/// On a light background, a colour darker than it needs to be (a theme's dark red and amber blend to rust)
/// taken lighter, toward the ~3:1 of Claude's own orange on white, so the mascot stays a bright orange.
fn vivid(c: Rgb, bg: Rgb) -> Rgb {
    let (h, s, mut l) = hsl(c);
    let mut best = c;
    while contrast(best, bg) > 4.0 && l < 0.95 {
        l += 0.01;
        let next = from_hsl(h, s, l);
        if contrast(next, bg) < LIGHT_BRAND_CONTRAST {
            break;
        }
        best = next;
    }
    best
}

/// `c` taken lighter (on a dark background) or darker (on a light one), its hue and saturation kept, until it
/// reads at `min`:1 against `bg`.
fn readable(c: Rgb, bg: Rgb, min: f32) -> Rgb {
    if contrast(c, bg) >= min {
        return c;
    }
    let (h, s, mut l) = hsl(c);
    let lighter = luminance(bg) < 0.4;
    loop {
        l = if lighter { (l + 0.01).min(1.0) } else { (l - 0.01).max(0.0) };
        let next = from_hsl(h, s, l);
        if contrast(next, bg) >= min || l <= 0.0 || l >= 1.0 {
            return next;
        }
    }
}

/// `<config_dir>/themes/bro.json`.
pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join("themes").join(format!("{SLUG}.json"))
}

/// Writes the theme into one Claude config dir; true when the file changed.
pub fn write(config_dir: &Path, p: &Palette) -> std::io::Result<bool> {
    let body = serde_json::to_string_pretty(&theme_json(p)).unwrap_or_default() + "\n";
    let file = path(config_dir);
    if std::fs::read_to_string(&file).is_ok_and(|old| old == body) {
        return Ok(false);
    }
    // atomic: Claude Code reloads the file the moment it changes
    crate::util::atomic_write(&file, body.as_bytes())?;
    Ok(true)
}

/// Every Claude config dir bro launches into that exists: `~/.claude` and each pooled account.
pub fn config_dirs() -> Vec<PathBuf> {
    let accounts = std::fs::read_dir(crate::paths::claude_accounts_dir()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir());
    std::iter::once(crate::paths::claude_local_dir()).filter(|d| d.is_dir()).chain(accounts).collect()
}

/// Writes the theme into every Claude config dir (a theme change in bro): the ones that changed.
pub fn write_everywhere(p: &Palette) -> Vec<PathBuf> {
    config_dirs().into_iter().filter(|dir| write(dir, p).unwrap_or(false)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn tokyo() -> Palette {
        let c = |s: &str| {
            let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap();
            Ink::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
        };
        let ansi = [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5",
        ]
        .map(c);
        Palette {
            id: "tokyo-night".into(),
            base: "tokyo-night".into(),
            light: false,
            rainbow: false,
            marker: "▶".into(),
            bg: c("#1a1b26"),
            fg: c("#c0caf5"),
            accent: c("#7aa2f7"),
            shine: c("#bb9af7"),
            frame: c("#3b4261"),
            muted: c("#565f89"),
            user: c("#283457"),
            inline: c("#e0af68"),
            good: c("#9ece6a"),
            danger: c("#f7768e"),
            ansi: Some(ansi),
            panel: Some(c("#16161e")),
        }
    }

    fn color(o: &Value, key: &str) -> [u8; 3] {
        rgb_of(o[key].as_str().unwrap_or_else(|| panic!("{key} missing"))).unwrap()
    }

    /// Claude's colours keep their parts and stand out: its orange on every theme, each readable on the
    /// theme's background, none of them just the accent.
    fn stands_out(p: &Palette) {
        let v = theme_json(p);
        let o = &v["overrides"];
        let bg = backdrop(p);
        let claude = color(o, "claude");
        let h = hue(claude);
        assert!(h <= 40.0 || h >= 350.0, "{}: claude {} isn't Claude's orange (hue {h})", p.id, hex(claude));
        assert_ne!(Some(claude), p.rgb(p.accent), "{}: the mascot isn't just the accent", p.id);
        for key in ["claude", "permission", "autoAccept", "planMode", "success", "error", "warning"] {
            let ratio = contrast(color(o, key), bg);
            let min = if key == "claude" && p.light { 2.95 } else { 4.45 };
            assert!(ratio >= min, "{}: {key} {} reads at {ratio:.2}:1", p.id, hex(color(o, key)));
        }
        assert!(contrast(color(o, "inactive"), bg) >= 3.95, "{}: hints stay legible", p.id);
        assert_ne!(color(o, "warning"), claude, "{}: a warning isn't the mascot's colour", p.id);
        for (key, value) in o.as_object().unwrap() {
            let s = value.as_str().unwrap();
            assert!(rgb_of(s).is_some(), "{}.{key}: {s}", p.id);
        }
    }

    #[test]
    fn painted_theme_gives_claude_its_own_colours_in_the_themes_hues() {
        let p = tokyo();
        stands_out(&p);
        let v = theme_json(&p);
        assert_eq!(v["name"], "bro · Tokyo Night");
        assert_eq!(v["base"], "dark");
        let o = &v["overrides"];
        // Tokyo Night has no orange: its red warmed toward its yellow
        assert_eq!(o["claude"], hex(mix([0xf7, 0x76, 0x8e], [0xe0, 0xaf, 0x68], 0.5)));
        assert_eq!(o["autoAccept"], "#bb9af7", "the theme's own violet");
        assert_eq!(o["planMode"], "#7dcfff", "the theme's own cyan");
        // the theme's own text and surfaces carry over
        assert_eq!(o["text"], "#c0caf5");
        assert_eq!(o["userMessageBackground"], "#283457");
        assert_eq!(o["composerSidebarBackground"], "#16161e");
        assert_eq!(o["diffAdded"], hex(mix([0x1a, 0x1b, 0x26], [0x9e, 0xce, 0x6a], 0.3)));
        // its dim hint colour is too faint to read on its background: lifted
        assert_ne!(o["inactive"], "#565f89");
        assert_eq!(v["bro"]["id"], "tokyo-night");
        assert_eq!(v["bro"]["palette"]["warn"], "#e0af68");
        assert_eq!(v["bro"]["marker"], "▶");
    }

    #[test]
    fn a_theme_with_its_own_orange_lends_it_to_claude() {
        // Dracula's orange, not its purple accent
        let p = Palette { id: "dracula".into(), accent: Ink::Rgb(0xbd, 0x93, 0xf9), inline: Ink::Rgb(0xff, 0xb8, 0x6c), ..tokyo() };
        stands_out(&p);
        assert_eq!(theme_json(&p)["overrides"]["claude"], "#ffb86c");
        // an accent that is orange is bro's own chrome: Claude takes another
        let ember = Palette { id: "ember".into(), accent: Ink::Rgb(0xff, 0x9b, 0x6b), inline: Ink::Rgb(0xff, 0x9b, 0x6b), ..tokyo() };
        stands_out(&ember);
    }

    #[test]
    fn grey_and_light_themes_still_get_a_mascot_that_pops() {
        let grey = |c: u8| Ink::Rgb(c, c, c);
        // mono: nothing but greys, so Claude's hues are made in the theme's manner
        let mono = Palette {
            id: "mono".into(),
            bg: Ink::Terminal,
            fg: Ink::Terminal,
            accent: grey(0xe0),
            shine: grey(0xff),
            frame: grey(0x3f),
            muted: grey(0x7c),
            user: grey(0x5b),
            inline: grey(0xcf),
            good: grey(0xd8),
            danger: grey(0x8c),
            ansi: None,
            panel: None,
            ..tokyo()
        };
        stands_out(&mono);
        // paper: near-black accent on white
        let paper = Palette {
            id: "paper".into(),
            light: true,
            bg: Ink::Rgb(0xfb, 0xfb, 0xfa),
            fg: Ink::Rgb(0x24, 0x29, 0x2f),
            accent: Ink::Rgb(0x1f, 0x23, 0x28),
            shine: Ink::Rgb(0x09, 0x69, 0xda),
            frame: Ink::Rgb(0xd0, 0xd7, 0xde),
            muted: Ink::Rgb(0x6e, 0x77, 0x81),
            user: Ink::Rgb(0xdb, 0xe4, 0xee),
            inline: Ink::Rgb(0x9a, 0x67, 0x00),
            good: Ink::Rgb(0x1a, 0x7f, 0x37),
            danger: Ink::Rgb(0xcf, 0x22, 0x2e),
            ansi: None,
            panel: None,
            ..tokyo()
        };
        stands_out(&paper);
        assert_eq!(theme_json(&paper)["base"], "light");
    }

    #[test]
    fn accent_palette_leaves_the_terminal_its_own() {
        let p = Palette { bg: Ink::Terminal, fg: Ink::Terminal, ansi: None, panel: None, id: "graphite".into(), base: "graphite".into(), ..tokyo() };
        stands_out(&p);
        let v = theme_json(&p);
        let o = &v["overrides"];
        assert!(o.get("text").is_none() && o.get("inverseText").is_none() && o.get("clawd_background").is_none());
        // the prompt's tint is washed into the dark bro assumes behind it
        assert_eq!(o["userMessageBackground"], hex(mix(DARK_BG, [0x28, 0x34, 0x57], 0.6)));
        assert!(v["bro"]["palette"].get("bg").is_none());
    }

    #[test]
    fn terminal_theme_keeps_claudes_own_ansi_colours() {
        let p = Palette {
            id: "terminal".into(),
            base: "terminal".into(),
            light: false,
            rainbow: false,
            marker: "▶".into(),
            bg: Ink::Terminal,
            fg: Ink::Terminal,
            accent: Ink::Ansi(4),
            shine: Ink::Ansi(14),
            frame: Ink::Ansi(8),
            muted: Ink::Ansi(8),
            user: Ink::Ansi(8),
            inline: Ink::Ansi(3),
            good: Ink::Ansi(2),
            danger: Ink::Ansi(1),
            ansi: None,
            panel: None,
        };
        let v = theme_json(&p);
        assert_eq!(v["base"], "dark-ansi");
        assert!(v["overrides"].as_object().unwrap().is_empty(), "the ANSI base keeps Claude's own picks");
        assert_eq!(v["bro"]["palette"]["accent"], "blue");
        assert_eq!(v["bro"]["palette"]["muted"], "blackBright");
    }

    #[test]
    fn colour_maths() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((hue([0xd7, 0x77, 0x57]) - 15.0).abs() < 0.5, "Claude's orange");
        let (h, s, l) = hsl([0xd7, 0x77, 0x57]);
        let back = from_hsl(h, s, l);
        assert!(alike(back, [0xd7, 0x77, 0x57]));
        // pushed until it reads, hue kept
        let dim = [0x56, 0x5f, 0x89];
        let up = readable(dim, [0x1a, 0x1b, 0x26], 4.5);
        assert!(contrast(up, [0x1a, 0x1b, 0x26]) >= 4.5 && (hue(up) - hue(dim)).abs() < 3.0);
        let down = readable([0xe0, 0xaf, 0x68], [0xff, 0xff, 0xff], 4.5);
        assert!(contrast(down, [0xff, 0xff, 0xff]) >= 4.5);
    }

    #[test]
    fn labels_read_like_names() {
        assert_eq!(label("tokyo-night-storm"), "Tokyo Night Storm");
        assert_eq!(label("github-dark"), "GitHub Dark");
        assert_eq!(label("synthwave-84"), "SynthWave '84");
        assert_eq!(label("my_theme"), "My Theme");
    }

    #[test]
    fn writes_into_each_config_dir_once() {
        let sb = sandbox();
        let local = sb.home().join(".claude");
        std::fs::create_dir_all(&local).unwrap();
        let account = crate::paths::claude_accounts_dir().join("work");
        std::fs::create_dir_all(&account).unwrap();
        let p = tokyo();
        let written = write_everywhere(&p);
        assert_eq!(written.len(), 2);
        let text = std::fs::read_to_string(path(&account)).unwrap();
        assert!(text.contains("bro · Tokyo Night"));
        assert!(write_everywhere(&p).is_empty(), "unchanged: nothing rewritten");
        let ember = Palette { id: "ember".into(), accent: Ink::Rgb(255, 95, 58), ..p };
        assert_eq!(write_everywhere(&ember).len(), 2);
    }
}
