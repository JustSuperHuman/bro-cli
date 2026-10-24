//! Skins: what a theme changes beyond colour — border shapes, how titles are set into the frame, the focused
//! session's marker, a backdrop drawn into empty space (stars, graph paper, hatching, digital rain) and
//! whole-screen effects (a background gradient, CRT phosphor + scanlines, a vignette, and in the desktop app a
//! neon glow).
//!
//! Backdrops only fill cells that are still blank and on the theme's background after the panes are drawn, so
//! they never sit under text. Effects run last, over everything bro drew.

use crate::theme::{Theme, mix};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
};

/// How title text is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Case {
    Keep,
    Upper,
    /// ｆｕｌｌｗｉｄｔｈ
    Wide,
}

/// What fills empty space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Backdrop {
    None,
    /// sparse twinkling stars
    Stars,
    /// blueprint graph paper
    Grid,
    /// faint diagonal hatching
    Hatch,
    /// falling katakana
    Rain,
}

impl Backdrop {
    /// Moves on its own, so the screen redraws every so often.
    pub fn animated(self) -> bool {
        self == Backdrop::Rain
    }
}

/// Whole-screen effects, applied after everything is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Fx {
    /// the theme background fades from the first colour (top) to the second (bottom)
    pub gradient: Option<(Color, Color)>,
    /// every colour becomes a shade of the theme's text colour, like a monochrome CRT
    pub phosphor: bool,
    /// every other row a touch darker
    pub scanlines: bool,
    /// edges and corners darken
    pub vignette: bool,
    /// bright, saturated colours bloom like neon (0 = off, 1 = SynthWave '84 strong); the desktop app only
    pub glow: f32,
}

impl Fx {
    /// Something the terminal UI can draw (the glow needs a GPU).
    pub fn in_terminal(&self) -> bool {
        self.gradient.is_some() || self.phosphor || self.scanlines || self.vignette
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skin {
    /// frames and unfocused sessions
    pub border: border::Set<'static>,
    /// focused frames (sidebar, views, popups)
    pub focus: border::Set<'static>,
    /// the focused session
    pub session_focus: border::Set<'static>,
    /// set either side of a frame title
    pub brackets: (&'static str, &'static str),
    pub case: Case,
    /// titles drawn as a solid tab (theme background on the border colour)
    pub invert_titles: bool,
    /// before the focused session's title
    pub marker: &'static str,
    pub backdrop: Backdrop,
    pub fx: Fx,
}

impl Default for Skin {
    fn default() -> Self {
        Skin {
            border: border::ROUNDED,
            focus: border::ROUNDED,
            session_focus: border::THICK,
            brackets: ("", ""),
            case: Case::Keep,
            invert_titles: false,
            marker: "▶",
            backdrop: Backdrop::None,
            fx: Fx::default(),
        }
    }
}

/// `+--+` boxes.
pub const ASCII: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// A border by name, for theme files: rounded, plain, double, thick, dashed, heavy-dashed, ascii, block.
pub fn border_named(name: &str) -> Option<border::Set<'static>> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "rounded" => border::ROUNDED,
        "plain" | "square" => border::PLAIN,
        "double" => border::DOUBLE,
        "thick" | "heavy" => border::THICK,
        "dashed" => border::LIGHT_DOUBLE_DASHED,
        "heavy-dashed" => border::HEAVY_DOUBLE_DASHED,
        "ascii" => ASCII,
        "block" => border::QUADRANT_OUTSIDE,
        _ => return None,
    })
}

fn hex(s: &str) -> Color {
    crate::theme::hex(s).unwrap_or(Color::Reset)
}

