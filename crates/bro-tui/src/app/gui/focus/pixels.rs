//! A new session's mark in focus mode: its agent's logo (Claude's spark, Codex's knot) set in square pixels. They
//! pop in from the middle when the session opens and a shine crosses them; the shine comes round again while the
//! agent gets ready and when the pointer comes onto the mark. The rest of the time it's still, so an empty session
//! doesn't keep the window drawing.
//!
//! Made for a see-through window (see `material`): the pixels are solid, so they read over whatever is behind the
//! window as text does, and the glow under them while they move is glass, tinting the window's material without
//! making it any less see-through (over a solid window it's an ordinary fill).

use super::super::kit::Ui;
use super::super::kit::brand::Logo;
use bro_core::Harness;
use bro_gfx::{Color, Point, Quad, Rect};
use std::f32::consts::PI;
use std::sync::OnceLock;

/// Pixels across (and down) a logo.
const GRID: usize = 24;
/// The pixels pop in from the middle out over this long, each taking [`POP`] (seconds).
const SPREAD: f64 = 0.42;
const POP: f64 = 0.34;
/// A shine takes this long to cross. The first starts as the last pixels land; while the agent gets ready another
/// starts every [`AGAIN`].
const SWEEP: f64 = 0.95;
const FIRST: f64 = 0.5;
const AGAIN: f64 = 2.6;
/// Where the shine's twinkles come out (cells; off the logo, round its corners).
const TWINKLES: [(f32, f32); 3] = [(22.5, 0.5), (0.5, 21.5), (24.5, 15.5)];

/// How a mark moves: when it showed up, the shine the pointer last started, whether the pointer is on it.
#[derive(Clone, Copy, Debug)]
pub struct Play {
    born: f64,
    shine: Option<f64>,
    over: bool,
}

impl Play {
    pub fn new(now: f64) -> Play {
        Play { born: now, shine: None, over: false }
    }

    /// How far the shine crossing now has got (0..1), if one is.
    fn sweep(&self, now: f64, working: bool) -> Option<f32> {
        let a = now - self.born;
        let again = (working && a > FIRST).then(|| FIRST + ((a - FIRST) / AGAIN).floor() * AGAIN);
        [Some(FIRST), again, self.shine.map(|s| s - self.born)].into_iter().flatten().map(|s| (a - s) / SWEEP).find(|p| (0.0..1.0).contains(p)).map(|p| p as f32)
    }
}

/// The logo a session of `harness` is marked with in pixels: Claude's and Codex's (the rest keep their plain mark).
pub fn logo(harness: Option<Harness>) -> Option<Logo> {
    match harness? {
        Harness::Claude => Some(Logo::Claude),
        Harness::Codex => Some(Logo::OpenAi),
        _ => None,
    }
}

/// The cells `logo` covers (column, row), from its vector path, worked out the first time it's wanted.
fn cells(logo: Logo) -> &'static [(u8, u8)] {
    static CLAUDE: OnceLock<Vec<(u8, u8)>> = OnceLock::new();
    static CODEX: OnceLock<Vec<(u8, u8)>> = OnceLock::new();
    let once = if logo == Logo::Claude { &CLAUDE } else { &CODEX };
    once.get_or_init(|| pixelate(logo.path()))
}

/// `path` (a 24×24 box) filled at eight samples a cell each way: a cell is on when the logo covers half of it.
fn pixelate(path: &str) -> Vec<(u8, u8)> {
    const SS: usize = 8;
    let w = GRID * SS;
    let mask = bro_gfx::raster_path(path, 24.0, w as u32, w as u32);
    let mut on = vec![];
    for gy in 0..GRID {
        for gx in 0..GRID {
            let covered: u32 = (0..SS).flat_map(|y| (0..SS).map(move |x| (x, y))).map(|(x, y)| mask[(gy * SS + y) * w + gx * SS + x] as u32).sum();
            if covered * 2 >= 255 * (SS * SS) as u32 {
                on.push((gx as u8, gy as u8));
            }
        }
    }
    on
}

/// A logo's colours: its body lit from the top left, the shine that crosses it, the glow under it.
struct Look {
    lit: Color,
    deep: Color,
    glow: Color,
    light: bool,
    claude: bool,
}

