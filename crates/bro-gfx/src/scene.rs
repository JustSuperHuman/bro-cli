//! A frame's worth of drawing, in physical pixels: layers of shadows, rounded rects, images and glyphs.
//!
//! Inside a layer the passes draw in a fixed order — shadows, quads, images, glyphs, then `top` quads
//! (cursor beams, underlines, scrollbars) — so a panel's background always sits under its text. Anything
//! that must cover text (a menu, a dialog, a toast) goes in a later layer.

use crate::color::Color;
use crate::geom::{Corners, Rect};
use bytemuck::{Pod, Zeroable};

/// A rounded rectangle instance (see `quad.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct QuadInstance {
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub fill: [f32; 4],
    pub fill2: [f32; 4],
    pub border: [f32; 4],
    pub clip: [f32; 4],
    /// border width, gradient (0 none, 1 vertical, 2 horizontal), clip corner radius, dashed border
    pub params: [f32; 4],
}

/// A soft shadow under a rounded rectangle (see `shadow.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ShadowInstance {
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub color: [f32; 4],
    pub clip: [f32; 4],
    /// blur sigma, clip corner radius, -, -
    pub params: [f32; 4],
}

/// A glyph from one of the atlases (see `glyph.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GlyphInstance {
    pub rect: [f32; 4],
    /// normalised atlas coordinates: u0, v0, u1, v1
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub clip: [f32; 4],
    /// kind (0 mask, 1 colour, 2 subpixel), clip corner radius, -, -
    pub params: [f32; 4],
}

/// How a glyph's atlas pixels read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphFormat {
    /// one coverage, drawn in the glyph's colour
    Mask,
    /// colour pixels (emoji)
    Color,
    /// a coverage per colour channel (subpixel text), drawn in the glyph's colour
    Lcd,
}

/// An image quad (see `image.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ImageInstance {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub clip: [f32; 4],
    /// corner radius, opacity, clip corner radius, greyscale (0..1)
    pub params: [f32; 4],
}

/// A texture registered with the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub u64);

/// How a quad is filled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    Solid(Color),
    /// top → bottom
    Vertical(Color, Color),
    /// left → right
    Horizontal(Color, Color),
}

impl From<Color> for Fill {
    fn from(c: Color) -> Fill {
        Fill::Solid(c)
    }
}

impl Fill {
    fn colors(&self) -> ([f32; 4], [f32; 4], f32) {
        match *self {
            Fill::Solid(c) => (c.to_array(), c.to_array(), 0.0),
            Fill::Vertical(a, b) => (a.to_array(), b.to_array(), 1.0),
            Fill::Horizontal(a, b) => (a.to_array(), b.to_array(), 2.0),
        }
    }
    fn invisible(&self) -> bool {
        match *self {
            Fill::Solid(c) => c.a <= 0.0,
            Fill::Vertical(a, b) | Fill::Horizontal(a, b) => a.a <= 0.0 && b.a <= 0.0,
        }
    }
}

/// A rounded rectangle to draw.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub rect: Rect,
    pub corners: Corners,
    pub fill: Fill,
    pub border: Color,
    pub border_width: f32,
    pub dashed: bool,
}

impl Quad {
    pub fn new(rect: Rect, fill: impl Into<Fill>) -> Quad {
        Quad { rect, corners: Corners::ZERO, fill: fill.into(), border: Color::TRANSPARENT, border_width: 0.0, dashed: false }
    }
    pub fn radius(mut self, r: f32) -> Quad {
        self.corners = Corners::all(r);
        self
    }
    pub fn corners(mut self, c: Corners) -> Quad {
        self.corners = c;
        self
    }
    pub fn border(mut self, width: f32, color: Color) -> Quad {
        self.border_width = width;
        self.border = color;
        self
    }
    pub fn dashed(mut self) -> Quad {
        self.dashed = true;
        self
    }
}

/// The current clip: a rectangle with optionally rounded corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub rect: Rect,
    pub radius: f32,
}

impl Clip {
    pub const NONE: Clip = Clip { rect: Rect::EVERYTHING, radius: 0.0 };
}

/// One layer.
#[derive(Default, Debug)]
pub struct Layer {
    pub shadows: Vec<ShadowInstance>,
    pub quads: Vec<QuadInstance>,
    pub images: Vec<(ImageId, ImageInstance)>,
    /// quads over this layer's images, under its text (controls and badges on a picture)
    pub mid: Vec<QuadInstance>,
    pub glyphs: Vec<GlyphInstance>,
    pub top: Vec<QuadInstance>,
}

