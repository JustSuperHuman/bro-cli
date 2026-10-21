//! Drawing a session's alacritty screen: cell backgrounds merged into runs, glyphs on the cell grid (box
//! drawing joined by bro-gfx), underline styles, strikeout, the selection, links and the cursor.

use super::mirror::{Mirror, Screen};
use crate::app::gui::kit::{Tokens, Ui, indexed};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor};
use bro_gfx::{Color, Point, Rect};
use std::sync::Arc;

/// The colours a screen is drawn with.
#[derive(Clone)]
pub struct Palette {
    pub fg: Color,
    pub bg: Color,
    pub ansi: [Color; 16],
    pub cursor: Color,
    pub selection: Color,
    pub link: Color,
    /// recolour dark-terminal greys and whites to sit well on this theme (see `themed` in the terminal UI)
    pub light: bool,
    pub painted: bool,
}

impl Palette {
    pub fn from_tokens(t: &Tokens, theme: &crate::theme::Theme) -> Palette {
        Palette { fg: t.term_fg, bg: t.term_bg, ansi: t.ansi, cursor: t.cursor, selection: t.selection, link: t.accent, light: t.light, painted: theme.painted() }
    }

    /// A cell colour on this theme: dark-mode CLIs' greys and whites are recast for light themes, and greys lean
    /// toward the theme's own background and text on painted dark themes (as the terminal UI does).
    fn resolve(&self, c: AnsiColor, fg: bool, colors: &Colors) -> Color {
        let grey = |r: u8, g: u8, b: u8| (r.max(g).max(b) - r.min(g).min(b) < 18).then(|| (r as f32 + g as f32 + b as f32) / 765.0);
        let toward = |lightness: f32| self.bg.mix(self.fg, ((lightness - 0.04) / 0.86).clamp(0.0, 1.0));
        match c {
            // the 16 ANSI colours (SGR 30–37, 40–47, 90–97, 100–107) follow the same rules as their 256-colour
            // indices: white text and black backgrounds are recast on light themes, and only as text / background
            AnsiColor::Named(n) if (n as usize) < 16 => self.resolve(AnsiColor::Indexed(n as u8), fg, colors),
            AnsiColor::Named(n) => self.named(n, colors),
            AnsiColor::Indexed(i) => {
                if let Some(rgb) = colors[i as usize] {
                    return Color::rgb8(rgb.r, rgb.g, rgb.b);
                }
                match i {
                    7 | 15 if fg && self.light => self.fg,
                    232..=243 if !fg && self.light => indexed(255 - (i - 232), &self.ansi),
                    0 | 8 if !fg && self.light => self.bg.mix(self.fg, 0.08),
                    232..=255 if self.painted && !self.light => toward((8.0 + 10.0 * (i - 232) as f32) / 255.0),
                    _ => indexed(i, &self.ansi),
                }
            }
            AnsiColor::Spec(rgb) => {
                let (r, g, b) = (rgb.r, rgb.g, rgb.b);
                let sum = r as u32 + g as u32 + b as u32;
                if fg && self.light && sum > 690 {
                    self.fg
                } else if !fg && self.light && sum < 240 {
                    Color::rgb8(r, g, b).mix(self.bg, 0.82)
                } else if self.painted && !self.light && let Some(l) = grey(r, g, b) {
                    toward(l)
                } else {
                    Color::rgb8(r, g, b)
                }
            }
        }
    }

    fn named(&self, n: NamedColor, colors: &Colors) -> Color {
        if let Some(rgb) = colors[n] {
            return Color::rgb8(rgb.r, rgb.g, rgb.b);
        }
        match n {
            NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
            NamedColor::Background => self.bg,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => self.fg.mix(self.bg, 0.34),
            NamedColor::DimBlack => self.ansi[0].mix(self.bg, 0.34),
            NamedColor::DimRed => self.ansi[1].mix(self.bg, 0.34),
            NamedColor::DimGreen => self.ansi[2].mix(self.bg, 0.34),
            NamedColor::DimYellow => self.ansi[3].mix(self.bg, 0.34),
            NamedColor::DimBlue => self.ansi[4].mix(self.bg, 0.34),
            NamedColor::DimMagenta => self.ansi[5].mix(self.bg, 0.34),
            NamedColor::DimCyan => self.ansi[6].mix(self.bg, 0.34),
            NamedColor::DimWhite => self.ansi[7].mix(self.bg, 0.34),
            other => {
                let i = other as usize;
                if i < 16 { self.ansi[i] } else { self.fg }
            }
        }
    }
}

