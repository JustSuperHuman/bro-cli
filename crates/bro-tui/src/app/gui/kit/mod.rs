//! The desktop app's drawing kit: design tokens taken from bro's theme, and [`Ui`], the per-frame context
//! every surface draws through (logical pixels in, physical pixels out, hit regions for the mouse, eased
//! animations).
//!
//! Immediate mode, like the terminal UI: each frame draws everything and records where the clickable things
//! are; the mouse is matched against the last frame's [`Hit`]s.

pub mod brand;
pub mod widgets;

use super::actions::Action;
use bro_gfx::text::{Align, Face, Layout, Span, TextOpts};
use bro_gfx::{Color, Corners, Point, Quad, Rect, Scene, Text};
use ratatui::style::Color as TuiColor;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// The interface's colours, derived from bro's theme so every theme (and custom theme files) restyles the app.
#[derive(Clone, Debug)]
pub struct Tokens {
    pub light: bool,
    /// the window behind everything
    pub bg: Color,
    /// session cards and panels
    pub surface: Color,
    /// what the window is filled with: `bg`, see-through when the window's material lets the desktop show. The
    /// window's surfaces (its content area, the session cards, the panels on them) are glass fills (`Ui::glass`)
    /// so they all stay exactly as see-through as this
    pub window: Color,
    /// menus, dialogs, popovers
    pub raised: Color,
    /// inputs and wells inside a surface
    pub sunken: Color,
    pub hover: Color,
    pub selected: Color,
    /// standard controls (buttons, fields, segmented): rest, hover, pressed fills and their stroke
    pub control: Color,
    pub control_hover: Color,
    pub control_pressed: Color,
    pub stroke: Color,
    /// a text field with the keyboard
    pub field_focus: Color,
    /// the content area the sessions sit on (a step up from the window, as WinUI's layer)
    pub layer: Color,
    pub hairline: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    /// text on an accent fill
    pub on_accent: Color,
    pub shine: Color,
    pub inline: Color,
    pub good: Color,
    pub danger: Color,
    pub gold: Color,
    /// "running low": amber, darker on light themes so it reads on white
    pub warn: Color,
    pub scrim: Color,
    pub shadow: Color,
    /// the terminals' default text and background, and their 16 colours
    pub term_fg: Color,
    pub term_bg: Color,
    pub ansi: [Color; 16],
    pub cursor: Color,
    pub selection: Color,
}

/// A balanced default palette for themes that leave the 16 colours to the terminal.
const DARK_ANSI: [u32; 16] = [
    0x1d2027, 0xf0717a, 0x89d185, 0xe8c27a, 0x76a8f5, 0xc897f2, 0x6fcfd6, 0xc9cdd6, //
    0x5c6370, 0xff8a92, 0xa2e39b, 0xf5d491, 0x93bcff, 0xd9b0ff, 0x8de2e8, 0xf2f4f8,
];
const LIGHT_ANSI: [u32; 16] = [
    0x2b2f36, 0xc7323d, 0x2f8a3c, 0x9a6b00, 0x2563c9, 0x8a3fc4, 0x1b8590, 0xd5d8de, //
    0x6b7280, 0xdc3f4a, 0x3a9b48, 0xb07c08, 0x3b78e0, 0x9b55d6, 0x23969f, 0x8a9099,
];

/// A ratatui colour as an sRGB colour; `Reset` takes `default`.
pub fn color(c: TuiColor, default: Color, ansi: &[Color; 16]) -> Color {
    match c {
        TuiColor::Reset => default,
        TuiColor::Rgb(r, g, b) => Color::rgb8(r, g, b),
        TuiColor::Indexed(i) => indexed(i, ansi),
        TuiColor::Black => ansi[0],
        TuiColor::Red => ansi[1],
        TuiColor::Green => ansi[2],
        TuiColor::Yellow => ansi[3],
        TuiColor::Blue => ansi[4],
        TuiColor::Magenta => ansi[5],
        TuiColor::Cyan => ansi[6],
        TuiColor::Gray => ansi[7],
        TuiColor::DarkGray => ansi[8],
        TuiColor::LightRed => ansi[9],
        TuiColor::LightGreen => ansi[10],
        TuiColor::LightYellow => ansi[11],
        TuiColor::LightBlue => ansi[12],
        TuiColor::LightMagenta => ansi[13],
        TuiColor::LightCyan => ansi[14],
        TuiColor::White => ansi[15],
    }
}

