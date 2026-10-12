//! Colour themes (ported from z4-oriel). An accent theme sets a handful of colours; background and body text
//! come from the terminal itself (`Color::Reset`) so bro looks native anywhere. A painted theme also brings
//! its own background, text and 16-colour terminal palette, so the agents' output is recoloured with it.
//!
//! * built-in accent palettes: `graphite` (the calm, neutral default), oriel's and `ultra`
//! * painted: `paper`, `tokyo-night`, `catppuccin-mocha`, `catppuccin-latte`, `rose-pine`, `rose-pine-dawn`,
//!   `gruvbox`, `nord`, `kanagawa`, `everforest` and bro's own `nebula`
//! * skinned (painted, plus borders, titles, a backdrop and screen effects — see [`crate::skin`]): `phosphor`,
//!   `outrun`, `blueprint`, `cyber`, `digital-rain`, `vaporwave`
//! * `terminal`: ANSI colours only, so it follows the terminal's own theme
//! * your own: `~/.bro/themes/<name>.toml`, live-reloaded when the file changes:
//!
//! ```toml
//! base = "ultra"      # start from any theme; only list what you change
//! accent = "#ff79c6"
//! rainbow = false
//! ansi = ["#000000", "#ff5555", ...]  # optional: all 16 terminal colours (black..white, then bright)
//! skin = "cyber"      # optional: borrow a built-in theme's borders, titles, backdrop and effects
//! borders = "double"  # optional: rounded, plain, double, thick, dashed, heavy-dashed, ascii, block
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
    /// The sessions' 16 ANSI colours (black, red, green, yellow, blue, magenta, cyan, white, then bright);
    /// None leaves them to the terminal.
    pub ansi: Option<[Color; 16]>,
    /// Borders, titles, backdrop and screen effects.
    pub skin: crate::skin::Skin,
}

// name, accent, shine, frame, muted, user, inline, good, danger
/// (name, accent, shine, frame, muted, user, inline, good, danger)
type Palette = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str);