impl Look {
    fn of(logo: Logo, text: Color, light: bool) -> Look {
        if logo == Logo::Claude {
            // Claude's coral, lighter where the light falls and rust where it doesn't
            return Look { lit: Color::hex(0xF4A784), deep: Color::hex(0xC25E3B), glow: Color::hex(0xD97757), light, claude: true };
        }
        // Codex is black or white by design: the text colour, cooling to periwinkle away from the light
        let cool = if light { Color::hex(0x4B55C8) } else { Color::hex(0x9AA8FF) };
        Look { lit: text, deep: text.mix(cool, 0.55), glow: cool, light, claude: false }
    }

    /// The shine at cell (x, y): Claude's warm cream; on Codex's chrome it turns from cyan to violet across the
    /// logo.
    fn shine(&self, x: f32, y: f32) -> Color {
        if self.claude {
            return Color::hex(0xFFF1DC);
        }
        // 188° to 280°
        let hue = (188.0 + 92.0 * ((x - y) / (2.0 * GRID as f32) + 0.5)) / 360.0;
        if self.light { Color::from_hsl(hue, 0.78, 0.56, 1.0) } else { Color::from_hsl(hue, 0.95, 0.8, 1.0) }
    }
}

/// Draw `logo` in pixels, `side` across, centred on `center`.
pub fn draw(ui: &mut Ui, logo: Logo, center: Point, side: f32, working: bool, play: &mut Play) {
    let now = ui.time;
    let a = now - play.born;
    let look = Look::of(logo, ui.t.text, ui.t.light);
    // the pointer coming onto it sends a shine across (once the first has, and not over one crossing)
    let over = ui.hovered(Rect::new(center.x - side / 2.0, center.y - side / 2.0, side, side));
    if over && !play.over && a > FIRST + SWEEP && play.sweep(now, working).is_none() {
        play.shine = Some(now);
    }
    play.over = over;
    let sweep = play.sweep(now, working);
    let blooming = a < SPREAD + 0.07 + POP;
    if blooming || sweep.is_some() || working {
        ui.ticking = true;
    }
    // under it, a glow on the glass as it blooms and while a shine crosses (still, it's just the pixels: a glow at
    // rest muddies the surface)
    let lift = ((1.0 - a / 1.1).clamp(0.0, 1.0) as f32 + sweep.map_or(0.0, |p| (p * PI).sin())).min(1.0);
    let strength = if look.light { 0.1 } else { 0.16 };
    glow(ui, center, side * 0.82, look.glow, strength * lift * (a / 0.4).clamp(0.0, 1.0) as f32);
    // the pixels, whole physical pixels apart so they're crisp when still
    let s = ui.scale;
    let cell = (side / GRID as f32 * s).round().max(2.0);
    let gap = if cell >= 4.0 { (cell * 0.16).round().max(1.0) } else { 0.0 };
    let unit = cell - gap;
    let full = cell * GRID as f32;
    let (ox, oy) = ((center.x * s - full / 2.0).round(), (center.y * s - full / 2.0).round());
    let mid = (GRID as f32 - 1.0) / 2.0;
    let reach = mid * std::f32::consts::SQRT_2;
    let band = sweep.map(|p| -0.25 + 1.5 * p * p * (3.0 - 2.0 * p));
    for &(gx, gy) in cells(logo) {
        let (x, y) = (gx as f32, gy as f32);
        // from the middle out, a little out of step
        let delay = SPREAD * ((x - mid).hypot(y - mid) / reach).min(1.0) as f64 + 0.07 * jitter(gx, gy);
        let p = ((a - delay) / POP).clamp(0.0, 1.0) as f32;
        if p <= 0.0 {
            continue;
        }
        let mut k = back_out(p);
        let shine = look.shine(x, y);
        // lit across from the top left; white hot as it lands
        let mut c = look.lit.mix(look.deep, (x + y) / (2.0 * (GRID as f32 - 1.0)));
        c = c.mix(shine, 0.9 * (1.0 - p).powi(2));
        if let Some(b) = band {
            let along = (x + y * 0.85) / ((GRID as f32 - 1.0) * 1.85);
            let i = (-((along - b) / 0.09).powi(2)).exp();
            c = c.mix(shine, 0.88 * i);
            k *= 1.0 + 0.18 * i;
        }
        let size = unit * k;
        let (cx, cy) = (ox + x * cell + unit / 2.0, oy + y * cell + unit / 2.0);
        pixel(ui, Point::new(cx, cy), size, c);
    }
    // twinkles round its corners as the shine passes
    if let Some(p) = sweep {
        for (n, &(tx, ty)) in TWINKLES.iter().enumerate() {
            // each out and gone before the shine ends
            let life = (p - 0.1 - 0.18 * n as f32) / 0.45;
            if !(0.0..1.0).contains(&life) {
                continue;
            }
            let k = (life * PI).sin();
            let (cx, cy) = (ox + tx * cell + unit / 2.0, oy + ty * cell + unit / 2.0);
            let c = look.shine(tx, ty);
            pixel(ui, Point::new(cx, cy), unit * (0.55 + 0.45 * k), c);
            for d in 1..=(k * 2.4).round() as i32 {
                let (d, sz) = (d as f32 * cell, unit * (0.8 - 0.22 * d as f32) * k);
                for (dx, dy) in [(d, 0.0), (-d, 0.0), (0.0, d), (0.0, -d)] {
                    pixel(ui, Point::new(cx + dx, cy + dy), sz, c);
                }
            }
        }
    }
}

