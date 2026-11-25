//! Groups: a stretch of what was just drawn, moved, scaled and faded as one thing after the fact, and ghosts, a
//! copy of that stretch kept so it can still be drawn for a few frames after whatever drew it is gone.
//!
//! Immediate mode draws a dialog or a page in one go and doesn't know how to arrive; so the caller takes a
//! [`Mark`] (`ui.group_start()`) before drawing, then asks for [`Ui::transform_since`] with how far along the arrival is. The scene's
//! instances drawn since the mark (and the hit rectangles registered since) are rewritten in place: scaled about a
//! point, shifted, their alpha multiplied. A dialog lifts and scales in on a spring, a page cross-fades with an
//! offset, a toast slides, all without the drawing code knowing.
//!
//! A ghost is the same stretch copied each frame it is drawn ([`Ui::ghost_keep`]); the frame after its thing
//! stops being drawn, [`Ui::ghost_leave`] plays the copy out fading, so a dialog closes instead of vanishing and a
//! toast that times out slides away while the ones under it glide up. Ghosts draw no hits: nothing under them
//! waits for them.

use super::{Ui, id};
use crate::app::gui::kit::motion::Spring;
use bro_gfx::scene::{GlyphInstance, ImageInstance, Layer, QuadInstance, ShadowInstance};
use bro_gfx::{Point, Rect};
use std::time::Instant;

/// Where in the scene a group starts: the layer drawn into and how much of it was drawn, the layers there were,
/// and how many hit rectangles were registered.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    layer: usize,
    used: usize,
    counts: [usize; 9],
    hits: usize,
}

/// A scale about a point, a shift and an alpha, in physical pixels.
#[derive(Clone, Copy)]
struct Xf {
    cx: f32,
    cy: f32,
    s: f32,
    dx: f32,
    dy: f32,
    a: f32,
}

impl Xf {
    fn rect(&self, r: [f32; 4]) -> [f32; 4] {
        [self.cx + (r[0] - self.cx) * self.s + self.dx, self.cy + (r[1] - self.cy) * self.s + self.dy, r[2] * self.s, r[3] * self.s]
    }
    /// A clip: left alone when it's the whole window's (nothing to follow).
    fn clip(&self, r: [f32; 4]) -> [f32; 4] {
        if r[2] > 1.0e6 { r } else { self.rect(r) }
    }
    fn radii(&self, r: [f32; 4]) -> [f32; 4] {
        r.map(|v| v * self.s)
    }
    fn alpha(&self, mut c: [f32; 4]) -> [f32; 4] {
        c[3] *= self.a;
        c
    }
    fn quad(&self, q: &mut QuadInstance) {
        q.rect = self.rect(q.rect);
        q.radii = self.radii(q.radii);
        q.fill = self.alpha(q.fill);
        q.fill2 = self.alpha(q.fill2);
        q.border = self.alpha(q.border);
        q.clip = self.clip(q.clip);
        q.params[0] *= self.s;
        q.params[2] *= self.s;
    }
    fn shadow(&self, q: &mut ShadowInstance) {
        q.rect = self.rect(q.rect);
        q.radii = self.radii(q.radii);
        q.color = self.alpha(q.color);
        q.clip = self.clip(q.clip);
        q.params[0] *= self.s;
        q.params[1] *= self.s;
    }
    fn glyph(&self, g: &mut GlyphInstance) {
        g.rect = self.rect(g.rect);
        g.color = self.alpha(g.color);
        g.clip = self.clip(g.clip);
        g.params[1] *= self.s;
    }
    fn image(&self, i: &mut ImageInstance) {
        i.rect = self.rect(i.rect);
        i.clip = self.clip(i.clip);
        i.params[0] *= self.s;
        i.params[1] *= self.a;
        i.params[2] *= self.s;
    }
    fn layer(&self, l: &mut Layer, from: [usize; 9]) {
        // a cut fades with the page over it: it wipes less of what's beneath
        l.cut[from[8]..].iter_mut().for_each(|q| self.quad(q));
        l.glass[from[0]..].iter_mut().for_each(|q| self.quad(q));
        l.shadows[from[1]..].iter_mut().for_each(|q| self.shadow(q));
        l.quads[from[2]..].iter_mut().for_each(|q| self.quad(q));
        l.images[from[3]..].iter_mut().for_each(|(_, i)| self.image(i));
        l.mid[from[4]..].iter_mut().for_each(|q| self.quad(q));
        l.glyphs[from[5]..].iter_mut().for_each(|g| self.glyph(g));
        l.top[from[6]..].iter_mut().for_each(|q| self.quad(q));
        l.top_glass[from[7]..].iter_mut().for_each(|q| self.quad(q));
    }
}

