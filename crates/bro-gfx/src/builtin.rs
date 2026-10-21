//! Box drawing, block elements, braille and powerline glyphs drawn to the exact cell size, so borders join
//! seamlessly between cells whatever the font (the way Ghostty, kitty and WezTerm do it).

/// True for characters this module draws itself.
pub fn handles(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0x2800..=0x28FF | 0xE0B0..=0xE0B7)
}

/// An alpha mask (`w`×`h`, one byte per pixel) for `c` in a `w`×`h` cell, or None if `c` isn't drawn here.
pub fn render(c: char, w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || !handles(c) {
        return None;
    }
    let mut m = Mask::new(w, h);
    let cp = c as u32;
    match cp {
        0x2500..=0x257F => box_drawing(&mut m, cp)?,
        0x2580..=0x259F => blocks(&mut m, cp),
        0x2800..=0x28FF => braille(&mut m, cp),
        0xE0B0..=0xE0B7 => powerline(&mut m, cp),
        _ => return None,
    }
    Some(m.px)
}

struct Mask {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Mask {
    fn new(w: u32, h: u32) -> Mask {
        Mask { w, h, px: vec![0; (w * h) as usize] }
    }
    /// Fill whole pixels in [x0, x1) × [y0, y1) with `a` (keeps the larger value where they overlap).
    fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, a: u8) {
        let (x0, x1) = (x0.max(0) as u32, (x1.max(0) as u32).min(self.w));
        let (y0, y1) = (y0.max(0) as u32, (y1.max(0) as u32).min(self.h));
        for y in y0..y1 {
            let row = (y * self.w) as usize;
            for x in x0..x1 {
                let p = &mut self.px[row + x as usize];
                *p = (*p).max(a);
            }
        }
    }
    /// Fractional rectangle, antialiased at its edges.
    fn fill_f(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, a: f32) {
        let (ix0, ix1) = (x0.floor().max(0.0) as u32, (x1.ceil().max(0.0) as u32).min(self.w));
        let (iy0, iy1) = (y0.floor().max(0.0) as u32, (y1.ceil().max(0.0) as u32).min(self.h));
        for y in iy0..iy1 {
            let cy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
            for x in ix0..ix1 {
                let cx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                let v = (cx * cy * a * 255.0).round() as u8;
                let p = &mut self.px[(y * self.w + x) as usize];
                *p = (*p).max(v);
            }
        }
    }
    /// Coverage of a shape given as an inside test, 4×4 supersampled.
    fn shape(&mut self, inside: impl Fn(f32, f32) -> bool) {
        const N: u32 = 4;
        for y in 0..self.h {
            for x in 0..self.w {
                let mut hit = 0;
                for sy in 0..N {
                    for sx in 0..N {
                        let px = x as f32 + (sx as f32 + 0.5) / N as f32;
                        let py = y as f32 + (sy as f32 + 0.5) / N as f32;
                        if inside(px, py) {
                            hit += 1;
                        }
                    }
                }
                if hit > 0 {
                    let v = (hit * 255 / (N * N)) as u8;
                    let p = &mut self.px[(y * self.w + x) as usize];
                    *p = (*p).max(v);
                }
            }
        }
    }
}

/// Line weight of one arm.
#[derive(Clone, Copy, PartialEq, Eq)]
enum W {
    None,
    Light,
    Heavy,
    Double,
}