/// xterm's 256 colours: the 16 from the palette, the 6×6×6 cube, the grey ramp.
pub fn indexed(i: u8, ansi: &[Color; 16]) -> Color {
    match i {
        0..=15 => ansi[i as usize],
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::rgb8(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Color::rgb8(v, v, v)
        }
    }
}

impl Tokens {
    pub fn from_theme(t: &crate::theme::Theme) -> Tokens {
        let light = t.is_light();
        let default_ansi = if light { LIGHT_ANSI } else { DARK_ANSI };
        let mut ansi = default_ansi.map(Color::hex);
        if let Some(a) = t.ansi {
            for (i, c) in a.iter().enumerate() {
                ansi[i] = color(*c, ansi[i], &default_ansi.map(Color::hex));
            }
        }
        let editor = match t.bg {
            TuiColor::Reset => Color::hex(if light { 0xf4f5f7 } else { 0x0e1015 }),
            c => color(c, Color::BLACK, &ansi),
        };
        // a theme with its own side bar colour (VS Code's) is the window around the sessions, which sit on the
        // theme's background as its editor does
        let panel = t.panel.filter(|_| t.painted()).map(|c| color(c, editor, &ansi));
        let bg = panel.unwrap_or(editor);
        let text = match t.fg {
            TuiColor::Reset => Color::hex(if light { 0x1f2328 } else { 0xe4e6eb }),
            c => color(c, Color::WHITE, &ansi),
        };
        let c = |x: TuiColor| color(x, text, &ansi);
        // surfaces step toward the text colour (lighter on dark themes, darker on light ones)
        let step = |amount: f32| bg.mix(text, amount);
        let surface = match panel {
            Some(_) => editor,
            None if light => Color::WHITE.mix(bg, 0.15),
            None => step(0.035),
        };
        let raised = if light { Color::WHITE } else { step(0.075) };
        let accent = c(t.accent);
        // white on an accent it reads on (3:1, a bold label), else a dark ink: pastel accents (Dracula's purple,
        // Catppuccin's mauve, SynthWave's pink) get dark labels
        let on_accent = if accent.contrast(Color::WHITE) >= 3.0 || accent.contrast(Color::WHITE) >= accent.contrast(Color::hex(0x0c0d10)) { Color::WHITE } else { Color::hex(0x0c0d10) };
        Tokens {
            light,
            bg,
            surface,
            window: bg,
            raised,
            sunken: if light { bg.mix(text, 0.04) } else { bg.mix(Color::BLACK, 0.25) },
            hover: text.with_alpha(if light { 0.055 } else { 0.06 }),
            selected: accent.with_alpha(if light { 0.13 } else { 0.16 }),
            control: if light { Color::WHITE.with_alpha(0.7) } else { text.with_alpha(0.061) },
            control_hover: if light { Color::hex(0xf9f9f9).with_alpha(0.5) } else { text.with_alpha(0.084) },
            control_pressed: if light { Color::hex(0xf9f9f9).with_alpha(0.3) } else { text.with_alpha(0.033) },
            stroke: if light { Color::rgba(0.0, 0.0, 0.0, 0.08) } else { text.with_alpha(0.08) },
            field_focus: if light { Color::WHITE } else { bg.mix(Color::BLACK, 0.3) },
            // see-through, so a theme's backdrop (stars, rain) still shows between the cards
            layer: if light { Color::WHITE.with_alpha(0.55) } else { text.with_alpha(0.028) },
            hairline: text.with_alpha(if light { 0.09 } else { 0.075 }),
            border: text.with_alpha(if light { 0.16 } else { 0.13 }),
            text,
            // WinUI's secondary and tertiary text: dim enough to rank, bright enough to read at caption sizes
            muted: text.mix(bg, 0.3),
            faint: text.mix(bg, 0.5),
            accent,
            on_accent,
            shine: c(t.shine),
            inline: c(t.inline),
            good: c(t.good),
            danger: c(t.danger),
            gold: color(crate::theme::gold(t), Color::hex(0xffc83d), &ansi),
            warn: if light { Color::hex(0xb45309) } else { Color::hex(0xffb86c) },
            scrim: Color::rgba(0.0, 0.0, 0.0, if light { 0.22 } else { 0.48 }),
            shadow: Color::rgba(0.0, 0.0, 0.0, if light { 0.16 } else { 0.5 }),
            term_fg: text,
            term_bg: surface,
            ansi,
            cursor: t.cursor.map(c).unwrap_or(accent.mix(text, 0.35)),
            selection: accent.with_alpha(if light { 0.28 } else { 0.34 }),
        }
    }

