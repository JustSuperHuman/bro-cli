//! Text: fonts, shaping (cosmic-text), rasterising (swash) into the atlases, and drawing glyphs into a
//! [`Scene`].
//!
//! Two kinds of text:
//! - **laid-out text** for the interface ([`Text::layout`] → [`Layout`], cached across frames), proportional
//!   or monospace, single line with an ellipsis or wrapped;
//! - **terminal cells** ([`Text::draw_cell`]): one grapheme cluster placed on the cell grid, with box drawing
//!   and blocks drawn by [`crate::builtin`] so they join between cells.
//!
//! The font system starts with bro's bundled fonts only (instant), then swaps in one that also knows the
//! system's fonts (emoji, CJK, symbols) once a background thread has scanned them.
//!
//! Glyph edges are smoothed in greyscale, or per colour channel on LCD screens ([`Antialias`]): outlines are
//! sampled three times a pixel across and filtered into red, green and blue coverage, as ClearType does.

use crate::atlas::{Atlas, Kind};
use crate::builtin;
use crate::color::Color;
use crate::geom::{Point, Rect, Size};
use crate::scene::{GlyphFormat, Scene};
use cosmic_text::{Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Inter, the interface font (variable weight).
pub const INTER: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");
pub const INTER_ITALIC: &[u8] = include_bytes!("../assets/fonts/InterVariable-Italic.ttf");

/// How glyph edges are smoothed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Antialias {
    /// one coverage a pixel: right on any screen
    #[default]
    Grayscale,
    /// a coverage per colour channel, for LCD screens whose pixels are red, green, blue from the left (most)
    Rgb,
    /// the same for screens whose pixels are blue, green, red from the left
    Bgr,
}

/// A font role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Face {
    /// the interface font (Inter)
    #[default]
    Ui,
    /// the terminal font
    Mono,
    /// icon glyphs (Nerd Font symbols)
    Icons,
}

/// One run of styled text.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub face: Face,
    pub weight: u16,
    pub italic: bool,
    pub color: Color,
    /// font size override in physical pixels
    pub size: Option<f32>,
}

impl Span {
    pub fn new(text: impl Into<String>, color: Color) -> Span {
        Span { text: text.into(), face: Face::Ui, weight: 400, italic: false, color, size: None }
    }
    pub fn face(mut self, f: Face) -> Span {
        self.face = f;
        self
    }
    pub fn weight(mut self, w: u16) -> Span {
        self.weight = w;
        self
    }
    pub fn italic(mut self) -> Span {
        self.italic = true;
        self
    }
    pub fn size(mut self, s: f32) -> Span {
        self.size = Some(s);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// How to lay text out. Sizes in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextOpts {
    pub size: f32,
    /// line height in pixels
    pub line_height: f32,
    pub max_width: Option<f32>,
    /// wrap at `max_width` (else one line per paragraph, cut with an ellipsis when `ellipsis`)
    pub wrap: bool,
    pub ellipsis: bool,
    pub align: Align,
    /// stop after this many lines (the last one gets the ellipsis)
    pub max_lines: Option<usize>,
}

impl TextOpts {
    pub fn new(size: f32) -> TextOpts {
        TextOpts { size, line_height: (size * 1.4).round(), max_width: None, wrap: false, ellipsis: false, align: Align::Left, max_lines: None }
    }
}

/// One glyph of a [`Layout`].
#[derive(Clone, Debug)]
pub struct LGlyph {
    pub font: cosmic_text::fontdb::ID,
    pub glyph: u16,
    pub size: f32,
    pub weight: u16,
    pub flags: CacheKeyFlags,
    /// pen position relative to the layout's top-left (y is the baseline)
    pub x: f32,
    pub y: f32,
    /// advance
    pub w: f32,
    /// byte range in the layout's text
    pub start: usize,
    pub end: usize,
    pub span: usize,
}

#[derive(Clone, Debug)]
pub struct LLine {
    pub top: f32,
    pub baseline: f32,
    pub height: f32,
    pub width: f32,
    pub glyphs: Range<usize>,
    /// byte range of the line in the layout's text
    pub text: Range<usize>,
}

/// Shaped text, ready to draw at any position.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub glyphs: Vec<LGlyph>,
    pub lines: Vec<LLine>,
    pub size: Size,
    /// span colours, indexed by `LGlyph::span`
    pub colors: Vec<Color>,
    /// all the text, spans concatenated
    pub text: String,
    /// the text didn't fit and was cut
    pub truncated: bool,
}

impl Layout {
    pub fn width(&self) -> f32 {
        self.size.w
    }
    pub fn height(&self) -> f32 {
        self.size.h
    }
    /// x of the caret before byte `idx` (single-line text).
    pub fn caret_x(&self, idx: usize) -> f32 {
        for g in &self.glyphs {
            if idx <= g.start {
                return g.x;
            }
            if idx < g.end {
                // inside a cluster (ligature): proportional
                let frac = (idx - g.start) as f32 / (g.end - g.start).max(1) as f32;
                return g.x + g.w * frac;
            }
        }
        self.glyphs.last().map(|g| g.x + g.w).unwrap_or(0.0)
    }
    /// Byte index of the caret closest to `x` (single-line text).
    pub fn index_at(&self, x: f32) -> usize {
        for g in &self.glyphs {
            if x < g.x + g.w * 0.5 {
                return g.start;
            }
        }
        self.glyphs.last().map(|g| g.end).unwrap_or(0)
    }
}

/// Terminal cell metrics, in physical pixels (whole numbers so the grid is crisp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonoMetrics {
    pub size: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    /// from the cell's top
    pub baseline: f32,
    pub underline_y: f32,
    pub strike_y: f32,
    pub stroke: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GlyphKey {
    Font(CacheKey, bool),
    Builtin(char, u16, u16),
    /// a vector icon (SVG path data) by the caller's id, at a pixel size
    Path(u32, u16, u16),
}

#[derive(Clone, Copy, Debug)]
struct Placed {
    kind: Kind,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
}