impl Layer {
    fn clear(&mut self) {
        self.shadows.clear();
        self.quads.clear();
        self.images.clear();
        self.mid.clear();
        self.glyphs.clear();
        self.top.clear();
    }
    pub fn is_empty(&self) -> bool {
        self.shadows.is_empty() && self.quads.is_empty() && self.images.is_empty() && self.mid.is_empty() && self.glyphs.is_empty() && self.top.is_empty()
    }
}

/// Everything to draw this frame.
pub struct Scene {
    pub layers: Vec<Layer>,
    /// layers in use (the rest are kept to reuse their allocations)
    used: usize,
    current: usize,
    clips: Vec<Clip>,
    /// clear colour behind everything
    pub background: Color,
}

impl Default for Scene {
    fn default() -> Self {
        Scene::new()
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene { layers: vec![Layer::default()], used: 1, current: 0, clips: vec![], background: Color::BLACK }
    }

    /// Start a new frame (keeps allocations).
    pub fn clear(&mut self) {
        for l in &mut self.layers {
            l.clear();
        }
        self.used = 1;
        self.current = 0;
        self.clips.clear();
    }

    /// The layers drawn this frame, bottom first.
    pub fn used_layers(&self) -> &[Layer] {
        &self.layers[..self.used]
    }

    /// Draw on top of everything so far from now on. Returns the previous layer index (see [`Scene::set_layer`]).
    pub fn push_layer(&mut self) -> usize {
        let prev = self.current;
        if self.used == self.layers.len() {
            self.layers.push(Layer::default());
        }
        self.current = self.used;
        self.used += 1;
        prev
    }

    /// Go back to drawing into an earlier layer.
    pub fn set_layer(&mut self, i: usize) {
        self.current = i.min(self.used - 1);
    }

    pub fn layer_index(&self) -> usize {
        self.current
    }

    /// Clip everything drawn until the matching [`Scene::pop_clip`] to `rect` (inside the current clip).
    pub fn push_clip(&mut self, rect: Rect, radius: f32) {
        let cur = self.clip();
        let rect = rect.intersect(&cur.rect);
        // the tighter rounded clip wins; a square clip inside a rounded one keeps the rounding
        let radius = if radius > 0.0 { radius } else if rect == cur.rect { cur.radius } else { 0.0 };
        self.clips.push(Clip { rect, radius });
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
    }

    pub fn clip(&self) -> Clip {
        self.clips.last().copied().unwrap_or(Clip::NONE)
    }

    /// Nothing drawn inside `r` would show.
    pub fn clipped_out(&self, r: &Rect) -> bool {
        !self.clip().rect.intersects(r)
    }

    fn quad_instance(&self, q: &Quad) -> Option<QuadInstance> {
        if q.rect.is_empty() || (q.fill.invisible() && (q.border_width <= 0.0 || q.border.a <= 0.0)) || self.clipped_out(&q.rect.outset(1.0)) {
            return None;
        }
        let clip = self.clip();
        let (fill, fill2, gradient) = q.fill.colors();
        // radii can't exceed half the short side
        let max_r = q.rect.w.min(q.rect.h) * 0.5;
        let c = q.corners;
        Some(QuadInstance {
            rect: q.rect.to_array(),
            radii: [c.tl.min(max_r), c.tr.min(max_r), c.br.min(max_r), c.bl.min(max_r)],
            fill,
            fill2,
            border: q.border.to_array(),
            clip: clip.rect.to_array(),
            params: [q.border_width, gradient, clip.radius, if q.dashed { 1.0 } else { 0.0 }],
        })
    }

    /// A rounded rectangle under this layer's text.
    pub fn quad(&mut self, q: Quad) {
        if let Some(i) = self.quad_instance(&q) {
            self.layers[self.current].quads.push(i);
        }
    }

    /// A rounded rectangle over this layer's images but under its text (a bar of controls on a video, a
    /// badge on a thumbnail).
    pub fn mid_quad(&mut self, q: Quad) {
        if let Some(i) = self.quad_instance(&q) {
            self.layers[self.current].mid.push(i);
        }
    }