/// Arms (up, right, down, left) for U+2500–U+257F; None for the dashed, arc and diagonal ones.
fn arms(cp: u32) -> Option<[W; 4]> {
    use W::{Double as D, Heavy as H, Light as L, None as N};
    // indexed by cp - 0x2500
    const T: [[W; 4]; 128] = [
        [N, L, N, L], [N, H, N, H], [L, N, L, N], [H, N, H, N], // ─ ━ │ ┃
        [N, N, N, N], [N, N, N, N], [N, N, N, N], [N, N, N, N], // dashes (special)
        [N, N, N, N], [N, N, N, N], [N, N, N, N], [N, N, N, N],
        [N, L, L, N], [N, H, L, N], [N, L, H, N], [N, H, H, N], // ┌ ┍ ┎ ┏
        [N, N, L, L], [N, N, L, H], [N, N, H, L], [N, N, H, H], // ┐ ┑ ┒ ┓
        [L, L, N, N], [L, H, N, N], [H, L, N, N], [H, H, N, N], // └ ┕ ┖ ┗
        [L, N, N, L], [L, N, N, H], [H, N, N, L], [H, N, N, H], // ┘ ┙ ┚ ┛
        [L, L, L, N], [L, H, L, N], [H, L, L, N], [L, L, H, N], // ├ ┝ ┞ ┟
        [H, L, H, N], [H, H, L, N], [L, H, H, N], [H, H, H, N], // ┠ ┡ ┢ ┣
        [L, N, L, L], [L, N, L, H], [H, N, L, L], [L, N, H, L], // ┤ ┥ ┦ ┧
        [H, N, H, L], [H, N, L, H], [L, N, H, H], [H, N, H, H], // ┨ ┩ ┪ ┫
        [N, L, L, L], [N, L, L, H], [N, H, L, L], [N, H, L, H], // ┬ ┭ ┮ ┯
        [N, L, H, L], [N, L, H, H], [N, H, H, L], [N, H, H, H], // ┰ ┱ ┲ ┳
        [L, L, N, L], [L, L, N, H], [L, H, N, L], [L, H, N, H], // ┴ ┵ ┶ ┷
        [H, L, N, L], [H, L, N, H], [H, H, N, L], [H, H, N, H], // ┸ ┹ ┺ ┻
        [L, L, L, L], [L, L, L, H], [L, H, L, L], [L, H, L, H], // ┼ ┽ ┾ ┿
        [H, L, L, L], [L, L, H, L], [H, L, H, L], [H, L, L, H], // ╀ ╁ ╂ ╃
        [H, H, L, L], [L, L, H, H], [L, H, H, L], [H, H, L, H], // ╄ ╅ ╆ ╇
        [L, H, H, H], [H, L, H, H], [H, H, H, L], [H, H, H, H], // ╈ ╉ ╊ ╋
        [N, N, N, N], [N, N, N, N], [N, N, N, N], [N, N, N, N], // dashes (special)
        [N, D, N, D], [D, N, D, N], [N, D, L, N], [N, L, D, N], // ═ ║ ╒ ╓
        [N, D, D, N], [N, N, L, D], [N, N, D, L], [N, N, D, D], // ╔ ╕ ╖ ╗
        [L, D, N, N], [D, L, N, N], [D, D, N, N], [L, N, N, D], // ╘ ╙ ╚ ╛
        [D, N, N, L], [D, N, N, D], [L, D, L, N], [D, L, D, N], // ╜ ╝ ╞ ╟
        [D, D, D, N], [L, N, L, D], [D, N, D, L], [D, N, D, D], // ╠ ╡ ╢ ╣
        [N, D, L, D], [N, L, D, L], [N, D, D, D], [L, D, N, D], // ╤ ╥ ╦ ╧
        [D, L, N, L], [D, D, N, D], [L, D, L, D], [D, L, D, L], // ╨ ╩ ╪ ╫
        [D, D, D, D], [N, N, N, N], [N, N, N, N], [N, N, N, N], // ╬ arcs (special)
        [N, N, N, N], [N, N, N, N], [N, N, N, N], [N, N, N, N], // ╰ diagonals (special)
        [N, N, N, L], [L, N, N, N], [N, L, N, N], [N, N, L, N], // ╴ ╵ ╶ ╷
        [N, N, N, H], [H, N, N, N], [N, H, N, N], [N, N, H, N], // ╸ ╹ ╺ ╻
        [N, H, N, L], [L, N, H, N], [N, L, N, H], [H, N, L, N], // ╼ ╽ ╾ ╿
    ];
    let a = T[(cp - 0x2500) as usize];
    (a != [N, N, N, N]).then_some(a)
}

/// Stroke thicknesses for a cell: (light, heavy).
fn strokes(m: &Mask) -> (i32, i32) {
    let light = ((m.h.min(m.w * 2) as f32) / 17.0).round().max(1.0) as i32;
    (light, (light * 2).max(light + 1))
}

