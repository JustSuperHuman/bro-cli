//! Rune: live ASCII art in the manner of rune-ascii (github.com/zeke-john/rune). A frame is a grid of characters
//! picked by density from Rune's ramp, each in its own colour. rune-ascii converts video into those frames; bro
//! draws them from small scenes instead, so they take the theme's colours, fit any size and ship no data.
//!
//! Both UIs draw the same frames: the desktop app through `app::gui::kit::rune` (glyphs at any size, faded by
//! their ink), the terminal UI as cells (`ui::rune`, blended into the background).

/// Rune's density ramp, empty to full.
pub const RAMP: &[u8] = b" .~-_=+*%#0oOxX@$";

/// A colour, sRGB channels 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Rgb {
    pub fn hex(v: u32) -> Rgb {
        Rgb(((v >> 16) & 0xff) as f32 / 255.0, ((v >> 8) & 0xff) as f32 / 255.0, (v & 0xff) as f32 / 255.0)
    }
    /// `self` → `o` by `t` (0..1).
    pub fn mix(self, o: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        Rgb(self.0 + (o.0 - self.0) * t, self.1 + (o.1 - self.1) * t, self.2 + (o.2 - self.2) * t)
    }
    /// Perceived brightness, 0..1.
    pub fn luma(self) -> f32 {
        0.299 * self.0 + 0.587 * self.1 + 0.114 * self.2
    }
    /// How colourful it is (0 for greys).
    pub fn chroma(self) -> f32 {
        self.0.max(self.1).max(self.2) - self.0.min(self.1).min(self.2)
    }
    pub fn to8(self) -> (u8, u8, u8) {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (c(self.0), c(self.1), c(self.2))
    }
}

/// The theme's colours a scene paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inks {
    /// the theme's accent
    pub accent: Rgb,
    /// its second highlight: seas, petals
    pub shine: Rgb,
    /// green: land
    pub good: Rgb,
    /// warm: a ring, city lights, a flame's body
    pub warm: Rgb,
    /// hot: a flame's edge
    pub hot: Rgb,
    /// body text: ice, cloud, the white of a flame
    pub text: Rgb,
    /// what it's drawn on (dim ink fades into it)
    pub bg: Rgb,
}

/// The agents that circle the welcome screen's world, in their marks' colours: Claude, Codex (drawn in the text
/// colour, as its mark is), Pi.
pub fn moons(inks: &Inks) -> [Rgb; 3] {
    [Rgb::hex(0xD97757), inks.text, Rgb::hex(0xA78BFA)]
}

/// One character of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    /// an ASCII character, b' ' for nothing
    pub ch: u8,
    pub color: Rgb,
    /// how strongly it shows, 0..1
    pub ink: f32,
}

impl Cell {
    pub const EMPTY: Cell = Cell { ch: b' ', color: Rgb(0.0, 0.0, 0.0), ink: 0.0 };

    /// The ramp's character for `ink`, in `color`.
    fn of(ink: f32, color: Rgb) -> Cell {
        let ink = ink.clamp(0.0, 1.0);
        let i = (ink * (RAMP.len() - 1) as f32).round() as usize;
        Cell { ch: RAMP[i.min(RAMP.len() - 1)], color, ink }
    }
}

/// What to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scene {
    /// A world turning under its own weather, a ring around it and the agents for moons (the welcome screen).
    World,
    /// A fire catching (a session on its way up).
    Kindle,
    /// Petals turning, sparks chasing round them (a resume waiting its turn).
    Bloom,
}

/// A rendered frame: `cols × rows` cells, row by row.
#[derive(Clone, Debug)]
pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
}

impl Frame {
    pub fn at(&self, col: usize, row: usize) -> Cell {
        self.cells[row * self.cols + col]
    }
    /// The frame as text, one line per row (tests, logs).
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.cells.chunks(self.cols.max(1)).map(|r| r.iter().map(|c| c.ch as char).collect::<String>()).collect::<Vec<_>>().join("\n")
    }
}

