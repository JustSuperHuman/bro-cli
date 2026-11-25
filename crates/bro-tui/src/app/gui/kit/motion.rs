//! Motion after Rune's (github.com/anomalyco/rune, `rune-motion` and `rune-kit`): springs tuned the way people
//! tune them, by a duration and a bounce (SwiftUI's `Spring(duration:bounce:)`, as Rune maps it), and the moves
//! bro takes from Rune's examples:
//!
//! - a value springing to its target, keeping its velocity when the target moves (`todo`'s progress bar);
//! - a highlight that glides from where it was to where it is now (`matched`'s shared element), riding along
//!   with its row while the list scrolls;
//! - labels whose changed characters roll like an odometer, up as the number rises and down as it falls, the
//!   rightmost first (`odometer`, `RollingText`);
//! - an aura that flows round a composer while its agent works and flares as it starts, with a shimmer over the
//!   line that says what it's doing (the showreel's `ask`).

use super::{Style, Ui, id};
use bro_gfx::{Color, Point, Rect};
use std::collections::HashMap;
use std::hash::Hash;
use std::time::Instant;

/// A spring with mass 1, described by its perceptual duration (seconds, the undamped period) and its bounce
/// (0 settles without overshoot, more overshoots).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub duration: f32,
    pub bounce: f32,
}

impl Spring {
    pub const fn new(duration: f32, bounce: f32) -> Spring {
        Spring { duration, bounce }
    }
    /// Rune's `GLIDE`: things moving to a new place.
    pub const GLIDE: Spring = Spring::new(0.32, 0.0);
    /// Rune's `RollingText::SPRING`: quick, with a hint of overshoot.
    pub const ROLL: Spring = Spring::new(0.36, 0.12);
    /// The springy bar of Rune's `todo`.
    pub const BAR: Spring = Spring::new(0.5, 0.2);
    /// A press going down and springing back (Rune's showreel pads).
    pub const PRESS: Spring = Spring::new(0.24, 0.32);
    /// Something arriving with a little overshoot: dialogs, toasts, a switch's knob.
    pub const ARRIVE: Spring = Spring::new(0.4, 0.2);
    /// A pop: a check landing, a badge appearing.
    pub const POP: Spring = Spring::new(0.42, 0.45);

    fn omega(self) -> f32 {
        std::f32::consts::TAU / self.duration.max(0.01)
    }
    fn zeta(self) -> f32 {
        (1.0 - self.bounce).clamp(0.05, 1.0)
    }

    /// A unit step's position `t` seconds in (0 → 1 from rest; past 1 while it bounces).
    pub fn step(self, t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        let (w, z) = (self.omega(), self.zeta());
        if z >= 1.0 {
            return 1.0 - (-w * t).exp() * (1.0 + w * t);
        }
        let wd = w * (1.0 - z * z).sqrt();
        1.0 - (-z * w * t).exp() * ((wd * t).cos() + z * w / wd * (wd * t).sin())
    }

    /// Seconds until a step stays within half a percent of its target.
    pub fn settle(self) -> f32 {
        let zw = self.zeta() * self.omega();
        // the critically damped tail carries a (1 + ωt) factor: a little longer
        (5.3 / zw * if self.zeta() >= 1.0 { 1.4 } else { 1.0 }).max(self.duration * 0.5)
    }

    /// Move `x` (with velocity `v`) toward `target` over `dt` seconds. True once it has come to rest there.
    fn advance(self, x: &mut f32, v: &mut f32, target: f32, dt: f32, eps: f32) -> bool {
        let w = self.omega();
        let (k, c) = (w * w, 2.0 * self.zeta() * w);
        let mut left = dt.clamp(0.0, 0.1);
        while left > 0.0 {
            let h = left.min(1.0 / 240.0);
            *v += (-k * (*x - target) - c * *v) * h;
            *x += *v * h;
            left -= h;
        }
        if (*x - target).abs() < eps && v.abs() < eps * 10.0 {
            *x = target;
            *v = 0.0;
            true
        } else {
            false
        }
    }
}