fn box_drawing(m: &mut Mask, cp: u32) -> Option<()> {
    let (light, heavy) = strokes(m);
    let (w, h) = (m.w as i32, m.h as i32);
    match cp {
        // dashed lines: (count, heavy, vertical)
        0x2504..=0x250B | 0x254C..=0x254F => {
            let (count, heavy_line, vertical) = match cp {
                0x2504 => (3, false, false),
                0x2505 => (3, true, false),
                0x2506 => (3, false, true),
                0x2507 => (3, true, true),
                0x2508 => (4, false, false),
                0x2509 => (4, true, false),
                0x250A => (4, false, true),
                0x250B => (4, true, true),
                0x254C => (2, false, false),
                0x254D => (2, true, false),
                0x254E => (2, false, true),
                _ => (2, true, true),
            };
            let t = if heavy_line { heavy } else { light };
            let len = if vertical { h } else { w } as f32;
            let seg = len / count as f32;
            let gap = (seg * 0.3).max(1.0);
            for i in 0..count {
                let a = i as f32 * seg + gap * 0.5;
                let b = (i + 1) as f32 * seg - gap * 0.5;
                if vertical {
                    let x = (w - t) / 2;
                    m.fill_f(x as f32, a, (x + t) as f32, b, 1.0);
                } else {
                    let y = (h - t) / 2;
                    m.fill_f(a, y as f32, b, (y + t) as f32, 1.0);
                }
            }
            return Some(());
        }
        // rounded corners
        0x256D..=0x2570 => {
            arc(m, cp, light);
            return Some(());
        }
        0x2571..=0x2573 => {
            let t = light as f32 * 0.9;
            let (wf, hf) = (w as f32, h as f32);
            let len = (wf * wf + hf * hf).sqrt();
            let rising = cp == 0x2571 || cp == 0x2573;
            let falling = cp == 0x2572 || cp == 0x2573;
            m.shape(|x, y| {
                // distance to the diagonals through the cell
                let d1 = ((hf * x + wf * y - wf * hf).abs()) / len; // ╱ from bottom-left to top-right
                let d2 = ((hf * x - wf * y).abs()) / len; // ╲
                (rising && d1 <= t * 0.5 + 0.25) || (falling && d2 <= t * 0.5 + 0.25)
            });
            return Some(());
        }
        _ => {}
    }
    let [up, right, down, left] = arms(cp)?;
    let thick = |a: W| match a {
        W::None => 0,
        W::Light | W::Double => light,
        W::Heavy => heavy,
    };
    // centre lines: a stroke of thickness t is centred on the cell
    let vx = |t: i32| (w - t) / 2;
    let hy = |t: i32| (h - t) / 2;
    let double = [up, right, down, left].contains(&W::Double);
    if !double {
        // each arm runs from its edge to the far side of the thickest crossing arm, so corners fill in
        let tv = thick(up).max(thick(down));
        let th = thick(left).max(thick(right));
        let ext_v = |t: i32| if th > 0 { (h - th) / 2 + th } else { (h - t) / 2 + t };
        let ext_h = |t: i32| if tv > 0 { (w - tv) / 2 + tv } else { (w - t) / 2 + t };
        if up != W::None {
            let t = thick(up);
            m.fill(vx(t), 0, vx(t) + t, ext_v(t), 255);
        }
        if down != W::None {
            let t = thick(down);
            let start = if th > 0 { (h - th) / 2 } else { (h - t) / 2 };
            m.fill(vx(t), start, vx(t) + t, h, 255);
        }
        if left != W::None {
            let t = thick(left);
            m.fill(0, hy(t), ext_h(t), hy(t) + t, 255);
        }
        if right != W::None {
            let t = thick(right);
            let start = if tv > 0 { (w - tv) / 2 } else { (w - t) / 2 };
            m.fill(start, hy(t), w, hy(t) + t, 255);
        }
        return Some(());
    }
    // double lines: two light strokes `off` either side of the centre
    let off = light; // gap between the two strokes = one stroke
    let cx = (w - light) / 2; // left edge of a centred light stroke
    let cy = (h - light) / 2;
    let (vdouble, hdouble) = (up == W::Double || down == W::Double, left == W::Double || right == W::Double);
    let tv = thick(up).max(thick(down));
    let th = thick(left).max(thick(right));
    // a horizontal double arm's two strokes (y positions)
    let (hy1, hy2) = (cy - off, cy + off);
    let (vx1, vx2) = (cx - off, cx + off);
    // where a horizontal stroke starts (right arm) / ends (left arm) given the vertical arms
    let right_start = |upper: bool| -> i32 {
        if vdouble {
            let crossing = if upper { up != W::None } else { down != W::None };
            if crossing { vx2 } else { vx1 }
        } else if tv > 0 {
            (w - tv) / 2
        } else {
            vx1
        }
    };
    let left_end = |upper: bool| -> i32 {
        if vdouble {
            let crossing = if upper { up != W::None } else { down != W::None };
            (if crossing { vx1 } else { vx2 }) + light
        } else if tv > 0 {
            (w - tv) / 2 + tv
        } else {
            vx2 + light
        }
    };
    let down_start = |leftside: bool| -> i32 {
        if hdouble {
            let crossing = if leftside { left != W::None } else { right != W::None };
            if crossing { hy2 } else { hy1 }
        } else if th > 0 {
            (h - th) / 2
        } else {
            hy1
        }
    };
    let up_end = |leftside: bool| -> i32 {
        if hdouble {
            let crossing = if leftside { left != W::None } else { right != W::None };
            (if crossing { hy1 } else { hy2 }) + light
        } else if th > 0 {
            (h - th) / 2 + th
        } else {
            hy2 + light
        }
    };
    let single_v_ext = |dir_up: bool| -> (i32, i32) {
        // a single vertical arm reaching the double horizontal's outer stroke
        if dir_up { (0, if hdouble { hy2 + light } else { (h - th) / 2 + th.max(light) }) } else { (if hdouble { hy1 } else { (h - th) / 2 }, h) }
    };
    let single_h_ext = |dir_left: bool| -> (i32, i32) {
        if dir_left { (0, if vdouble { vx2 + light } else { (w - tv) / 2 + tv.max(light) }) } else { (if vdouble { vx1 } else { (w - tv) / 2 }, w) }
    };
    match right {
        W::Double => {
            m.fill(right_start(true), hy1, w, hy1 + light, 255);
            m.fill(right_start(false), hy2, w, hy2 + light, 255);
        }
        W::None => {}
        a => {
            let t = thick(a);
            let (x0, x1) = single_h_ext(false);
            m.fill(x0, hy(t), x1, hy(t) + t, 255);
        }
    }
    match left {
        W::Double => {
            m.fill(0, hy1, left_end(true), hy1 + light, 255);
            m.fill(0, hy2, left_end(false), hy2 + light, 255);
        }
        W::None => {}
        a => {
            let t = thick(a);
            let (x0, x1) = single_h_ext(true);
            m.fill(x0, hy(t), x1, hy(t) + t, 255);
        }
    }
    match down {
        W::Double => {
            m.fill(vx1, down_start(true), vx1 + light, h, 255);
            m.fill(vx2, down_start(false), vx2 + light, h, 255);
        }
        W::None => {}
        a => {
            let t = thick(a);
            let (y0, y1) = single_v_ext(false);
            m.fill(vx(t), y0, vx(t) + t, y1, 255);
        }
    }
    match up {
        W::Double => {
            m.fill(vx1, 0, vx1 + light, up_end(true), 255);
            m.fill(vx2, 0, vx2 + light, up_end(false), 255);
        }
        W::None => {}
        a => {
            let t = thick(a);
            let (y0, y1) = single_v_ext(true);
            m.fill(vx(t), y0, vx(t) + t, y1, 255);
        }
    }
    Some(())
}