/// A glyph of a terminal cluster, relative to the cell origin and baseline.
#[derive(Clone, Debug)]
struct CellGlyph {
    font: cosmic_text::fontdb::ID,
    glyph: u16,
    size: f32,
    weight: u16,
    flags: CacheKeyFlags,
    x: f32,
    y: f32,
    embolden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CellStyle {
    bold: bool,
    italic: bool,
}

/// A glyph of a run shaped as a whole, anchored to the cell (column within the run) its characters start in.
#[derive(Clone, Debug)]
struct RunGlyph {
    col: u16,
    glyph: CellGlyph,
}

struct Mono {
    family: String,
    metrics: MonoMetrics,
    clusters: HashMap<(String, CellStyle), Arc<[CellGlyph]>>,
    /// ASCII fast path per style (bold, italic)
    ascii: [Vec<Option<Arc<[CellGlyph]>>>; 4],
    /// the font's own advance (cells are this rounded to whole pixels)
    advance: f32,
    /// runs shaped together for ligatures
    runs: HashMap<(String, CellStyle), Arc<[RunGlyph]>>,
    /// whether the font joins sequences like `->` (found out on first use)
    ligatures: Option<bool>,
}

/// Family names for the roles.
#[derive(Clone, Debug, Default)]
struct Families {
    ui: String,
    mono: String,
    icons: String,
}

/// Fonts, shaping, glyph atlases.
pub struct Text {
    fs: FontSystem,
    scaler: swash::scale::ScaleContext,
    families: Families,
    /// every font we loaded ourselves (kept to rebuild the font system with the system fonts)
    sources: Vec<Arc<Vec<u8>>>,
    glyphs: HashMap<GlyphKey, Option<Placed>>,
    /// vector icons by id: SVG path data and its view box
    paths: HashMap<u32, (Arc<str>, f32)>,
    pub(crate) mask: Atlas,
    pub(crate) color: Atlas,
    layouts: HashMap<u64, (Arc<Layout>, u64)>,
    frame: u64,
    mono: Option<Mono>,
    /// variable-weight fonts (no fake bold needed), by font id
    variable: HashMap<cosmic_text::fontdb::ID, bool>,
    pending: Option<(Receiver<FontSystem>, usize)>,
    /// An atlas filled up this frame: draw the frame again.
    pub overflow: bool,
    /// how glyph edges are smoothed
    aa: Antialias,
}

fn load(fs: &mut FontSystem, data: &Arc<Vec<u8>>) -> Vec<String> {
    let ids = fs.db_mut().load_font_source(cosmic_text::fontdb::Source::Binary(data.clone()));
    ids.iter()
        .filter_map(|id| fs.db().face(*id))
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        .collect()
}

impl Text {
    /// A text system with Inter, the terminal font `mono` and the icon font `icons`; system fonts are added
    /// in the background (see [`Text::poll_fonts`]).
    pub fn new(mono: &[u8], icons: &[u8], system_fonts: bool) -> Text {
        let mut fs = FontSystem::new_with_locale_and_db(sys_locale(), cosmic_text::fontdb::Database::new());
        let mut sources = vec![];
        let mut families = Families::default();
        for (bytes, role) in [(INTER, Some(Face::Ui)), (INTER_ITALIC, None), (mono, Some(Face::Mono)), (icons, Some(Face::Icons))] {
            if bytes.is_empty() {
                continue;
            }
            let data = Arc::new(bytes.to_vec());
            let names = load(&mut fs, &data);
            sources.push(data);
            if let (Some(role), Some(name)) = (role, names.first()) {
                match role {
                    Face::Ui => families.ui = name.clone(),
                    Face::Mono => families.mono = name.clone(),
                    Face::Icons => families.icons = name.clone(),
                }
            }
        }
        let mut t = Text {
            fs,
            scaler: swash::scale::ScaleContext::new(),
            families,
            sources,
            glyphs: HashMap::new(),
            paths: HashMap::new(),
            mask: Atlas::new(Kind::Mask, 2048),
            color: Atlas::new(Kind::Color, 1024),
            layouts: HashMap::new(),
            frame: 0,
            mono: None,
            variable: HashMap::new(),
            pending: None,
            overflow: false,
            aa: Antialias::Grayscale,
        };
        if system_fonts {
            t.load_system_fonts();
        }
        t
    }

    /// Scan the system's fonts on a thread; [`Text::poll_fonts`] picks the result up.
    fn load_system_fonts(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sources = self.sources.clone();
        let count = sources.len();
        std::thread::Builder::new()
            .name("bro-fonts-scan".into())
            .spawn(move || {
                let mut db = cosmic_text::fontdb::Database::new();
                for s in &sources {
                    db.load_font_source(cosmic_text::fontdb::Source::Binary(s.clone()));
                }
                db.load_system_fonts();
                let _ = tx.send(FontSystem::new_with_locale_and_db(sys_locale(), db));
            })
            .ok();
        self.pending = Some((rx, count));
    }

    /// Swap in the font system with the system fonts once it's ready. True when it changed (redraw).
    pub fn poll_fonts(&mut self) -> bool {
        let Some((rx, count)) = &self.pending else { return false };
        match rx.try_recv() {
            Ok(mut fs) => {
                // fonts loaded since the scan started
                for s in &self.sources[*count..] {
                    fs.db_mut().load_font_source(cosmic_text::fontdb::Source::Binary(s.clone()));
                }
                self.fs = fs;
                self.pending = None;
                self.reset_caches();
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                false
            }
        }
    }

    /// Forget every shaped layout and glyph (fonts or atlases changed).
    fn reset_caches(&mut self) {
        self.layouts.clear();
        self.glyphs.clear();
        self.variable.clear();
        self.mask.reset(false);
        self.color.reset(false);
        if let Some(m) = &mut self.mono {
            m.clusters.clear();
            m.ascii = Default::default();
        }
    }