/// A sprung value between frames.
struct Sprung {
    x: f32,
    v: f32,
    at: Instant,
    /// it had come to rest: the next move starts from now, not from when it was last drawn
    rest: bool,
    /// the frame it last moved in
    stepped: Option<Instant>,
}

/// A highlight gliding to its row: how far it still is from it (x, y, w, h), and how fast that closes.
struct Glide {
    ident: u64,
    drawn: Rect,
    off: [f32; 4],
    vel: [f32; 4],
    at: Instant,
}

/// A rolling label: what it reads, what it read before the change rolling now, and which way it rolls.
struct Roll {
    now: String,
    before: String,
    since: Instant,
    /// 1 rolls up (the number rose), -1 down
    dir: f32,
    value: Option<f64>,
    seen: Instant,
}

/// Springs, glides and rolling labels that persist between frames (part of [`super::Anims`]).
#[derive(Default)]
pub struct Motion {
    springs: HashMap<u64, Sprung>,
    glides: HashMap<u64, Glide>,
    rolls: HashMap<u64, Roll>,
    /// whether each thing watched is on, since when it turned on (None: on since it was first seen), last seen
    onsets: HashMap<u64, (bool, Option<Instant>, Instant)>,
    /// copies of things that were drawn, kept to play out when they go ([`super::group`])
    pub(super) ghosts: HashMap<u64, super::group::Ghost>,
    /// what each [`super::Ui::page`] showed last frame
    pub(super) pages: HashMap<u64, u64>,
    /// the frame being drawn and the one before it (the window only draws when something changes, so frames can be
    /// far apart: "drawn last frame" is what says a highlight was on screen)
    frame: Option<Instant>,
    last_frame: Option<Instant>,
}

impl Motion {
    fn tick(&mut self, now: Instant) {
        if self.frame != Some(now) {
            self.last_frame = self.frame;
            self.frame = Some(now);
        }
    }
    /// Drawn in the frame before this one (or earlier in this one).
    fn on_screen(&self, at: Instant) -> bool {
        self.last_frame.is_some_and(|f| at >= f)
    }
}

impl Motion {
    /// Forget what hasn't been drawn for a while.
    pub fn sweep(&mut self, now: Instant) {
        let fresh = |at: Instant| now.saturating_duration_since(at).as_secs() < 30;
        self.springs.retain(|_, s| fresh(s.at));
        self.glides.retain(|_, g| fresh(g.at));
        self.rolls.retain(|_, r| fresh(r.seen));
        self.onsets.retain(|_, o| fresh(o.2));
        self.ghosts.retain(|_, g| !g.stale(now));
    }
}