    /// Over a see-through window: the window at `alpha`, and with it every glass surface. Everything else, text,
    /// controls, menus and dialogs, stays solid so it reads over anything.
    pub fn with_material(mut self, alpha: f32) -> Tokens {
        self.window = self.bg.with_alpha(alpha.clamp(0.0, 1.0));
        self
    }

    /// A project's colour (the same one the terminal UI uses).
    pub fn project(&self, key: &str, t: &crate::theme::Theme) -> Color {
        color(crate::theme::project_color(key, t), self.accent, &self.ansi)
    }

    /// Text that reads on a solid `fill`: whichever of a dark ink or white contrasts more (WCAG), as the terminal
    /// UI's `ui::ink_on` picks, so a light red or yellow fill gets dark text. The dark ink is the theme's own
    /// background on dark themes (the terminal UI's `deep()`), a near-black on light ones.
    pub fn ink_on(&self, fill: Color) -> Color {
        let fill = fill.with_alpha(1.0);
        let dark = if self.light { Color::hex(0x121216) } else { self.bg.with_alpha(1.0) };
        if fill.contrast(dark) >= fill.contrast(Color::WHITE) { dark } else { Color::WHITE }
    }
}

/// Type scale (logical pixels).
pub mod size {
    pub const CAPTION: f32 = 11.0;
    pub const SMALL: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const TITLE: f32 = 14.0;
    pub const HEADING: f32 = 16.0;
    pub const DISPLAY: f32 = 26.0;
}

/// Corner radii (logical px), WinUI's: controls and list rows 4, floating things and cards 8.
pub mod radius {
    pub const CONTROL: f32 = 4.0;
    pub const OVERLAY: f32 = 8.0;
}

/// Font weights.
pub mod weight {
    pub const REGULAR: u16 = 400;
    pub const MEDIUM: u16 = 500;
    pub const SEMIBOLD: u16 = 600;
    pub const BOLD: u16 = 700;
}

/// Something clickable this frame.
#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub layer: usize,
    pub action: Action,
    pub cursor: Cursor,
}

/// The pointer shape over a hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    Text,
    ResizeH,
    ResizeV,
    Grab,
}

/// Eased values that persist between frames (hover fades, panel slides).
#[derive(Default)]
pub struct Anims {
    values: HashMap<u64, (f32, Instant)>,
    /// something is still moving: draw another frame
    pub moving: bool,
}

impl Anims {
    /// Move the value for `id` toward `target`; `ms` is roughly how long the move takes.
    pub fn ease(&mut self, id: u64, target: f32, ms: f32, now: Instant) -> f32 {
        let e = self.values.entry(id).or_insert((target, now));
        let dt = now.saturating_duration_since(e.1).as_secs_f32().min(0.1);
        e.1 = now;
        let k = 1.0 - (-dt * 4.6 / (ms / 1000.0).max(0.001)).exp();
        e.0 += (target - e.0) * k;
        if (target - e.0).abs() < 0.002 {
            e.0 = target;
        } else {
            self.moving = true;
        }
        e.0
    }
    /// Forget values not touched for a while.
    pub fn sweep(&mut self, now: Instant) {
        self.values.retain(|_, (_, at)| now.saturating_duration_since(*at).as_secs() < 30);
    }
}