/// A link on screen: visible row, columns `[start, end)`, target.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkSpan {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub uri: Arc<str>,
}

/// Where a screen was drawn (for the mouse).
#[derive(Clone, Copy, Debug, Default)]
pub struct Drawn {
    /// top-left of cell (0, 0), logical
    pub origin: Point,
    /// cell size, logical
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: usize,
    pub rows: usize,
    pub display_offset: usize,
    pub history: usize,
    pub mode: Option<TermMode>,
    /// the cursor's visible (row, column), when it shows (input methods place their window there)
    pub cursor: Option<(usize, usize)>,
}

impl Drawn {
    /// The grid point under `p` (logical), clamped to the screen; and whether it's in the cell's right half.
    pub fn point_at(&self, p: Point) -> (GridPoint, bool) {
        let col_f = ((p.x - self.origin.x) / self.cell_w.max(0.1)).max(0.0);
        let col = (col_f as usize).min(self.cols.saturating_sub(1));
        let row = (((p.y - self.origin.y) / self.cell_h.max(0.1)).max(0.0) as usize).min(self.rows.saturating_sub(1));
        let right = col_f - col as f32 >= 0.5;
        (GridPoint::new(Line(row as i32 - self.display_offset as i32), Column(col)), right)
    }
    /// The visible row and column under `p`, when inside the grid.
    pub fn cell_at(&self, p: Point) -> Option<(usize, usize)> {
        let x = p.x - self.origin.x;
        let y = p.y - self.origin.y;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (c, r) = ((x / self.cell_w) as usize, (y / self.cell_h) as usize);
        (c < self.cols && r < self.rows).then_some((r, c))
    }
}

/// The terminal size (rows, cols) that fits `area` (logical) with the current font.
pub fn fit(ui: &Ui, area: Rect) -> (u16, u16) {
    let Some(m) = ui.text.mono_metrics() else { return (24, 80) };
    let cols = ((area.w * ui.scale) / m.cell_w).floor().max(2.0) as u16;
    let rows = ((area.h * ui.scale) / m.cell_h).floor().max(1.0) as u16;
    (rows, cols)
}

/// Draw `mirror`'s screen in `area`, scrolled back `scroll` lines (clamped to the history).
pub struct View<'a> {
    pub area: Rect,
    pub scroll: usize,
    /// the pane has the keyboard and the window is focused: a solid cursor
    pub focused: bool,
    /// draw the cursor at all (not while a queued pane, not scrolled back)
    pub cursor: bool,
    pub links: &'a [LinkSpan],
    pub hover_link: Option<&'a LinkSpan>,
    /// the top visible screen row (a remote viewer grew the pty past the tile)
    pub view_top: usize,
}