/// The skin a built-in theme wears.
pub fn for_theme(name: &str) -> Skin {
    let d = Skin::default();
    match name {
        "phosphor" => Skin {
            border: border::PLAIN,
            focus: border::DOUBLE,
            session_focus: border::DOUBLE,
            brackets: ("[ ", " ]"),
            case: Case::Upper,
            marker: ">",
            fx: Fx { phosphor: true, scanlines: true, vignette: true, ..Fx::default() },
            ..d
        },
        "outrun" => Skin {
            border: border::PLAIN,
            focus: border::DOUBLE,
            session_focus: border::DOUBLE,
            brackets: ("◢ ", " ◣"),
            backdrop: Backdrop::Stars,
            fx: Fx { gradient: Some((hex("#0a001c"), hex("#3a0a4a"))), glow: 0.6, ..Fx::default() },
            ..d
        },
        // the editor's famous neon: every bright colour glows, on a dusk-purple sky
        "synthwave-84" => Skin { marker: "✦", fx: Fx { gradient: Some((hex("#2b1f42"), hex("#1c1727"))), glow: 1.0, ..Fx::default() }, ..d },
        // a galaxy: stars in every empty corner
        "andromeda" => Skin { marker: "✦", backdrop: Backdrop::Stars, ..d },
        "blueprint" => Skin {
            border: border::LIGHT_DOUBLE_DASHED,
            focus: border::PLAIN,
            session_focus: border::DOUBLE,
            brackets: ("┤ ", " ├"),
            case: Case::Upper,
            marker: "◇",
            backdrop: Backdrop::Grid,
            ..d
        },
        "cyber" => Skin {
            border: border::PLAIN,
            focus: border::THICK,
            session_focus: border::THICK,
            brackets: ("▐", "▌"),
            case: Case::Upper,
            invert_titles: true,
            marker: "◆",
            backdrop: Backdrop::Hatch,
            fx: Fx { glow: 0.45, ..Fx::default() },
        },
        "digital-rain" => Skin {
            border: border::PLAIN,
            focus: border::PLAIN,
            session_focus: border::THICK,
            brackets: ("[ ", " ]"),
            marker: "»",
            backdrop: Backdrop::Rain,
            ..d
        },
        "vaporwave" => Skin {
            border: border::DOUBLE,
            focus: border::DOUBLE,
            session_focus: border::DOUBLE,
            case: Case::Wide,
            marker: "✦",
            backdrop: Backdrop::Stars,
            fx: Fx { gradient: Some((hex("#2a1048"), hex("#0c2d3d"))), glow: 0.6, ..Fx::default() },
            ..d
        },
        "nebula" => Skin { marker: "✦", backdrop: Backdrop::Stars, fx: Fx { vignette: true, glow: 0.3, ..Fx::default() }, ..d },
        _ => d,
    }
}

// ------------------------------------------------------------------ titles

fn wide(s: &str) -> String {
    s.chars().map(|c| if ('!'..='~').contains(&c) { char::from_u32(c as u32 + 0xFEE0).unwrap_or(c) } else { c }).collect()
}

/// A frame title dressed in the skin: brackets, case, an inverted tab. `edge` is the border's colour.
pub fn title<'a>(line: Line<'a>, edge: Color, t: &Theme) -> Line<'a> {
    let s = &t.skin;
    if line.spans.is_empty() || (s.brackets == ("", "") && s.case == Case::Keep && !s.invert_titles) {
        return line;
    }
    let tab = Style::default().fg(if t.is_light() { t.bg } else { t.deep() }).bg(edge).add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'a>> = line
        .spans
        .into_iter()
        .map(|sp| {
            let text = match s.case {
                Case::Keep => sp.content,
                Case::Upper => sp.content.to_uppercase().into(),
                Case::Wide => wide(&sp.content).into(),
            };
            Span::styled(text, if s.invert_titles { tab } else { sp.style })
        })
        .collect();
    let edge_style = Style::default().fg(edge);
    if !s.brackets.0.is_empty() {
        spans.insert(0, Span::styled(s.brackets.0, edge_style));
    }
    if !s.brackets.1.is_empty() {
        spans.push(Span::styled(s.brackets.1, edge_style));
    }
    Line::from(spans)
}

// ------------------------------------------------------------------ backdrop