/// Draw `scene` `t` seconds in, in a `cols × rows` grid whose cells are `aspect` times taller than they are wide.
pub fn render(scene: Scene, cols: usize, rows: usize, aspect: f32, t: f64, inks: &Inks) -> Frame {
    let mut f = Frame { cols, rows, cells: vec![Cell::EMPTY; cols * rows] };
    if cols == 0 || rows == 0 {
        return f;
    }
    match scene {
        Scene::World => world(&mut f, aspect.max(0.5), t, inks),
        Scene::Kindle => kindle(&mut f, aspect.max(0.5), t, inks),
        Scene::Bloom => bloom(&mut f, aspect.max(0.5), t, inks),
    }
    f
}

/// The grid size that shows `scene` at its natural proportions `rows` tall.
pub fn cols_for(scene: Scene, rows: usize, aspect: f32) -> usize {
    let h = rows as f32 * aspect;
    let w = match scene {
        // the moons' orbits are about 2.2 times as wide as the scene is tall
        Scene::World => h * WORLD_W / WORLD_H,
        Scene::Kindle => h * 1.25,
        Scene::Bloom => h,
    };
    w.round().max(1.0) as usize
}

/// Where the world's planet sits in a frame: its centre (column, row) and its radius in columns.
pub fn world_planet(cols: usize, rows: usize, aspect: f32) -> (f32, f32, f32) {
    let u = (cols as f32 / WORLD_W).min(rows as f32 * aspect / WORLD_H);
    (cols as f32 / 2.0, rows as f32 / 2.0, PLANET * u)
}

// ------------------------------------------------------------------ noise

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 0x00ff_ffff as f32
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    smooth(((x - a) / (b - a)).clamp(0.0, 1.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Value noise in 3D, 0..1.
fn noise3(x: f32, y: f32, z: f32) -> f32 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (smooth(x - xi), smooth(y - yi), smooth(z - zi));
    let (x0, y0, z0) = (xi as i32, yi as i32, zi as i32);
    let c = |dx: i32, dy: i32, dz: i32| hash3(x0 + dx, y0 + dy, z0 + dz);
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), fx);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), fx);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), fx);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), fx);
    lerp(lerp(x00, x10, fy), lerp(x01, x11, fy), fz)
}

/// Fractal noise: `octaves` layers, each twice as fine and half as strong, 0..1.
fn fbm3(x: f32, y: f32, z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 0.5, 0.0, 1.0);
    for i in 0..octaves {
        // each layer shifted, so their grids don't line up
        let o = i as f32 * 17.31;
        sum += amp * noise3(x * f + o, y * f - o, z * f + o * 0.5);
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm
}

fn fbm2(x: f32, y: f32, octaves: u32) -> f32 {
    fbm3(x, y, 0.37, octaves)
}

type V3 = [f32; 3];

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Where the light comes from: up and to the left, in front (screen space, y down, z towards you).
const SUN: V3 = [-0.62, -0.48, 0.62];

// ------------------------------------------------------------------ the world

/// The world scene's extent in its own units (the planet's radius is 0.46): moons' orbits across, ring and
/// planet up and down.
const WORLD_W: f32 = 2.6;
const WORLD_H: f32 = 1.06;
const PLANET: f32 = 0.46;
const RING_IN: f32 = 0.62;
const RING_OUT: f32 = 0.86;
/// The ring in strokes that run along it.
const RING_RAMP: &[u8] = b".-~=+*";
const MOON: f32 = 0.085;

struct Moon {
    color: Rgb,
    center: V3,
}

/// A moon's orbit: its radius, its speed (radians a second), where it starts and how its plane leans.
const ORBITS: [(f32, f32, f32, f32); 3] = [(1.0, 0.55, 0.4, 0.05), (1.1, -0.38, 2.6, -0.08), (1.19, 0.27, 4.4, 0.13)];

fn ring_normal() -> V3 {
    // nearly edge on, leaning a little to the right
    norm([0.16, -0.95, 0.27])
}