    /// Smooth glyph edges this way from now on (subpixel text needs [`crate::Renderer::subpixel_text`]).
    pub fn set_antialias(&mut self, aa: Antialias) {
        if aa == self.aa {
            return;
        }
        self.aa = aa;
        // subpixel glyphs live in the RGBA atlas: give it the room the greyscale ones had
        if aa != Antialias::Grayscale {
            self.color.size = self.color.size.max(self.mask.size);
        }
        self.glyphs.clear();
        self.mask.reset(false);
        self.color.reset(false);
    }

    pub fn antialias(&self) -> Antialias {
        self.aa
    }

    /// Call once per frame before drawing: ages the layout cache.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.overflow = false;
        if self.frame.is_multiple_of(120) {
            let now = self.frame;
            self.layouts.retain(|_, (_, used)| now - *used < 600);
        }
    }

    /// An atlas filled up: make it bigger and forget the placements (the caller draws the frame again).
    pub fn grow_atlases(&mut self) {
        let mask_full = self.mask.size;
        self.mask.reset(true);
        self.color.reset(mask_full >= self.color.size);
        self.glyphs.clear();
    }

    fn family(&self, face: Face) -> Family<'_> {
        let name = match face {
            Face::Ui => &self.families.ui,
            Face::Mono => &self.families.mono,
            Face::Icons => &self.families.icons,
        };
        if name.is_empty() {
            match face {
                Face::Mono => Family::Monospace,
                _ => Family::SansSerif,
            }
        } else {
            Family::Name(name)
        }
    }

    // ------------------------------------------------------------------ layout

    /// Shape `spans` (cached: the same text and options come back from the cache).
    pub fn layout(&mut self, spans: &[Span], opts: &TextOpts) -> Arc<Layout> {
        let key = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for s in spans {
                s.text.hash(&mut h);
                s.face.hash(&mut h);
                s.weight.hash(&mut h);
                s.italic.hash(&mut h);
                s.size.map(f32::to_bits).hash(&mut h);
            }
            opts.size.to_bits().hash(&mut h);
            opts.line_height.to_bits().hash(&mut h);
            opts.max_width.map(|w| w.round() as i32).hash(&mut h);
            opts.wrap.hash(&mut h);
            opts.ellipsis.hash(&mut h);
            opts.align.hash(&mut h);
            opts.max_lines.hash(&mut h);
            h.finish()
        };
        let frame = self.frame;
        if let Some((l, used)) = self.layouts.get_mut(&key) {
            *used = frame;
            // colours aren't part of the key: refresh them in place when they changed
            if l.colors.len() == spans.len() && l.colors.iter().zip(spans).any(|(c, s)| *c != s.color) {
                Arc::make_mut(l).colors = spans.iter().map(|s| s.color).collect();
            }
            return l.clone();
        }
        let l = Arc::new(self.shape(spans, opts));
        self.layouts.insert(key, (l.clone(), frame));
        l
    }

    /// One line of plain text.
    pub fn line(&mut self, text: &str, face: Face, size: f32, weight: u16, color: Color, max_width: Option<f32>) -> Arc<Layout> {
        let mut opts = TextOpts::new(size);
        opts.max_width = max_width;
        opts.ellipsis = max_width.is_some();
        self.layout(&[Span { text: text.to_string(), face, weight, italic: false, color, size: None }], &opts)
    }

    fn shape(&mut self, spans: &[Span], opts: &TextOpts) -> Layout {
        let metrics = Metrics::new(opts.size.max(1.0), opts.line_height.max(1.0));
        let mut buffer = Buffer::new(&mut self.fs, metrics);
        let wrap = if opts.wrap && opts.max_width.is_some() { Wrap::WordOrGlyph } else { Wrap::None };
        buffer.set_wrap(wrap);
        if opts.ellipsis && opts.max_width.is_some() {
            // a single line ends in "…"; wrapped text only when it has a line limit
            let limit = match (opts.wrap, opts.max_lines) {
                (true, Some(n)) => Some(cosmic_text::EllipsizeHeightLimit::Lines(n)),
                (true, None) => None,
                (false, _) => Some(cosmic_text::EllipsizeHeightLimit::Lines(1)),
            };
            if let Some(limit) = limit {
                buffer.set_ellipsize(cosmic_text::Ellipsize::End(limit));
            }
        }
        buffer.set_size(opts.max_width, None);
        let families: Vec<Family> = spans.iter().map(|s| self.family(s.face)).collect();
        let owned: Vec<(String, Attrs)> = spans
            .iter()
            .zip(families.iter())
            .enumerate()
            .map(|(i, (s, fam))| {
                let mut a = Attrs::new().family(*fam).weight(Weight(s.weight)).style(if s.italic { Style::Italic } else { Style::Normal }).metadata(i);
                if let Some(sz) = s.size {
                    a = a.metrics(Metrics::new(sz, opts.line_height.max(sz)));
                }
                (s.text.clone(), a)
            })
            .collect();
        let default = Attrs::new().family(families.first().copied().unwrap_or(Family::SansSerif));
        let align = match opts.align {
            Align::Left => None,
            Align::Center => Some(cosmic_text::Align::Center),
            Align::Right => Some(cosmic_text::Align::Right),
        };
        buffer.set_rich_text(owned.iter().map(|(t, a)| (t.as_str(), a.clone())), &default, Shaping::Advanced, align);
        buffer.shape_until_scroll(&mut self.fs, false);

        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        // byte offset of each original line (cosmic splits at '\n')
        let mut line_starts = vec![0usize];
        for (i, c) in text.char_indices() {
            if c == '\n' {
                line_starts.push(i + 1);
            }
        }
        let mut out = Layout { colors: spans.iter().map(|s| s.color).collect(), text: text.clone(), ..Default::default() };
        let mut width: f32 = 0.0;
        let mut bottom: f32 = 0.0;
        for run in buffer.layout_runs() {
            if opts.max_lines.is_some_and(|n| out.lines.len() >= n) {
                out.truncated = true;
                break;
            }
            let base = line_starts.get(run.line_i).copied().unwrap_or(0);
            let first = out.glyphs.len();
            let (mut lo, mut hi) = (usize::MAX, 0usize);
            for g in run.glyphs {
                lo = lo.min(base + g.start);
                hi = hi.max(base + g.end);
                out.glyphs.push(LGlyph {
                    font: g.font_id,
                    glyph: g.glyph_id,
                    size: g.font_size,
                    weight: g.font_weight.0,
                    flags: g.cache_key_flags,
                    x: g.x + g.font_size * g.x_offset,
                    y: run.line_y - g.font_size * g.y_offset,
                    w: g.w,
                    start: base + g.start,
                    end: base + g.end,
                    span: g.metadata,
                });
            }
            if lo == usize::MAX {
                lo = base;
                hi = base;
            }
            width = width.max(run.line_w);
            bottom = bottom.max(run.line_top + run.line_height);
            out.lines.push(LLine { top: run.line_top, baseline: run.line_y, height: run.line_height, width: run.line_w, glyphs: first..out.glyphs.len(), text: lo..hi });
        }
        if out.lines.is_empty() {
            bottom = opts.line_height;
        }
        // an ellipsis or wrap at max_width never reports more than max_width
        let shown_w = match opts.max_width {
            Some(m) if opts.wrap || opts.ellipsis => width.min(m),
            _ => width,
        };
        if let Some(m) = opts.max_width
            && width > m + 0.5
            && !opts.wrap
        {
            out.truncated = true;
        }
        out.size = Size::new(shown_w.ceil(), bottom.ceil());
        out
    }

    /// Width of `text` on one line.
    pub fn measure(&mut self, text: &str, face: Face, size: f32, weight: u16) -> f32 {
        self.line(text, face, size, weight, Color::WHITE, None).width()
    }

    // ------------------------------------------------------------------ drawing

    /// Draw a layout with its top-left at `origin` (snapped to whole pixels). `colors` overrides the span
    /// colours; `alpha` fades everything.
    pub fn draw(&mut self, scene: &mut Scene, layout: &Layout, origin: Point, colors: Option<&[Color]>, alpha: f32) {
        let (ox, oy) = (origin.x.round(), origin.y.round());
        // whole layout clipped out: nothing to do
        if scene.clipped_out(&Rect::new(ox, oy, layout.size.w.max(1.0), layout.size.h.max(1.0)).outset(4.0)) {
            return;
        }
        for g in &layout.glyphs {
            let color = colors.and_then(|c| c.get(g.span)).copied().or_else(|| layout.colors.get(g.span).copied()).unwrap_or(Color::WHITE);
            if color.a * alpha <= 0.0 {
                continue;
            }
            // baselines on whole pixels: horizontal strokes stay one crisp row instead of smearing over two
            let (key, ix, iy) = CacheKey::new(g.font, g.glyph, g.size, (ox + g.x, (oy + g.y).round()), Weight(g.weight), g.flags);
            self.put_glyph(scene, GlyphKey::Font(key, false), ix, iy, color.fade(alpha));
        }
    }

    fn put_glyph(&mut self, scene: &mut Scene, key: GlyphKey, ix: i32, iy: i32, color: Color) {
        let Some(p) = self.raster(key) else { return };
        if p.w == 0 || p.h == 0 {
            return;
        }
        let rect = Rect::new((ix + p.left) as f32, (iy - p.top) as f32, p.w as f32, p.h as f32);
        let atlas = if p.kind == Kind::Mask { &self.mask } else { &self.color };
        let uv = atlas.uv(p.x, p.y, p.w, p.h);
        let format = match p.kind {
            Kind::Mask => GlyphFormat::Mask,
            Kind::Color => GlyphFormat::Color,
            Kind::Lcd => GlyphFormat::Lcd,
        };
        scene.glyph_as(rect, uv, color, format);
    }

    /// The atlas placement of a glyph, rasterising it on first use.
    fn raster(&mut self, key: GlyphKey) -> Option<Placed> {
        if let Some(p) = self.glyphs.get(&key) {
            return *p;
        }
        let placed = match key {
            GlyphKey::Font(ck, embolden) => self.raster_font(ck, embolden),
            GlyphKey::Builtin(c, w, h) => builtin::render(c, w as u32, h as u32).and_then(|px| self.place(Kind::Mask, w as u32, h as u32, 0, 0, px)),
            GlyphKey::Path(id, w, h) => {
                let (path, view) = self.paths.get(&id).cloned()?;
                let px = raster_path(&path, view, w as u32, h as u32);
                self.place(Kind::Mask, w as u32, h as u32, 0, 0, px)
            }
        };
        // a full atlas isn't remembered: the frame is drawn again after growing it
        if placed.is_some() || !self.overflow {
            self.glyphs.insert(key, placed);
        }
        placed
    }

    fn place(&mut self, kind: Kind, w: u32, h: u32, left: i32, top: i32, data: Vec<u8>) -> Option<Placed> {
        let atlas = if kind == Kind::Mask { &mut self.mask } else { &mut self.color };
        match atlas.alloc(w, h) {
            Some((x, y)) => {
                atlas.put(x, y, w, h, data);
                Some(Placed { kind, x, y, w, h, left, top })
            }
            None => {
                self.overflow = true;
                None
            }
        }
    }

    fn raster_font(&mut self, ck: CacheKey, embolden: bool) -> Option<Placed> {
        use swash::scale::{Render, Source, StrikeWith, image::Content};
        use swash::zeno::{Angle, Format, Transform, Vector};
        let font = self.fs.get_font(ck.font_id, ck.font_weight)?;
        let size = f32::from_bits(ck.font_size_bits);
        let swash_font = font.as_swash();
        let wght = swash_font.variations().find_by_tag(swash::Tag::from_be_bytes(*b"wght"));
        let mut builder = self.scaler.builder(swash_font).size(size).hint(!ck.flags.contains(CacheKeyFlags::DISABLE_HINTING));
        if let Some(v) = wght {
            builder = builder.normalized_coords(swash_font.variations().normalized_coords([(swash::Tag::from_be_bytes(*b"wght"), (ck.font_weight.0 as f32).clamp(v.min_value(), v.max_value()))]));
        }
        let mut scaler = builder.build();
        let offset = Vector::new(ck.x_bin.as_float(), ck.y_bin.as_float());
        let bolden = if embolden { (size / 28.0).max(0.4) } else { 0.0 };
        let italic = ck.flags.contains(CacheKeyFlags::FAKE_ITALIC).then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)));
        // colour glyphs come out as they are; outlines here only when smoothing in greyscale
        let subpixel = self.aa != Antialias::Grayscale;
        let sources: &[Source] = if subpixel { &[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)] } else { &[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline] };
        if let Some(image) = Render::new(sources).format(Format::Alpha).offset(offset).embolden(bolden).transform(italic).render(&mut scaler, ck.glyph_id) {
            let (w, h) = (image.placement.width, image.placement.height);
            let (left, top) = (image.placement.left, image.placement.top);
            return match image.content {
                Content::Mask => self.place(Kind::Mask, w, h, left, top, image.data),
                Content::Color => self.place(Kind::Color, w, h, left, top, image.data),
                // subpixel masks aren't requested (Format::Alpha)
                Content::SubpixelMask => None,
            };
        }
        if !subpixel {
            return None;
        }
        // subpixel: the outline three samples a pixel across, filtered into a coverage per colour channel
        let wide = italic.unwrap_or(Transform::IDENTITY).then_scale(3.0, 1.0);
        let image = Render::new(&[Source::Outline]).format(Format::Alpha).offset(Vector::new(offset.x * 3.0, offset.y)).embolden(bolden).transform(Some(wide)).render(&mut scaler, ck.glyph_id)?;
        let p = image.placement;
        let (data, w, left) = lcd_filter(&image.data, p.width, p.height, p.left, self.aa == Antialias::Bgr);
        self.place(Kind::Lcd, w, if w == 0 { 0 } else { p.height }, left, p.top, data)
    }

    // ------------------------------------------------------------------ terminal cells

    /// Use `size`-pixel cells of the terminal font; `line_height` multiplies the font's natural line height.
    pub fn set_mono(&mut self, size: f32, line_height: f32) -> MonoMetrics {
        let family = self.families.mono.clone();
        if let Some(m) = &self.mono
            && m.family == family
            && m.metrics.size == size
            && (m.metrics.cell_h - Self::cell_height(&mut self.fs, &family, size, line_height)).abs() < 0.5
        {
            return m.metrics;
        }
        let metrics = self.compute_mono(&family, size, line_height);
        let advance = Self::advance(&mut self.fs, &family, size).unwrap_or(metrics.cell_w);
        self.mono = Some(Mono { family, metrics, clusters: HashMap::new(), ascii: Default::default(), advance, runs: HashMap::new(), ligatures: None });
        metrics
    }

    /// The font's advance at `size`, unrounded.
    fn advance(fs: &mut FontSystem, family: &str, size: f32) -> Option<f32> {
        let id = Self::face_id(fs, family)?;
        let font = fs.get_font(id, Weight::NORMAL)?;
        let sf = font.as_swash();
        let gid = sf.charmap().map('M');
        Some(sf.glyph_metrics(&[]).scale(size).advance_width(gid)).filter(|a| *a > 0.0)
    }

    fn face_id(fs: &FontSystem, family: &str) -> Option<cosmic_text::fontdb::ID> {
        let q = cosmic_text::fontdb::Query { families: &[cosmic_text::fontdb::Family::Name(family)], ..Default::default() };
        fs.db().query(&q)
    }

    fn cell_height(fs: &mut FontSystem, family: &str, size: f32, line_height: f32) -> f32 {
        let Some(id) = Self::face_id(fs, family) else { return (size * 1.25 * line_height).round() };
        let Some(font) = fs.get_font(id, Weight::NORMAL) else { return (size * 1.25 * line_height).round() };
        let m = font.as_swash().metrics(&[]);
        let scale = size / m.units_per_em as f32;
        ((m.ascent + m.descent + m.leading) * scale * line_height).round().max(1.0)
    }

    fn compute_mono(&mut self, family: &str, size: f32, line_height: f32) -> MonoMetrics {
        let fallback = MonoMetrics { size, cell_w: (size * 0.6).round(), cell_h: (size * 1.25).round(), baseline: size.round(), underline_y: (size * 1.1).round(), strike_y: (size * 0.65).round(), stroke: 1.0 };
        let Some(id) = Self::face_id(&self.fs, family) else { return fallback };
        let Some(font) = self.fs.get_font(id, Weight::NORMAL) else { return fallback };
        let sf = font.as_swash();
        let m = sf.metrics(&[]);
        let scale = size / m.units_per_em as f32;
        let gid = sf.charmap().map('M');
        let adv = sf.glyph_metrics(&[]).scale(size).advance_width(gid);
        let cell_w = adv.round().max(1.0);
        let natural = (m.ascent + m.descent + m.leading) * scale;
        let cell_h = (natural * line_height).round().max(1.0);
        // centre the font's line in the cell
        let pad = ((cell_h - natural) / 2.0).max(0.0);
        let baseline = (pad + m.ascent * scale).round();
        let stroke = (m.stroke_size * scale).round().max(1.0);
        let underline_y = (baseline - m.underline_offset * scale).round().min(cell_h - stroke);
        let strike_y = (baseline - m.strikeout_offset * scale).round();
        MonoMetrics { size, cell_w, cell_h, baseline, underline_y, strike_y, stroke }
    }

    pub fn mono_metrics(&self) -> Option<MonoMetrics> {
        self.mono.as_ref().map(|m| m.metrics)
    }

    /// Load another terminal font (by its file bytes) and use it from now on.
    pub fn set_mono_font(&mut self, bytes: &[u8]) {
        let data = Arc::new(bytes.to_vec());
        let names = load(&mut self.fs, &data);
        self.sources.push(data);
        if let Some(name) = names.first() {
            self.families.mono = name.clone();
        }
        self.mono = None;
        self.layouts.clear();
    }

    /// The shaped glyphs of one terminal cluster.
    fn cluster(&mut self, text: &str, style: CellStyle) -> Arc<[CellGlyph]> {
        let si = style.bold as usize * 2 + style.italic as usize;
        let ascii = text.len() == 1 && text.as_bytes()[0] < 128;
        if ascii {
            let b = text.as_bytes()[0] as usize;
            if let Some(Some(g)) = self.mono.as_ref().and_then(|m| m.ascii[si].get(b)) {
                return g.clone();
            }
        } else if let Some(g) = self.mono.as_ref().and_then(|m| m.clusters.get(&(text.to_string(), style))) {
            return g.clone();
        }
        let Some(m) = &self.mono else { return Arc::from(vec![]) };
        let size = m.metrics.size;
        let family = m.family.clone();
        let mut buffer = Buffer::new(&mut self.fs, Metrics::new(size, m.metrics.cell_h));
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            .weight(if style.bold { Weight::BOLD } else { Weight::NORMAL })
            .style(if style.italic { Style::Italic } else { Style::Normal });
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        let mut out = vec![];
        if let Some(run) = buffer.layout_runs().next() {
            for g in run.glyphs {
                let embolden = style.bold && !self.is_bold_capable(g.font_id);
                out.push(CellGlyph {
                    font: g.font_id,
                    glyph: g.glyph_id,
                    size: g.font_size,
                    weight: g.font_weight.0,
                    flags: g.cache_key_flags,
                    x: g.x + g.font_size * g.x_offset,
                    y: -g.font_size * g.y_offset,
                    embolden,
                });
            }
        }
        let glyphs: Arc<[CellGlyph]> = Arc::from(out);
        if let Some(m) = &mut self.mono {
            if ascii {
                let b = text.as_bytes()[0] as usize;
                if m.ascii[si].len() < 128 {
                    m.ascii[si] = vec![None; 128];
                }
                m.ascii[si][b] = Some(glyphs.clone());
            } else {
                m.clusters.insert((text.to_string(), style), glyphs.clone());
            }
        }
        glyphs
    }

    /// A bold request shows real bold: the face is bold, or variable with a weight axis.
    fn is_bold_capable(&mut self, id: cosmic_text::fontdb::ID) -> bool {
        if let Some(v) = self.variable.get(&id) {
            return *v;
        }
        let bold_face = self.fs.db().face(id).is_some_and(|f| f.weight.0 >= 600);
        let variable = self.fs.get_font(id, Weight::NORMAL).is_some_and(|f| f.as_swash().variations().find_by_tag(swash::Tag::from_be_bytes(*b"wght")).is_some());
        let v = bold_face || variable;
        self.variable.insert(id, v);
        v
    }

    /// Draw a vector icon in `color`, fitted into the pixel square of `side` at (x, y). `path` is SVG path data
    /// in a `view`×`view` box; `id` names it (the path is only read the first time an id is seen).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_path(&mut self, scene: &mut Scene, id: u32, path: &str, view: f32, x: f32, y: f32, side: f32, color: Color) {
        let px = side.round().max(1.0) as u16;
        self.paths.entry(id).or_insert_with(|| (Arc::from(path), view));
        self.put_glyph(scene, GlyphKey::Path(id, px, px), x.round() as i32, y.round() as i32, color);
    }

    /// Draw one terminal cluster with its cell's top-left at (x, y); `cells` is 2 for wide characters.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_cell(&mut self, scene: &mut Scene, text: &str, bold: bool, italic: bool, x: f32, y: f32, cells: u8, color: Color) {
        let Some(m) = self.mono.as_ref().map(|m| m.metrics) else { return };
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && builtin::handles(c)
        {
            let (w, h) = ((m.cell_w * cells.max(1) as f32) as u16, m.cell_h as u16);
            self.put_glyph(scene, GlyphKey::Builtin(c, w, h), x as i32, y as i32, color);
            return;
        }
        let glyphs = self.cluster(text, CellStyle { bold, italic });
        let baseline = y + m.baseline;
        for g in glyphs.iter() {
            let (key, ix, iy) = CacheKey::new(g.font, g.glyph, g.size, (x + g.x, baseline + g.y), Weight(g.weight), g.flags);
            self.put_glyph(scene, GlyphKey::Font(key, g.embolden), ix, iy, color);
        }
    }

    /// Whether the terminal font joins sequences like `->` and `!=` into ligatures (Fira Code does).
    pub fn mono_ligatures(&mut self) -> bool {
        match self.mono.as_ref().map(|m| m.ligatures) {
            None => return false,
            Some(Some(known)) => return known,
            Some(None) => {}
        }
        let style = CellStyle { bold: false, italic: false };
        let mut joins = false;
        for probe in ["->", "!=", "==", "=>", "::", "//"] {
            let shaped: Vec<u16> = self.shape_run(probe, style).iter().map(|g| g.glyph.glyph).collect();
            let mut alone: Vec<u16> = vec![];
            for c in probe.chars() {
                alone.extend(self.cluster(c.encode_utf8(&mut [0; 4]), style).iter().map(|g| g.glyph));
            }
            if shaped != alone {
                joins = true;
                break;
            }
        }
        if let Some(m) = &mut self.mono {
            m.ligatures = Some(joins);
        }
        joins
    }

    /// Draw a run of single-cell characters shaped together, so the font's ligatures join up. Each glyph stays
    /// anchored to the cell its characters start in, whatever the rounding of the cell width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_run(&mut self, scene: &mut Scene, text: &str, bold: bool, italic: bool, x: f32, y: f32, color: Color) {
        let Some((m, adv)) = self.mono.as_ref().map(|m| (m.metrics, m.advance)) else { return };
        let glyphs = self.shape_run(text, CellStyle { bold, italic });
        let baseline = y + m.baseline;
        for g in glyphs.iter() {
            let gx = x + g.col as f32 * m.cell_w + g.glyph.x - g.col as f32 * adv;
            let c = &g.glyph;
            let (key, ix, iy) = CacheKey::new(c.font, c.glyph, c.size, (gx, baseline + c.y), Weight(c.weight), c.flags);
            self.put_glyph(scene, GlyphKey::Font(key, c.embolden), ix, iy, color);
        }
    }

    /// The shaped glyphs of a run of single-cell characters, `x` relative to the run's start.
    fn shape_run(&mut self, text: &str, style: CellStyle) -> Arc<[RunGlyph]> {
        if let Some(g) = self.mono.as_ref().and_then(|m| m.runs.get(&(text.to_string(), style))) {
            return g.clone();
        }
        let Some(m) = &self.mono else { return Arc::from(vec![]) };
        let (size, family) = (m.metrics.size, m.family.clone());
        let mut buffer = Buffer::new(&mut self.fs, Metrics::new(size, m.metrics.cell_h));
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            .weight(if style.bold { Weight::BOLD } else { Weight::NORMAL })
            .style(if style.italic { Style::Italic } else { Style::Normal });
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);
        let mut out = vec![];
        if let Some(run) = buffer.layout_runs().next() {
            for g in run.glyphs {
                let embolden = style.bold && !self.is_bold_capable(g.font_id);
                let col = text.get(..g.start).map_or(0, |s| s.chars().count()) as u16;
                out.push(RunGlyph {
                    col,
                    glyph: CellGlyph {
                        font: g.font_id,
                        glyph: g.glyph_id,
                        size: g.font_size,
                        weight: g.font_weight.0,
                        flags: g.cache_key_flags,
                        x: g.x + g.font_size * g.x_offset,
                        y: -g.font_size * g.y_offset,
                        embolden,
                    },
                });
            }
        }
        let glyphs: Arc<[RunGlyph]> = Arc::from(out);
        if let Some(m) = &mut self.mono {
            if m.runs.len() > 4096 {
                m.runs.clear();
            }
            m.runs.insert((text.to_string(), style), glyphs.clone());
        }
        glyphs
    }

    /// Glyphs waiting to be copied to the GPU, and the atlases' sizes/generations (for the renderer).
    pub(crate) fn atlases_mut(&mut self) -> (&mut Atlas, &mut Atlas) {
        (&mut self.mask, &mut self.color)
    }

    /// The interface font's family name (for diagnostics).
    pub fn ui_family(&self) -> &str {
        &self.families.ui
    }
    pub fn mono_family(&self) -> &str {
        &self.families.mono
    }
}

