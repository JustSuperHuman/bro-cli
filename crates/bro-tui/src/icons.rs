//! The real harness logos (Claude, Codex) drawn as small images where bro shows a harness glyph.
//!
//! Port of bro v1's `icons.js` + `term-images.js`. When a terminal can show pictures, [`crate::ui::harness_glyph`]
//! hands out a marker character instead of the text glyph (one cell, like the glyph, so layout is unchanged; the
//! marker's cell plus the space every call site puts after it make the 2-cell icon). After the frame is drawn into
//! the buffer, [`place`] turns each marker into:
//!
//! - **sixel** (Windows Terminal 1.22+, xterm, foot, Konsole): two blank cells, painted over with a sixel image by
//!   [`flush`] once `Terminal::draw` has written the frame. Only placements that are new or whose cells changed are
//!   re-emitted; placements that vanish get their cells rewritten so no picture is left behind.
//! - **iterm** (iTerm2, WezTerm): the same, with an OSC 1337 inline PNG.
//! - **kitty** (kitty, Ghostty): Unicode placeholder characters (U+10EEEE + row/column diacritics, image id in the
//!   foreground colour). They behave like text, so ratatui's own diff handles repaints; the image is uploaded once.
//!
//! Where an icon can't go (no room for the second cell, the last screen row for the cursor-moving protocols) the
//! marker becomes the text glyph again. Text mode (the default, and always under tests) never hands out markers,
//! so nothing here runs.
//!
//! Mode: `BRO_ICONS=sixel|kitty|iterm|text|auto`, else `icons` in `~/.bro/v2.toml`, else detected from the
//! environment (see [`detect`]).

use bro_core::Harness;
use parking_lot::Mutex;
use ratatui::{
    DefaultTerminal,
    backend::Backend,
    buffer::{Buffer, Cell},
    layout::Rect,
    style::Color,
};
use std::{
    collections::HashMap,
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

const CLAUDE_PNG: &[u8] = include_bytes!("../assets/claude.png");
const CODEX_PNG: &[u8] = include_bytes!("../assets/codex.png");

/// Supplementary Private Use Area plane 16: never in real text. One cell wide, like the glyphs they stand for.
const CLAUDE_MARK: &str = "\u{10FF00}";
const CODEX_MARK: &str = "\u{10FF01}";

/// Every icon is two cells wide, one row tall.
const ICON_CELLS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Text,
    Sixel,
    Kitty,
    Iterm,
}

impl Mode {
    fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sixel" => Some(Mode::Sixel),
            "kitty" => Some(Mode::Kitty),
            "iterm" | "iterm2" => Some(Mode::Iterm),
            "text" | "off" | "none" | "0" | "false" => Some(Mode::Text),
            _ => None,
        }
    }
    /// Sixel and iTerm images are drawn at the cursor and move it; on the last row that would scroll the screen.
    fn moves_cursor(self) -> bool {
        matches!(self, Mode::Sixel | Mode::Iterm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Icon {
    Claude,
    Codex,
}

impl Icon {
    const ALL: [Icon; 2] = [Icon::Claude, Icon::Codex];
    fn from_symbol(s: &str) -> Option<Icon> {
        match s {
            CLAUDE_MARK => Some(Icon::Claude),
            CODEX_MARK => Some(Icon::Codex),
            _ => None,
        }
    }
    fn png(self) -> &'static [u8] {
        match self {
            Icon::Claude => CLAUDE_PNG,
            Icon::Codex => CODEX_PNG,
        }
    }
    /// The text glyph shown when the image can't be.
    fn fallback(self) -> &'static str {
        crate::ui::icon(match self {
            Icon::Claude => "claude",
            Icon::Codex => "codex",
        })
    }
    /// kitty image id; carried in a 256-colour foreground, so < 256.
    fn kitty_id(self) -> u8 {
        match self {
            Icon::Claude => 201,
            Icon::Codex => 202,
        }
    }
}

// ------------------------------------------------------------------ detection