fn counts(l: &Layer) -> [usize; 9] {
    [l.glass.len(), l.shadows.len(), l.quads.len(), l.images.len(), l.mid.len(), l.glyphs.len(), l.top.len(), l.top_glass.len(), l.cut.len()]
}

/// A copy of what `l` holds from `from` on.
fn copy_layer(l: &Layer, from: [usize; 9]) -> Layer {
    Layer {
        cut: l.cut[from[8]..].to_vec(),
        glass: l.glass[from[0]..].to_vec(),
        shadows: l.shadows[from[1]..].to_vec(),
        quads: l.quads[from[2]..].to_vec(),
        images: l.images[from[3]..].to_vec(),
        mid: l.mid[from[4]..].to_vec(),
        glyphs: l.glyphs[from[5]..].to_vec(),
        top: l.top[from[6]..].to_vec(),
        top_glass: l.top_glass[from[7]..].to_vec(),
    }
}

/// What a ghost keeps: the copied layers, when it was last drawn for real and when it began to leave.
pub(super) struct Ghost {
    layers: Vec<Layer>,
    seen: Instant,
    left: Option<Instant>,
}

/// How long a ghost takes to go (seconds).
pub const LEAVE: f32 = 0.16;

impl Ui<'_> {
    /// The start of a group: everything drawn from here is what [`Ui::transform_since`] moves.
    pub fn group_start(&self) -> Mark {
        let layer = self.scene.layer_index();
        Mark { layer, used: self.scene.used_layers().len(), counts: counts(&self.scene.layers[layer]), hits: self.hits.len() }
    }

    /// Scale what was drawn since `from` by `scale` about `about`, shift it by `by` and multiply its alpha by
    /// `alpha` (all logical px). Its hit rectangles follow, so a click lands on what's drawn. Does nothing at
    /// rest (scale 1, no shift, alpha 1).
    pub fn transform_since(&mut self, from: Mark, about: Point, scale: f32, by: Point, alpha: f32) {
        if scale == 1.0 && by.x == 0.0 && by.y == 0.0 && alpha >= 1.0 {
            return;
        }
        let k = self.scale;
        let xf = Xf { cx: about.x * k, cy: about.y * k, s: scale, dx: by.x * k, dy: by.y * k, a: alpha.clamp(0.0, 1.0) };
        let used = self.scene.used_layers().len();
        for i in from.layer..used {
            if i != from.layer && i < from.used {
                continue;
            }
            let start = if i == from.layer { from.counts } else { [0; 9] };
            xf.layer(&mut self.scene.layers[i], start);
        }
        let (cx, cy) = (about.x, about.y);
        let first = from.hits.min(self.hits.len());
        for h in &mut self.hits[first..] {
            let r = h.rect;
            h.rect = Rect::new(cx + (r.x - cx) * scale + by.x, cy + (r.y - cy) * scale + by.y, r.w * scale, r.h * scale);
        }
    }

    /// Keep a copy of what was drawn since `from`, under `key`: call it every frame the thing is on screen, so
    /// [`Ui::ghost_leave`] can play it out once it isn't.
    pub fn ghost_keep(&mut self, key: impl std::hash::Hash, from: Mark) {
        let used = self.scene.used_layers().len();
        let mut layers = vec![];
        for i in from.layer..used {
            if i != from.layer && i < from.used {
                continue;
            }
            let start = if i == from.layer { from.counts } else { [0; 9] };
            layers.push(copy_layer(&self.scene.layers[i], start));
        }
        self.anim.motion.ghosts.insert(id(("ghost", key)), Ghost { layers, seen: self.now, left: None });
    }

    /// Draw `key`'s ghost, called each frame it is gone: it fades over [`LEAVE`] seconds while sliding by `slide`
    /// (logical px; (0, 0) stays put) and scaling down to `shrink` (1 stays full size). True while it's still
    /// showing; false when it's done, or if nothing was kept.
    pub fn ghost_leave(&mut self, key: impl std::hash::Hash, slide: Point, shrink: f32, about: Point) -> bool {
        self.ghost_leave_for(key, LEAVE, slide, shrink, about)
    }

    /// [`Ui::ghost_leave`] over `secs` seconds.
    pub fn ghost_leave_for(&mut self, key: impl std::hash::Hash, secs: f32, slide: Point, shrink: f32, about: Point) -> bool {
        let now = self.now;
        let k = id(("ghost", key));
        let Some(g) = self.anim.motion.ghosts.get_mut(&k) else { return false };
        let left = *g.left.get_or_insert(now);
        g.seen = now;
        let t = now.saturating_duration_since(left).as_secs_f32() / secs.max(0.01);
        if t >= 1.0 {
            self.anim.motion.ghosts.remove(&k);
            return false;
        }
        self.anim.moving = true;
        // ease-in: it lingers a moment, then goes
        let p = t * t * (3.0 - 2.0 * t);
        let (scale, by) = (1.0 + (shrink - 1.0) * p, Point::new(slide.x * p, slide.y * p));
        let layers: Vec<Layer> = self.anim.motion.ghosts[&k].layers.iter().map(|l| copy_layer(l, [0; 9])).collect();
        let kk = self.scale;
        let xf = Xf { cx: about.x * kk, cy: about.y * kk, s: scale, dx: by.x * kk, dy: by.y * kk, a: (1.0 - p).clamp(0.0, 1.0) };
        let prev = self.layer();
        for mut l in layers {
            xf.layer(&mut l, [0; 9]);
            self.scene.push_layer();
            let at = self.scene.layer_index();
            self.scene.layers[at] = l;
        }
        self.set_layer(prev);
        true
    }

    /// Forget `key`'s ghost (what it copied is no longer what should play out).
    pub fn ghost_drop(&mut self, key: impl std::hash::Hash) {
        self.anim.motion.ghosts.remove(&id(("ghost", key)));
    }

    /// Whether `key` has a ghost still playing out.
    pub fn ghost_alive(&self, key: impl std::hash::Hash) -> bool {
        self.anim.motion.ghosts.get(&id(("ghost", key))).is_some_and(|g| g.left.is_some())
    }

    /// Draw `f`, a page of a view, and when `ident` (which page it is: the section, the tab) differs from last
    /// frame, change over: the page that was there slides up a little and fades out as the new one rises into place
    /// and fades in (~250 ms, on springs that keep their velocity if you switch again). The first time it is drawn
    /// it just appears. Draw the page's own contents in `f` and what stays put (the nav, the title bar) outside.
    pub fn page<R>(&mut self, key: impl std::hash::Hash, ident: impl std::hash::Hash, f: impl FnOnce(&mut Ui) -> R) -> R {
        let (k, ident) = (id(("page", key)), id(ident));
        let changed = self.anim.motion.pages.insert(k, ident).is_some_and(|was| was != ident);
        if changed {
            self.spring_set(("page-in", k), 0.0);
        }
        let from = self.group_start();
        let out = f(self);
        let p = self.spring(("page-in", k), 1.0, Spring::new(0.24, 0.0), 1.0);
        self.transform_since(from, Point::new(0.0, 0.0), 1.0, Point::new(0.0, (1.0 - p) * 10.0), p);
        if !((changed || self.ghost_alive(("page", k))) && self.ghost_leave(("page", k), Point::new(0.0, -8.0), 1.0, Point::new(0.0, 0.0))) {
            self.ghost_keep(("page", k), from);
        }
        out
    }
}

impl Ghost {
    pub(super) fn stale(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.seen).as_secs() >= 30
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transform_scales_about_its_point_and_fades() {
        let xf = Xf { cx: 100.0, cy: 100.0, s: 0.5, dx: 0.0, dy: 10.0, a: 0.5 };
        // the fixed point stays (bar the shift); everything else closes in on it
        assert_eq!(xf.rect([100.0, 100.0, 0.0, 0.0]), [100.0, 110.0, 0.0, 0.0]);
        assert_eq!(xf.rect([0.0, 0.0, 40.0, 20.0]), [50.0, 60.0, 20.0, 10.0]);
        assert_eq!(xf.alpha([1.0, 0.5, 0.25, 0.8])[3], 0.4);
        // a clip over the whole window stays that
        assert_eq!(xf.clip(bro_gfx::Rect::EVERYTHING.to_array()), bro_gfx::Rect::EVERYTHING.to_array());
    }
}