fn orbit_point(radius: f32, angle: f32, lean: f32) -> V3 {
    let n = norm([lean, -0.96, 0.3]);
    let e1 = norm(cross(n, [0.0, 0.0, 1.0]));
    let e1 = if dot(e1, [1.0, 0.0, 0.0]) < 0.0 { [-e1[0], -e1[1], -e1[2]] } else { e1 };
    let e2 = cross(n, e1);
    let (c, s) = (angle.cos(), angle.sin());
    [radius * (c * e1[0] + s * e2[0]), radius * (c * e1[1] + s * e2[1]), radius * (c * e1[2] + s * e2[2])]
}

fn world(f: &mut Frame, aspect: f32, t: f64, inks: &Inks) {
    let (cols, rows) = (f.cols as f32, f.rows as f32);
    // scene units → cell widths: the whole scene fits, centred
    let u = (cols / WORLD_W).min(rows * aspect / WORLD_H);
    let tf = t as f32;
    let sun = norm(SUN);
    let half = norm([sun[0], sun[1], sun[2] + 1.0]);
    let spin = (t * 0.21 % std::f64::consts::TAU) as f32;
    let drift = (t * 0.045 % 1000.0) as f32;
    let (tilt_c, tilt_s) = (0.38f32.cos(), 0.38f32.sin());
    let ring_n = ring_normal();
    let moons: Vec<Moon> = ORBITS
        .iter()
        .zip(moons(inks))
        .map(|(&(radius, speed, start, lean), color)| Moon { color, center: orbit_point(radius, start + tf * speed, lean) })
        .collect();
    // seas lean blue and deep, land green, whatever the theme's greys: a world should look like one
    let sea = inks.shine.mix(Rgb::hex(0x3f86d8), 0.45);
    let deep = sea.mix(inks.bg, 0.45);
    let land = inks.good.mix(Rgb::hex(0x6cc76f), 0.3);
    let high = land.mix(inks.warm, 0.6);
    let ice = inks.text;
    let ring = inks.warm.mix(inks.text, 0.25);
    let air = inks.shine.mix(inks.text, 0.2);
    for r in 0..f.rows {
        for c in 0..f.cols {
            let x = (c as f32 + 0.5 - cols / 2.0) / u;
            let y = ((r as f32 + 0.5) * aspect - rows * aspect / 2.0) / u;
            // what's nearest along the line of sight: (depth, cell)
            let mut best: Option<(f32, Cell)> = None;
            let mut take = |z: f32, cell: Cell| {
                if best.is_none_or(|(bz, _)| z > bz) {
                    best = Some((z, cell));
                }
            };
            let d2 = x * x + y * y;
            let planet_z = (d2 < PLANET * PLANET).then(|| (PLANET * PLANET - d2).sqrt());
            // the planet
            if let Some(z) = planet_z {
                let n = [x / PLANET, y / PLANET, z / PLANET];
                // into the planet's frame: lean its axis, then turn it
                let (x1, y1) = (n[0] * tilt_c - n[1] * tilt_s, n[0] * tilt_s + n[1] * tilt_c);
                let (sc, ss) = (spin.cos(), spin.sin());
                let q = [x1 * sc + n[2] * ss, y1, -x1 * ss + n[2] * sc];
                let h = fbm3(q[0] * 1.9 + 3.7, q[1] * 1.9 - 1.3, q[2] * 1.9 + 0.4, 5);
                let coast = smoothstep(0.5, 0.545, h);
                let polar = smoothstep(0.8, 0.93, q[1].abs() + (h - 0.5) * 0.5);
                // clouds turn a little faster than the ground
                let (dc, ds) = (drift.cos(), drift.sin());
                let qc = [q[0] * dc + q[2] * ds, q[1], -q[0] * ds + q[2] * dc];
                let cloud = smoothstep(0.58, 0.74, fbm3(qc[0] * 3.1 + 9.0, qc[1] * 5.2, qc[2] * 3.1 - 4.0, 4));
                let diffuse = dot(n, sun);
                let day = smoothstep(-0.18, 0.3, diffuse);
                let lit = day * (0.35 + 0.65 * diffuse.max(0.0));
                let ground = deep.mix(sea, smoothstep(0.3, 0.5, h)).mix(land.mix(high, smoothstep(0.6, 0.72, h)), coast);
                let ground = ground.mix(ice, polar);
                let shine = dot(n, half).max(0.0).powf(60.0) * (1.0 - coast) * (1.0 - polar) * day;
                let base = ground.mix(ice, cloud * 0.85);
                // the air at the edge, lit on the sunny side
                let rim = (1.0 - z / PLANET).powi(3) * (0.4 + 0.6 * day);
                let color = inks.bg.mix(base, 0.3 + 0.7 * lit).mix(air, rim * 0.7).mix(inks.text, shine * 0.6);
                // the seas in sparse letters, the land dense: the continents stand out as Rune's world's do
                let solid = 0.34 + 0.66 * coast.max(polar);
                let mut ink = 0.08 + 0.72 * lit * solid + 0.3 * cloud * day + 0.15 * shine + rim * 0.2;
                let mut cell = Cell::of(ink, color);
                // the night side: lights where people live
                if day < 0.35 && coast > 0.5 && polar < 0.3 {
                    let k = hash3((q[0] * 34.0) as i32, (q[1] * 34.0) as i32, (q[2] * 34.0) as i32);
                    if k > 0.88 {
                        let flicker = 0.75 + 0.25 * (tf * 3.0 + k * 40.0).sin();
                        ink = 0.32 + 0.2 * flicker;
                        cell = Cell { ch: if k > 0.96 { b'*' } else { b'+' }, color: inks.bg.mix(inks.warm, 0.6 + 0.4 * flicker), ink };
                    }
                }
                take(z, cell);
            } else if d2 < (PLANET * 1.2).powi(2) {
                // the glow around it
                let d = d2.sqrt();
                let g = (1.0 - (d - PLANET) / (PLANET * 0.2)).clamp(0.0, 1.0).powi(2);
                let toward = ((x * sun[0] + y * sun[1]) / (d * (sun[0] * sun[0] + sun[1] * sun[1]).sqrt())).max(0.0);
                let ink = g * (0.12 + 0.3 * toward);
                if ink > 0.04 {
                    take(-PLANET, Cell::of(ink, inks.bg.mix(air, 0.4 + 0.6 * g)));
                }
            }
            // the ring: where the line of sight crosses its plane
            if ring_n[2].abs() > 1e-3 {
                let z = -(ring_n[0] * x + ring_n[1] * y) / ring_n[2];
                let p = [x, y, z];
                let rho = dot(p, p).sqrt();
                if (RING_IN..=RING_OUT).contains(&rho) && planet_z.is_none_or(|pz| z > pz) {
                    let band = fbm2(rho * 26.0, 0.0, 3);
                    let gap = 1.0 - 0.85 * (-((rho - 0.77) / 0.016).powi(2)).exp();
                    let edge = smoothstep(RING_IN, RING_IN + 0.03, rho) * smoothstep(RING_OUT, RING_OUT - 0.04, rho);
                    // clumps going round, so the ring is seen to turn
                    let around = z.atan2(x);
                    let clump = fbm2(rho * 30.0, around * 3.0 - tf * 0.35, 2);
                    // the planet's shadow falls across it
                    let b = dot(p, sun);
                    let shadowed = b < 0.0 && b * b - (dot(p, p) - PLANET * PLANET) > 0.0;
                    let light = if shadowed { 0.25 } else { 1.0 };
                    let ink = (0.12 + 0.42 * band + 0.16 * clump) * gap * edge * light;
                    if ink > 0.05 {
                        let k = ((ink / 0.62) * (RING_RAMP.len() - 1) as f32).round() as usize;
                        let color = inks.bg.mix(ring.mix(inks.text, band * 0.3), 0.35 + 0.65 * light * edge);
                        take(z, Cell { ch: RING_RAMP[k.min(RING_RAMP.len() - 1)], color, ink });
                    }
                }
            }
            // the moons
            for m in &moons {
                let (dx, dy) = (x - m.center[0], y - m.center[1]);
                let dd = dx * dx + dy * dy;
                if dd < MOON * MOON {
                    let dz = (MOON * MOON - dd).sqrt();
                    let z = m.center[2] + dz;
                    // behind the planet: hidden
                    if planet_z.is_some() && m.center[2] < 0.0 {
                        continue;
                    }
                    let n = [dx / MOON, dy / MOON, dz / MOON];
                    let lit = smoothstep(-0.2, 0.6, dot(n, sun));
                    // round letters from the middle of the ramp: a ball, not a smudge
                    let ch = if lit > 0.75 { b'O' } else if lit > 0.4 { b'o' } else { b'.' };
                    take(z, Cell { ch, color: inks.bg.mix(m.color, 0.45 + 0.55 * lit), ink: 0.5 + 0.4 * lit });
                }
            }
            let cell = match best {
                Some((_, cell)) => cell,
                None => star(c, r, f.cols, f.rows, tf, inks),
            };
            f.cells[r * f.cols + c] = cell;
        }
    }
    // the moons' wakes: dots trailing each one round its orbit
    for (&(radius, speed, start, lean), m) in ORBITS.iter().zip(&moons) {
        let now = start + tf * speed;
        for i in 1..56 {
            let back = i as f32 * 0.02 * speed.signum();
            let p = orbit_point(radius, now - back, lean);
            let c = (p[0] * u + cols / 2.0 - 0.5).round();
            let r = ((p[1] * u + rows * aspect / 2.0) / aspect - 0.5).round();
            if c < 0.0 || r < 0.0 || c >= cols || r >= rows {
                continue;
            }
            // hidden behind the planet
            if p[2] < 0.0 && p[0] * p[0] + p[1] * p[1] < PLANET * PLANET {
                continue;
            }
            let fade = 1.0 - i as f32 / 56.0;
            let idx = r as usize * f.cols + c as usize;
            let ink = 0.06 + 0.3 * fade * fade;
            if f.cells[idx].ink < ink {
                let ch = if fade > 0.66 { b'~' } else if fade > 0.33 { b'-' } else { b'.' };
                f.cells[idx] = Cell { ch, color: inks.bg.mix(m.color, 0.25 + 0.6 * fade), ink };
            }
        }
    }
}