/// A solid pixel `size` across (physical px) centred on `c`, its corners just softened.
fn pixel(ui: &mut Ui, c: Point, size: f32, color: Color) {
    if size <= 0.2 {
        return;
    }
    let r = Rect::new(c.x - size / 2.0, c.y - size / 2.0, size, size);
    let round = if size > 4.0 { (size * 0.2).min(2.0 * ui.scale) } else { 0.0 };
    ui.scene.quad(Quad::new(r, color.with_alpha(1.0)).radius(round));
}

/// A soft round glow of `color` painted into the glass, `strength` its alpha in the middle: rings stacked from the
/// edge in, each a little more.
fn glow(ui: &mut Ui, center: Point, radius: f32, color: Color, strength: f32) {
    const RINGS: usize = 14;
    if strength <= 0.002 {
        return;
    }
    let each = 1.0 - (1.0 - strength.min(0.9)).powf(1.0 / RINGS as f32);
    for i in 0..RINGS {
        let r = radius * (1.0 - 0.72 * i as f32 / RINGS as f32);
        ui.glass_round(Rect::new(center.x - r, center.y - r, r * 2.0, r * 2.0), r, color.with_alpha(each));
    }
}

/// A steady 0..1 for a cell, so the pixels land a little out of step.
fn jitter(x: u8, y: u8) -> f64 {
    let h = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    (h % 1000) as f64 / 1000.0
}

/// Ease out with a little overshoot: a pixel lands a touch big and settles.
fn back_out(t: f32) -> f32 {
    let (c1, c3) = (1.701_58, 2.701_58);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logos_come_out_as_pixels() {
        for logo in [Logo::Claude, Logo::OpenAi] {
            let on = cells(logo);
            // enough to read as the logo, not a filled square
            assert!(on.len() > 150 && on.len() < GRID * GRID / 2, "{logo:?}: {} cells", on.len());
            assert!(on.iter().all(|&(x, y)| (x as usize) < GRID && (y as usize) < GRID));
            // the twinkles sit off it
            for (tx, ty) in TWINKLES {
                assert!(!on.contains(&(tx.round() as u8, ty.round() as u8)), "{logo:?}: a twinkle on the logo at {tx},{ty}");
            }
        }
        assert_eq!(logo(Some(Harness::Claude)), Some(Logo::Claude));
        assert_eq!(logo(Some(Harness::Codex)), Some(Logo::OpenAi));
        assert_eq!(logo(Some(Harness::Pi)), None);
        assert_eq!(logo(None), None);
    }

    #[test]
    fn it_shines_as_it_opens_while_the_agent_gets_ready_and_under_the_pointer_and_is_still_otherwise() {
        let p = Play::new(10.0);
        assert_eq!(p.sweep(10.2, false), None, "the pixels are still landing");
        assert!(p.sweep(10.0 + FIRST + 0.1, false).is_some(), "the first shine");
        assert_eq!(p.sweep(10.0 + FIRST + SWEEP + 0.1, false), None, "then still");
        assert_eq!(p.sweep(10.0 + FIRST + AGAIN + 0.1, false), None, "still while ready");
        assert!(p.sweep(10.0 + FIRST + AGAIN + 0.1, true).is_some(), "again while it gets ready");
        let hovered = Play { shine: Some(30.0), ..p };
        assert!(hovered.sweep(30.4, false).is_some(), "under the pointer");
        assert_eq!(hovered.sweep(30.0 + SWEEP + 0.01, false), None);
        // lands a touch big and settles where it belongs
        assert_eq!(back_out(0.0), 0.0);
        assert!((back_out(1.0) - 1.0).abs() < 1e-6);
        assert!((0..100).map(|i| back_out(i as f32 / 100.0)).fold(0.0, f32::max) > 1.05);
    }
}