/// ╭ ╮ ╯ ╰: a quarter circle joining the two arms, with straight runs out to the cell edges.
fn arc(m: &mut Mask, cp: u32, t: i32) {
    let (w, h) = (m.w as f32, m.h as f32);
    let t = t as f32;
    // centre of the strokes
    let cx = ((m.w as i32 - t as i32) / 2) as f32 + t * 0.5;
    let cy = ((m.h as i32 - t as i32) / 2) as f32 + t * 0.5;
    let r = (w * 0.5).min(h * 0.5).max(t);
    // the circle's centre sits diagonally inside the corner
    let (sx, sy) = match cp {
        0x256D => (1.0, 1.0),   // ╭ down and right
        0x256E => (-1.0, 1.0),  // ╮ down and left
        0x256F => (-1.0, -1.0), // ╯ up and left
        _ => (1.0, -1.0),       // ╰ up and right
    };
    let (ox, oy) = (cx + sx * r, cy + sy * r);
    let half = t * 0.5;
    m.shape(|x, y| {
        let (dx, dy) = (x - ox, y - oy);
        // inside the quadrant facing the corner: the arc
        let in_quadrant = dx * sx <= 0.0 && dy * sy <= 0.0;
        if in_quadrant {
            let d = (dx * dx + dy * dy).sqrt();
            return (d - r).abs() <= half;
        }
        // straight runs: vertical from the arc's end to the edge, horizontal likewise
        let vertical = (x - cx).abs() <= half && (y - oy) * sy >= 0.0;
        let horizontal = (y - cy).abs() <= half && (x - ox) * sx >= 0.0;
        vertical || horizontal
    });
}