/// The icon mode for this run. `env` looks up an environment variable; `setting` is `Settings::icons`.
pub fn detect(setting: &str, env: &dyn Fn(&str) -> Option<String>) -> Mode {
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(m) = var("BRO_ICONS").as_deref().and_then(Mode::parse) {
        return m;
    }
    if let Some(m) = Mode::parse(setting) {
        return m;
    }
    let term = var("TERM").unwrap_or_default();
    let program = var("TERM_PROGRAM").unwrap_or_default();
    // Nested in a bro pane (our vt100 parser draws no images), a multiplexer (would need passthrough wrapping),
    // or plain mode.
    if var("BRO").is_some() || var("BRO_PLAIN").is_some() || var("TMUX").is_some() || term.starts_with("screen") || term.starts_with("tmux") {
        return Mode::Text;
    }
    if var("KITTY_WINDOW_ID").is_some() || term == "xterm-kitty" || program == "ghostty" || term == "xterm-ghostty" {
        return Mode::Kitty;
    }
    if program == "iTerm.app" || var("LC_TERMINAL").as_deref() == Some("iTerm2") || program == "WezTerm" {
        return Mode::Iterm;
    }
    // VS Code's terminal inherits WT_SESSION when launched from Windows Terminal but draws no sixel by default.
    if program == "vscode" {
        return Mode::Text;
    }
    if var("WT_SESSION").is_some() || term.starts_with("foot") || var("KONSOLE_VERSION").is_some() || var("XTERM_VERSION").is_some() {
        return Mode::Sixel;
    }
    Mode::Text
}

/// Cell size in pixels for sixel. Windows Terminal draws sixels as if every cell were 10×20 and scales them to the
/// real cell, whatever the font; elsewhere ask the tty, and assume 10×20 if it won't say.
fn cell_pixels() -> (u16, u16) {
    if std::env::var_os("WT_SESSION").is_some() {
        return (10, 20);
    }
    match crossterm::terminal::window_size() {
        Ok(s) if s.width > 0 && s.height > 0 && s.columns > 0 && s.rows > 0 => ((s.width / s.columns).max(4), (s.height / s.rows).max(8)),
        _ => (10, 20),
    }
}

// ------------------------------------------------------------------ global hooks

static ENABLED: AtomicBool = AtomicBool::new(false);
static PAINTER: Mutex<Option<Painter>> = Mutex::new(None);

/// Settle the icon mode for this run (call once, with a real terminal). No-op under tests.
pub fn init(setting: &str) {
    if cfg!(test) {
        return;
    }
    let mode = detect(setting, &|k| std::env::var(k).ok());
    if mode == Mode::Text {
        return;
    }
    if let Some(p) = Painter::new(mode, cell_pixels()) {
        *PAINTER.lock() = Some(p);
        ENABLED.store(true, Ordering::Relaxed);
    }
}

/// The marker [`crate::ui::harness_glyph`] should return instead of its glyph, when images are on.
pub fn marker(h: Option<Harness>) -> Option<&'static str> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    match h {
        Some(Harness::Claude) => Some(CLAUDE_MARK),
        Some(Harness::Codex) => Some(CODEX_MARK),
        _ => None,
    }
}

/// Inside the draw closure, after everything is drawn: resolve the frame's markers (see module docs).
pub fn place(buf: &mut Buffer) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(p) = PAINTER.lock().as_mut() {
        p.place(buf);
    }
}

/// After `Terminal::draw`: paint the images the last [`place`] asked for.
pub fn flush(term: &mut DefaultTerminal) -> std::io::Result<()> {
    if !ENABLED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let Some(out) = PAINTER.lock().as_mut().map(Painter::take) else { return Ok(()) };
    if out.restore.is_empty() && out.bytes.is_empty() {
        return Ok(());
    }
    let b = term.backend_mut();
    // Synchronized update (ignored where unsupported) so the cursor's trip around the screen never shows;
    // save/restore cursor + attributes so ratatui's cursor stays where it put it.
    b.write_all(b"\x1b[?2026h\x1b7")?;
    if !out.restore.is_empty() {
        Backend::draw(b, out.restore.iter().map(|(x, y, c)| (*x, *y, c)))?;
    }
    b.write_all(&out.bytes)?;
    b.write_all(b"\x1b8\x1b[?2026l")?;
    std::io::Write::flush(b)
}