fn sys_locale() -> String {
    "en-US".to_string()
}

/// FreeType's default LCD filter: each sample spreads over its two neighbours either side, which keeps colour
/// fringes faint (the weights sum to 256).
const LCD_TAPS: [u32; 5] = [8, 77, 86, 77, 8];

/// A glyph's coverage sampled three times a pixel across (`w3` samples a row, the first at subpixel `left3`)
/// filtered into RGBA pixels: a coverage per colour channel (red on the left, blue when `bgr`) and the largest
/// as alpha. Returns the pixels, their width and the first one's x.
fn lcd_filter(data: &[u8], w3: u32, h: u32, left3: i32, bgr: bool) -> (Vec<u8>, u32, i32) {
    if w3 == 0 || h == 0 {
        return (vec![], 0, 0);
    }
    // the filter reaches two samples past either end
    let first = (left3 - 2).div_euclid(3);
    let end = (left3 + w3 as i32 + 1).div_euclid(3) + 1;
    let w = (end - first) as u32;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (y, row) in data.chunks_exact(w3 as usize).take(h as usize).enumerate() {
        let sample = |s: i32| usize::try_from(s - left3).ok().and_then(|i| row.get(i)).map_or(0, |&v| v as u32);
        for x in 0..w as usize {
            let s0 = 3 * (first + x as i32);
            let mut c = [0u8; 3];
            for (ch, v) in c.iter_mut().enumerate() {
                let s = s0 + ch as i32;
                let sum: u32 = LCD_TAPS.iter().enumerate().map(|(k, t)| t * sample(s + k as i32 - 2)).sum();
                *v = ((sum + 128) >> 8).min(255) as u8;
            }
            if bgr {
                c.swap(0, 2);
            }
            let o = (y * w as usize + x) * 4;
            out[o..o + 3].copy_from_slice(&c);
            out[o + 3] = c[0].max(c[1]).max(c[2]);
        }
    }
    (out, w, first)
}

