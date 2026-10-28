//! bro-gfx: the GPU renderer behind bro's desktop app.
//!
//! - [`scene`]: what to draw this frame — layers of soft shadows, rounded rectangles (borders, gradients),
//!   images and glyphs, clipped to rounded rectangles.
//! - [`text`]: fonts (Inter for the interface, the terminal font, Nerd Font symbols, the system's fonts for
//!   fallback), shaping with cosmic-text, rasterising with swash into glyph atlases.
//! - [`builtin`]: box drawing, blocks, braille and powerline glyphs drawn to the cell so they join.
//! - [`renderer`]: wgpu pipelines, textures and presenting.
//!
//! Everything is in physical pixels; colours are sRGB with straight alpha and blend like a browser.

pub mod atlas;
pub mod builtin;
pub mod color;
mod display;
pub mod geom;
pub mod renderer;
pub mod scene;
pub mod text;

pub use color::Color;
pub use geom::{Corners, Edges, Point, Rect, Size};
pub use renderer::{Effects, FrameError, Options, Renderer};
pub use scene::{Fill, GlyphFormat, ImageId, Quad, Scene};
pub use text::{Align, Antialias, Face, Layout, MonoMetrics, Span, Text, TextOpts, raster_path};

/// Re-exported so the app can name surface targets and adapter info without its own wgpu dependency.
pub use wgpu;