const PALETTES: &[Palette] = &[
    // neutral: greys with a cool off-white accent — the harness colours carry the colour
    ("graphite", "#d9dde4", "#9fb4cc", "#34363b", "#7c7f87", "#4a4d55", "#e3c27d", "#8fcf9c", "#ef7b7b"),
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

/// A theme that paints everything: its own background, text and terminal palette.
struct Painted {
    name: &'static str,
    bg: &'static str,
    fg: &'static str,
    accent: &'static str,
    shine: &'static str,
    frame: &'static str,
    muted: &'static str,
    user: &'static str,
    inline: &'static str,
    good: &'static str,
    danger: &'static str,
    rainbow: bool,
    /// black, red, green, yellow, blue, magenta, cyan, white, then the bright eight
    ansi: [&'static str; 16],
}

#[rustfmt::skip]
const PAINTED: &[Painted] = &[
    Painted {
        name: "tokyo-night",
        bg: "#1a1b26", fg: "#c0caf5", accent: "#7aa2f7", shine: "#bb9af7", frame: "#3b4261", muted: "#565f89",
        user: "#283457", inline: "#e0af68", good: "#9ece6a", danger: "#f7768e", rainbow: false,
        ansi: ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
               "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5"],
    },
    Painted {
        name: "catppuccin-mocha",
        bg: "#1e1e2e", fg: "#cdd6f4", accent: "#cba6f7", shine: "#89dceb", frame: "#45475a", muted: "#7f849c",
        user: "#313244", inline: "#fab387", good: "#a6e3a1", danger: "#f38ba8", rainbow: false,
        ansi: ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
               "#585b70", "#f37799", "#89d88b", "#ebd391", "#74a8fc", "#f2aede", "#6bd7ca", "#a6adc8"],
    },
    Painted {
        name: "rose-pine",
        bg: "#191724", fg: "#e0def4", accent: "#c4a7e7", shine: "#ebbcba", frame: "#403d52", muted: "#6e6a86",
        user: "#2a283e", inline: "#f6c177", good: "#9ccfd8", danger: "#eb6f92", rainbow: false,
        ansi: ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
               "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"],
    },
    Painted {
        name: "kanagawa",
        bg: "#1f1f28", fg: "#dcd7ba", accent: "#7e9cd8", shine: "#7fb4ca", frame: "#54546d", muted: "#727169",
        user: "#2d4f67", inline: "#e6c384", good: "#98bb6c", danger: "#ff5d62", rainbow: false,
        ansi: ["#16161d", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093",
               "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba"],
    },
    Painted {
        name: "gruvbox",
        bg: "#282828", fg: "#ebdbb2", accent: "#fe8019", shine: "#8ec07c", frame: "#504945", muted: "#928374",
        user: "#3c3836", inline: "#fabd2f", good: "#b8bb26", danger: "#fb4934", rainbow: false,
        ansi: ["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
               "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"],
    },
    Painted {
        name: "nord",
        bg: "#2e3440", fg: "#d8dee9", accent: "#88c0d0", shine: "#81a1c1", frame: "#4c566a", muted: "#7b88a1",
        user: "#434c5e", inline: "#ebcb8b", good: "#a3be8c", danger: "#bf616a", rainbow: false,
        ansi: ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
               "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"],
    },
    Painted {
        name: "everforest",
        bg: "#2d353b", fg: "#d3c6aa", accent: "#a7c080", shine: "#83c092", frame: "#4f585e", muted: "#859289",
        user: "#3d484d", inline: "#dbbc7f", good: "#a7c080", danger: "#e67e80", rainbow: false,
        ansi: ["#343f44", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa",
               "#859289", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa"],
    },
    // bro's own: deep space, neon edges, the rainbow logo
    Painted {
        name: "nebula",
        bg: "#0e0b1e", fg: "#e6defe", accent: "#a682ff", shine: "#5ce1ff", frame: "#2f2756", muted: "#7a70a8",
        user: "#261f4a", inline: "#ffb86b", good: "#6dffb3", danger: "#ff5c8a", rainbow: true,
        ansi: ["#1b1636", "#ff5c8a", "#6dffb3", "#ffd76b", "#6b9bff", "#c77dff", "#5ce1ff", "#d8d0f5",
               "#4a4078", "#ff85a8", "#9dffcc", "#ffe599", "#97b8ff", "#dca8ff", "#8debff", "#ffffff"],
    },
    // CRT terminal: everything in green phosphor, scanlines, a dark vignette
    Painted {
        name: "phosphor",
        bg: "#020a04", fg: "#3dff7d", accent: "#8affab", shine: "#c4ffd6", frame: "#1d6b35", muted: "#2f9a52",
        user: "#0c3018", inline: "#b0ffc6", good: "#3dff7d", danger: "#d8ffe4", rainbow: false,
        ansi: ["#03140a", "#a8ffbe", "#3dff7d", "#c4ffd6", "#2fc864", "#7dffa6", "#5cff92", "#3dff7d",
               "#1d6b35", "#c4ffd6", "#6dff9a", "#e0ffe9", "#4ae07a", "#9dffbb", "#7dffa6", "#e8fff0"],
    },
    // synthwave sunset: a purple gradient sky full of stars, neon edges
    Painted {
        name: "outrun",
        bg: "#12002b", fg: "#f5e6ff", accent: "#ff2e97", shine: "#00e5ff", frame: "#5a2380", muted: "#a07cc0",
        user: "#2e0a4a", inline: "#ffd319", good: "#3cf0a0", danger: "#ff3864", rainbow: true,
        ansi: ["#2b0a3d", "#fe4450", "#72f1b8", "#fede5d", "#03edf9", "#ff7edb", "#36f9f6", "#f5e6ff",
               "#614d85", "#ff6b78", "#9bf7d0", "#ffeb8f", "#5cf3fb", "#ffa6e8", "#7afbf8", "#ffffff"],
    },
    // engineering drawing: white ink on blueprint blue, graph paper, dashed construction lines
    Painted {
        name: "blueprint",
        bg: "#0b2e5c", fg: "#e8f1ff", accent: "#ffffff", shine: "#8fd3ff", frame: "#5b8cc8", muted: "#9ab9e0",
        user: "#174a85", inline: "#ffe08a", good: "#9ef0c0", danger: "#ff9a8a", rainbow: false,
        ansi: ["#0a2548", "#ff9a8a", "#9ef0c0", "#ffe08a", "#8fd3ff", "#d7b8ff", "#7fe8e8", "#e8f1ff",
               "#5d86bb", "#ffb8ab", "#bff7d6", "#fff0b8", "#b9e4ff", "#e6d1ff", "#a8f2f2", "#ffffff"],
    },
    // night city HUD: black and hazard yellow, solid title tabs, heavy frames
    Painted {
        name: "cyber",
        bg: "#07070c", fg: "#e6f7ff", accent: "#fcee0a", shine: "#00f0ff", frame: "#4a4a24", muted: "#8494a3",
        user: "#25250c", inline: "#ff9e00", good: "#00ff9c", danger: "#ff003c", rainbow: false,
        ansi: ["#0d0d14", "#ff003c", "#00ff9c", "#fcee0a", "#00b8ff", "#ff00e5", "#00f0ff", "#d0e0ea",
               "#3a3f4b", "#ff4d73", "#5cffc0", "#fff35c", "#5cd0ff", "#ff5cf0", "#7af7ff", "#ffffff"],
    },
    // there is no spoon: katakana rain falls through every empty corner
    Painted {
        name: "digital-rain",
        bg: "#000400", fg: "#b8ffc8", accent: "#00ff41", shine: "#a6ffbf", frame: "#11592a", muted: "#45995c",
        user: "#072a10", inline: "#d7ff6b", good: "#00ff41", danger: "#ff4545", rainbow: false,
        ansi: ["#001a06", "#ff4545", "#00ff41", "#d7ff6b", "#3cb8ff", "#c084ff", "#4dffd2", "#b8ffc8",
               "#1e5e2e", "#ff7a7a", "#66ff88", "#e8ffa0", "#7ad0ff", "#d6b0ff", "#8affe3", "#eaffef"],
    },
    // ａｅｓｔｈｅｔｉｃ: pastel neon on a dusk gradient, double frames, full-width titles
    Painted {
        name: "vaporwave",
        bg: "#1c1433", fg: "#fdf2ff", accent: "#ff71ce", shine: "#01cdfe", frame: "#6a4c9c", muted: "#ad94d6",
        user: "#3a2560", inline: "#fffb96", good: "#05ffa1", danger: "#ff4d8d", rainbow: true,
        ansi: ["#2a1c4a", "#ff4d8d", "#05ffa1", "#fffb96", "#01cdfe", "#ff71ce", "#7af9ff", "#fdf2ff",
               "#6b55a0", "#ff7fab", "#6dffc6", "#fffdc4", "#6be3ff", "#ffa3e0", "#b2fcff", "#ffffff"],
    },
    Painted {
        name: "catppuccin-latte",
        bg: "#eff1f5", fg: "#4c4f69", accent: "#8839ef", shine: "#1e66f5", frame: "#bcc0cc", muted: "#7c7f93",
        user: "#ccd0da", inline: "#fe640b", good: "#40a02b", danger: "#d20f39", rainbow: false,
        ansi: ["#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
               "#6c6f85", "#de293e", "#49af3d", "#eea02d", "#456eff", "#fe85d8", "#2d9fa8", "#bcc0cc"],
    },
    Painted {
        name: "rose-pine-dawn",
        bg: "#faf4ed", fg: "#575279", accent: "#907aa9", shine: "#d7827e", frame: "#dfdad9", muted: "#797593",
        user: "#f2e9e1", inline: "#ea9d34", good: "#56949f", danger: "#b4637a", rainbow: false,
        ansi: ["#f2e9e1", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
               "#9893a5", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279"],
    },
];

/// The default theme name.
pub const DEFAULT: &str = "graphite";

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
    let mut v: Vec<String> = vec![PALETTES[0].0.to_string(), "paper".into()];
    v.extend(PAINTED.iter().map(|p| p.name.to_string()));
    v.extend(PALETTES[1..].iter().map(|p| p.0.to_string()));
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

pub const PROJECT_COLORS: usize = 8;

/// A stable preferred slot; the workspace resolves collisions among its projects.
pub fn project_color_slot(key: &str) -> usize {
    // FNV-1a, explicitly fixed instead of a platform-dependent/randomized hasher.
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    (hash % PROJECT_COLORS as u64) as usize
}

/// Light themes use darker counterparts; monochrome/terminal themes retain their palette.
pub fn project_color(slot: usize, t: &Theme) -> Color {
    if t.name == "mono" || t.name == "terminal" {
        return t.accent;
    }
    // painted themes pick from their own palette (never red, which reads as an error): blue, magenta, cyan,
    // yellow, green, bright blue, bright magenta, bright cyan
    if let Some(a) = t.ansi {
        return a[[4, 5, 6, 3, 2, 12, 13, 14][slot % 8]];
    }
    let colors = if t.is_light() {
        [0x2864ad, 0x8761b4, 0x237d78, 0xa66028, 0xa84d76, 0x587c2c, 0x4e62ad, 0x96701f]
    } else {
        [0x7eb8f6, 0xc3a1ef, 0x69c8c2, 0xe9ad79, 0xe896b8, 0xa6c879, 0xa0adf7, 0xe4c579]
    };
    let rgb = colors[slot % colors.len()];
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub fn project_tint(color: Color, t: &Theme) -> Color {
    mix(color, if t.is_light() { t.bg } else { t.deep() }, if t.is_light() { 0.9 } else { 0.83 })
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
    if name == "paper" {
        return paper();
    }
    if let Some(p) = PAINTED.iter().find(|p| p.name == name) {
        return painted(p);
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
        ansi: None,
        skin: Default::default(),
    }
}

fn painted(p: &Painted) -> Theme {
    let c = |s| hex(s).unwrap_or(Color::Reset);
    Theme {
        name: p.name.into(),
        bg: c(p.bg),
        fg: c(p.fg),
        accent: c(p.accent),
        shine: c(p.shine),
        frame: c(p.frame),
        muted: c(p.muted),
        user: c(p.user),
        inline: c(p.inline),
        good: c(p.good),
        danger: c(p.danger),
        animated: p.rainbow,
        ansi: Some(p.ansi.map(c)),
        skin: crate::skin::for_theme(p.name),
    }
}

/// Light: bro paints its own white background (and the sessions' default colours with it).
fn paper() -> Theme {
    let c = |s| hex(s).unwrap_or(Color::Reset);
    Theme {
        name: "paper".into(),
        bg: c("#fbfbfa"),
        fg: c("#24292f"),
        accent: c("#1f2328"),
        shine: c("#0969da"),
        frame: c("#d0d7de"),
        muted: c("#6e7781"),
        user: c("#dbe4ee"),
        inline: c("#9a6700"),
        good: c("#1a7f37"),
        danger: c("#cf222e"),
        animated: false,
        ansi: None,
        skin: Default::default(),
    }
}

impl Theme {
    /// A light theme (dark text on a painted light background).
    pub fn is_light(&self) -> bool {
        match self.bg {
            Color::Rgb(r, g, b) => (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 > 140,
            _ => false,
        }
    }

    /// Paints its own background.
    pub fn painted(&self) -> bool {
        !matches!(self.bg, Color::Reset)
    }

    /// The darkest surface: a painted dark theme's background, else a near-black that suits any dark terminal.
    pub fn deep(&self) -> Color {
        if self.painted() && !self.is_light() { self.bg } else { Color::Rgb(18, 18, 22) }
    }

    /// A few colours that sum the theme up, for swatches.
    pub fn swatch(&self) -> [Color; 5] {
        [self.accent, self.shine, self.good, self.inline, self.danger]
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
        ansi: None,
        skin: Default::default(),
    }
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
    if let Some(a) = tbl.get("ansi").and_then(ansi_of) {
        t.ansi = Some(a);
    }
    if let Some(s) = tbl.get("skin").and_then(|v| v.as_str()) {
        t.skin = crate::skin::for_theme(s);
    }
    if let Some(b) = tbl.get("borders").and_then(|v| v.as_str()).and_then(crate::skin::border_named) {
        t.skin.border = b;
        t.skin.focus = b;
    }
    let mut last = LAST_GOOD.lock();
    last.retain(|(n, _)| n != name);
    last.push((name.to_string(), t.clone()));
    Some(t)
}

/// A theme file's `ansi = [16 colours]`.
fn ansi_of(v: &toml::Value) -> Option<[Color; 16]> {
    let v = v.as_array()?;
    let cs: Vec<Color> = v.iter().filter_map(|c| c.as_str().and_then(color)).collect();
    (v.len() == 16 && cs.len() == 16).then(|| std::array::from_fn(|i| cs[i]))
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
                "ansi" => ansi_of(v).is_none().then(|| format!("{file}: ansi is a list of 16 colours")),
                "skin" => v.as_str().filter(|s| builtin_names().iter().any(|n| n == s)).is_none().then(|| format!("{file}: there's no theme called '{v}' to borrow a skin from")),
                "borders" => v.as_str().and_then(crate::skin::border_named).is_none().then(|| format!("{file}: borders is rounded, plain, double, thick, dashed, heavy-dashed, ascii or block")),
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
    fn painted_themes_are_complete_and_readable() {
        let names = builtin_names();
        let lum = |c: Color| match c {
            Color::Rgb(r, g, b) => (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) / 255.0,
            _ => 0.0,
        };
        for p in PAINTED {
            assert!(names.iter().any(|n| n == p.name), "{} listed", p.name);
            let t = get(p.name);
            assert_eq!(t.name, p.name);
            assert!(t.painted() && t.ansi.is_some());
            for c in [t.bg, t.fg, t.accent, t.shine, t.frame, t.muted, t.user, t.inline, t.good, t.danger].into_iter().chain(t.ansi.unwrap()) {
                assert!(matches!(c, Color::Rgb(..)), "{}: every colour parses", p.name);
            }
            // body text and hints stand clear of the background
            assert!((lum(t.fg) - lum(t.bg)).abs() > 0.4, "{}: text contrast", p.name);
            assert!((lum(t.muted) - lum(t.bg)).abs() > 0.2, "{}: hint contrast", p.name);
        }
        let mut seen = std::collections::HashSet::new();
        assert!(names.iter().all(|n| seen.insert(n)), "no duplicate names");
    }

    #[test]
    fn builtins_and_custom_files() {
        let dir = tempfile::tempdir().unwrap();
        TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
        assert!(get("ultra").animated);
        assert_eq!(get("nope").name, "graphite", "unknown names fall back to the default");
        assert!(!get("graphite").animated && !get("graphite").is_light());
        assert!(get("paper").is_light() && names().contains(&"paper".to_string()));
        assert!(names().contains(&"terminal".to_string()));
        std::fs::write(dir.path().join("mine.toml"), "base = \"ocean\"\naccent = \"#ff0000\"\nrainbow = true\nacent = \"x\"\n").unwrap();
        let t = get("mine");
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert!(t.animated);
        assert!(is_custom("mine"));
        assert_eq!(problems("mine").len(), 1);
        assert_eq!(color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
        let sixteen = (0..16).map(|i| format!("\"#0000{i:02x}\"")).collect::<Vec<_>>().join(", ");
        std::fs::write(dir.path().join("pal.toml"), format!("base = \"nord\"\nansi = [{sixteen}]\n")).unwrap();
        assert_eq!(get("pal").ansi.unwrap()[15], Color::Rgb(0, 0, 15));
        assert!(problems("pal").is_empty(), "{:?}", problems("pal"));
        std::fs::write(dir.path().join("pal.toml"), "ansi = [\"#000000\"]\n").unwrap();
        assert_eq!(problems("pal").len(), 1);
        std::fs::write(dir.path().join("pal.toml"), "base = \"nord\"\nskin = \"cyber\"\nborders = \"ascii\"\n").unwrap();
        let t = get("pal");
        assert!(t.skin.invert_titles && t.skin.border == crate::skin::ASCII, "borrowed skin, own borders");
        assert!(problems("pal").is_empty(), "{:?}", problems("pal"));
        std::fs::write(dir.path().join("pal.toml"), "skin = \"nope\"\nborders = \"wiggly\"\n").unwrap();
        assert_eq!(problems("pal").len(), 2);
        TEST_DIR.with(|d| *d.borrow_mut() = None);
    }
}