pub fn draw(ui: &mut Ui, mirror: &Mirror, pal: &Palette, v: &View) -> Drawn {
    let Some(m) = ui.text.mono_metrics() else { return Drawn::default() };
    let mut screen = mirror.lock();
    screen.flush_sync();
    let Screen { term, .. } = &mut *screen;
    // follow the scroll the session asked for
    let history = term.grid().history_size();
    let want = v.scroll.min(history) as i32;
    let have = term.grid().display_offset() as i32;
    if want != have {
        term.scroll_display(Scroll::Delta(want - have));
        // the links on screen were found before this scroll: find them again next frame, and draw one
        mirror.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ui.ticking = true;
    }
    let scale = ui.scale;
    let ox = (v.area.x * scale).round();
    let oy = (v.area.y * scale).round();
    let (cw, ch) = (m.cell_w, m.cell_h);
    let cols = term.grid().columns();
    let rows = term.grid().screen_lines();
    let colors = *term.colors();
    let content = term.renderable_content();
    let offset = content.display_offset;
    let selection = content.selection;
    let cursor = content.cursor;
    let mode = content.mode;
    let visible_rows = ((v.area.h * scale) / ch).floor().max(1.0) as usize;
    // rows above the tile's top when a remote viewer grew the pty
    let top = v.view_top.min(rows.saturating_sub(1));

    let mut bg_run: Option<(usize, usize, usize, Color)> = None; // (row, start col, end col, colour)
    let flush_bg = |ui: &mut Ui, run: &mut Option<(usize, usize, usize, Color)>| {
        if let Some((row, a, b, c)) = run.take() {
            let r = Rect::new(ox + a as f32 * cw, oy + row as f32 * ch, (b - a) as f32 * cw, ch);
            ui.scene.rect(r, c);
        }
    };
    let mut glyphs: Vec<(String, bool, bool, f32, f32, u8, Color)> = Vec::with_capacity(rows * 40);
    let mut decos: Vec<(Rect, Color)> = vec![];
    let mut buf = [0u8; 4];
    for cell in content.display_iter {
        let row_i = cell.point.line.0 + offset as i32;
        if row_i < 0 {
            continue;
        }
        let row_screen = row_i as usize;
        if row_screen < top {
            continue;
        }
        let row = row_screen - top;
        if row >= visible_rows {
            continue;
        }
        let col = cell.point.column.0;
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let mut fg = pal.resolve(cell.fg, true, &colors);
        let mut bg = match cell.bg {
            AnsiColor::Named(NamedColor::Background) => None,
            c => Some(pal.resolve(c, false, &colors)),
        };
        if flags.contains(Flags::DIM) {
            fg = fg.mix(bg.unwrap_or(pal.bg), 0.38);
        }
        if flags.contains(Flags::INVERSE) {
            let b = bg.unwrap_or(pal.bg);
            bg = Some(fg);
            fg = b;
        }
        let selected = selection.is_some_and(|s| s.contains(cell.point));
        if selected {
            bg = Some(pal.selection.over(bg.unwrap_or(pal.bg)));
        }
        let width = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        // merge background runs
        match (bg, &mut bg_run) {
            (Some(c), Some((r, _, end, rc))) if *r == row && *end == col && *rc == c => *end = col + width,
            (Some(c), _) => {
                flush_bg(ui, &mut bg_run);
                bg_run = Some((row, col, col + width, c));
            }
            (None, _) => flush_bg(ui, &mut bg_run),
        }
        let x = ox + col as f32 * cw;
        let y = oy + row as f32 * ch;
        if !flags.contains(Flags::HIDDEN) && cell.c != ' ' && cell.c != '\0' {
            let mut s = String::from(cell.c.encode_utf8(&mut buf) as &str);
            if let Some(zw) = cell.zerowidth() {
                s.extend(zw.iter());
            }
            glyphs.push((s, flags.contains(Flags::BOLD), flags.contains(Flags::ITALIC), x, y, width as u8, fg));
        }
        // decorations
        let under = cell.underline_color().map(|c| pal.resolve(c, true, &colors)).unwrap_or(fg);
        let w = cw * width as f32;
        if flags.intersects(Flags::ALL_UNDERLINES) {
            let uy = y + m.underline_y;
            let t = m.stroke;
            if flags.contains(Flags::DOUBLE_UNDERLINE) {
                decos.push((Rect::new(x, uy - t, w, t), under));
                decos.push((Rect::new(x, uy + t, w, t), under));
            } else if flags.contains(Flags::UNDERCURL) {
                // a wave: alternating short segments up and down
                let seg = (cw / 4.0).max(2.0);
                let mut sx = x;
                let mut up = true;
                while sx < x + w {
                    let sw = seg.min(x + w - sx);
                    decos.push((Rect::new(sx, if up { uy - t } else { uy }, sw, t), under));
                    sx += seg;
                    up = !up;
                }
            } else if flags.contains(Flags::DOTTED_UNDERLINE) {
                let mut sx = x;
                while sx < x + w {
                    decos.push((Rect::new(sx, uy, t, t), under));
                    sx += t * 2.0;
                }
            } else if flags.contains(Flags::DASHED_UNDERLINE) {
                let dash = (cw * 0.6).max(2.0);
                decos.push((Rect::new(x, uy, dash.min(w), t), under));
            } else {
                decos.push((Rect::new(x, uy, w, t), under));
            }
        }
        if flags.contains(Flags::STRIKEOUT) {
            decos.push((Rect::new(x, y + m.strike_y, w, m.stroke), fg));
        }
    }
    flush_bg(ui, &mut bg_run);
    // links: a hairline under each, solid under the one being pointed at
    for l in v.links {
        if l.row < top || l.row - top >= visible_rows {
            continue;
        }
        let hot = v.hover_link.is_some_and(|h| h == l);
        let r = Rect::new(ox + l.start as f32 * cw, oy + (l.row - top) as f32 * ch + m.underline_y, (l.end - l.start) as f32 * cw, m.stroke);
        decos.push((r, if hot { pal.link } else { pal.link.with_alpha(0.45) }));
    }
    // the cursor: a block when focused (text drawn in the background colour over it), else an outline
    let mut cursor_cell: Option<(usize, usize)> = None;
    let mut cursor_at: Option<(usize, usize)> = None;
    if v.cursor && offset == 0 && cursor.shape != CursorShape::Hidden && mode.contains(TermMode::SHOW_CURSOR) {
        let cr = cursor.point.line.0 as usize;
        if cr >= top && cr - top < visible_rows {
            let (row, col) = (cr - top, cursor.point.column.0);
            cursor_at = Some((row, col));
            let x = ox + col as f32 * cw;
            let y = oy + row as f32 * ch;
            let wide = term.grid()[cursor.point].flags.contains(Flags::WIDE_CHAR);
            let w = cw * if wide { 2.0 } else { 1.0 };
            let stroke = (ui.scale * 1.0).round().max(1.0);
            match (v.focused, cursor.shape) {
                (true, CursorShape::Block) => {
                    ui.scene.rect(Rect::new(x, y, w, ch), pal.cursor);
                    cursor_cell = Some((row, col));
                }
                (true, CursorShape::Beam) => decos.push((Rect::new(x, y, (stroke * 2.0).min(cw), ch), pal.cursor)),
                (true, CursorShape::Underline) => decos.push((Rect::new(x, y + ch - stroke * 2.0, w, stroke * 2.0), pal.cursor)),
                _ => {
                    // unfocused: a hollow box
                    let c = pal.cursor.with_alpha(0.75);
                    decos.push((Rect::new(x, y, w, stroke), c));
                    decos.push((Rect::new(x, y + ch - stroke, w, stroke), c));
                    decos.push((Rect::new(x, y, stroke, ch), c));
                    decos.push((Rect::new(x + w - stroke, y, stroke, ch), c));
                }
            }
        }
    }
    let ink_on_cursor = pal.bg;
    for g in glyphs.iter_mut() {
        let row = ((g.4 - oy) / ch).round() as usize;
        let col = ((g.3 - ox) / cw).round() as usize;
        if cursor_cell == Some((row, col)) {
            g.6 = ink_on_cursor;
        }
    }
    // runs of punctuation in one style are shaped together, so a font's ligatures (-> != ==) join up
    let ligatures = ui.text.mono_ligatures();
    let joinable = |s: &str| s.len() == 1 && s.as_bytes()[0].is_ascii_punctuation();
    let mut i = 0;
    while i < glyphs.len() {
        let (s, bold, italic, x, y, width, fg) = &glyphs[i];
        let mut j = i + 1;
        if ligatures && *width == 1 && joinable(s) {
            while let Some(n) = glyphs.get(j) {
                let next_cell = (n.3 - (x + (j - i) as f32 * cw)).abs() < 0.5 && n.4 == *y;
                if !(next_cell && n.5 == 1 && joinable(&n.0) && n.1 == *bold && n.2 == *italic && n.6 == *fg) {
                    break;
                }
                j += 1;
            }
        }
        if j - i >= 2 {
            let run: String = glyphs[i..j].iter().map(|g| g.0.as_str()).collect();
            ui.text.draw_run(ui.scene, &run, *bold, *italic, *x, *y, *fg);
        } else {
            ui.text.draw_cell(ui.scene, s, *bold, *italic, *x, *y, *width, *fg);
        }
        i = j;
    }
    for (r, c) in decos {
        ui.scene.top_quad(bro_gfx::Quad::new(r, c));
    }
    Drawn {
        origin: Point::new(ox / scale, oy / scale),
        cell_w: cw / scale,
        cell_h: ch / scale,
        cols,
        rows: visible_rows.min(rows),
        display_offset: offset,
        history,
        mode: Some(mode),
        cursor: cursor_at,
    }
}