fn blocks(m: &mut Mask, cp: u32) {
    let (w, h) = (m.w as f32, m.h as f32);
    let eighth_v = |n: f32| h * n / 8.0;
    let eighth_h = |n: f32| w * n / 8.0;
    match cp {
        0x2580 => m.fill_f(0.0, 0.0, w, (h / 2.0).round(), 1.0),
        0x2581..=0x2588 => {
            let n = (cp - 0x2580) as f32;
            m.fill_f(0.0, (h - eighth_v(n)).round(), w, h, 1.0)
        }
        0x2589..=0x258F => {
            let n = (0x2590 - cp) as f32;
            m.fill_f(0.0, 0.0, eighth_h(n).round(), h, 1.0)
        }
        0x2590 => m.fill_f((w / 2.0).round(), 0.0, w, h, 1.0),
        0x2591 => m.fill_f(0.0, 0.0, w, h, 0.25),
        0x2592 => m.fill_f(0.0, 0.0, w, h, 0.5),
        0x2593 => m.fill_f(0.0, 0.0, w, h, 0.75),
        0x2594 => m.fill_f(0.0, 0.0, w, eighth_v(1.0).round().max(1.0), 1.0),
        0x2595 => m.fill_f(w - eighth_h(1.0).round().max(1.0), 0.0, w, h, 1.0),
        _ => {
            // quadrants: bits ul, ur, ll, lr
            let q: u8 = match cp {
                0x2596 => 0b0010,
                0x2597 => 0b0001,
                0x2598 => 0b1000,
                0x2599 => 0b1011,
                0x259A => 0b1001,
                0x259B => 0b1110,
                0x259C => 0b1101,
                0x259D => 0b0100,
                0x259E => 0b0110,
                _ => 0b0111, // 0x259F
            };
            let (mx, my) = ((w / 2.0).round(), (h / 2.0).round());
            if q & 0b1000 != 0 {
                m.fill_f(0.0, 0.0, mx, my, 1.0);
            }
            if q & 0b0100 != 0 {
                m.fill_f(mx, 0.0, w, my, 1.0);
            }
            if q & 0b0010 != 0 {
                m.fill_f(0.0, my, mx, h, 1.0);
            }
            if q & 0b0001 != 0 {
                m.fill_f(mx, my, w, h, 1.0);
            }
        }
    }
}

fn braille(m: &mut Mask, cp: u32) {
    let bits = cp - 0x2800;
    if bits == 0 {
        return;
    }
    let (w, h) = (m.w as f32, m.h as f32);
    // dots in a 2×4 grid with a margin; radius from the smaller spacing
    let (mx, my) = (w * 0.18, h * 0.1);
    let (sx, sy) = ((w - 2.0 * mx) / 2.0, (h - 2.0 * my) / 4.0);
    let r = (sx.min(sy) * 0.36).max(0.7);
    // bit → (column, row)
    const DOTS: [(u32, u32); 8] = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
    let centres: Vec<(f32, f32)> = DOTS
        .iter()
        .enumerate()
        .filter(|(i, _)| bits & (1 << i) != 0)
        .map(|(_, &(c, row))| (mx + sx * (c as f32 + 0.5), my + sy * (row as f32 + 0.5)))
        .collect();
    m.shape(|x, y| centres.iter().any(|&(cx, cy)| (x - cx).powi(2) + (y - cy).powi(2) <= r * r));
}