// ------------------------------------------------------------------ placement diffing

#[derive(Debug, Clone, PartialEq)]
struct Placement {
    x: u16,
    y: u16,
    icon: Icon,
    /// the two cells as written to the terminal; unchanged next frame = ratatui left them (and the image) alone
    cells: [Cell; 2],
}

#[derive(Debug, Default)]
struct Output {
    /// cells to rewrite (erasing an image left on them)
    restore: Vec<(u16, u16, Cell)>,
    /// image sequences, each preceded by a cursor move
    bytes: Vec<u8>,
}

struct Painter {
    mode: Mode,
    /// escape sequence per icon: the sixel / iTerm image, or the kitty upload
    images: HashMap<Icon, Vec<u8>>,
    kitty_uploaded: Vec<Icon>,
    prev: Vec<Placement>,
    prev_area: Option<Rect>,
    out: Output,
}

impl Painter {
    /// None if an image can't be built (the caller stays in text mode).
    fn new(mode: Mode, (cw, ch): (u16, u16)) -> Option<Painter> {
        let mut images = HashMap::new();
        for icon in Icon::ALL {
            let img = Rgba::decode(icon.png())?.inked(INK);
            let seq = match mode {
                Mode::Text => return None,
                Mode::Sixel => sixel_mark(&img, (cw * ICON_CELLS) as u32, ch as u32).into_bytes(),
                Mode::Iterm => iterm_image(&img.encode_png()?, ICON_CELLS, 1).into_bytes(),
                Mode::Kitty => kitty_upload(&img, icon.kitty_id(), ICON_CELLS, 1).into_bytes(),
            };
            images.insert(icon, seq);
        }
        Some(Painter { mode, images, kitty_uploaded: vec![], prev: vec![], prev_area: None, out: Output::default() })
    }

    fn take(&mut self) -> Output {
        std::mem::take(&mut self.out)
    }

    fn place(&mut self, buf: &mut Buffer) {
        let area = buf.area;
        if self.prev_area != Some(area) {
            // ratatui cleared the screen for the new size: nothing of ours is on it any more
            self.prev.clear();
            self.out = Output::default();
            self.prev_area = Some(area);
        }
        let mut cur = vec![];
        for y in area.top()..area.bottom() {
            let mut x = area.left();
            while x < area.right() {
                let Some(icon) = Icon::from_symbol(buf[(x, y)].symbol()) else {
                    x += 1;
                    continue;
                };
                let room = x + 1 < area.right() && buf[(x + 1, y)].symbol() == " ";
                let last_row = y + 1 >= area.bottom();
                if !room || (self.mode.moves_cursor() && last_row) {
                    buf[(x, y)].set_symbol(icon.fallback());
                    x += 1;
                    continue;
                }
                if self.mode == Mode::Kitty {
                    let id = Color::Indexed(icon.kitty_id());
                    buf[(x, y)].set_symbol(KITTY_CELLS[0]).set_fg(id);
                    buf[(x + 1, y)].set_symbol(KITTY_CELLS[1]).set_fg(id);
                } else {
                    buf[(x, y)].set_symbol(" ");
                }
                cur.push(Placement { x, y, icon, cells: [buf[(x, y)].clone(), buf[(x + 1, y)].clone()] });
                x += ICON_CELLS;
            }
        }

        if self.mode == Mode::Kitty {
            for p in &cur {
                if !self.kitty_uploaded.contains(&p.icon) {
                    self.kitty_uploaded.push(p.icon);
                    self.out.bytes.extend_from_slice(&self.images[&p.icon]);
                }
            }
            self.prev = cur;
            return;
        }

        // Placements gone (or changed) since last frame: rewrite their cells from this frame, so an image the text
        // redraw didn't touch (a blank that stayed a blank) isn't left on screen. Done before the new images, which
        // may land on the same cells.
        for q in self.prev.iter().filter(|q| !cur.contains(q)) {
            for dx in 0..ICON_CELLS {
                let (x, y) = (q.x + dx, q.y);
                if x < area.right() && y < area.bottom() && !self.out.restore.iter().any(|r| (r.0, r.1) == (x, y)) {
                    self.out.restore.push((x, y, buf[(x, y)].clone()));
                }
            }
        }
        for p in cur.iter().filter(|p| !self.prev.contains(p)) {
            self.out.bytes.extend_from_slice(format!("\x1b[{};{}H", p.y + 1, p.x + 1).as_bytes());
            self.out.bytes.extend_from_slice(&self.images[&p.icon]);
        }
        self.prev = cur;
    }
}