/// A stable id for an animation or widget from anything hashable.
pub fn id(v: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Laid-out text with its size in logical pixels.
#[derive(Clone)]
pub struct TextBox {
    pub layout: Arc<Layout>,
    pub w: f32,
    pub h: f32,
}

/// Text style for the helpers.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub size: f32,
    pub weight: u16,
    pub color: Color,
    pub face: Face,
    pub italic: bool,
}

impl Style {
    pub fn new(size: f32, weight: u16, color: Color) -> Style {
        Style { size, weight, color, face: Face::Ui, italic: false }
    }
    pub fn mono(size: f32, color: Color) -> Style {
        Style { size, weight: 400, color, face: Face::Mono, italic: false }
    }
    pub fn color(mut self, c: Color) -> Style {
        self.color = c;
        self
    }
    pub fn weight(mut self, w: u16) -> Style {
        self.weight = w;
        self
    }
}

/// Everything a surface needs while drawing one frame. Coordinates are logical pixels.
pub struct Ui<'a> {
    pub scene: &'a mut Scene,
    pub text: &'a mut Text,
    pub t: &'a Tokens,
    pub theme: &'a crate::theme::Theme,
    pub scale: f32,
    /// the pointer, when it's over the window
    pub pointer: Option<Point>,
    /// a mouse button is held (pressed looks)
    pub pressing: bool,
    pub hits: &'a mut Vec<Hit>,
    /// the highest layer with a hit under the pointer last frame (hover is only shown at or above it)
    pub top_layer: usize,
    pub anim: &'a mut Anims,
    pub now: Instant,
    /// seconds since the app started (spinners)
    pub time: f64,
    /// something on screen animates by itself (spinners): draw again soon
    pub ticking: bool,
    /// the window, logical px (tooltips and popovers stay inside it)
    pub screen: Rect,
}

impl<'a> Ui<'a> {
    // ------------------------------------------------------------------ geometry

    /// Logical → physical, snapped to whole pixels.
    pub fn px(&self, r: Rect) -> Rect {
        r.scale(self.scale).snap()
    }
    /// One physical pixel in logical units (hairlines).
    pub fn hair(&self) -> f32 {
        1.0 / self.scale
    }

    // ------------------------------------------------------------------ layers, clips, hits

    /// Draw above everything so far (menus, dialogs, toasts). Returns the layer to go back to.
    pub fn layer(&mut self) -> usize {
        self.scene.push_layer()
    }
    pub fn set_layer(&mut self, l: usize) {
        self.scene.set_layer(l);
    }
    pub fn clip(&mut self, r: Rect, radius: f32) {
        let p = self.px(r);
        self.scene.push_clip(p, radius * self.scale);
    }
    pub fn unclip(&mut self) {
        self.scene.pop_clip();
    }

    /// Make `r` clickable with `action` (topmost wins).
    pub fn hit(&mut self, r: Rect, action: Action) {
        self.hit_with(r, action, Cursor::Pointer);
    }
    pub fn hit_with(&mut self, r: Rect, action: Action, cursor: Cursor) {
        let clip = self.scene.clip().rect.scale(1.0 / self.scale);
        let r = r.intersect(&clip);
        if !r.is_empty() {
            self.hits.push(Hit { rect: r, layer: self.scene.layer_index(), action, cursor });
        }
    }
    /// Swallow clicks over `r` (panels, scrims) without doing anything.
    pub fn block(&mut self, r: Rect) {
        self.hit_with(r, Action::None, Cursor::Default);
    }

    /// The pointer is over `r` and nothing drawn on a higher layer covers it.
    pub fn hovered(&self, r: Rect) -> bool {
        let clip = self.scene.clip().rect.scale(1.0 / self.scale);
        self.pointer.is_some_and(|p| r.contains(p) && clip.contains(p)) && self.scene.layer_index() >= self.top_layer
    }

    /// 0..1 hover amount for `key` over `r`, eased.
    pub fn hover_amount(&mut self, key: impl std::hash::Hash, r: Rect) -> f32 {
        let on = self.hovered(r);
        self.anim.ease(id(("hover", key)), if on { 1.0 } else { 0.0 }, 110.0, self.now)
    }