/// A star in an empty cell (most cells have none), twinkling; sparser towards the frame's edges.
fn star(c: usize, r: usize, cols: usize, rows: usize, t: f32, inks: &Inks) -> Cell {
    let k = hash3(c as i32, r as i32, 7);
    let (dx, dy) = ((c as f32 + 0.5) / cols as f32 - 0.5, (r as f32 + 0.5) / rows as f32 - 0.5);
    let edge = 1.0 - smoothstep(0.25, 0.5, (dx * dx + dy * dy).sqrt());
    if k < 1.0 - 0.035 * edge {
        return Cell::EMPTY;
    }
    let phase = hash3(c as i32, r as i32, 11);
    let twinkle = 0.5 + 0.5 * (t * (0.7 + phase * 1.8) + phase * 40.0).sin();
    let ink = 0.1 + 0.3 * twinkle * twinkle;
    let ch = if twinkle > 0.93 && phase > 0.6 { b'+' } else if twinkle > 0.6 { b'~' } else { b'.' };
    Cell { ch, color: inks.bg.mix(inks.text.mix(inks.shine, phase), 0.25 + 0.55 * twinkle), ink }
}

// ------------------------------------------------------------------ a fire catching

fn kindle(f: &mut Frame, aspect: f32, t: f64, inks: &Inks) {
    let (cols, rows) = (f.cols as f32, f.rows as f32);
    let tf = (t % 10_000.0) as f32;
    // the flame's height fills the frame; its width keeps the proportions
    let h = rows * aspect;
    let w = (h * 1.25).min(cols);
    let core = inks.text.mix(inks.warm, 0.25);
    let body = inks.warm;
    let edge = inks.warm.mix(inks.hot, 0.6);
    let ember = inks.hot;
    // it catches over the first moments
    let catch = smoothstep(0.0, 0.9, tf);
    for r in 0..f.rows {
        for c in 0..f.cols {
            // x across the flame (-1..1), y up from its base (0..1)
            let x = (c as f32 + 0.5 - cols / 2.0) / (w / 2.0);
            let y = 1.0 - ((r as f32 + 0.5) * aspect) / h;
            let n1 = fbm2(x * 1.8, y * 2.0 - tf * 1.7, 3);
            let n2 = fbm2(x * 4.2 + 3.1, y * 4.6 - tf * 3.1, 3);
            // licks of flame lean and sway higher up
            let xw = x + (n1 - 0.5) * 0.9 * y + (tf * 1.3 + y * 3.0).sin() * 0.05 * y;
            let width = 0.82 * (1.0 - y).max(0.0).powf(0.6) * (0.85 + 0.3 * n2) * (0.5 + 0.5 * catch);
            if width <= 0.0 {
                continue;
            }
            let across = (1.0 - xw.abs() / width).max(0.0);
            let heat = across.powf(0.7) * (1.0 - y).max(0.0).powf(0.25) * (0.7 + 0.6 * n2) - y * (0.55 - 0.25 * catch) + (n1 - 0.5) * 0.3;
            let heat = (heat * (0.4 + 0.6 * catch)).clamp(0.0, 1.0);
            if heat < 0.08 {
                continue;
            }
            let color = if heat > 0.78 {
                body.mix(core, smoothstep(0.78, 0.95, heat))
            } else if heat > 0.5 {
                edge.mix(body, smoothstep(0.5, 0.78, heat))
            } else {
                ember.mix(edge, smoothstep(0.2, 0.5, heat))
            };
            let color = inks.bg.mix(color, 0.35 + 0.65 * smoothstep(0.08, 0.6, heat));
            f.cells[r * f.cols + c] = Cell::of(0.08 + 0.92 * heat, color);
        }
    }
    // sparks rising off it
    for i in 0..16 {
        let s = hash3(i, 3, 5);
        let s2 = hash3(i, 9, 1);
        let life = (tf * (0.35 + 0.4 * s) + s2 * 7.0).fract();
        let x = (s - 0.5) * 0.9 + (tf * 2.0 + s2 * 9.0).sin() * 0.12 * life;
        let y = 0.25 + life * 0.85;
        let c = (x * w / 2.0 + cols / 2.0 - 0.5).round();
        let r = ((1.0 - y) * h / aspect - 0.5).round();
        if c < 0.0 || r < 0.0 || c >= cols || r >= rows || catch < 0.5 {
            continue;
        }
        let idx = r as usize * f.cols + c as usize;
        if f.cells[idx].ink > 0.3 {
            continue;
        }
        let fade = 1.0 - life;
        let ch = if fade > 0.7 { b'*' } else if fade > 0.35 { b'+' } else { b'.' };
        f.cells[idx] = Cell { ch, color: inks.bg.mix(body.mix(ember, life), 0.3 + 0.7 * fade), ink: 0.2 + 0.4 * fade };
    }
}