/// An SVG path filled into a `w`×`h` alpha mask (its `view`-sized box fitted and centred).
pub fn raster_path(path: &str, view: f32, w: u32, h: u32) -> Vec<u8> {
    use swash::zeno::{Mask, Transform};
    let s = (w as f32 / view.max(0.001)).min(h as f32 / view.max(0.001));
    let (dx, dy) = ((w as f32 - view * s) / 2.0, (h as f32 - view * s) / 2.0);
    let mut buf = vec![0u8; (w * h) as usize];
    Mask::new(path).transform(Some(Transform::scale(s, s).then_translate(dx, dy))).size(w, h).render_into(&mut buf, None);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_measures_and_caches() {
        let mut t = Text::new(&[], &[], false);
        assert!(!t.ui_family().is_empty(), "Inter is bundled");
        t.begin_frame();
        let a = t.line("Hello bro", Face::Ui, 14.0, 400, Color::WHITE, None);
        assert!(a.width() > 40.0 && a.width() < 120.0, "width {}", a.width());
        assert_eq!(a.lines.len(), 1);
        let b = t.line("Hello bro", Face::Ui, 14.0, 400, Color::BLACK, None);
        assert!(Arc::ptr_eq(&a, &b) || a.width() == b.width(), "cached");
        assert_eq!(b.colors[0], Color::BLACK, "colours refresh in place");
        // an ellipsis keeps it inside the width
        let long = t.line("a very long line of text that will not fit in the box", Face::Ui, 14.0, 400, Color::WHITE, Some(80.0));
        assert!(long.width() <= 80.0, "width {}", long.width());
        // carets move left to right
        assert!(a.caret_x(0) < a.caret_x(5));
        assert_eq!(a.index_at(-5.0), 0);
        assert_eq!(a.index_at(1000.0), "Hello bro".len());
    }

    #[test]
    fn wraps_into_lines() {
        let mut t = Text::new(&[], &[], false);
        let mut o = TextOpts::new(14.0);
        o.max_width = Some(60.0);
        o.wrap = true;
        let l = t.layout(&[Span::new("one two three four five six", Color::WHITE)], &o);
        assert!(l.lines.len() >= 3, "{} lines", l.lines.len());
        assert!(l.width() <= 60.0);
        o.max_lines = Some(2);
        let l = t.layout(&[Span::new("one two three four five six", Color::WHITE)], &o);
        assert_eq!(l.lines.len(), 2);
        // an ellipsis without a line limit still wraps every line
        o.max_lines = None;
        o.ellipsis = true;
        let l = t.layout(&[Span::new("one two three four five six", Color::WHITE)], &o);
        assert!(l.lines.len() >= 3, "{} lines", l.lines.len());
    }

    #[test]
    fn draws_glyphs_into_the_scene() {
        let mut t = Text::new(&[], &[], false);
        let mut s = Scene::new();
        let l = t.line("Hi", Face::Ui, 16.0, 400, Color::WHITE, None);
        t.draw(&mut s, &l, Point::new(10.0, 10.0), None, 1.0);
        assert_eq!(s.used_layers()[0].glyphs.len(), 2);
        assert!(!t.mask.uploads.is_empty(), "rasterised into the atlas");
    }

    #[test]
    fn subpixel_text_filters_into_colour_channels() {
        // a solid run of samples well inside the glyph: every channel fully covered
        let (px, w, left) = lcd_filter(&[255; 30], 30, 1, 0, false);
        assert_eq!(left, -1, "the filter reaches two samples before the first");
        assert_eq!(w, 12);
        assert_eq!(&px[4 * 5..4 * 5 + 4], &[255, 255, 255, 255]);
        // a stem one pixel wide: its pixel stays strong, its neighbours keep a faint fringe, nothing is lost
        let (px, w, left) = lcd_filter(&[255, 255, 255], 3, 1, 3, false);
        assert_eq!((w, left), (3, 0));
        let at = |x: usize| &px[x * 4..x * 4 + 4];
        assert_eq!(&at(1)[..3], &[170, 239, 170]);
        assert_eq!((0..3).flat_map(|x| at(x)[..3].to_vec()).map(u32::from).sum::<u32>(), 3 * 255);
        assert!(at(0)[2] > 0 && at(0)[2] < 120 && at(0)[0] == 0, "red side of the left neighbour is clear: {:?}", at(0));
        assert_eq!(at(0)[3], at(0)[2], "alpha is the largest channel");
        // blue-first screens get the channels the other way round
        let (bgr, ..) = lcd_filter(&[255, 255, 255], 3, 1, 3, true);
        assert_eq!(bgr[0], px[2]);
        assert_eq!(bgr[2], px[0]);

        let mut t = Text::new(&[], &[], false);
        t.set_antialias(Antialias::Rgb);
        let mut s = Scene::new();
        let l = t.line("Hi", Face::Ui, 16.0, 400, Color::WHITE, None);
        t.draw(&mut s, &l, Point::new(10.0, 10.0), None, 1.0);
        let glyphs = &s.used_layers()[0].glyphs;
        assert_eq!(glyphs.len(), 2);
        assert!(glyphs.iter().all(|g| g.params[0] == 2.0), "drawn as subpixel glyphs");
        assert!(!t.color.uploads.is_empty() && t.mask.uploads.is_empty(), "rasterised into the RGBA atlas");
    }

    #[test]
    fn vector_icons_fill_their_square() {
        // a diamond in a 24 box: solid in the middle, empty in the corners
        let px = raster_path("M12 0 24 12 12 24 0 12Z", 24.0, 32, 32);
        assert_eq!(px.len(), 32 * 32);
        assert!(px[16 * 32 + 16] > 200, "inside");
        assert_eq!(px[0], 0, "corner");
        let mut t = Text::new(&[], &[], false);
        let mut s = Scene::new();
        t.draw_path(&mut s, 7, "M12 0 24 12 12 24 0 12Z", 24.0, 10.0, 10.0, 20.0, Color::WHITE);
        t.draw_path(&mut s, 7, "ignored the second time", 24.0, 40.0, 10.0, 20.0, Color::WHITE);
        assert_eq!(s.used_layers()[0].glyphs.len(), 2);
    }

    #[test]
    fn ligature_fonts_join_runs_on_the_cell_grid() {
        let fira = include_bytes!("../../bro-tui/assets/fonts/FiraCode-Variable.ttf");
        let mut t = Text::new(fira, &[], false);
        let m = t.set_mono(14.0, 1.2);
        assert!(t.mono_ligatures(), "Fira Code joins -> and !=");
        let mut s = Scene::new();
        t.draw_run(&mut s, "->", false, false, 100.0, 0.0, Color::WHITE);
        let glyphs = &s.used_layers()[0].glyphs;
        assert!(!glyphs.is_empty());
        // nothing drifts off the two cells (Fira's arrow reaches back over the first)
        assert!(glyphs.iter().all(|g| g.rect[0] >= 100.0 - m.cell_w && g.rect[0] < 100.0 + 2.0 * m.cell_w), "{glyphs:?}");

        let plex = include_bytes!("../../bro-tui/assets/fonts/IBMPlexMono-Regular.ttf");
        let mut t = Text::new(plex, &[], false);
        t.set_mono(14.0, 1.2);
        assert!(!t.mono_ligatures(), "Plex Mono has none");
    }
}