fn hash(x: u16, y: u16, z: u64) -> u64 {
    let mut h = 0xcbf29ce484222325u64 ^ z.wrapping_mul(0x9e3779b97f4a7c15);
    for b in [x as u64, y as u64] {
        h ^= b;
        h = h.wrapping_mul(0x100000001b3);
        h ^= h >> 29;
    }
    h
}

/// A cell that's blank and on the theme's own background.
fn empty(buf: &Buffer, x: u16, y: u16, bg: Color) -> bool {
    let c = &buf[(x, y)];
    c.symbol() == " " && c.bg == bg
}

/// Cells the backdrop may fill: inside a run of at least 6 blanks, 2 cells clear of whatever ends the run, so
/// gaps between words, short indents and padding inside titles stay blank.
fn open_space(buf: &Buffer, area: Rect, bg: Color) -> Vec<bool> {
    const MARGIN: usize = 2;
    let w = area.width as usize;
    let mut open = vec![false; w * area.height as usize];
    for (row, y) in (area.top()..area.bottom()).enumerate() {
        let mut x = 0;
        while x < w {
            if !empty(buf, area.x + x as u16, y, bg) {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && empty(buf, area.x + x as u16, y, bg) {
                x += 1;
            }
            if x - start >= 6 {
                open[row * w + start + MARGIN..row * w + x - MARGIN].fill(true);
            }
        }
    }
    open
}

const KANA: &[&str] = &[
    "ｱ", "ｲ", "ｳ", "ｴ", "ｵ", "ｶ", "ｷ", "ｸ", "ｹ", "ｺ", "ｻ", "ｼ", "ｽ", "ｾ", "ｿ", "ﾀ", "ﾁ", "ﾂ", "ﾃ", "ﾄ", "ﾅ", "ﾆ", "ﾇ", "ﾈ", "ﾊ", "ﾋ",
    "ﾌ", "ﾍ", "ﾎ", "ﾏ", "ﾐ", "ﾑ", "ﾒ", "ﾓ", "ﾔ", "ﾕ", "ﾗ", "ﾘ", "ﾜ", "0", "1", "2", "3", "4", "5", "7", "8", "9", "Z", ":", "=",
];

/// Fill empty space in `area`, outside every rect in `skip`, with backdrop `bd` (usually the skin's own).
/// `time` in seconds drives the moving ones.
pub fn backdrop(buf: &mut Buffer, area: Rect, t: &Theme, time: f64, bd: Backdrop, skip: &[Rect]) {
    if bd == Backdrop::None || area.width == 0 || area.height == 0 {
        return;
    }
    // blank cells are on the theme background; colours blend from it (or a near-black on accent-only themes)
    let ink = if t.painted() { t.bg } else { t.deep() };
    let bg = ink;
    let faint = |k: f32| mix(bg, t.fg, k);
    let open = open_space(buf, area, t.bg);
    let mut put = |buf: &mut Buffer, x: u16, y: u16, sym: &str, c: Color| {
        if open[(y - area.y) as usize * area.width as usize + (x - area.x) as usize] && !skip.iter().any(|r| r.contains((x, y).into())) {
            buf[(x, y)].set_symbol(sym).set_fg(c);
        }
    };
    let stars = |buf: &mut Buffer, put: &mut dyn FnMut(&mut Buffer, u16, u16, &str, Color), rows: std::ops::Range<u16>| {
        for y in rows {
            for x in area.left()..area.right() {
                let h = hash(x, y, 7);
                if !h.is_multiple_of(89) {
                    continue;
                }
                let twinkle = 0.5 + 0.5 * ((time * 1.7 + (h % 628) as f64 / 100.0).sin() as f32);
                let (sym, base) = if h.is_multiple_of(7) { ("✦", 0.55) } else if h.is_multiple_of(3) { ("+", 0.35) } else { ("·", 0.4) };
                let tint = if h.is_multiple_of(5) { t.shine } else if h.is_multiple_of(11) { t.accent } else { t.fg };
                put(buf, x, y, sym, mix(bg, tint, base * (0.55 + 0.45 * twinkle)));
            }
        }
    };
    match bd {
        Backdrop::None => {}
        Backdrop::Stars => stars(buf, &mut put, area.top()..area.bottom()),
        Backdrop::Grid => {
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    let (gx, gy) = (x - area.x, y - area.y);
                    if gx.is_multiple_of(16) && gy.is_multiple_of(8) {
                        put(buf, x, y, "+", faint(0.42));
                    } else if gx.is_multiple_of(4) && gy.is_multiple_of(2) {
                        put(buf, x, y, "·", faint(0.3));
                    }
                }
            }
        }
        Backdrop::Hatch => {
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    if (x as u32 + y as u32).is_multiple_of(6) {
                        put(buf, x, y, "╱", mix(bg, t.accent, 0.13));
                    }
                }
            }
        }
        Backdrop::Rain => {
            let h_ = area.height as i64;
            for x in area.left()..area.right() {
                let h = hash(x, 0, 41);
                if !h.is_multiple_of(3) {
                    continue;
                }
                let speed = 5.0 + (h >> 8) as f64 % 9.0;
                let len = 5 + ((h >> 16) % 14) as i64;
                let period = h_ + len + ((h >> 24) % 24) as i64;
                let head = ((time * speed) as i64 + (h >> 32) as i64 % period) % period;
                for d in 0..len {
                    let y = head - d;
                    if !(0..h_).contains(&y) {
                        continue;
                    }
                    let y = area.y + y as u16;
                    // glyphs flicker as the drop falls
                    let g = hash(x, y, (time * 6.0) as u64 / (1 + (d as u64 % 3)));
                    let sym = KANA[(g % KANA.len() as u64) as usize];
                    let c = if d == 0 { mix(t.good, Color::Rgb(255, 255, 255), 0.6) } else { mix(bg, t.good, 0.85 * (1.0 - d as f32 / len as f32) + 0.1) };
                    put(buf, x, y, sym, c);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ effects

/// The xterm 256-colour palette as RGB (16..=255; the first 16 are the theme's or the terminal's).
fn indexed_rgb(i: u8) -> Option<(u8, u8, u8)> {
    match i {
        16..=231 => {
            let i = i - 16;
            let v = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            Some((v(i / 36), v((i / 6) % 6), v(i % 6)))
        }
        232..=255 => {
            let g = 8 + (i - 232) * 10;
            Some((g, g, g))
        }
        _ => None,
    }
}

fn rgb(c: Color, t: &Theme) -> Option<(u8, u8, u8)> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(i) => match t.ansi {
            Some(a) if i < 16 => rgb(a[i as usize], t),
            _ => indexed_rgb(i),
        },
        _ => None,
    }
}

fn darken(c: Color, k: f32) -> Color {
    match c {
        Color::Rgb(..) => mix(c, Color::Rgb(0, 0, 0), k),
        other => other,
    }
}

/// Run the skin's whole-screen effects over what's been drawn.
pub fn effects(buf: &mut Buffer, area: Rect, t: &Theme) {
    let fx = t.skin.fx;
    if !fx.in_terminal() || area.width == 0 || area.height == 0 {
        return;
    }
    let (w, h) = (area.width as f32, area.height as f32);
    for y in area.top()..area.bottom() {
        let fy = (y - area.y) as f32 / (h - 1.0).max(1.0);
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            // kitty image placeholders carry their image id in the colour: leave them alone
            if cell.symbol().starts_with('\u{10EEEE}') {
                continue;
            }
            if let Some((top, bottom)) = fx.gradient
                && cell.bg == t.bg
            {
                cell.bg = mix(top, bottom, fy);
            }
            if fx.phosphor {
                let glow = |c: Color| match rgb(c, t) {
                    Some((r, g, b)) => {
                        let l = (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) / 255.0;
                        mix(t.bg, t.fg, l.powf(0.8))
                    }
                    None => c,
                };
                cell.fg = glow(cell.fg);
                cell.bg = glow(cell.bg);
            }
            let mut dim = 0.0;
            if fx.vignette {
                let dx = ((x - area.x) as f32 / (w - 1.0).max(1.0) - 0.5) * 2.0;
                let dy = (fy - 0.5) * 2.0;
                dim += ((dx * dx + dy * dy) / 2.0).powf(1.5) * 0.4;
            }
            if fx.scanlines && (y - area.y) % 2 == 1 {
                dim += 0.12;
            }
            if dim > 0.0 {
                cell.fg = darken(cell.fg, dim * 0.6);
                cell.bg = darken(cell.bg, dim);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_take_the_skin() {
        let mut t = crate::theme::get("graphite");
        let plain = Line::from(" bro ");
        assert_eq!(title(plain.clone(), t.accent, &t).to_string(), " bro ", "the default skin leaves titles alone");
        t.skin = for_theme("blueprint");
        assert_eq!(title(plain.clone(), t.accent, &t).to_string(), "┤  BRO  ├");
        t.skin = for_theme("vaporwave");
        assert_eq!(title(plain, t.accent, &t).to_string(), " ｂｒｏ ");
        assert_eq!(border_named("ascii"), Some(ASCII));
        assert!(border_named("wiggly").is_none());
    }

    #[test]
    fn backdrops_only_fill_empty_space() {
        for name in ["digital-rain", "blueprint", "outrun", "cyber", "vaporwave", "graphite"] {
            let t = crate::theme::get(name);
            // any theme can have rain
            let bd = if t.skin.backdrop == Backdrop::None { Backdrop::Rain } else { t.skin.backdrop };
            let area = Rect::new(0, 0, 60, 20);
            let mut buf = Buffer::empty(area);
            buf.set_style(area, Style::default().bg(t.bg));
            buf.set_string(2, 3, "hello", Style::default().bg(t.bg));
            // gaps between words and short indents stay blank, on every row
            for y in 0..20 {
                buf.set_string(30, y, "a b  c    d", Style::default().bg(t.bg));
            }
            backdrop(&mut buf, area, &t, 3.0, bd, &[]);
            // a skipped rect stays blank
            let mut kept = Buffer::empty(area);
            kept.set_style(area, Style::default().bg(t.bg));
            backdrop(&mut kept, area, &t, 3.0, bd, &[area]);
            assert!(area.positions().all(|p| kept[(p.x, p.y)].symbol() == " "), "{name}: skip");
            let row: String = (2..7).map(|x| buf[(x, 3)].symbol().to_string()).collect();
            assert_eq!(row, "hello", "{name}: text untouched");
            for y in 0..20 {
                let gaps: String = (30..41).map(|x| buf[(x, y)].symbol().to_string()).collect();
                assert_eq!(gaps, "a b  c    d", "{name}: row {y} word gaps stay blank");
            }
            let filled = area.positions().filter(|p| buf[(p.x, p.y)].symbol() != " ").count();
            assert!(filled > 10, "{name}: backdrop drew something ({filled})");
        }
    }

    #[test]
    fn effects_recolour_the_screen() {
        let t = crate::theme::get("phosphor");
        let area = Rect::new(0, 0, 10, 4);
        let mut buf = Buffer::empty(area);
        buf.set_style(area, Style::default().bg(t.bg));
        buf.set_string(0, 0, "x", Style::default().fg(Color::Rgb(255, 0, 0)).bg(t.bg));
        effects(&mut buf, area, &t);
        let Color::Rgb(r, g, _) = buf[(0, 0)].fg else { panic!("rgb") };
        assert!(g > r, "red text turns phosphor green");
        let Color::Rgb(_, g0, _) = buf[(5, 0)].bg else { panic!("rgb") };
        let Color::Rgb(_, g1, _) = buf[(5, 1)].bg else { panic!("rgb") };
        assert!(g1 <= g0, "scanline row is darker");
    }
}