/// The text of the visible rows (logical lines joined across soft wraps are kept as separate rows here) and
/// the links in them: OSC 8 targets first, then web addresses, then paths the cache knows are files.
pub fn find_links(screen: &Screen, files: &mut dyn FnMut(&bro_bridge::file_links::FileLinkMatch) -> Option<Arc<str>>) -> Vec<LinkSpan> {
    let term = &screen.term;
    let grid = term.grid();
    let offset = grid.display_offset();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let mut out: Vec<LinkSpan> = vec![];
    let mut taken = vec![vec![false; cols]; rows];
    // logical line: (text, cells: (byte start, byte end, row, col, width))
    let mut text = String::new();
    let mut cells: Vec<(usize, usize, usize, usize, usize)> = vec![];
    let mut explicit: Vec<LinkSpan> = vec![];
    for row in 0..rows {
        let line = Line(row as i32 - offset as i32);
        let r = &grid[line];
        let mut col = 0;
        while col < cols {
            let cell = &r[Column(col)];
            if let Some(h) = cell.hyperlink()
                && let Some(uri) = crate::panes::links::target(h.uri())
            {
                // an OSC 8 target owns its cells: a URL-shaped label keeps it, and a path over it isn't one
                taken[row][col] = true;
                match explicit.last_mut() {
                    Some(l) if l.row == row && l.end == col && &*l.uri == uri.as_str() => l.end = col + 1,
                    _ => explicit.push(LinkSpan { row, start: col, end: col + 1, uri: Arc::from(uri.as_str()) }),
                }
            }
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                col += 1;
                continue;
            }
            let start = text.len();
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
            let w = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            cells.push((start, text.len(), row, col, w));
            col += 1;
        }
        let wrapped = r[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        if !wrapped || row + 1 == rows {
            for (start, end, uri) in crate::panes::links::web_urls(&text) {
                let uri: Arc<str> = Arc::from(uri.as_str());
                push_spans(&mut out, &cells, start, end, &uri, &mut taken);
            }
            for found in bro_bridge::file_links::find_file_links(&text) {
                let covered: Vec<&(usize, usize, usize, usize, usize)> = cells.iter().filter(|(a, b, ..)| *a < found.end && *b > found.start).collect();
                if covered.iter().any(|&&(_, _, r, c, w)| (c..(c + w).min(cols)).any(|cc| taken[r][cc])) {
                    continue;
                }
                let Some(uri) = files(&found) else { continue };
                push_spans(&mut out, &cells, found.start, found.end, &uri, &mut taken);
            }
            text.clear();
            cells.clear();
        }
    }
    out.extend(explicit);
    out
}

