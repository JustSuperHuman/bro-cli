//! The themes' backdrops (stars, blueprint grid, hatching, digital rain) on the window behind everything, and
//! the rain setting: everywhere, or around the sessions plus inside the ones whose agent is mid-turn (as the
//! terminal UI does). Laid on the terminal font's grid, like the terminal UI's.

use super::kit::Ui;
use crate::skin::Backdrop;
use bro_gfx::{Color, Rect};

/// The bundled Aurora stylesheet's native counterpart: palette-driven ambient color, with no CSS execution.
///
/// Each glow is one radial gradient, fading evenly from its centre to its edge: the same light 24 faint rings
/// stacked on each other gave, at one quad instead of 24 (each of them as big as the card, every frame).
pub fn aurora(ui: &mut Ui, area: Rect) {
    if !ui.theme.aurora() {
        return;
    }
    let t = ui.t;
    ui.clip(area, 0.0);
    ui.glass(area, bro_gfx::Corners::all(0.0), t.role("canvas", t.bg));
    // what 24 rings of this alpha add up to at the centre, where they all overlap
    let ring = if t.light { 0.004f32 } else { 0.007 };
    let peak = 1.0 - (1.0 - ring).powi(24);
    let radius = area.w.max(area.h) * 0.64;
    for (cx, cy, color) in [(area.x + area.w * 0.08, area.y, t.accent), (area.right(), area.bottom(), t.role("messageAction", t.shine))] {
        let r = ui.px(Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0));
        let fill = bro_gfx::Fill::Radial(color.with_alpha(peak), color.with_alpha(0.0));
        ui.scene.quad(bro_gfx::Quad::new(r, fill).radius(r.w / 2.0));
    }
    ui.unclip();
}

const KANA: &[&str] = &[
    "ｱ", "ｲ", "ｳ", "ｴ", "ｵ", "ｶ", "ｷ", "ｸ", "ｹ", "ｺ", "ｻ", "ｼ", "ｽ", "ｾ", "ｿ", "ﾀ", "ﾁ", "ﾂ", "ﾃ", "ﾄ", "ﾅ", "ﾆ", "ﾇ", "ﾈ", "ﾊ", "ﾋ",
    "ﾌ", "ﾍ", "ﾎ", "ﾏ", "ﾐ", "ﾑ", "ﾒ", "ﾓ", "ﾔ", "ﾕ", "ﾗ", "ﾘ", "ﾜ", "0", "1", "2", "3", "4", "5", "7", "8", "9", "Z", ":", "=",
];

fn hash(x: u32, y: u32, z: u64) -> u64 {
    let mut h = 0xcbf29ce484222325u64 ^ z.wrapping_mul(0x9e3779b97f4a7c15);
    for b in [x as u64, y as u64] {
        h ^= b;
        h = h.wrapping_mul(0x100000001b3);
        h ^= h >> 29;
    }
    h
}

/// Where rain falls inside sessions too.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Inside {
    No,
    /// every session (rain everywhere)
    All,
    /// sessions whose agent is mid-turn (rain on working sessions)
    Busy,
}

/// Which backdrop shows on the window, and where rain also falls inside sessions.
pub fn choice(app: &crate::app::App) -> (Backdrop, Inside) {
    use bro_core::config::RainMode;
    let own = app.theme.skin.backdrop;
    match app.svc.settings().rain {
        RainMode::Off => (own, Inside::No),
        RainMode::Everywhere => (Backdrop::Rain, Inside::All),
        RainMode::Working => (Backdrop::Rain, Inside::Busy),
    }
}

/// Draw backdrop `bd` over `area` (logical px). `ink` is the ground it's drawn on.
pub fn draw(ui: &mut Ui, area: Rect, bd: Backdrop, ink: Color) {
    if bd == Backdrop::None || area.is_empty() {
        return;
    }
    let Some(m) = ui.text.mono_metrics() else { return };
    let t = ui.t;
    let (cw, ch) = (m.cell_w, m.cell_h);
    let ox = (area.x * ui.scale).round();
    let oy = (area.y * ui.scale).round();
    let cols = ((area.w * ui.scale) / cw).floor() as u32;
    let rows = ((area.h * ui.scale) / ch).floor() as u32;
    let time = ui.time;
    let faint = |k: f32| ink.mix(t.text, k);
    let put = |ui: &mut Ui, x: u32, y: u32, sym: &str, c: Color| {
        ui.text.draw_cell(ui.scene, sym, false, false, ox + x as f32 * cw, oy + y as f32 * ch, 1, c);
    };
    ui.clip(area, 0.0);
    match bd {
        Backdrop::None => {}
        Backdrop::Stars => {
            ui.wake_after(std::time::Duration::from_millis(400));
            for y in 0..rows {
                for x in 0..cols {
                    let h = hash(x, y, 7);
                    if !h.is_multiple_of(89) {
                        continue;
                    }
                    let twinkle = 0.5 + 0.5 * ((time * 1.7 + (h % 628) as f64 / 100.0).sin() as f32);
                    let (sym, base) = if h.is_multiple_of(7) { ("✦", 0.55) } else if h.is_multiple_of(3) { ("+", 0.35) } else { ("·", 0.4) };
                    let tint = if h.is_multiple_of(5) { t.shine } else if h.is_multiple_of(11) { t.accent } else { t.text };
                    put(ui, x, y, sym, ink.mix(tint, base * (0.55 + 0.45 * twinkle)));
                }
            }
        }
        Backdrop::Grid => {
            for y in 0..rows {
                for x in 0..cols {
                    if x.is_multiple_of(16) && y.is_multiple_of(8) {
                        put(ui, x, y, "+", faint(0.42));
                    } else if x.is_multiple_of(4) && y.is_multiple_of(2) {
                        put(ui, x, y, "·", faint(0.3));
                    }
                }
            }
        }
        Backdrop::Hatch => {
            for y in 0..rows {
                for x in 0..cols {
                    if (x + y).is_multiple_of(6) {
                        put(ui, x, y, "╱", ink.mix(t.accent, 0.13));
                    }
                }
            }
        }
        Backdrop::Rain => {
            ui.wake_after(std::time::Duration::from_millis(110));
            let h_ = rows as i64;
            for x in 0..cols {
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
                    let g = hash(x, y as u32, (time * 6.0) as u64 / (1 + (d as u64 % 3)));
                    let sym = KANA[(g % KANA.len() as u64) as usize];
                    let c = if d == 0 { t.good.mix(Color::WHITE, 0.6) } else { ink.mix(t.good, 0.85 * (1.0 - d as f32 / len as f32) + 0.1) };
                    put(ui, x, y as u32, sym, c);
                }
            }
        }
    }
    ui.unclip();
}