    /// An eased value.
    pub fn ease(&mut self, key: impl std::hash::Hash, target: f32, ms: f32) -> f32 {
        self.anim.ease(id(key), target, ms, self.now)
    }

    // ------------------------------------------------------------------ shapes

    pub fn fill(&mut self, r: Rect, c: Color) {
        let p = self.px(r);
        self.scene.rect(p, c);
    }
    pub fn round(&mut self, r: Rect, radius: f32, c: Color) {
        let p = self.px(r);
        self.scene.quad(Quad::new(p, c).radius(radius * self.scale));
    }
    pub fn round_corners(&mut self, r: Rect, corners: Corners, c: Color) {
        let p = self.px(r);
        self.scene.quad(Quad::new(p, c).corners(corners.scale(self.scale)));
    }
    /// A surface of the window itself (its content area, a session card, a panel on one): its colour painted into
    /// the window without making it any less see-through, so every surface over a see-through window stays as
    /// see-through as the window. Over a solid window it's an ordinary fill. Drawn before the rest of the layer.
    pub fn glass(&mut self, r: Rect, corners: Corners, c: Color) {
        let p = self.px(r);
        self.scene.glass_quad(Quad::new(p, c).corners(corners.scale(self.scale)));
    }
    pub fn glass_round(&mut self, r: Rect, radius: f32, c: Color) {
        self.glass(r, Corners::all(radius), c);
    }
    /// [`Ui::panel`] as a surface of the window: a glass fill, an ordinary hairline border.
    pub fn glass_panel(&mut self, r: Rect, radius: f32, fill: Color, border: Color) {
        self.glass_round(r, radius, fill);
        // the same whole-pixel edge as `panel`
        self.outline(r, radius, self.scale.round().max(1.0) / self.scale, border);
    }