fn powerline(m: &mut Mask, cp: u32) {
    let (w, h) = (m.w as f32, m.h as f32);
    let t = strokes(m).0 as f32;
    match cp {
        // solid triangles pointing right / left
        0xE0B0 => m.shape(|x, y| x <= w * (1.0 - (2.0 * y / h - 1.0).abs())),
        0xE0B2 => m.shape(|x, y| x >= w * (2.0 * y / h - 1.0).abs()),
        // thin chevrons
        0xE0B1 | 0xE0B3 => {
            let right = cp == 0xE0B1;
            m.shape(|x, y| {
                let target = w * (1.0 - (2.0 * y / h - 1.0).abs());
                let xx = if right { x } else { w - x };
                (xx - target).abs() <= t * 0.75
            })
        }
        // half circles
        0xE0B4 => m.shape(|x, y| {
            let (dx, dy) = (x / w, (y - h / 2.0) / (h / 2.0));
            dx * dx + dy * dy <= 1.0
        }),
        0xE0B6 => m.shape(|x, y| {
            let (dx, dy) = ((w - x) / w, (y - h / 2.0) / (h / 2.0));
            dx * dx + dy * dy <= 1.0
        }),
        // half circle outlines
        _ => {
            let right = cp == 0xE0B5;
            m.shape(|x, y| {
                let xx = if right { x } else { w - x };
                let (dx, dy) = (xx / w, (y - h / 2.0) / (h / 2.0));
                let d = (dx * dx + dy * dy).sqrt();
                (d - 1.0).abs() * w.min(h / 2.0) <= t * 0.75
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: &[u8], w: u32, x: u32, y: u32) -> u8 {
        m[(y * w + x) as usize]
    }

    #[test]
    fn lines_reach_the_edges_and_join() {
        let (w, h) = (10, 20);
        let hline = render('─', w, h).unwrap();
        assert!(at(&hline, w, 0, h / 2 - 1) > 0 || at(&hline, w, 0, h / 2) > 0);
        assert!(at(&hline, w, w - 1, h / 2 - 1) > 0 || at(&hline, w, w - 1, h / 2) > 0);
        assert_eq!(at(&hline, w, 0, 0), 0);
        let corner = render('┌', w, h).unwrap();
        // reaches the right and bottom edges, not the top or left
        assert!((0..h).any(|y| at(&corner, w, w - 1, y) > 0));
        assert!((0..w).any(|x| at(&corner, w, x, h - 1) > 0));
        assert!((0..h).all(|y| at(&corner, w, 0, y) == 0));
        assert!((0..w).all(|x| at(&corner, w, x, 0) == 0));
        let cross = render('┼', w, h).unwrap();
        assert!((0..h).any(|y| at(&cross, w, 0, y) > 0) && (0..w).any(|x| at(&cross, w, x, 0) > 0));
        // every box-drawing character renders something
        for cp in 0x2500..=0x257F {
            let c = char::from_u32(cp).unwrap();
            let m = render(c, w, h).unwrap_or_else(|| panic!("U+{cp:04X} not drawn"));
            assert!(m.iter().any(|&p| p > 0), "U+{cp:04X} is blank");
        }
    }

    #[test]
    fn blocks_and_braille() {
        let (w, h) = (8, 16);
        let full = render('█', w, h).unwrap();
        assert!(full.iter().all(|&p| p == 255));
        let lower = render('▄', w, h).unwrap();
        assert_eq!(at(&lower, w, 3, 0), 0);
        assert_eq!(at(&lower, w, 3, h - 1), 255);
        let shade = render('▒', w, h).unwrap();
        assert!(shade.iter().all(|&p| (120..=135).contains(&p)));
        assert!(render('\u{2800}', w, h).unwrap().iter().all(|&p| p == 0), "blank braille");
        let all = render('\u{28FF}', w, h).unwrap();
        assert!(all.iter().any(|&p| p > 0));
        assert!(render('a', w, h).is_none());
        assert!(render('\u{E0B0}', w, h).unwrap().iter().any(|&p| p > 0));
    }
}