// ------------------------------------------------------------------ images

/// The marks are drawn white (bro's themes are dark).
const INK: [u8; 3] = [255, 255, 255];

struct Rgba {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Rgba {
    fn decode(bytes: &[u8]) -> Option<Rgba> {
        let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
        let mut reader = dec.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        let px = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            _ => return None,
        };
        (px.len() == (w * h * 4) as usize).then_some(Rgba { w, h, px })
    }

    /// Every pixel the ink colour, keeping its alpha.
    fn inked(mut self, ink: [u8; 3]) -> Rgba {
        for p in self.px.chunks_exact_mut(4) {
            p[..3].copy_from_slice(&ink);
        }
        self
    }

    fn encode_png(&self) -> Option<Vec<u8>> {
        let mut out = vec![];
        let mut enc = png::Encoder::new(&mut out, self.w, self.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&self.px).ok()?;
        w.finish().ok()?;
        Some(out)
    }

    /// Area-averaged scale to `tw`×`th`, in premultiplied alpha so transparent edges don't darken.
    fn resized(&self, tw: u32, th: u32) -> Rgba {
        let mut out = vec![0u8; (tw * th * 4) as usize];
        let (sx, sy) = (self.w as f64 / tw as f64, self.h as f64 / th as f64);
        for ty in 0..th {
            for tx in 0..tw {
                let (x0, y0) = (tx as f64 * sx, ty as f64 * sy);
                let (x1, y1) = (x0 + sx, y0 + sy);
                let (mut rgb, mut a, mut area) = ([0f64; 3], 0f64, 0f64);
                for y in y0.floor() as u32..(y1.ceil() as u32).min(self.h) {
                    let wy = (y1.min(y as f64 + 1.0) - y0.max(y as f64)).max(0.0);
                    for x in x0.floor() as u32..(x1.ceil() as u32).min(self.w) {
                        let wgt = (x1.min(x as f64 + 1.0) - x0.max(x as f64)).max(0.0) * wy;
                        let o = ((y * self.w + x) * 4) as usize;
                        let alpha = self.px[o + 3] as f64 / 255.0;
                        for (c, v) in rgb.iter_mut().enumerate() {
                            *v += self.px[o + c] as f64 * alpha * wgt;
                        }
                        a += alpha * wgt;
                        area += wgt;
                    }
                }
                let o = ((ty * tw + tx) * 4) as usize;
                if a > 0.0 {
                    for (c, v) in rgb.iter().enumerate() {
                        out[o + c] = (v / a).round() as u8;
                    }
                }
                out[o + 3] = if area > 0.0 { ((a / area) * 255.0).round() as u8 } else { 0 };
            }
        }
        Rgba { w: tw, h: th, px: out }
    }
}

/// Sixel has no partial transparency: soft edges are thickened a little (at one text row Codex's strokes are
/// thinner than a pixel) and then cut at half coverage; uncovered pixels stay unpainted so the cell's own
/// background (selection highlight included) shows through.
const STROKE_GAMMA: f64 = 0.7;

fn sixel_mark(img: &Rgba, w: u32, h: u32) -> String {
    let small = img.resized(w, h);
    let mut palette: Vec<[u8; 3]> = vec![];
    let idx: Vec<i16> = small
        .px
        .chunks_exact(4)
        .map(|p| {
            if (p[3] as f64 / 255.0).powf(STROKE_GAMMA) < 0.5 {
                return -1;
            }
            let rgb = [p[0], p[1], p[2]];
            match palette.iter().position(|c| *c == rgb) {
                Some(i) => i as i16,
                None if palette.len() < 256 => {
                    palette.push(rgb);
                    (palette.len() - 1) as i16
                }
                None => 0,
            }
        })
        .collect();
    encode_sixel(w, h, &palette, &idx)
}