    /// A rounded rectangle over this layer's text (cursor beams, underlines, scrollbars).
    pub fn top_quad(&mut self, q: Quad) {
        if let Some(i) = self.quad_instance(&q) {
            self.layers[self.current].top.push(i);
        }
    }

    /// A plain filled rectangle (no rounding): cell backgrounds, rules.
    pub fn rect(&mut self, rect: Rect, color: Color) {
        self.quad(Quad::new(rect, color));
    }

    /// A soft shadow cast by a rounded rectangle at `rect` (already offset), `blur` ≈ the CSS blur radius.
    pub fn shadow(&mut self, rect: Rect, corners: Corners, blur: f32, color: Color) {
        if color.a <= 0.0 || rect.is_empty() {
            return;
        }
        let sigma = (blur * 0.5).max(0.5);
        if self.clipped_out(&rect.outset(sigma * 3.0)) {
            return;
        }
        let clip = self.clip();
        let max_r = rect.w.min(rect.h) * 0.5;
        self.layers[self.current].shadows.push(ShadowInstance {
            rect: rect.to_array(),
            radii: [corners.tl.min(max_r), corners.tr.min(max_r), corners.br.min(max_r), corners.bl.min(max_r)],
            color: color.to_array(),
            clip: clip.rect.to_array(),
            params: [sigma, clip.radius, 0.0, 0.0],
        });
    }

    /// A glyph already in an atlas.
    pub fn glyph(&mut self, rect: Rect, uv: [f32; 4], color: Color, colored: bool) {
        self.glyph_as(rect, uv, color, if colored { GlyphFormat::Color } else { GlyphFormat::Mask });
    }

    /// A glyph already in an atlas, its pixels read as `format`.
    pub fn glyph_as(&mut self, rect: Rect, uv: [f32; 4], color: Color, format: GlyphFormat) {
        if self.clipped_out(&rect) {
            return;
        }
        let clip = self.clip();
        self.layers[self.current].glyphs.push(GlyphInstance {
            rect: rect.to_array(),
            uv,
            color: color.to_array(),
            clip: clip.rect.to_array(),
            params: [
                match format {
                    GlyphFormat::Mask => 0.0,
                    GlyphFormat::Color => 1.0,
                    GlyphFormat::Lcd => 2.0,
                },
                clip.radius,
                0.0,
                0.0,
            ],
        });
    }

    /// An image: `uv` picks the part of the texture (0..1), `radius` rounds its corners.
    pub fn image(&mut self, id: ImageId, rect: Rect, uv: [f32; 4], radius: f32, opacity: f32) {
        self.image_ex(id, rect, uv, radius, opacity, 0.0);
    }

    /// [`Scene::image`] with a greyscale amount (0 colour … 1 grey).
    pub fn image_ex(&mut self, id: ImageId, rect: Rect, uv: [f32; 4], radius: f32, opacity: f32, grey: f32) {
        if rect.is_empty() || opacity <= 0.0 || self.clipped_out(&rect) {
            return;
        }
        let clip = self.clip();
        self.layers[self.current].images.push((id, ImageInstance { rect: rect.to_array(), uv, clip: clip.rect.to_array(), params: [radius, opacity, clip.radius, grey] }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_and_clips() {
        let mut s = Scene::new();
        s.rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        let base = s.push_layer();
        s.push_clip(Rect::new(0.0, 0.0, 5.0, 5.0), 2.0);
        s.rect(Rect::new(100.0, 100.0, 10.0, 10.0), Color::WHITE); // clipped out: dropped
        s.rect(Rect::new(1.0, 1.0, 10.0, 10.0), Color::WHITE);
        s.pop_clip();
        assert_eq!(s.used_layers().len(), 2);
        assert_eq!(s.used_layers()[0].quads.len(), 1);
        assert_eq!(s.used_layers()[1].quads.len(), 1);
        assert_eq!(s.used_layers()[1].quads[0].clip, [0.0, 0.0, 5.0, 5.0]);
        assert_eq!(s.used_layers()[1].quads[0].params[2], 2.0);
        s.set_layer(base);
        s.rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::WHITE);
        assert_eq!(s.used_layers()[0].quads.len(), 2);
        s.clear();
        assert_eq!(s.used_layers().len(), 1);
        assert!(s.used_layers()[0].is_empty());
        // invisible quads aren't drawn
        s.rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::TRANSPARENT);
        assert!(s.used_layers()[0].quads.is_empty());
    }
}
