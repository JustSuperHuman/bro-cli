//! Drawing the terminal UI's own ratatui output inside the desktop app, cell for cell in the terminal font.
//!
//! Every overlay and view already draws itself with ratatui; this lets any of them appear (and take clicks,
//! translated to their cell coordinates) in the window before it has a native design of its own, so nothing is
//! ever missing. Native designs replace these one surface at a time.

use super::kit::{Ui, color};
use bro_gfx::{Color, Point, Quad, Rect};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect as Cells;
use ratatui::style::{Color as TuiColor, Modifier};

/// Marks cells a drawing left alone (they stay see-through).
const UNTOUCHED: &str = "\u{10FFFD}";

#[derive(Default)]
pub struct TuiCanvas {
    term: Option<Terminal<TestBackend>>,
}

/// Where a ratatui grid was painted (for translating clicks into its cells).
#[derive(Clone, Copy, Debug, Default)]
pub struct Grid {
    pub origin: Point,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: u16,
    pub rows: u16,
}

impl Grid {
    /// The cell under `p`, when inside.
    pub fn cell(&self, p: Point) -> Option<ratatui::layout::Position> {
        let x = ((p.x - self.origin.x) / self.cell_w).floor();
        let y = ((p.y - self.origin.y) / self.cell_h).floor();
        (x >= 0.0 && y >= 0.0 && (x as u16) < self.cols && (y as u16) < self.rows).then(|| ratatui::layout::Position { x: x as u16, y: y as u16 })
    }
}

impl TuiCanvas {
    /// Run `draw` on a ratatui frame of the cells that fit `area`, then paint the result there. Cells the drawing
    /// cleared but left without a colour get `fill` (None: see-through).
    pub fn paint(&mut self, ui: &mut Ui, area: Rect, fill: Option<Color>, draw: impl FnOnce(&mut ratatui::Frame, Cells)) -> Grid {
        let Some(m) = ui.text.mono_metrics() else { return Grid::default() };
        let cols = ((area.w * ui.scale) / m.cell_w).floor().clamp(1.0, 1000.0) as u16;
        let rows = ((area.h * ui.scale) / m.cell_h).floor().clamp(1.0, 500.0) as u16;
        let term = match &mut self.term {
            Some(t) if t.backend().buffer().area == Cells::new(0, 0, cols, rows) => t,
            slot => {
                *slot = Terminal::new(TestBackend::new(cols, rows)).ok();
                match slot {
                    Some(t) => t,
                    None => return Grid::default(),
                }
            }
        };
        let _ = term.draw(|f| {
            let full = f.area();
            for y in 0..full.height {
                for x in 0..full.width {
                    if let Some(c) = f.buffer_mut().cell_mut((x, y)) {
                        c.set_symbol(UNTOUCHED);
                    }
                }
            }
            draw(f, full);
        });
        let buf = term.backend().buffer().clone();
        let ox = (area.x * ui.scale).round();
        let oy = (area.y * ui.scale).round();
        let (cw, ch) = (m.cell_w, m.cell_h);
        let t = ui.t;
        let mut glyphs = vec![];
        let mut decos = vec![];
        for y in 0..rows {
            let mut run: Option<(u16, u16, Color)> = None;
            let flush = |ui: &mut Ui, run: &mut Option<(u16, u16, Color)>| {
                if let Some((a, b, c)) = run.take() {
                    ui.scene.rect(Rect::new(ox + a as f32 * cw, oy + y as f32 * ch, (b - a) as f32 * cw, ch), c);
                }
            };
            for x in 0..cols {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                if sym == UNTOUCHED {
                    flush(ui, &mut run);
                    continue;
                }
                let mut fg = color(cell.fg, t.text, &t.ansi);
                let mut bg = match cell.bg {
                    TuiColor::Reset => fill,
                    c => Some(color(c, t.surface, &t.ansi)),
                };
                if cell.modifier.contains(Modifier::REVERSED) {
                    let b = bg.unwrap_or(fill.unwrap_or(t.surface));
                    bg = Some(fg);
                    fg = b;
                }
                if cell.modifier.contains(Modifier::DIM) {
                    fg = fg.mix(bg.unwrap_or(t.surface), 0.4);
                }
                match (bg, &mut run) {
                    (Some(c), Some((_, end, rc))) if *end == x && *rc == c => *end = x + 1,
                    (Some(c), _) => {
                        flush(ui, &mut run);
                        run = Some((x, x + 1, c));
                    }
                    (None, _) => flush(ui, &mut run),
                }
                let px = ox + x as f32 * cw;
                let py = oy + y as f32 * ch;
                if !sym.trim().is_empty() {
                    let wide = unicode_width::UnicodeWidthStr::width(sym) >= 2;
                    glyphs.push((sym.to_string(), cell.modifier.contains(Modifier::BOLD), cell.modifier.contains(Modifier::ITALIC), px, py, if wide { 2u8 } else { 1u8 }, fg));
                }
                if cell.modifier.contains(Modifier::UNDERLINED) {
                    decos.push((Rect::new(px, py + m.underline_y, cw, m.stroke), fg));
                }
                if cell.modifier.contains(Modifier::CROSSED_OUT) {
                    decos.push((Rect::new(px, py + m.strike_y, cw, m.stroke), fg));
                }
            }
            flush(ui, &mut run);
        }
        for (s, b, i, x, y, w, c) in glyphs {
            ui.text.draw_cell(ui.scene, &s, b, i, x, y, w, c);
        }
        for (r, c) in decos {
            ui.scene.top_quad(Quad::new(r, c));
        }
        Grid { origin: Point::new(ox / ui.scale, oy / ui.scale), cell_w: cw / ui.scale, cell_h: ch / ui.scale, cols, rows }
    }
}

/// The open overlay, drawn by the terminal UI's own code: the launcher fills the body; the others float over a
/// dimmed window. Returns where it was painted (for the mouse).
pub fn draw_overlay(app: &mut crate::app::App, ui: &mut Ui, canvas: &mut TuiCanvas, window: Rect, body: Rect) -> Option<Grid> {
    use crate::app::Overlay;
    let theme = app.theme.clone();
    let time = app.start.elapsed().as_secs_f64();
    if let Overlay::Launcher(l) = &mut app.overlay {
        let prev = ui.layer();
        let r = Rect::new(body.x - 2.0, body.y - 2.0, body.w + 4.0, body.h + 4.0);
        ui.panel(r, super::kit::radius::OVERLAY, ui.t.surface, ui.t.border);
        ui.block(r);
        let fill = Some(ui.t.surface);
        let grid = canvas.paint(ui, super::layout::inset(r, 6.0), fill, |f, area| crate::launcher::view::draw(f, area, l, &theme, time));
        ui.hit(Rect::new(grid.origin.x, grid.origin.y, grid.cols as f32 * grid.cell_w, grid.rows as f32 * grid.cell_h), super::actions::Action::Overlay(0));
        ui.set_layer(prev);
        return Some(grid);
    }
    let prev = ui.layer();
    super::kit::widgets::scrim(ui, window, 1.0, super::actions::Action::CloseOverlay);
    let fill = Some(ui.t.raised);
    let grid = canvas.paint(ui, window, fill, |f, area| app.draw_overlay(f, area));
    // the painted popup takes clicks (translated to its cells); clicks outside reach the scrim
    ui.hit(Rect::new(grid.origin.x, grid.origin.y, grid.cols as f32 * grid.cell_w, grid.rows as f32 * grid.cell_h), super::actions::Action::Overlay(0));
    ui.set_layer(prev);
    Some(grid)
}
