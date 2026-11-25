//! Rune scenes in the desktop app: a frame of [`crate::rune`] drawn as monospace glyphs, each in its own colour.
//! One shaped layout per character (the ramp's few), so a frame costs a glyph per inked cell and no shaping.

use super::{Style, Tokens, Ui};
use crate::rune::{self, Inks, Rgb, Scene};
use bro_gfx::{Color, Point, Rect};

pub fn rgb(c: Color) -> Rgb {
    Rgb(c.r, c.g, c.b)
}

fn color(c: Rgb) -> Color {
    Color::rgba(c.0, c.1, c.2, 1.0)
}

/// The theme's inks, fading into `on` (what the scene is drawn over).
pub fn inks(t: &Tokens, on: Color) -> Inks {
    Inks { accent: rgb(t.accent), shine: rgb(t.shine), good: rgb(t.good), warm: rgb(t.gold.mix(t.inline, 0.4)), hot: rgb(t.danger), text: rgb(t.text), bg: rgb(on) }
}

/// A scene's cell size at `font` px: (width, height) in logical px.
pub fn cell(ui: &mut Ui, font: f32) -> (f32, f32) {
    let w = ui.measure("0", Style::mono(font, Color::WHITE)).max(1.0);
    // tighter than text: the picture reads as one image, not lines
    (w, (font * 1.12).round().max(1.0))
}

/// Draw `scene` at `t` seconds into `r` with `font`-px glyphs, centred; `alpha` fades all of it. Returns where
/// the frame landed.
pub fn draw(ui: &mut Ui, r: Rect, font: f32, scene: Scene, t: f64, inks: &Inks, alpha: f32) -> Rect {
    let (cw, ch) = cell(ui, font);
    let rows = (r.h / ch).floor().max(0.0) as usize;
    let cols = ((r.w / cw).floor().max(0.0) as usize).min(rune::cols_for(scene, rows, ch / cw) + 2);
    if rows == 0 || cols == 0 || alpha <= 0.0 {
        return Rect::new(r.x, r.y, 0.0, 0.0);
    }
    ui.ticking = true;
    let frame = rune::render(scene, cols, rows, ch / cw, t, inks);
    let at = Rect::new((r.x + (r.w - cols as f32 * cw) / 2.0).round(), (r.y + (r.h - rows as f32 * ch) / 2.0).round(), cols as f32 * cw, rows as f32 * ch);
    // the world's planet glows faintly behind its letters
    if scene == Scene::World {
        let (pc, pr, pw) = rune::world_planet(cols, rows, ch / cw);
        let (x, y, rad) = (at.x + pc * cw, at.y + pr * ch, pw * cw);
        let glow = Rect::new(x - rad, y - rad, rad * 2.0, rad * 2.0);
        ui.shadow(glow, rad, rad * 0.9, 0.0, color(inks.shine.mix(inks.accent, 0.3)).fade(if ui.t.light { 0.16 } else { 0.22 } * alpha));
    }
    // a layout per character, its line centred on the cell
    let mut glyphs: [Option<super::TextBox>; 128] = std::array::from_fn(|_| None);
    let scale = ui.scale;
    for row in 0..rows {
        for col in 0..cols {
            let c = frame.at(col, row);
            if c.ch == b' ' || c.ch >= 128 {
                continue;
            }
            let b = glyphs[c.ch as usize].get_or_insert_with(|| ui.line(&(c.ch as char).to_string(), Style::mono(font, Color::WHITE), None)).clone();
            let p = Point::new((at.x + col as f32 * cw) * scale, (at.y + row as f32 * ch - (b.h - ch) / 2.0) * scale);
            ui.text.draw(ui.scene, &b.layout, p, Some(&[color(c.color)]), alpha * (0.55 + 0.45 * c.ink));
        }
    }
    at
}