    /// A rounded rectangle with a hairline border.
    pub fn panel(&mut self, r: Rect, radius: f32, fill: Color, border: Color) {
        let p = self.px(r);
        let w = self.scale.round().max(1.0);
        self.scene.quad(Quad::new(p, fill).radius(radius * self.scale).border(w, border));
    }
    pub fn outline(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        let p = self.px(r);
        self.scene.quad(Quad::new(p, Color::TRANSPARENT).radius(radius * self.scale).border((width * self.scale).max(1.0), c));
    }
    pub fn shadow(&mut self, r: Rect, radius: f32, blur: f32, dy: f32, c: Color) {
        let p = self.px(r.translate(0.0, dy));
        self.scene.shadow(p, Corners::all(radius * self.scale), blur * self.scale, c);
    }
    /// Over the pictures drawn in this layer, under its text: controls on a video, badges on a thumbnail.
    pub fn round_over(&mut self, r: Rect, radius: f32, c: Color) {
        let p = self.px(r);
        self.scene.mid_quad(Quad::new(p, c).radius(radius * self.scale));
    }
    pub fn outline_over(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        let p = self.px(r);
        self.scene.mid_quad(Quad::new(p, Color::TRANSPARENT).radius(radius * self.scale).border((width * self.scale).max(1.0), c));
    }
    pub fn circle_over(&mut self, center: Point, radius: f32, c: Color) {
        self.round_over(Rect::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0), radius, c);
    }
    /// Over text in this layer (cursor beams, underlines, scrollbars).
    pub fn top_fill(&mut self, r: Rect, radius: f32, c: Color) {
        let p = self.px(r);
        self.scene.top_quad(Quad::new(p, c).radius(radius * self.scale));
    }
    pub fn circle(&mut self, center: Point, radius: f32, c: Color) {
        self.round(Rect::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0), radius, c);
    }
    pub fn ring(&mut self, center: Point, radius: f32, width: f32, c: Color) {
        self.outline(Rect::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0), radius, width, c);
    }
    /// A horizontal hairline.
    pub fn hline(&mut self, x: f32, y: f32, w: f32, c: Color) {
        let p = Rect::new((x * self.scale).round(), (y * self.scale).round(), (w * self.scale).round(), self.scale.round().max(1.0));
        self.scene.rect(p, c);
    }
    pub fn vline(&mut self, x: f32, y: f32, h: f32, c: Color) {
        let p = Rect::new((x * self.scale).round(), (y * self.scale).round(), self.scale.round().max(1.0), (h * self.scale).round());
        self.scene.rect(p, c);
    }
    /// An image (a texture from the renderer) in `r`, `uv` the part of it to show (0..1), rounded corners.
    pub fn image(&mut self, id: bro_gfx::ImageId, r: Rect, uv: [f32; 4], radius: f32, opacity: f32) {
        let p = r.scale(self.scale);
        self.scene.image(id, p, uv, radius * self.scale, opacity);
    }
    /// A spinning arc (working agents): `radius` and stroke in logical pixels.
    pub fn spinner(&mut self, center: Point, radius: f32, stroke: f32, c: Color) {
        self.ticking = true;
        let a = (self.time * 5.5) as f32;
        // eight dots fading around the circle: crisp at any size
        for i in 0..8 {
            let ang = a + i as f32 * std::f32::consts::TAU / 8.0;
            let alpha = 0.18 + 0.82 * (i as f32 / 7.0);
            let p = Point::new(center.x + ang.cos() * radius, center.y + ang.sin() * radius);
            self.circle(p, stroke * 0.62, c.fade(alpha));
        }
    }
    /// A softly pulsing dot (waiting on you).
    pub fn pulse(&mut self, center: Point, radius: f32, c: Color) {
        self.ticking = true;
        let k = ((self.time * 3.2).sin() * 0.5 + 0.5) as f32;
        self.circle(center, radius + 2.6 * k, c.fade(0.22 * (1.0 - k)));
        self.circle(center, radius, c);
    }

    // ------------------------------------------------------------------ text

    /// Lay out one line of `text` (cached), cut with an ellipsis at `max_w`. Line breaks in it read as spaces: a label
    /// stays on its line whatever it's given (a skill's description can run over several).
    pub fn line(&mut self, text: &str, st: Style, max_w: Option<f32>) -> TextBox {
        let mut o = TextOpts::new(st.size * self.scale);
        o.line_height = (st.size * 1.36 * self.scale).round();
        o.max_width = max_w.map(|w| (w * self.scale).max(1.0));
        o.ellipsis = max_w.is_some();
        let text = if text.contains(['\n', '\r']) { text.split_whitespace().collect::<Vec<_>>().join(" ") } else { text.to_string() };
        let span = Span { text, face: st.face, weight: st.weight, italic: st.italic, color: st.color, size: None };
        let layout = self.text.layout(&[span], &o);
        TextBox { w: layout.width() / self.scale, h: layout.height() / self.scale, layout }
    }

    /// Lay out styled runs on one line or wrapped at `max_w`.
    pub fn rich(&mut self, spans: &[(String, Style)], size: f32, max_w: Option<f32>, wrap: bool, max_lines: Option<usize>) -> TextBox {
        let mut o = TextOpts::new(size * self.scale);
        o.line_height = (size * 1.42 * self.scale).round();
        o.max_width = max_w.map(|w| (w * self.scale).max(1.0));
        o.wrap = wrap;
        o.ellipsis = max_w.is_some();
        o.max_lines = max_lines;
        let spans: Vec<Span> = spans
            .iter()
            .map(|(s, st)| Span { text: s.clone(), face: st.face, weight: st.weight, italic: st.italic, color: st.color, size: (st.size != size).then_some(st.size * self.scale) })
            .collect();
        let layout = self.text.layout(&spans, &o);
        TextBox { w: layout.width() / self.scale, h: layout.height() / self.scale, layout }
    }

    /// Wrapped paragraph.
    pub fn para(&mut self, text: &str, st: Style, max_w: f32, max_lines: Option<usize>) -> TextBox {
        self.rich(&[(text.to_string(), st)], st.size, Some(max_w), true, max_lines)
    }

    /// Draw laid-out text with its top-left at `p`.
    pub fn draw(&mut self, b: &TextBox, p: Point) {
        self.draw_faded(b, p, 1.0);
    }
    pub fn draw_faded(&mut self, b: &TextBox, p: Point, alpha: f32) {
        let o = Point::new(p.x * self.scale, p.y * self.scale);
        self.text.draw(self.scene, &b.layout, o, None, alpha);
    }
    /// Draw with every run in `c`.
    #[allow(dead_code)]
    pub fn draw_color(&mut self, b: &TextBox, p: Point, c: Color) {
        let o = Point::new(p.x * self.scale, p.y * self.scale);
        let colors = vec![c; b.layout.colors.len().max(1)];
        self.text.draw(self.scene, &b.layout, o, Some(&colors), 1.0);
    }

    /// One line at `p` (top-left); returns its size.
    pub fn label(&mut self, p: Point, text: &str, st: Style) -> TextBox {
        let b = self.line(text, st, None);
        self.draw(&b, p);
        b
    }
    /// One line inside `r`: vertically centred, aligned, cut to fit.
    pub fn label_in(&mut self, r: Rect, text: &str, st: Style, align: Align) -> TextBox {
        let b = self.line(text, st, Some(r.w.max(1.0)));
        let x = match align {
            Align::Left => r.x,
            Align::Center => r.x + (r.w - b.w) / 2.0,
            Align::Right => r.right() - b.w,
        };
        let y = r.y + (r.h - b.h) / 2.0;
        self.draw(&b, Point::new(x, y));
        b
    }
    pub fn measure(&mut self, text: &str, st: Style) -> f32 {
        self.line(text, st, None).w
    }

    /// A Nerd Font icon centred in `r`.
    pub fn icon(&mut self, r: Rect, glyph: &str, size: f32, c: Color) {
        let st = Style { size, weight: 400, color: c, face: Face::Icons, italic: false };
        let b = self.line(glyph, st, None);
        self.draw(&b, Point::new(r.x + (r.w - b.w) / 2.0, r.y + (r.h - b.h) / 2.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_on_picks_the_higher_contrast_like_the_terminal_ui() {
        // graphite's danger red is light: dark text reads on it (white was 2.7:1)
        let dark = Tokens::from_theme(&crate::theme::get("graphite"));
        let ink = dark.ink_on(Color::hex(0xef7b7b));
        assert_eq!(ink, dark.bg, "dark ink is the theme's background on dark themes");
        assert!(Color::hex(0xef7b7b).contrast(ink) >= 4.5);
        assert_eq!(dark.ink_on(Color::hex(0x1e2878)), Color::WHITE, "white on a deep colour");
        assert_eq!(dark.ink_on(dark.gold), dark.bg, "dark text on gold");
        // a light theme: dark text on a yellow band, white on a deep one (as `ui::ink_on`'s test)
        let light = Tokens::from_theme(&crate::theme::get("catppuccin-latte"));
        assert_ne!(light.ink_on(Color::hex(0xdf8e1d)), Color::WHITE);
        assert_eq!(light.ink_on(Color::hex(0x1e2878)), Color::WHITE);
        // whatever the theme, the pick is never the worse of the two
        for name in crate::theme::builtin_names() {
            let t = Tokens::from_theme(&crate::theme::get(&name));
            // a button's label reads on the accent: white from 3:1, else whichever reads better
            let (white, dark) = (t.accent.contrast(Color::WHITE), t.accent.contrast(Color::hex(0x0c0d10)));
            assert!(t.on_accent.contrast(t.accent) >= 3.0 || t.on_accent.contrast(t.accent) >= white.min(dark), "{name}: label on the accent");
            if white < 3.0 && dark > white {
                assert_ne!(t.on_accent, Color::WHITE, "{name}: a pastel accent gets a dark label");
            }
            for fill in [t.danger, t.good, t.gold, t.accent, t.inline, t.shine] {
                let ink = t.ink_on(fill);
                let other = if ink == Color::WHITE { if t.light { Color::hex(0x121216) } else { t.bg } } else { Color::WHITE };
                assert!(fill.contrast(ink) >= fill.contrast(other), "{name}: ink on {fill:?}");
            }
        }
    }
}