/// Link the cells of `text[start..end]` to `uri`, skipping cells another link already has (an OSC 8 target or
/// an earlier URL keeps its cells, as in the terminal UI).
fn push_spans(out: &mut Vec<LinkSpan>, cells: &[(usize, usize, usize, usize, usize)], start: usize, end: usize, uri: &Arc<str>, taken: &mut [Vec<bool>]) {
    let mut cur: Option<LinkSpan> = None;
    for &(a, b, row, col, w) in cells {
        if a >= end || b <= start {
            continue;
        }
        if (col..col + w).any(|c| taken.get(row).and_then(|r| r.get(c)).copied().unwrap_or(false)) {
            if let Some(l) = cur.take() {
                out.push(l);
            }
            continue;
        }
        for c in col..col + w {
            if let Some(t) = taken.get_mut(row).and_then(|r| r.get_mut(c)) {
                *t = true;
            }
        }
        match &mut cur {
            Some(l) if l.row == row && l.end == col => l.end = col + w,
            _ => {
                if let Some(l) = cur.take() {
                    out.push(l);
                }
                cur = Some(LinkSpan { row, start: col, end: col + w, uri: uri.clone() });
            }
        }
    }
    if let Some(l) = cur {
        out.push(l);
    }
}

/// The text of the screen's visible rows (copy, search, tests).
pub fn screen_text(screen: &Screen) -> Vec<String> {
    let grid = screen.term.grid();
    let offset = grid.display_offset();
    (0..grid.screen_lines())
        .map(|row| {
            let r = &grid[Line(row as i32 - offset as i32)];
            let s: String = (0..grid.columns()).map(|c| r[Column(c)].c).collect();
            s.trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::gui::term::mirror::Env;
    use crate::panes::mirror::ScreenMirror;
    use parking_lot::Mutex;

    fn palette(light: bool) -> Palette {
        let ansi: [Color; 16] = std::array::from_fn(|i| Color::rgb8(i as u8 * 10, i as u8 * 10, i as u8 * 10));
        let (fg, bg) = if light { (Color::rgb8(20, 20, 20), Color::rgb8(250, 250, 250)) } else { (Color::rgb8(230, 230, 230), Color::rgb8(15, 15, 15)) };
        Palette { fg, bg, ansi, cursor: fg, selection: fg, link: fg, light, painted: false }
    }

    #[test]
    fn ansi_colours_on_light_themes_are_recast_as_text_or_background_only() {
        let p = palette(true);
        let colors = Colors::default();
        let named = |n, fg| p.resolve(AnsiColor::Named(n), fg, &colors);
        // white text (SGR 37 / 97) reads as the theme's text
        assert_eq!(named(NamedColor::White, true), p.fg);
        assert_eq!(named(NamedColor::BrightWhite, true), p.fg);
        // a white background (SGR 47) stays white, never the theme's dark text colour
        assert_eq!(named(NamedColor::White, false), p.ansi[7]);
        // a black background (SGR 40 / 100) becomes a light tint, like `ESC[48;5;0m`
        assert_eq!(named(NamedColor::Black, false), p.bg.mix(p.fg, 0.08));
        assert_eq!(named(NamedColor::BrightBlack, false), p.resolve(AnsiColor::Indexed(8), false, &colors));
        assert_eq!(named(NamedColor::Red, true), p.ansi[1]);
        // dark themes keep the palette
        let d = palette(false);
        assert_eq!(d.resolve(AnsiColor::Named(NamedColor::White), false, &colors), d.ansi[7]);
        assert_eq!(d.resolve(AnsiColor::Named(NamedColor::Black), false, &colors), d.ansi[0]);
    }

    fn screen(rows: u16, cols: u16, bytes: &[u8]) -> Mirror {
        let m = Mirror::new(Arc::new(Mutex::new(Env::default())));
        m.resize(rows, cols);
        m.feed(bytes);
        m
    }

    #[test]
    fn osc8_targets_keep_their_cells_from_urls_and_paths() {
        // an OSC 8 label shaped like a URL, then a plain URL and a path
        let m = screen(3, 80, b"\x1b]8;;https://actual.example/\x07https://label.example/\x1b]8;;\x07 https://plain.example/ src/app.rs");
        let any_file = |f: &bro_bridge::file_links::FileLinkMatch| Some(Arc::from(format!("file:///{}", f.path).as_str()));
        let links = find_links(&m.lock(), &mut { any_file });
        let at = |col: usize| links.iter().find(|l| l.row == 0 && (l.start..l.end).contains(&col)).map(|l| l.uri.to_string());
        assert_eq!(at(0).as_deref(), Some("https://actual.example/"));
        assert_eq!(at(21).as_deref(), Some("https://actual.example/"), "the whole label keeps its target");
        assert_eq!(at(23).as_deref(), Some("https://plain.example/"));
        assert!(at(46).is_some_and(|u| u.starts_with("file:")), "{links:?}");
        // nothing overlaps
        for (i, a) in links.iter().enumerate() {
            for b in &links[i + 1..] {
                assert!(!(a.row == b.row && a.start < b.end && b.start < a.end), "{a:?} overlaps {b:?}");
            }
        }
        // a path inside a hyperlink's label is the hyperlink's, not a file link
        let m = screen(3, 80, b"\x1b]8;;https://docs.example/\x07docs/readme.md\x1b]8;;\x07");
        let links = find_links(&m.lock(), &mut { any_file });
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(&*links[0].uri, "https://docs.example/");
    }

    #[test]
    fn links_follow_the_scrolled_view() {
        let m = screen(3, 40, b"https://one.example/\r\nb\r\nc\r\nd\r\ne");
        // live: the URL has scrolled off the top
        assert!(find_links(&m.lock(), &mut |_| None).is_empty());
        m.lock().term.scroll_display(Scroll::Delta(2));
        let links = find_links(&m.lock(), &mut |_| None);
        assert_eq!(links.first().map(|l| (l.row, &*l.uri)), Some((0, "https://one.example/")));
    }
}