/// A DECSIXEL image: `idx[y * w + x]` is a palette index, or -1 for an unpainted (transparent, P2=1) pixel.
/// The raster attributes ask for square pixels.
fn encode_sixel(w: u32, h: u32, palette: &[[u8; 3]], idx: &[i16]) -> String {
    use std::fmt::Write as _;
    let mut out = format!("\x1bP0;1;0q\"1;1;{w};{h}");
    for (i, [r, g, b]) in palette.iter().enumerate() {
        let pct = |v: u8| (v as u32 * 100 + 127) / 255;
        let _ = write!(out, "#{i};2;{};{};{}", pct(*r), pct(*g), pct(*b));
    }
    let mut band = 0;
    while band < h {
        let mut rows = vec![];
        for color in 0..palette.len() as i16 {
            let mut line = String::new();
            let mut used = false;
            let (mut run, mut n) = ('?', 0u32);
            let flush = |line: &mut String, run: char, n: u32| {
                if n > 3 {
                    let _ = write!(line, "!{n}{run}");
                } else {
                    line.extend(std::iter::repeat_n(run, n as usize));
                }
            };
            for x in 0..w {
                let mut bits = 0u8;
                for bit in 0..6 {
                    let y = band + bit;
                    if y < h && idx[(y * w + x) as usize] == color {
                        bits |= 1 << bit;
                    }
                }
                used |= bits != 0;
                let ch = (63 + bits) as char;
                if ch == run {
                    n += 1;
                } else {
                    flush(&mut line, run, n);
                    (run, n) = (ch, 1);
                }
            }
            // trailing empty columns needn't be sent
            if run != '?' {
                flush(&mut line, run, n);
            }
            if used {
                rows.push(format!("#{color}{line}"));
            }
        }
        out.push_str(&rows.join("$"));
        band += 6;
        if band < h {
            out.push('-');
        }
    }
    out.push_str("\x1b\\");
    out
}

/// iTerm2 inline image sized in cells.
fn iterm_image(png: &[u8], cols: u16, rows: u16) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    format!("\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=1:{b64}\x07", png.len())
}

/// Upload raw RGBA under `id` with a virtual placement `cols`×`rows` cells big, shown wherever placeholder
/// characters for it are printed. Quiet (q=2): no reply lands in our input.
fn kitty_upload(img: &Rgba, id: u8, cols: u16, rows: u16) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&img.px);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let chunk = std::str::from_utf8(chunk).unwrap_or("");
        if i == 0 {
            out.push_str(&format!("\x1b_Ga=T,U=1,f=32,s={},v={},i={id},c={cols},r={rows},q=2,m={more};{chunk}\x1b\\", img.w, img.h));
        } else {
            out.push_str(&format!("\x1b_Gm={more};{chunk}\x1b\\"));
        }
    }
    out
}