// ------------------------------------------------------------------ petals turning

fn bloom(f: &mut Frame, aspect: f32, t: f64, inks: &Inks) {
    let (cols, rows) = (f.cols as f32, f.rows as f32);
    let tf = (t % 10_000.0) as f32;
    let u = (cols / 2.0).min(rows * aspect / 2.0);
    let outer = inks.accent.mix(inks.shine, 0.45);
    let inner = inks.shine.mix(inks.text, 0.45);
    let spark = inks.warm.mix(inks.text, 0.2);
    let breathe = 1.0 + 0.05 * (tf * 2.0).sin();
    let tau = std::f32::consts::TAU;
    for r in 0..f.rows {
        for c in 0..f.cols {
            let x = (c as f32 + 0.5 - cols / 2.0) / u;
            let y = ((r as f32 + 0.5) * aspect - rows * aspect / 2.0) / u;
            let rad = (x * x + y * y).sqrt() / breathe;
            let th = y.atan2(x);
            let mut best = Cell::EMPTY;
            // six long petals turning one way, open sky between them
            let a = 0.5 + 0.5 * (6.0 * (th - tf * 0.7)).cos();
            let reach = 0.22 + 0.7 * a.powf(2.2);
            if rad < reach && rad > 0.1 {
                let along = rad / reach;
                let k = (1.0 - along).powf(0.5) * (0.35 + 0.65 * a);
                let color = inks.bg.mix(outer.mix(inner, 1.0 - along), 0.35 + 0.65 * k);
                best = Cell::of(0.12 + 0.75 * k, color);
            }
            // six short ones between them, turning the other way
            let b = 0.5 + 0.5 * (6.0 * (th + tf * 1.1) + tau / 2.0).cos();
            let reach_b = 0.12 + 0.34 * b.powf(2.0);
            if rad < reach_b {
                let k = (1.0 - rad / reach_b).powf(0.5) * (0.4 + 0.6 * b);
                if 0.2 + 0.7 * k > best.ink {
                    best = Cell::of(0.2 + 0.7 * k, inks.bg.mix(inner, 0.45 + 0.55 * k));
                }
            }
            if rad < 0.08 {
                best = Cell::of(1.0, inks.text);
            }
            // three sparks chasing round the rim, tails behind them
            if (0.86..1.0).contains(&rad) {
                for i in 0..3 {
                    let head = tf * 2.1 + i as f32 * tau / 3.0;
                    let behind = (head - th).rem_euclid(tau);
                    if behind < 1.4 {
                        let k = (1.0 - behind / 1.4).powi(2) * (1.0 - ((rad - 0.93) / 0.07).abs());
                        if k > 0.06 {
                            let ch = if behind < 0.12 { b'*' } else if k > 0.45 { b'~' } else { b'.' };
                            best = Cell { ch, color: inks.bg.mix(spark, 0.3 + 0.7 * k), ink: 0.1 + 0.8 * k };
                        }
                    }
                }
            }
            f.cells[r * f.cols + c] = best;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inks(light: bool) -> Inks {
        if light {
            Inks { accent: Rgb::hex(0x1e66f5), shine: Rgb::hex(0x04a5e5), good: Rgb::hex(0x40a02b), warm: Rgb::hex(0xdf8e1d), hot: Rgb::hex(0xd20f39), text: Rgb::hex(0x4c4f69), bg: Rgb::hex(0xeff1f5) }
        } else {
            Inks { accent: Rgb::hex(0xd9dde4), shine: Rgb::hex(0x9fb4cc), good: Rgb::hex(0x8fcf9c), warm: Rgb::hex(0xe3c27d), hot: Rgb::hex(0xef7b7b), text: Rgb::hex(0xe6e6e6), bg: Rgb::hex(0x101114) }
        }
    }

    fn inked(f: &Frame) -> usize {
        f.cells.iter().filter(|c| c.ch != b' ').count()
    }

    #[test]
    fn every_scene_fills_its_grid_and_stays_inside_the_ramp() {
        for scene in [Scene::World, Scene::Kindle, Scene::Bloom] {
            for (cols, rows) in [(1, 1), (7, 3), (90, 24), (200, 9)] {
                let f = render(scene, cols, rows, 2.0, 3.7, &inks(false));
                assert_eq!(f.cells.len(), cols * rows, "{scene:?} {cols}x{rows}");
                for c in &f.cells {
                    assert!(c.ch.is_ascii_graphic() || c.ch == b' ', "{scene:?}: {:?}", c.ch as char);
                    assert!((0.0..=1.0).contains(&c.ink), "{scene:?}: ink {}", c.ink);
                }
            }
            assert!(render(scene, 0, 5, 2.0, 0.0, &inks(false)).cells.is_empty());
        }
    }

    #[test]
    fn the_world_is_a_lit_planet_in_the_middle_with_its_ring_and_moons() {
        let (rows, aspect) = (24, 2.0);
        let cols = cols_for(Scene::World, rows, aspect);
        let f = render(Scene::World, cols, rows, aspect, 12.0, &inks(false));
        // the planet's lit (upper left) side is dense, its night side sparse
        let lit = f.at(cols / 2 - 3, rows / 2 - 2);
        let night = f.at(cols / 2 + 18, rows / 2);
        assert!(lit.ink > night.ink, "lit {lit:?} night {night:?}\n{}", f.text());
        // the ring reaches past the planet, either side of it
        let row = rows / 2;
        let line: String = (0..cols).map(|c| f.at(c, row).ch as char).collect();
        let planet_w = (2.0 * PLANET * (cols as f32 / WORLD_W).min(rows as f32 * aspect / WORLD_H)) as usize;
        assert!(line.trim().len() > planet_w + 6, "ring wider than the planet: {line:?}\n{}", f.text());
        // the moons are drawn in the agents' colours
        let claude = moons(&inks(false))[0];
        assert!(f.cells.iter().any(|c| c.ink > 0.4 && (c.color.0 - claude.0).abs() < 0.12 && (c.color.2 - claude.2).abs() < 0.12), "Claude's moon\n{}", f.text());
        // mostly sky
        assert!(inked(&f) < cols * rows / 2, "{}", f.text());
    }

    #[test]
    fn scenes_move() {
        for scene in [Scene::World, Scene::Kindle, Scene::Bloom] {
            let a = render(scene, 60, 16, 2.0, 2.0, &inks(false));
            let b = render(scene, 60, 16, 2.0, 2.6, &inks(false));
            let changed = a.cells.iter().zip(&b.cells).filter(|(x, y)| x.ch != y.ch).count();
            assert!(changed > 20, "{scene:?} barely moved: {changed}");
            // and the same moment draws the same
            assert_eq!(render(scene, 60, 16, 2.0, 2.0, &inks(false)).text(), a.text());
        }
    }

    #[test]
    fn a_fire_burns_up_from_its_base() {
        let f = render(Scene::Kindle, 40, 16, 2.0, 5.0, &inks(false));
        let bottom: usize = (12..16).map(|r| (0..40).filter(|&c| f.at(c, r).ink > 0.3).count()).sum();
        let top: usize = (0..4).map(|r| (0..40).filter(|&c| f.at(c, r).ink > 0.3).count()).sum();
        assert!(bottom > top * 2 && bottom > 10, "bottom {bottom} top {top}\n{}", f.text());
        // it starts small and catches
        let early = render(Scene::Kindle, 40, 16, 2.0, 0.05, &inks(false));
        assert!(inked(&early) < inked(&f), "{}\n--\n{}", early.text(), f.text());
    }

    #[test]
    fn the_bloom_is_round_and_centred() {
        let f = render(Scene::Bloom, 32, 16, 2.0, 1.0, &inks(false));
        assert_eq!(f.at(16, 8).ch, b'$', "a bright centre\n{}", f.text());
        assert_eq!(f.at(0, 0).ch, b' ', "{}", f.text());
        assert_eq!(f.at(31, 15).ch, b' ', "{}", f.text());
    }

    #[test]
    fn colours_sit_between_the_background_and_the_ink() {
        // on a light theme dim ink fades towards the light background, never towards black
        let light = inks(true);
        let f = render(Scene::World, 80, 20, 2.0, 4.0, &light);
        for c in f.cells.iter().filter(|c| c.ch != b' ') {
            assert!(c.color.luma() > 0.2, "{c:?}");
        }
    }
}