impl Ui<'_> {
    /// `target`, reached on `spring` (from `from` the first time it's asked for): moving targets keep the
    /// velocity they had.
    pub fn spring(&mut self, key: impl Hash, target: f32, spring: Spring, from: f32) -> f32 {
        let now = self.now;
        let s = self.anim.motion.springs.entry(id(("spring", key))).or_insert(Sprung { x: from, v: 0.0, at: now, rest: false, stepped: None });
        // asked again in the same frame (a second widget under one key): it moved for the first, which keeps it.
        // Moving it toward each in turn would keep it from ever resting, and the window drawing flat out.
        if s.stepped == Some(now) {
            if !s.rest {
                self.anim.moving = true;
            }
            return s.x;
        }
        s.stepped = Some(now);
        let mut dt = now.saturating_duration_since(s.at).as_secs_f32();
        if s.rest {
            dt = dt.min(1.0 / 60.0);
        }
        s.at = now;
        s.rest = spring.advance(&mut s.x, &mut s.v, target, dt, 1e-3);
        if !s.rest {
            self.anim.moving = true;
        }
        s.x
    }

    /// Start the spring for `key` over from `x`, at rest.
    pub fn spring_set(&mut self, key: impl Hash, x: f32) {
        let now = self.now;
        self.anim.motion.springs.insert(id(("spring", key)), Sprung { x, v: 0.0, at: now, rest: false, stepped: None });
    }

    /// Seconds since `on` turned true for `key`, while it stays true; None while it's false, and for something
    /// already on when it was first seen (nothing to celebrate at start-up).
    pub fn since_on(&mut self, key: impl Hash, on: bool) -> Option<f32> {
        let now = self.now;
        let o = self.anim.motion.onsets.entry(id(("onset", key))).or_insert((on, None, now));
        if on && !o.0 {
            o.1 = Some(now);
        }
        if !on {
            o.1 = None;
        }
        o.0 = on;
        o.2 = now;
        o.1.map(|at| now.saturating_duration_since(at).as_secs_f32())
    }

    /// A finish being celebrated `s` seconds in, round the mark at `at` in `r`'s row: the mark's size pops on a
    /// bouncy spring (returned, 0..~1.1), a ring bursts out of it and a band of `ink` sweeps across the row.
    pub fn celebrate(&mut self, r: Rect, at: Point, s: f32, ink: Color) -> f32 {
        if s > 1.6 {
            return 1.0;
        }
        self.anim.moving = true;
        let p = (s / 0.7).min(1.0);
        self.ring(at, 4.0 + 13.0 * p, 1.6 * (1.0 - p) + 0.4, ink.fade(0.6 * (1.0 - p)));
        let k = (s / 1.0).min(1.0);
        let sweep = k * k * (3.0 - 2.0 * k);
        let cx = r.x - 50.0 + (r.w + 100.0) * sweep;
        self.clip(r, super::radius::CONTROL);
        for (w, a) in [(110.0, 0.05), (70.0, 0.05), (34.0, 0.06)] {
            self.round(Rect::new(cx - w / 2.0, r.y, w, r.h), w / 2.0, ink.fade(a * (1.0 - k * 0.6)));
        }
        self.unclip();
        Spring::POP.step(s)
    }

    /// Where a highlight is drawn: `r`, its row's place now, once it has glided there. When it moves to another row
    /// (`ident` changes) it springs from where it was; while it stays on one it rides along with it, so a list
    /// that scrolls doesn't leave it behind. One that wasn't drawn a moment ago appears in place.
    pub fn glide(&mut self, key: impl Hash, ident: impl Hash, r: Rect, spring: Spring) -> Rect {
        let (now, ident) = (self.now, id(ident));
        self.anim.motion.tick(now);
        let k = id(("glide", key));
        let shown = self.anim.motion.glides.get(&k).is_some_and(|g| self.anim.motion.on_screen(g.at));
        let g = self.anim.motion.glides.entry(k).or_insert(Glide { ident, drawn: r, off: [0.0; 4], vel: [0.0; 4], at: now });
        let mut dt = now.saturating_duration_since(g.at).as_secs_f32();
        // two highlights under one key this frame: the first keeps it, the other stays put
        if g.at == now && g.ident != ident {
            return r;
        }
        // at rest since it was last drawn: the move starts now
        if g.off == [0.0; 4] {
            dt = dt.min(1.0 / 60.0);
        }
        if g.ident != ident {
            if shown {
                // from where it was drawn, toward its new row
                g.off = [g.drawn.x - r.x, g.drawn.y - r.y, g.drawn.w - r.w, g.drawn.h - r.h];
            } else {
                g.off = [0.0; 4];
                g.vel = [0.0; 4];
            }
            g.ident = ident;
        }
        g.at = now;
        let mut rest = true;
        for i in 0..4 {
            rest &= spring.advance(&mut g.off[i], &mut g.vel[i], 0.0, dt, 0.2);
        }
        if !rest {
            self.anim.moving = true;
        }
        g.drawn = Rect::new(r.x + g.off[0], r.y + g.off[1], (r.w + g.off[2]).max(0.0), (r.h + g.off[3]).max(0.0));
        g.drawn
    }

    /// Draw `text` with its right edge at `right` and its top at `y`, rolling the characters that changed since it
    /// last read differently: up as `value` rises, down as it falls (up without a value), the rightmost first and
    /// each one to its left a beat later. Returns its width.
    pub fn rolling(&mut self, key: impl Hash, right: f32, y: f32, text: &str, st: Style, value: Option<f64>) -> f32 {
        let now = self.now;
        let k = id(("roll", key));
        // a second label under one key this frame, reading differently: it shows as it is and the first keeps the
        // roll (rolling between the two every frame would never land, and the window would draw flat out)
        if self.anim.motion.rolls.get(&k).is_some_and(|r| r.seen == now && r.now != text) {
            let w = self.measure(text, st);
            self.label(Point::new(right - w, y), text, st);
            return w;
        }
        let roll = self.anim.motion.rolls.entry(k).or_insert_with(|| Roll { now: text.to_string(), before: text.to_string(), since: now, dir: 1.0, value, seen: now });
        roll.seen = now;
        if roll.now != text {
            roll.dir = match (roll.value, value) {
                (Some(a), Some(b)) if b < a => -1.0,
                _ => 1.0,
            };
            roll.before = std::mem::replace(&mut roll.now, text.to_string());
            roll.since = now;
            roll.value = value;
        }
        let (before, dir, t) = (roll.before.clone(), roll.dir, now.saturating_duration_since(roll.since).as_secs_f32());
        let new: Vec<char> = text.chars().collect();
        let old: Vec<char> = before.chars().collect();
        let nw: Vec<f32> = new.iter().map(|c| self.measure(c.encode_utf8(&mut [0; 4]), st)).collect();
        let ow: Vec<f32> = old.iter().map(|c| self.measure(c.encode_utf8(&mut [0; 4]), st)).collect();
        let total: f32 = nw.iter().sum();
        const STAGGER: f32 = 0.035;
        let spring = Spring::ROLL;
        let changed = before != text;
        if changed && t > spring.settle() + STAGGER * 6.0 {
            // landed: rest until the next change
            if let Some(r) = self.anim.motion.rolls.get_mut(&k) {
                r.before = r.now.clone();
            }
        } else if changed {
            self.anim.moving = true;
        }
        let line_h = self.line("0", st, None).h;
        // a band the characters roll through, a little taller than the line so descents and rises show
        let band = Rect::new(right - total.max(ow.iter().sum::<f32>()) - 2.0, y - 1.0, total.max(ow.iter().sum::<f32>()) + 4.0, line_h + 2.0);
        let travel = line_h * 0.8;
        let (mut xn, mut xo) = (right, right);
        let cols = new.len().max(old.len());
        let mut stagger = 0usize;
        for j in 0..cols {
            let n = new.len().checked_sub(j + 1).map(|i| (new[i], nw[i]));
            let o = old.len().checked_sub(j + 1).map(|i| (old[i], ow[i]));
            if let Some((_, w)) = n {
                xn -= w;
            }
            if let Some((_, w)) = o {
                xo -= w;
            }
            let same = matches!((n, o), (Some((a, _)), Some((b, _))) if a == b);
            if same || !changed {
                if let Some((c, _)) = n {
                    self.label(Point::new(xn, y), c.encode_utf8(&mut [0; 4]), st);
                }
                continue;
            }
            let p = spring.step(t - STAGGER * stagger.min(6) as f32);
            stagger += 1;
            self.clip(band, 0.0);
            if let Some((c, _)) = o {
                let b = self.line(c.encode_utf8(&mut [0; 4]), st, None);
                self.draw_faded(&b, Point::new(xo, y - dir * p * travel), (1.0 - p).clamp(0.0, 1.0));
            }
            if let Some((c, _)) = n {
                let b = self.line(c.encode_utf8(&mut [0; 4]), st, None);
                self.draw_faded(&b, Point::new(xn, y + dir * (1.0 - p) * travel), p.clamp(0.0, 1.0));
            }
            self.unclip();
        }
        total
    }

    /// Where the aura's blooms are along `r`'s edge now (clockwise from the top left), and each one's colour index.
    fn aura_blooms(&self, r: Rect) -> [(Point, usize, f32); 6] {
        let t = (self.time % 10_000.0) as f32;
        let per = 2.0 * (r.w + r.h);
        std::array::from_fn(|i| {
            let s = ((t * 0.075 + i as f32 / 6.0).fract()) * per;
            let p = if s < r.w {
                Point::new(r.x + s, r.y)
            } else if s < r.w + r.h {
                Point::new(r.right(), r.y + s - r.w)
            } else if s < 2.0 * r.w + r.h {
                Point::new(r.right() - (s - r.w - r.h), r.bottom())
            } else {
                Point::new(r.x, r.bottom() - (s - 2.0 * r.w - r.h))
            };
            (p, i % 3, 0.75 + 0.25 * (t * 2.3 + i as f32 * 1.7).sin())
        })
    }

    /// A light flowing round `r`'s edge (Rune's aura glow), drawn under it: soft blooms in three colours chasing
    /// each other along its border over an even halo. `intensity` 0 hides it, 1 is at rest, more flares. Pair it
    /// with [`Ui::aura_rim`] after the surface is drawn.
    pub fn aura(&mut self, r: Rect, radius: f32, colors: [Color; 3], intensity: f32) {
        if intensity <= 0.01 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        self.ticking = true;
        let k = intensity.min(2.6);
        let base = if self.t.light { 0.26 } else { 0.42 };
        let t = (self.time % 10_000.0) as f32;
        // an even halo in the colours' blend, breathing
        let breathe = 0.85 + 0.15 * (t * 1.7).sin();
        self.shadow(r, radius, 14.0 + 8.0 * k, 0.0, colors[0].mix(colors[2], 0.5).fade(base * 0.5 * k * breathe));
        let side = (r.h * 0.9).clamp(64.0, 160.0);
        for (p, c, pulse) in self.aura_blooms(r) {
            let bloom = Rect::new(p.x - side / 2.0, p.y - side / 5.0, side, side / 2.5);
            self.shadow(bloom, side / 5.0, 18.0 + 10.0 * k, 0.0, colors[c].fade(base * k * pulse));
        }
    }

    /// The aura's light on `r`'s border itself, over the surface: the edge lit in the colours' blend, brightest where
    /// each bloom is passing.
    pub fn aura_rim(&mut self, r: Rect, radius: f32, colors: [Color; 3], intensity: f32) {
        if intensity <= 0.01 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let k = intensity.min(2.0);
        let width = 1.4;
        self.outline(r, radius, width, colors[0].mix(colors[2], 0.5).fade(0.22 * k));
        let side = (r.h * 0.9).clamp(64.0, 160.0);
        for (p, c, pulse) in self.aura_blooms(r) {
            // nested windows round the bloom: the edge brightens towards its middle
            for (scale, alpha) in [(1.0, 0.12), (0.78, 0.14), (0.56, 0.17), (0.36, 0.2), (0.18, 0.22)] {
                let w = side * scale;
                self.clip(Rect::new(p.x - w / 2.0, p.y - w / 2.0, w, w), 0.0);
                self.outline(r, radius, width, colors[c].fade(alpha * k * pulse));
                self.unclip();
            }
        }
    }

    /// `text` at `p` with a band of light sweeping across it (Rune's shimmer over a streaming reply), cut to `max_w`.
    pub fn shimmer(&mut self, p: Point, text: &str, st: Style, glint: Color, max_w: Option<f32>) -> super::TextBox {
        self.ticking = true;
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len() as f32;
        const BAND: f32 = 5.0;
        const PERIOD: f32 = 1.9;
        let at = ((self.time % 1000.0) as f32 / PERIOD).fract() * (n + 2.0 * BAND * 2.0) - BAND * 2.0;
        let spans: Vec<(String, Style)> = chars
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let k = (1.0 - (i as f32 - at).abs() / BAND).max(0.0);
                (c.to_string(), st.color(st.color.mix(glint, k * k * (3.0 - 2.0 * k))))
            })
            .collect();
        let b = self.rich(&spans, st.size, max_w, false, Some(1));
        self.draw(&b, p);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_start_still_end_on_target_and_bounce_as_tuned() {
        for s in [Spring::GLIDE, Spring::ROLL, Spring::BAR] {
            assert_eq!(s.step(0.0), 0.0);
            assert!((s.step(s.settle()) - 1.0).abs() < 0.01, "{s:?} settles: {}", s.step(s.settle()));
            assert!((s.step(5.0) - 1.0).abs() < 1e-3);
        }
        let peak = |s: Spring| (1..200).map(|i| s.step(i as f32 * 0.005)).fold(0.0, f32::max);
        assert!(peak(Spring::GLIDE) <= 1.0 + 1e-3, "no bounce, no overshoot");
        // Rune's mapping (ζ = 1 − bounce): a 0.2 bounce overshoots by about 1.5%
        assert!(peak(Spring::BAR) > 1.01, "a bounce overshoots: {}", peak(Spring::BAR));
        assert!(peak(Spring::BAR) > peak(Spring::ROLL));
    }

    #[test]
    fn a_sprung_value_comes_to_rest_and_keeps_its_velocity_when_retargeted() {
        let s = Spring::BAR;
        let (mut x, mut v) = (0.0, 0.0);
        let mut frames = 0;
        while !s.advance(&mut x, &mut v, 1.0, 1.0 / 60.0, 1e-3) {
            frames += 1;
            assert!(frames < 600, "never settles");
        }
        assert_eq!(x, 1.0);
        assert!(frames as f32 / 60.0 < s.settle() * 2.0, "{frames} frames");
        // moving toward 1, retargeted back to 0: it carries on a little before it turns
        let (mut x, mut v) = (0.0, 0.0);
        for _ in 0..6 {
            s.advance(&mut x, &mut v, 1.0, 1.0 / 60.0, 1e-3);
        }
        let before = x;
        s.advance(&mut x, &mut v, 0.0, 1.0 / 60.0, 1e-3);
        assert!(x > before, "momentum: {before} → {x}");
    }

    /// Two usage rows both called "local" once shared their meter's spring and rolling figure: pulled between two
    /// levels every frame, they never came to rest and the window drew at the display's full rate for good.
    #[test]
    fn two_widgets_under_one_key_still_come_to_rest() {
        use std::time::Duration;
        let theme = crate::theme::get("graphite");
        let tokens = super::super::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        let mut scene = bro_gfx::Scene::new();
        let mut anim = super::super::Anims::default();
        let start = Instant::now();
        let mut still = 0;
        for frame in 0..600u32 {
            let mut hits = vec![];
            text.begin_frame();
            anim.moving = false;
            let mut ui = Ui {
                scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0,
                pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim,
                now: start + Duration::from_millis(16 * frame as u64), time: 0.0, ticking: false, screen: Rect::new(0.0, 0.0, 400.0, 300.0),
            };
            let st = super::super::Style::new(13.0, 400, tokens.text);
            for (level, figure) in [(0.91, "91%"), (0.49, "49%")] {
                ui.spring(("usage-meter", "local"), level, Spring::BAR, 0.0);
                ui.rolling(("usage-wk", "local"), 200.0, 10.0, figure, st, Some(level as f64 * 100.0));
            }
            still = if ui.anim.moving { 0 } else { still + 1 };
            if still >= 3 {
                return;
            }
        }
        panic!("still moving after 600 frames");
    }
}