/// kitty Unicode placeholders for row 0, columns 0 and 1 (row/column numbers are combining diacritics).
const KITTY_CELLS: [&str; 2] = ["\u{10EEEE}\u{0305}\u{0305}", "\u{10EEEE}\u{0305}\u{030D}"];

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn detection() {
        assert_eq!(detect("auto", &env(&[("WT_SESSION", "x")])), Mode::Sixel);
        assert_eq!(detect("auto", &env(&[("KITTY_WINDOW_ID", "1")])), Mode::Kitty);
        assert_eq!(detect("auto", &env(&[("TERM_PROGRAM", "ghostty")])), Mode::Kitty);
        assert_eq!(detect("auto", &env(&[("TERM_PROGRAM", "iTerm.app")])), Mode::Iterm);
        assert_eq!(detect("auto", &env(&[("TERM_PROGRAM", "WezTerm")])), Mode::Iterm);
        assert_eq!(detect("auto", &env(&[("TERM", "foot")])), Mode::Sixel);
        assert_eq!(detect("auto", &env(&[])), Mode::Text);
        // nested in a bro pane, tmux, VS Code: text
        assert_eq!(detect("auto", &env(&[("WT_SESSION", "x"), ("BRO", "1")])), Mode::Text);
        assert_eq!(detect("auto", &env(&[("WT_SESSION", "x"), ("TMUX", "/tmp/t")])), Mode::Text);
        assert_eq!(detect("auto", &env(&[("WT_SESSION", "x"), ("TERM_PROGRAM", "vscode")])), Mode::Text);
        // setting, then env override
        assert_eq!(detect("text", &env(&[("WT_SESSION", "x")])), Mode::Text);
        assert_eq!(detect("kitty", &env(&[])), Mode::Kitty);
        assert_eq!(detect("text", &env(&[("BRO_ICONS", "sixel")])), Mode::Sixel);
        assert_eq!(detect("auto", &env(&[("BRO_ICONS", "off"), ("KITTY_WINDOW_ID", "1")])), Mode::Text);
        assert_eq!(detect("auto", &env(&[("BRO_ICONS", "iterm"), ("BRO", "1")])), Mode::Iterm);
    }

    #[test]
    fn markers_off_by_default() {
        assert_eq!(marker(Some(Harness::Claude)), None);
    }

    #[test]
    fn logos_decode() {
        for icon in Icon::ALL {
            let img = Rgba::decode(icon.png()).expect("decodes");
            assert_eq!((img.w, img.h), (64, 64));
            assert!(img.px.chunks_exact(4).any(|p| p[3] > 200), "{icon:?} has visible pixels");
            assert!(img.px.chunks_exact(4).any(|p| p[3] == 0), "{icon:?} has transparent pixels");
        }
    }

    #[test]
    fn sixel_framing() {
        let img = Rgba::decode(CLAUDE_PNG).unwrap().inked(INK);
        let s = sixel_mark(&img, 20, 20);
        assert!(s.starts_with("\x1bP0;1;0q\"1;1;20;20#0;2;100;100;100"), "{s:?}");
        assert!(s.ends_with("\x1b\\"));
        // 20 rows = 4 bands of 6 → 3 band separators
        assert_eq!(s.matches('-').count(), 3);
        // body is only sixel data characters and controls
        let body = &s[s.find("100;100;100").unwrap() + 11..s.len() - 2];
        assert!(body.chars().all(|c| ('?'..='~').contains(&c) || "#!$-0123456789".contains(c)), "{body:?}");
    }

    #[test]
    fn sixel_encoder_runs_and_bands() {
        // 5×7, colour 0 on the whole first column, colour 1 on the pixel (4, 6)
        let (w, h) = (5, 7);
        let mut idx = vec![-1i16; (w * h) as usize];
        for y in 0..h {
            idx[(y * w) as usize] = 0;
        }
        idx[(6 * w + 4) as usize] = 1;
        let s = encode_sixel(w, h, &[[255, 0, 0], [0, 0, 255]], &idx);
        assert_eq!(s, "\x1bP0;1;0q\"1;1;5;7#0;2;100;0;0#1;2;0;0;100#0~-#0@$#1!4?@\x1b\\");
        // runs longer than 3 are compressed
        let s = encode_sixel(6, 1, &[[0, 0, 0]], &[0; 6]);
        assert!(s.contains("#0!6@"), "{s:?}");
    }

    #[test]
    fn iterm_and_kitty_sequences() {
        let img = Rgba::decode(CODEX_PNG).unwrap().inked(INK);
        let png = img.encode_png().unwrap();
        assert!(Rgba::decode(&png).is_some());
        let s = iterm_image(&png, 2, 1);
        assert!(s.starts_with("\x1b]1337;File=inline=1;") && s.contains("width=2;height=1") && s.ends_with('\x07'));
        let k = kitty_upload(&img, 202, 2, 1);
        assert!(k.starts_with("\x1b_Ga=T,U=1,f=32,s=64,v=64,i=202,c=2,r=1,q=2,m=1;"));
        assert!(k.ends_with("m=0;") || k.contains("\x1b_Gm=0;"));
    }

    fn painter(mode: Mode) -> Painter {
        Painter::new(mode, (10, 20)).unwrap()
    }

    fn frame(w: u16, h: u16, marks: &[(u16, u16, &str)]) -> Buffer {
        let mut b = Buffer::empty(Rect::new(0, 0, w, h));
        for (x, y, s) in marks {
            b.set_string(*x, *y, s, Style::default());
        }
        b
    }

    fn images(out: &Output) -> usize {
        String::from_utf8_lossy(&out.bytes).matches("\x1bP").count()
    }

    #[test]
    fn placement_diff() {
        let mut p = painter(Mode::Sixel);
        let mut b = frame(20, 5, &[(1, 0, "\u{10FF00} claude"), (1, 1, "\u{10FF01} codex")]);
        p.place(&mut b);
        // markers become blanks under the images
        assert_eq!(b[(1, 0)].symbol(), " ");
        let out = p.take();
        assert_eq!(images(&out), 2);
        assert!(String::from_utf8_lossy(&out.bytes).starts_with("\x1b[1;2H\x1bP"));
        assert!(out.restore.is_empty());

        // same frame again: nothing to do
        let mut b = frame(20, 5, &[(1, 0, "\u{10FF00} claude"), (1, 1, "\u{10FF01} codex")]);
        p.place(&mut b);
        let out = p.take();
        assert_eq!((images(&out), out.restore.len()), (0, 0));

        // the codex row's cells restyled (selected): re-emit that one only
        let mut b = frame(20, 5, &[(1, 0, "\u{10FF00} claude"), (1, 1, "\u{10FF01} codex")]);
        b.set_style(Rect::new(0, 1, 20, 1), Style::default().bg(Color::Blue));
        p.place(&mut b);
        let out = p.take();
        assert_eq!(images(&out), 1);
        assert_eq!(out.restore.len(), 2, "old image cells rewritten first");

        // claude row gone (collapsed / covered by a popup): its cells are rewritten, no new image
        let mut b = frame(20, 5, &[(1, 1, "\u{10FF01} codex")]);
        b.set_style(Rect::new(0, 1, 20, 1), Style::default().bg(Color::Blue));
        p.place(&mut b);
        let out = p.take();
        assert_eq!(images(&out), 0);
        assert_eq!(out.restore.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(), vec![(1, 0), (2, 0)]);

        // resize: everything re-emitted, nothing restored (the screen was cleared)
        let mut b = frame(30, 5, &[(1, 1, "\u{10FF01} codex")]);
        p.place(&mut b);
        let out = p.take();
        assert_eq!((images(&out), out.restore.len()), (1, 0));
    }

    #[test]
    fn no_room_falls_back_to_text() {
        let mut p = painter(Mode::Sixel);
        // no space after the marker; the last row (a sixel there would scroll the screen)
        let mut b = frame(10, 3, &[(0, 0, "\u{10FF00}x"), (9, 1, "\u{10FF00}"), (0, 2, "\u{10FF01} ")]);
        p.place(&mut b);
        assert_eq!(b[(0, 0)].symbol(), Icon::Claude.fallback());
        assert_eq!(b[(9, 1)].symbol(), Icon::Claude.fallback());
        assert_eq!(b[(0, 2)].symbol(), Icon::Codex.fallback());
        assert_eq!(images(&p.take()), 0);
    }

    #[test]
    fn kitty_placeholders_and_single_upload() {
        let mut p = painter(Mode::Kitty);
        let mut b = frame(10, 2, &[(0, 1, "\u{10FF00} a"), (5, 1, "\u{10FF00} b")]);
        p.place(&mut b);
        assert_eq!(b[(0, 1)].symbol(), KITTY_CELLS[0]);
        assert_eq!(b[(1, 1)].symbol(), KITTY_CELLS[1]);
        assert_eq!(b[(1, 1)].fg, Color::Indexed(201));
        let out = p.take();
        assert_eq!(String::from_utf8_lossy(&out.bytes).matches("a=T").count(), 1);
        let mut b = frame(10, 2, &[(0, 1, "\u{10FF00} a")]);
        p.place(&mut b);
        assert!(p.take().bytes.is_empty());
    }
}
