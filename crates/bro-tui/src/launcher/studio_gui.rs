//! The Create Slop tab in the desktop app ([`super::studio`] is its state and keys).
//!
//! On the left the cast: a field to find a character by, who is cast so far as chips, and the roster as portrait
//! cards (Claudia first, then SLOPCANNON's). A card's picture is cropped round the face; its name sits on a shade
//! at the bottom, and once cast it carries a ring and a check in Create Slop's pink. On the right the toolkit:
//! every media tool under its own mark, with what this computer has for it said by an icon and a few words. A
//! tool that is ready has a switch; one that isn't has the button that gets it going (Install, Start, Add key),
//! and the key is typed into a field in its row. The selected tool opens to show how work with it goes. Under
//! the toolkit, where the session runs. The button bottom-right starts it.
//!
//! Narrow pages put the tools and where it runs in two lines under the cast.
//!
//! Portraits come from the window ([`crate::app::gui::cast`]): this draws the ones in `Studio::pictures` and
//! says which it wanted in `Studio::wanted`.

use super::gui::{self as page, FIELD_H, FOOTER_H, I_ALERT, I_CHECK, I_CLOSE, I_SEARCH};
use super::studio::{Picture, StudioFocus};
use super::{Item, Launcher, Outcome, SessionTab};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand;
use crate::app::gui::kit::motion::Spring;
use crate::app::gui::kit::{Cursor, Style, Tokens, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use bro_core::Harness;
use bro_core::studio::{self as tools, Character, Fix, Ready, Source, TOOLS};
use bro_gfx::text::Align;
use bro_gfx::{Color, Corners, Fill, ImageId, Point, Quad, Rect};

// ------------------------------------------------------------------ targets (inside the launcher's `STUDIO` block)

pub(crate) const FIELD: usize = 1;
const REFRESH: usize = 2;
const RUNS_ON: usize = 3;
/// the cast grid's background (the wheel scrolls it)
const GRID: usize = 4;
const MAIN: usize = 5;
const INSTALL: usize = 6;
/// the key field, and its Save and Cancel
const KEY_FIELD: usize = 7;
const KEY_SAVE: usize = 8;
const KEY_CANCEL: usize = 9;
/// + the tool's place in the toolkit: its row (selects it)
const TOOL: usize = 100;
/// + the tool's place in the toolkit: its switch
const SWITCH: usize = 200;
/// + the tool's place in the toolkit: the button that gets it going (Install, Start, Add key)
const FIX: usize = 300;
/// + the tool's place in the toolkit: "Add key" on a tool that works without one
const ADD_KEY: usize = 400;
/// + the place among those cast: the × on a chip
const UNCAST: usize = 1_000;
/// + the place in the filtered cast: a card
const CARD: usize = 10_000;

// ------------------------------------------------------------------ metrics

/// Below this the toolkit goes under the cast (the New tab's own breakpoints).
const SIDE_MIN_W: f32 = 860.0;
const SIDE_MIN_H: f32 = 380.0;
/// The narrowest a card gets, and the room between cards.
const CARD_MIN: f32 = 148.0;
const GAP: f32 = 14.0;
/// A card's height over its width: a portrait.
const CARD_ASPECT: f32 = 1.34;
const CARD_RADIUS: f32 = 8.0;
/// Room round the grid inside its clip, for the keyboard's ring.
const GRID_PAD: f32 = 5.0;
/// A notch of the wheel.
const WHEEL: f32 = 110.0;
const CHIP_H: f32 = 32.0;
/// A tool's row; open, it also holds what the tool makes (a line or two), its three steps, and its key.
const TOOL_H: f32 = 52.0;
const STEP_H: f32 = 19.0;
/// Where a tool's text starts: past its mark.
const TOOL_TEXT_X: f32 = 46.0;
/// The key field's block in an open row: the field with its buttons, and the line under it.
const KEY_H: f32 = 32.0 + 6.0 + 18.0;
/// "Add key" in an open row of a tool that works without one.
const ADD_KEY_H: f32 = 28.0;
/// Where it runs: heading, agents, the login, permissions.
const RUNS_H: f32 = 142.0;

const I_REFRESH: &str = "\u{F0450}";
const I_KEY: &str = "\u{F0306}";
const I_DOWNLOAD: &str = "\u{F01DA}";
const I_PLAY: &str = "\u{F040A}";
const I_SIGN_IN: &str = "\u{F0342}";

/// Create Slop's colour: a hot pink, deeper on light themes so it reads on white.
pub(crate) fn tone(t: &Tokens) -> Color {
    Color::hex(if t.light { 0xD6247A } else { 0xFF5FA8 })
}

// ------------------------------------------------------------------ clicks and the wheel

/// Target `k` of the tab was clicked.
pub(crate) fn click(l: &mut Launcher, k: usize) -> Outcome {
    if let Some(i) = k.checked_sub(CARD) {
        let cast = l.studio_cast();
        if let Some(c) = cast.get(i).and_then(|&n| l.data.studio.cast.get(n)).cloned() {
            l.studio.focus = StudioFocus::Cast;
            l.studio.sel = i;
            l.studio_toggle(c);
        }
        return Outcome::None;
    }
    if let Some(i) = k.checked_sub(UNCAST) {
        if let Some(c) = l.studio.picked.get(i).cloned() {
            l.studio_toggle(c);
        }
        return Outcome::None;
    }
    if let Some(i) = k.checked_sub(ADD_KEY).filter(|i| *i < TOOLS.len()) {
        l.studio_open_key(i);
        return Outcome::None;
    }
    if let Some(i) = k.checked_sub(FIX).filter(|i| *i < TOOLS.len()) {
        l.studio.key = None;
        return l.studio_act(i);
    }
    if let Some(tool) = k.checked_sub(SWITCH).and_then(|i| TOOLS.get(i)) {
        l.studio.tool_sel = k - SWITCH;
        l.studio_toggle_tool(tool.id);
        return Outcome::None;
    }
    if let Some(i) = k.checked_sub(TOOL).filter(|i| *i < TOOLS.len()) {
        l.studio.focus = StudioFocus::Tools;
        // another tool: the key field closes with the row it was in
        if l.studio.tool_sel != i {
            l.studio.key = None;
        }
        l.studio.tool_sel = i;
        return Outcome::None;
    }
    match k {
        KEY_FIELD => {}
        KEY_SAVE => return l.studio_save_key(),
        KEY_CANCEL => l.studio.key = None,
        FIELD | GRID => {
            l.studio.focus = StudioFocus::Cast;
            l.studio.key = None;
        }
        REFRESH => l.studio.ask = Some(true),
        RUNS_ON => l.set_tab(SessionTab::New),
        MAIN => return l.studio_launch(),
        INSTALL => return Outcome::Install(l.harness),
        _ => {}
    }
    Outcome::None
}

/// The wheel over the tab scrolls the cast (and the cursor stays where it is: nothing pulls the view back).
pub(crate) fn wheel(l: &mut Launcher, down: bool) {
    l.studio.scroll = (l.studio.scroll + if down { WHEEL } else { -WHEEL }).max(0.0);
    l.studio.reveal = false;
}

// ------------------------------------------------------------------ the tab

pub(crate) fn draw(l: &mut Launcher, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    let footer = Rect::new(r.x, r.bottom() - FOOTER_H, r.w, FOOTER_H);
    let body = Rect::new(r.x, r.y, r.w, (footer.y - 20.0 - r.y).max(0.0));
    if body.w >= SIDE_MIN_W && body.h >= SIDE_MIN_H {
        let side_w = (body.w * 0.34).clamp(320.0, 400.0);
        let gap = 40.0;
        let cast = Rect::new(body.x, body.y, body.w - side_w - gap, body.h);
        let side = Rect::new(cast.right() + gap, body.y, side_w, body.h);
        ui.vline(cast.right() + gap / 2.0, body.y, body.h, t.hairline);
        draw_cast(l, ui, cast, cx);
        let runs = Rect::new(side.x, side.bottom() - RUNS_H, side.w, RUNS_H);
        draw_tools(l, ui, Rect::new(side.x, side.y, side.w, (runs.y - 16.0 - side.y).max(0.0)), cx);
        draw_runs_on(l, ui, runs);
    } else {
        // the tools as pills, then where it runs
        let under = 30.0 + 10.0 + 28.0;
        let cast = Rect::new(body.x, body.y, body.w, (body.h - under - 14.0).max(0.0));
        draw_cast(l, ui, cast, cx);
        let pills = Rect::new(body.x, cast.bottom() + 14.0, body.w, 30.0);
        draw_tool_pills(l, ui, pills);
        draw_runs_line(l, ui, Rect::new(body.x, pills.bottom() + 10.0, body.w, 28.0));
    }
    draw_footer(l, ui, footer);
}

// ------------------------------------------------------------------ the cast

/// The field, the roster's state, who is cast, and the cards.
fn draw_cast(l: &mut Launcher, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    let focused = l.studio.focus == StudioFocus::Cast;
    let (loading, failed, total) = (l.data.studio.loading, l.data.studio.error.is_some(), l.data.studio.cast.len());
    // on the right: how many there are (or why not), and the button that fetches them again
    let refresh = Rect::new(r.right() - FIELD_H, r.y, FIELD_H, FIELD_H);
    let state = if loading {
        "Fetching the roster".to_string()
    } else if failed {
        "character.slop.cc didn't answer".to_string()
    } else if !l.studio.filter.is_empty() {
        format!("{} of {total}", l.studio_cast().len())
    } else if total == 1 {
        "1 character".to_string()
    } else {
        format!("{total} characters")
    };
    let ss = Style::new(size::SMALL, weight::REGULAR, if failed && !loading { t.warn } else { t.muted });
    let sw = ui.measure(&state, ss);
    let mut field_w = (r.w - FIELD_H - 12.0).clamp(120.0, 420.0);
    if r.w - field_w - FIELD_H - 24.0 >= sw {
        ui.label_in(Rect::new(refresh.x - 10.0 - sw, r.y, sw + 2.0, FIELD_H), &state, ss, Align::Left);
    } else {
        field_w = (r.w - FIELD_H - 10.0).max(80.0);
    }
    let typing = focused && l.studio.key.is_none();
    page::input(ui, Rect::new(r.x, r.y, field_w, FIELD_H), I_SEARCH, &l.studio.filter, "Find a character", typing, cx.focused, false, Action::Overlay(page::STUDIO + FIELD));
    if loading {
        ui.spinner(refresh.center(), 6.0, 2.0, t.muted);
    } else if widgets::icon_button(ui, refresh, I_REFRESH, t.muted, Action::Overlay(page::STUDIO + REFRESH)) {
        widgets::tooltip(ui, refresh, "Fetch the roster again · ctrl+r");
    }
    let mut y = r.y + FIELD_H + 12.0;
    if !l.studio.picked.is_empty() && r.h >= 260.0 {
        draw_picked(l, ui, Rect::new(r.x, y, r.w, CHIP_H));
        y += CHIP_H + 12.0;
    }
    draw_grid(l, ui, Rect::new(r.x, y, r.w, (r.bottom() - y).max(0.0)), cx);
}

/// Who is cast, in the order chosen: a face, a name and an × each. What doesn't fit is counted.
fn draw_picked(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let pink = tone(t);
    let ns = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let more = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let mut x = r.x;
    let n = l.studio.picked.len();
    for (i, c) in l.studio.picked.iter().enumerate() {
        let nw = ui.measure(&c.name, ns).min(150.0);
        let w = 4.0 + 24.0 + 8.0 + nw + 4.0 + 22.0 + 4.0;
        // leave room to say how many more there are
        let left = n - i;
        let rest = if left > 1 { ui.measure(&format!("+{}", left - 1), more) + 10.0 } else { 0.0 };
        if x + w + rest > r.right() && i > 0 {
            ui.label_in(Rect::new(x, r.y, rest + 20.0, r.h), &format!("+{left}"), more, Align::Left);
            return;
        }
        let chip = Rect::new(x, r.y, w.min(r.right() - x), r.h);
        ui.panel(chip, r.h / 2.0, pink.with_alpha(0.12), pink.with_alpha(0.55));
        let face = Rect::new(chip.x + 4.0, chip.y + 4.0, 24.0, 24.0);
        match l.studio.pictures.get(&c.key()) {
            Some(p) => ui.image(ImageId(p.image), face, cover_uv(p, 1.0, c.focus, 0.5), 12.0, 1.0),
            None => {
                ui.round(face, 12.0, tile_colors(&c.name).0);
                ui.label_in(face, &initials(&c.name, 1), Style::new(size::CAPTION, weight::BOLD, Color::WHITE), Align::Center);
            }
        }
        let name = ui.line(&c.name, ns, Some(nw + 1.0));
        ui.draw(&name, Point::new(face.right() + 8.0, chip.y + (chip.h - name.h) / 2.0));
        let close = Rect::new(chip.right() - 26.0, chip.y + 5.0, 22.0, 22.0);
        if widgets::icon_button(ui, close, I_CLOSE, t.muted, Action::Overlay(page::STUDIO + UNCAST + i)) {
            widgets::tooltip(ui, close, &format!("Take {} out of the cast", c.name));
        }
        x += w + 8.0;
    }
}

/// The roster as cards, scrolled by the wheel and by the keyboard's cursor.
fn draw_grid(l: &mut Launcher, ui: &mut Ui, view: Rect, cx: &Cx) {
    let t = ui.t;
    let pink = tone(t);
    ui.hit_with(view, Action::Overlay(page::STUDIO + GRID), Cursor::Default);
    let cast = l.studio_cast();
    // the scrollbar's lane stays clear of the cards
    let inner_w = (view.w - 2.0 * GRID_PAD - 10.0).max(CARD_MIN);
    let cols = (((inner_w + GAP) / (CARD_MIN + GAP)).floor() as usize).max(2);
    let cw = ((inner_w - GAP * (cols - 1) as f32) / cols as f32).floor();
    let ch = (cw * CARD_ASPECT).round();
    l.studio.cols = cols;
    if cast.is_empty() {
        l.studio.wanted.clear();
        let (head, sub) = if l.data.studio.cast.is_empty() {
            ("The roster is on its way".to_string(), "Characters come from character.slop.cc and claudia.gallery.")
        } else {
            (format!("No character matches \u{201c}{}\u{201d}", l.studio.filter), "Esc clears the search.")
        };
        let mid = view.y + (view.h * 0.36).min(120.0);
        ui.label_in(Rect::new(view.x, mid, view.w, 24.0), &head, Style::new(size::HEADING, weight::SEMIBOLD, t.text), Align::Center);
        ui.label_in(Rect::new(view.x, mid + 28.0, view.w, 20.0), sub, Style::new(size::BODY, weight::REGULAR, t.muted), Align::Center);
        return;
    }
    let pitch = ch + GAP;
    let rows = cast.len().div_ceil(cols);
    let content = rows as f32 * pitch - GAP + 2.0 * GRID_PAD;
    let max = (content - view.h).max(0.0);
    // a key moved the cursor: its row comes into view
    if std::mem::take(&mut l.studio.reveal) {
        let top = (l.studio.sel / cols) as f32 * pitch;
        if top < l.studio.scroll {
            l.studio.scroll = top;
        } else if top + ch + 2.0 * GRID_PAD > l.studio.scroll + view.h {
            l.studio.scroll = top + ch + 2.0 * GRID_PAD - view.h;
        }
    }
    l.studio.scroll = l.studio.scroll.clamp(0.0, max);
    let off = ui.ease("launcher-slop-scroll", l.studio.scroll, 150.0).clamp(0.0, max);
    let cursor = (l.studio.focus == StudioFocus::Cast && cx.focused).then_some(l.studio.sel);
    let mut wanted = vec![];
    // what the card under the pointer says, shown once the grid's clip is gone
    let mut tip = None;
    ui.clip(view, 0.0);
    for (k, &i) in cast.iter().enumerate() {
        let card = Rect::new(view.x + GRID_PAD + (k % cols) as f32 * (cw + GAP), view.y + GRID_PAD + (k / cols) as f32 * pitch - off, cw, ch);
        // a row past each edge is fetched ahead, so scrolling meets pictures
        if card.bottom() < view.y - pitch || card.y > view.bottom() + pitch {
            continue;
        }
        let c = &l.data.studio.cast[i];
        let key = c.key();
        if let Some(url) = &c.portrait {
            wanted.push((key.clone(), url.clone()));
        }
        if card.bottom() < view.y || card.y > view.bottom() {
            continue;
        }
        let picked = l.studio.picked.iter().any(|p| p.key() == key);
        draw_card(ui, card, c, &key, l.studio.pictures.get(&key), picked, cursor == Some(k), pink, Action::Overlay(page::STUDIO + CARD + k));
        if ui.hovered(card) && !c.tagline.is_empty() {
            tip = Some((card.intersect(&view), c.tagline.clone()));
        }
    }
    ui.unclip();
    if let Some((card, tagline)) = tip {
        // over the top row a tooltip would cover who is cast and the search: it goes under the card there
        if card.y - view.y < 40.0 {
            widgets::tooltip_under(ui, card, &tagline, 300.0);
        } else {
            widgets::tooltip_wrapped(ui, card, &tagline, 300.0);
        }
    }
    l.studio.wanted = wanted;
    let vis = 0.5 + 0.5 * ui.hover_amount("launcher-slop-bar", view);
    widgets::scrollbar(ui, view, content, view.h, off, vis);
}

/// One character: the portrait cropped round the face (a tinted tile with initials until it arrives), the name
/// and who made it on a shade, and once cast a ring and a check.
#[allow(clippy::too_many_arguments)]
fn draw_card(ui: &mut Ui, card: Rect, c: &Character, key: &str, pic: Option<&Picture>, picked: bool, cursor: bool, pink: Color, action: Action) {
    let t = ui.t;
    let hot = ui.hover_amount(("launcher-slop-card", key), card);
    if hot > 0.0 {
        ui.shadow(card, CARD_RADIUS, 18.0 * hot, 5.0 * hot, t.shadow.fade(0.9 * hot));
    }
    let (top, bottom) = tile_colors(&c.name);
    let px = ui.px(card);
    ui.scene.quad(Quad::new(px, Fill::Vertical(top, bottom)).radius(CARD_RADIUS * ui.scale));
    match pic {
        Some(p) => {
            // a picture arrives over its tile
            let k = ui.spring(("launcher-slop-pic", key), 1.0, Spring::GLIDE, 0.0).clamp(0.0, 1.0);
            ui.image(ImageId(p.image), card, cover_uv(p, card.w / card.h.max(1.0), c.focus, 0.4), CARD_RADIUS, k);
        }
        None => {
            let st = Style::new((card.w * 0.26).clamp(26.0, 44.0), weight::BOLD, Color::WHITE.with_alpha(0.5));
            ui.label_in(Rect::new(card.x, card.y, card.w, card.h * 0.66), &initials(&c.name, 2), st, Align::Center);
        }
    }
    // the name reads on any picture: a shade rising from the bottom edge
    let shade = Rect::new(card.x, card.y + card.h * 0.44, card.w, card.h * 0.56);
    let sp = ui.px(shade);
    ui.scene.mid_quad(Quad::new(sp, Fill::Vertical(Color::BLACK.with_alpha(0.0), Color::BLACK.with_alpha(0.84))).corners(Corners::bottom(CARD_RADIUS).scale(ui.scale)));
    if hot > 0.0 {
        ui.round_over(card, CARD_RADIUS, Color::WHITE.with_alpha(0.07 * hot));
    }
    let by = match c.source {
        Source::Claudia => c.home().to_string(),
        Source::Slop if c.creator.is_empty() => c.home().to_string(),
        Source::Slop => format!("by {}", c.creator),
    };
    let bs = Style::new(size::CAPTION, weight::MEDIUM, Color::WHITE.with_alpha(0.74));
    let ns = Style::new(size::TITLE, weight::SEMIBOLD, Color::WHITE);
    let room = card.w - 24.0;
    let bb = ui.line(&by, bs, Some(room));
    let nb = ui.line(&c.name, ns, Some(room));
    let by_y = card.bottom() - 11.0 - bb.h;
    ui.draw(&bb, Point::new(card.x + 12.0, by_y));
    ui.draw(&nb, Point::new(card.x + 12.0, by_y - 1.0 - nb.h));

    // cast: a ring round the card and a check that pops in the corner
    let rest = if picked { 1.0 } else { 0.0 };
    let on = ui.spring(("launcher-slop-cast", key), rest, Spring::POP, rest);
    let at = Point::new(card.right() - 19.0, card.y + 19.0);
    if on > 0.02 {
        ui.outline_over(card, CARD_RADIUS, 2.0, pink.fade(on.clamp(0.0, 1.0)));
        ui.circle_over(at, 11.0 * on.max(0.0), pink);
        ui.icon(Rect::new(at.x - 10.0, at.y - 10.0, 20.0, 20.0), I_CHECK, 13.0 * on.clamp(0.0, 1.15), t.on_bold());
    } else if hot > 0.0 {
        // what a click would do
        ui.outline_over(Rect::new(at.x - 10.0, at.y - 10.0, 20.0, 20.0), 10.0, 1.5, Color::WHITE.with_alpha(0.8 * hot));
    }
    if cursor {
        ui.outline_over(card.outset(3.0), CARD_RADIUS + 3.0, 1.5, t.text.with_alpha(0.72));
    }
    ui.hit(card, action);
}

/// The part of picture `p` that covers a box `aspect` wide for its height, kept round the face at `focus`
/// (the middle top when no face was found); `lift`: how far down the box the face sits (0.5 = centred).
fn cover_uv(p: &Picture, aspect: f32, focus: Option<(f32, f32)>, lift: f32) -> [f32; 4] {
    let ia = p.w.max(1) as f32 / p.h.max(1) as f32;
    let (fx, fy) = focus.unwrap_or((0.5, 0.3));
    if ia > aspect {
        // wider than the box: a slice across
        let w = aspect / ia;
        let u = (fx - w / 2.0).clamp(0.0, 1.0 - w);
        [u, 0.0, u + w, 1.0]
    } else {
        let h = ia / aspect;
        let v = (fy - h * lift).clamp(0.0, 1.0 - h);
        [0.0, v, 1.0, v + h]
    }
}

/// The first letters of a name's first `n` words: "Chinese Bone" → "CB".
fn initials(name: &str, n: usize) -> String {
    name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(n).flat_map(char::to_uppercase).collect()
}

/// A tile's two colours (top, bottom) from a name: the same character is always the same colour.
fn tile_colors(name: &str) -> (Color, Color) {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    (Color::from_hsl(hue, 0.42, 0.44, 1.0), Color::from_hsl((hue + 0.06).fract(), 0.5, 0.2, 1.0))
}

// ------------------------------------------------------------------ the toolkit

/// What a tool's row says about it: the icon and its colour, and the words.
fn tool_state(l: &Launcher, id: &str, t: &Tokens) -> (&'static str, Color, String) {
    let Some(status) = l.studio_tool(id) else { return (I_ALERT, t.faint, String::new()) };
    // a key just handed to bro: said until the toolkit has been looked at again
    if l.studio.saved == Some(id) && status.keyless {
        return (I_CHECK, t.good, "key saved".to_string());
    }
    let note = sentence(&status.note);
    match (status.ready, status.fix) {
        (Ready::Yes, _) => (I_CHECK, t.good, note),
        (Ready::SignIn, _) => (I_SIGN_IN, t.gold, note),
        (Ready::No, Some(Fix::Key)) => (I_KEY, t.warn, note),
        (Ready::No, Some(Fix::Start(_))) => (I_PLAY, t.warn, note),
        (Ready::No, Some(Fix::Install(_))) => (I_DOWNLOAD, t.warn, note),
        (Ready::No, None) => (I_ALERT, t.warn, note),
    }
}

/// "running on port 8793" → "Running on port 8793".
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The button that gets a tool going: its label and what it says when pointed at.
fn fix_label(l: &Launcher, id: &str) -> Option<(String, String)> {
    let name = tools::tool(id).map_or(id, |t| t.name);
    Some(match l.studio_fix(id)? {
        Fix::Install(what) => ("Install".to_string(), format!("Install {} in a tab, the way Quick Install does", what.name())),
        Fix::Start(what) => ("Start".to_string(), format!("Start {} again", what.name())),
        Fix::Key => {
            let spec = tools::key_spec(id)?;
            ("Add key".to_string(), format!("Paste your {name} {}. bro keeps it with your other keys.", spec.label.to_lowercase()))
        }
    })
}

/// A key as the field shows it: dots, with its last four characters so a paste can be checked.
fn masked(key: &str) -> String {
    let n = key.chars().count();
    let tail: String = key.chars().skip(n.saturating_sub(4)).collect();
    if n <= 4 { key.to_string() } else { format!("{}{tail}", "\u{2022}".repeat((n - 4).min(28))) }
}

/// Every tool under its mark, with what this computer has for it and a switch (or the button that gets it
/// going); the selected one opens to how work with it goes, and to its key.
fn draw_tools(l: &Launcher, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    let pink = tone(t);
    // level with the cast's field
    let top = r.y + (FIELD_H - 20.0) / 2.0;
    widgets::section(ui, Rect::new(r.x, top, r.w, 20.0), "Toolkit");
    if !l.studio_ready() {
        ui.spinner(Point::new(r.x + 8.0, top + 46.0), 5.0, 1.8, t.muted);
        ui.label_in(Rect::new(r.x + 24.0, top + 36.0, r.w - 24.0, 20.0), "Looking at what this computer has", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
        return;
    }
    // how much of the kit can be used, as a row of segments and a count
    let usable = TOOLS.iter().filter(|tool| l.studio_tool(tool.id).is_some_and(|s| s.ready != Ready::No)).count();
    let cs = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let count = if usable == TOOLS.len() { "All ready".to_string() } else { format!("{usable} of {} ready", TOOLS.len()) };
    let cw = ui.measure(&count, cs);
    ui.label_in(Rect::new(r.right() - cw - 2.0, top, cw + 2.0, 20.0), &count, cs, Align::Left);
    let seg_w = TOOLS.len() as f32 * 9.0;
    widgets::segments(ui, Rect::new(r.right() - cw - 12.0 - seg_w, top + 7.0, seg_w, 6.0), usable as f32 / TOOLS.len() as f32, TOOLS.len(), t.good);

    let focused = l.studio.focus == StudioFocus::Tools;
    let view = Rect::new(r.x, top + 30.0, r.w, (r.bottom() - top - 30.0).max(0.0));
    // what a tool makes wraps to a second line when the panel is narrow
    let ds = Style::new(size::SMALL, weight::REGULAR, t.text.mix(t.muted, 0.25));
    let text_w = view.w - TOOL_TEXT_X - 14.0;
    let more = |ui: &mut Ui, i: usize| {
        let id = TOOLS[i].id;
        let does = ui.para(TOOLS[i].does, ds, text_w, Some(2));
        let key = if l.studio.key.as_ref().is_some_and(|k| k.tool == id) {
            8.0 + KEY_H
        } else if l.studio_keyless(id) && l.studio_fix(id).is_none() && l.studio.saved != Some(id) {
            6.0 + ADD_KEY_H
        } else {
            0.0
        };
        let extra = does.h + 8.0 + TOOLS[i].steps.len() as f32 * STEP_H + key + 10.0;
        (does, extra)
    };
    // The selected tool opens when the panel has the room for it. In a short window every row stays closed
    // (pointing at one says how it works) and the list follows the cursor. A row whose key is being typed is
    // always open.
    let sel = l.studio.tool_sel.min(TOOLS.len() - 1);
    let closed = TOOLS.len() as f32 * (TOOL_H + 2.0);
    let roomy = view.h >= closed + more(ui, sel).1 || l.studio.key.is_some();
    let off = if roomy {
        // an open row taller than the room under it (its key field): the list moves up just enough to show it whole
        let above = sel as f32 * (TOOL_H + 2.0);
        (above + TOOL_H + more(ui, sel).1 - view.h).clamp(0.0, above)
    } else {
        ((sel + 1) as f32 * (TOOL_H + 2.0) - view.h).clamp(0.0, (closed - view.h).max(0.0))
    };
    let off = ui.ease("launcher-slop-tools-scroll", off, 150.0);
    let mut tip = None;
    ui.clip(view, 0.0);
    let mut y = view.y - off;
    for (i, tool) in TOOLS.iter().enumerate() {
        let state = l.studio_tool(tool.id).map(|s| s.ready).unwrap_or(Ready::No);
        let on = l.studio_tool_on(tool.id);
        let usable = state != Ready::No;
        let open = i == sel && roomy;
        let (does, extra) = more(ui, i);
        let h = ui.ease(("launcher-slop-tool", i), if open { TOOL_H + extra } else { TOOL_H }, 170.0);
        let row = Rect::new(view.x, y, view.w, h);
        y += h + 2.0;
        if row.y > view.bottom() {
            break;
        }
        let hot = ui.hover_amount(("launcher-slop-tool-row", i), row);
        if i == sel {
            ui.round(row, 6.0, if focused { t.selected } else { t.text.with_alpha(0.045) });
        } else if hot > 0.0 {
            ui.round(row, 6.0, t.hover.fade(hot));
        }
        if !roomy && ui.hovered(row) {
            tip = Some((row.intersect(&view), format!("{}. {}, then {}, then {}.", tool.does, tool.steps[0], lower_first(tool.steps[1]), lower_first(tool.steps[2]))));
        }
        ui.hit(row, Action::Overlay(page::STUDIO + TOOL + i));

        // its own mark, standing on the row: in its colour when it can be used, greyed when it can't or is off
        let mark = brand::studio_tool(tool.id);
        ui.mark(mark, Rect::new(row.x + 10.0, row.y + (TOOL_H - 26.0) / 2.0, 26.0, 26.0), 22.0, (!usable || !on).then_some(t.faint));

        // on the right: a switch for a tool that can be used, else the button that gets it going
        let text_x = row.x + TOOL_TEXT_X;
        let mut right = row.right() - 12.0;
        if usable {
            right -= widgets::SWITCH_W;
            widgets::switch(ui, Point::new(right, row.y + (TOOL_H - widgets::SWITCH_H) / 2.0), on, Action::Overlay(page::STUDIO + SWITCH + i));
        } else if let Some((label, about)) = fix_label(l, tool.id) {
            let bw = widgets::button_width(ui, &label);
            right -= bw;
            let button = Rect::new(right, row.y + (TOOL_H - 28.0) / 2.0, bw, 28.0);
            let typing = l.studio.key.as_ref().is_some_and(|k| k.tool == tool.id);
            if !typing && widgets::button(ui, button, &label, widgets::Kind::Secondary, Action::Overlay(page::STUDIO + FIX + i)) {
                tip = Some((button, about));
            }
        }
        let name_w = (right - 12.0 - text_x).max(40.0);
        let ns = Style::new(size::BODY, weight::SEMIBOLD, if usable && on { t.text } else { t.muted });
        let name = ui.line(tool.name, ns, Some(name_w));
        ui.draw(&name, Point::new(text_x, row.y + 9.0));
        // where it stands: an icon and a few words
        let (glyph, ink, note) = tool_state(l, tool.id, t);
        let dim = usable && !on;
        ui.icon(Rect::new(text_x - 1.0, row.y + 28.0, 14.0, 16.0), glyph, 13.0, if dim { t.faint } else { ink });
        let note = if dim { "Switched off for this session".to_string() } else { note };
        let nb = ui.line(&note, Style::new(size::SMALL, weight::REGULAR, t.muted), Some(name_w - 18.0));
        ui.draw(&nb, Point::new(text_x + 18.0, row.y + 28.0 + (16.0 - nb.h) / 2.0));

        // open: what it makes, how the work goes, and its key
        let shown = ((h - TOOL_H) / extra).clamp(0.0, 1.0);
        if shown > 0.05 {
            ui.clip(Rect::new(row.x, row.y + TOOL_H, row.w, h - TOOL_H), 0.0);
            ui.draw_faded(&does, Point::new(text_x, row.y + TOOL_H + 2.0), shown);
            let mut sy = row.y + TOOL_H + 2.0 + does.h + 8.0;
            for (n, step) in tool.steps.iter().enumerate() {
                ui.label_in(Rect::new(text_x, sy, 12.0, 18.0), &(n + 1).to_string(), Style::new(size::SMALL, weight::SEMIBOLD, pink.fade(shown)), Align::Left);
                let sb = ui.line(step, Style::new(size::SMALL, weight::REGULAR, t.muted.fade(shown)), Some(text_w - 18.0));
                ui.draw(&sb, Point::new(text_x + 18.0, sy + (18.0 - sb.h) / 2.0));
                sy += STEP_H;
            }
            if let Some(entry) = l.studio.key.as_ref().filter(|k| k.tool == tool.id) {
                draw_key_field(ui, Rect::new(text_x, sy + 8.0, text_w, KEY_H), tool.id, &entry.text, cx);
            } else if l.studio_keyless(tool.id) && l.studio_fix(tool.id).is_none() && l.studio.saved != Some(tool.id) {
                // it works without a key; with one it connects by itself
                if let Some(spec) = tools::key_spec(tool.id) {
                    let label = format!("Add {}", spec.label.to_lowercase());
                    let bw = widgets::button_width(ui, &label) + 22.0;
                    let button = Rect::new(text_x - 8.0, sy + 6.0, bw, ADD_KEY_H);
                    ui.hit(button, Action::Overlay(page::STUDIO + ADD_KEY + i));
                    let hot = ui.hover_amount(("launcher-slop-add-key", i), button);
                    if hot > 0.0 {
                        ui.round(button, radius::CONTROL, t.text.with_alpha(0.06 * hot));
                    }
                    let ink = t.muted.mix(t.text, 0.25 + 0.55 * hot).fade(shown);
                    ui.icon(Rect::new(button.x + 8.0, button.y, 16.0, button.h), I_KEY, 13.0, ink);
                    ui.label_in(Rect::new(button.x + 28.0, button.y, bw - 34.0, button.h), &label, Style::new(size::SMALL, weight::MEDIUM, ink), Align::Left);
                    if ui.hovered(button) {
                        let why = match tool.id {
                            tools::ELEVENLABS => "With its own key a session also gets ElevenLabs' server: voice design, sound effects and music.",
                            tools::SLOPCANNON => "With a token it connects by itself, in Codex too, and its command line works. Make one at slop.cc/account.",
                            _ => "With a key it connects without the sign-in, and its API works from scripts. Keys are at cloud.higgsfield.ai.",
                        };
                        tip = Some((button, format!("{why} · ctrl+k")));
                    }
                }
            }
            ui.unclip();
        }
    }
    ui.unclip();
    if let Some((row, text)) = tip {
        widgets::tooltip_wrapped(ui, row, &text, 320.0);
    }
}

/// The key field of the open tool: the key as dots, Save and Cancel beside it, and under it where a key comes
/// from.
fn draw_key_field(ui: &mut Ui, r: Rect, tool: &str, text: &str, cx: &Cx) {
    let t = ui.t;
    let Some(spec) = tools::key_spec(tool) else { return };
    let save_w = widgets::button_width(ui, "Save");
    let cancel = Rect::new(r.right() - 28.0, r.y + 2.0, 28.0, 28.0);
    let save = Rect::new(cancel.x - 6.0 - save_w, r.y + 2.0, save_w, 28.0);
    let field = Rect::new(r.x, r.y, (save.x - 8.0 - r.x).max(60.0), 32.0);
    let shown = masked(text);
    widgets::field_with(ui, field, Some(I_KEY), &shown, Some(shown.len()), spec.looks_like, cx.focused, Action::Overlay(page::STUDIO + KEY_FIELD));
    let kind = if text.trim().is_empty() { widgets::Kind::Secondary } else { widgets::Kind::Primary };
    widgets::button(ui, save, "Save", kind, Action::Overlay(page::STUDIO + KEY_SAVE));
    if widgets::icon_button(ui, cancel, I_CLOSE, t.muted, Action::Overlay(page::STUDIO + KEY_CANCEL)) {
        widgets::tooltip(ui, cancel, "Cancel · esc");
    }
    let from = format!("Paste your {} from {}", spec.label.to_lowercase(), spec.page);
    ui.label_in(Rect::new(r.x, r.y + 38.0, r.w, 18.0), &from, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
}

/// "Draw a keyframe" → "draw a keyframe", for a step in the middle of a sentence.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Narrow pages: the tools as pills in a line, each under its mark; a click switches one, or gets it going
/// when it can't be used yet.
fn draw_tool_pills(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    if !l.studio_ready() {
        ui.label_in(r, "Looking at what this computer has", Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        return;
    }
    let mut x = r.x;
    for (i, tool) in TOOLS.iter().enumerate() {
        let usable = l.studio_tool(tool.id).is_some_and(|s| s.ready != Ready::No);
        let on = l.studio_tool_on(tool.id);
        let st = Style::new(size::SMALL, weight::SEMIBOLD, if on { t.text } else { t.muted });
        let fix = fix_label(l, tool.id).filter(|_| !usable);
        let w = 10.0 + 16.0 + 7.0 + ui.measure(tool.name, st) + if fix.is_some() { 22.0 } else { 0.0 } + 12.0;
        if x + w > r.right() {
            break;
        }
        let pill = Rect::new(x, r.y, w, r.h);
        let hot = ui.hover_amount(("launcher-slop-pill", i), pill);
        ui.panel(pill, r.h / 2.0, t.control.mix(t.control_hover, hot), t.stroke);
        ui.mark(brand::studio_tool(tool.id), Rect::new(pill.x + 10.0, pill.y, 16.0, pill.h), 14.0, (!usable || !on).then_some(t.faint));
        ui.label_in(Rect::new(pill.x + 33.0, pill.y, w - 41.0, pill.h), tool.name, st, Align::Left);
        let (glyph, ink, note) = tool_state(l, tool.id, t);
        if fix.is_some() {
            ui.icon(Rect::new(pill.right() - 28.0, pill.y, 16.0, pill.h), glyph, 13.0, ink);
        }
        ui.hit(pill, Action::Overlay(page::STUDIO + if usable { SWITCH } else { FIX } + i));
        if ui.hovered(pill) {
            let text = match &fix {
                Some((label, _)) => format!("{note} · click to {}", lower_first(label)),
                None => format!("{} · {note}", tool.does),
            };
            widgets::tooltip(ui, pill, &text);
        }
        x += w + 8.0;
    }
}

// ------------------------------------------------------------------ where it runs

/// The agents as chips at `at`, `h` high (the chosen one outlined); returns where they end.
fn agent_chips(l: &Launcher, ui: &mut Ui, at: Point, h: f32, right: f32) -> f32 {
    let t = ui.t;
    let cs = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let mut x = at.x;
    for (i, harness) in Harness::ALL.into_iter().enumerate() {
        let on = harness == l.harness;
        let installed = l.installed(harness);
        let w = 10.0 + 16.0 + 6.0 + ui.measure(harness.label(), cs) + 12.0;
        if x + w > right {
            break;
        }
        let c = Rect::new(x, at.y, w, h);
        let hot = ui.hover_amount(("launcher-slop-agent", i), c);
        if on {
            ui.panel(c, radius::CONTROL, t.selected, t.accent.with_alpha(0.85));
        } else if hot > 0.0 {
            ui.round(c, radius::CONTROL, t.hover.fade(hot));
        }
        ui.mark(brand::harness(harness), Rect::new(c.x + 10.0, c.y, 16.0, c.h), 14.0, (!installed).then_some(t.faint));
        let ink = if on {
            t.text
        } else if installed {
            t.muted.mix(t.text, 0.5 * hot)
        } else {
            t.faint
        };
        ui.label_in(Rect::new(c.x + 32.0, c.y, w - 38.0, c.h), harness.label(), cs.color(ink), Align::Left);
        ui.hit(c, Action::Overlay(page::HARNESS + i));
        x += w + 6.0;
    }
    x
}

/// Who the session runs as, in a line: the login the New tab has selected and what's left of it, or what is
/// missing. Returns the text and whether it's a problem.
fn runs_as(l: &Launcher) -> (String, Option<f64>, bool) {
    if !l.installed(l.harness) {
        return (format!("{} isn't installed yet", l.harness.label()), None, true);
    }
    match l.spec() {
        Some(spec) => {
            let label = crate::services::launch::label_for(&spec);
            let left = match l.current() {
                Some(Item::Account(a)) => a.left,
                _ => None,
            };
            (label.trim_start_matches("slop · ").to_string(), left, false)
        }
        None if l.needs_model() => ("Pick a model for it".to_string(), None, true),
        None => ("Nothing to run it on yet".to_string(), None, true),
    }
}

/// The side panel's last block: the agent, the login, the permissions.
fn draw_runs_on(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    widgets::section(ui, Rect::new(r.x, r.y, r.w, 20.0), "Runs on");
    agent_chips(l, ui, Point::new(r.x, r.y + 28.0), 28.0, r.right());
    let line = Rect::new(r.x, r.y + 64.0, r.w, 30.0);
    let (text, left, problem) = runs_as(l);
    let installed = l.installed(l.harness);
    let (label, action) = if !installed { ("Install", INSTALL) } else if problem { ("Choose", RUNS_ON) } else { ("Change", RUNS_ON) };
    let bw = widgets::button_width(ui, label);
    let button = Rect::new(line.right() - bw, line.y + 1.0, bw, 28.0);
    widgets::button(ui, button, label, if problem { widgets::Kind::Secondary } else { widgets::Kind::Ghost }, Action::Overlay(page::STUDIO + action));
    if ui.hovered(button) && installed {
        widgets::tooltip(ui, button, "The New session tab picks the login and model · alt+n");
    }
    let mut x = line.x;
    if problem {
        ui.icon(Rect::new(x, line.y, 18.0, line.h), I_ALERT, size::TITLE, t.warn);
        x += 24.0;
    } else {
        ui.mark(brand::harness(l.harness), Rect::new(x, line.y, 18.0, line.h), 15.0, None);
        x += 26.0;
    }
    let mut room = button.x - 10.0 - x;
    if let Some(left) = left {
        let s = format!("{left:.0}% left");
        let col = if left <= 20.0 { t.danger } else if left <= 50.0 { t.warn } else { t.good };
        let ps = Style::new(size::SMALL, weight::SEMIBOLD, col);
        let pw = ui.measure(&s, ps);
        if room > pw + 80.0 {
            ui.label_in(Rect::new(button.x - 10.0 - pw, line.y, pw + 1.0, line.h), &s, ps, Align::Left);
            room -= pw + 10.0;
        }
    }
    let ts = Style::new(size::BODY, weight::SEMIBOLD, if problem { t.warn } else { t.text });
    let tb = ui.line(&text, ts, Some(room.max(20.0)));
    ui.draw(&tb, Point::new(x, line.y + (line.h - tb.h) / 2.0));
    page::permission_row(l, ui, Rect::new(r.x, line.bottom() + 12.0, r.w, 32.0));
}

/// Narrow pages: the agents and who it runs as, in one line.
fn draw_runs_line(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let lead = Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.35));
    let lw = ui.measure("Runs on", lead);
    ui.label_in(Rect::new(r.x, r.y, lw + 2.0, r.h), "Runs on", lead, Align::Left);
    let x = agent_chips(l, ui, Point::new(r.x + lw + 12.0, r.y), r.h, r.right());
    let (text, _, problem) = runs_as(l);
    let room = r.right() - x - 10.0;
    if room >= 90.0 {
        let at = Rect::new(x + 10.0, r.y, room, r.h);
        let tb = ui.line(&text, Style::new(size::SMALL, weight::MEDIUM, if problem { t.warn } else { t.muted }), Some(room));
        ui.draw(&tb, Point::new(at.x, at.y + (at.h - tb.h) / 2.0));
        ui.hit(Rect::new(at.x, at.y, tb.w, at.h), Action::Overlay(page::STUDIO + RUNS_ON));
    }
}

// ------------------------------------------------------------------ the footer

/// The keys on the left; on the right the button that starts it, in Create Slop's pink.
fn draw_footer(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let pink = tone(t);
    let (label, fill, ink, go, target) = if !l.installed(l.harness) {
        (format!("Install {}", l.harness.label()), t.bold_fill(t.danger), t.on_bold(), true, INSTALL)
    } else if !l.studio_ready() {
        ("Looking at the toolkit".to_string(), t.control, t.muted, false, MAIN)
    } else if l.spec().is_none() {
        ("Pick where it runs".to_string(), t.bold_fill(pink), t.on_bold(), true, MAIN)
    } else {
        let label = match l.studio_cast_names() {
            Some(names) => format!("Start cooking with {names}"),
            None => "Start cooking".to_string(),
        };
        (label, t.bold_fill(pink), t.on_bold(), true, MAIN)
    };
    let button = page::main_button(ui, r, &label, fill, ink, go, "launcher-slop-main", Action::Overlay(page::STUDIO + target));
    let tools = l.studio.focus == StudioFocus::Tools;
    let hints = Rect::new(r.x, r.y, (button.x - r.x - 24.0).max(0.0), r.h);
    // the key field has the keyboard: only its own keys
    if l.studio.key.is_some() {
        page::hints(ui, hints, &[("⏎", "save the key"), ("ctrl+v", "paste"), ("esc", "cancel")]);
        return;
    }
    let tool = TOOLS[l.studio.tool_sel.min(TOOLS.len() - 1)].id;
    // on a tool that isn't ready, enter gets it going
    let enter = match l.studio_fix(tool).filter(|_| tools) {
        Some(Fix::Install(_)) => "install",
        Some(Fix::Start(_)) => "start it",
        Some(Fix::Key) => "add key",
        None => "start",
    };
    let space = match (tools, l.studio_fix(tool)) {
        (true, None) => "on / off",
        (true, Some(_)) => "",
        (false, _) => "cast",
    };
    let add_key = if tools && l.studio_keyless(tool) && l.studio_fix(tool).is_none() { "add key" } else { "" };
    let keys: Vec<(&str, &str)> = [
        ("⏎", enter),
        ("space", space),
        ("ctrl+k", add_key),
        ("type", "find"),
        ("tab", if tools { "cast" } else { "toolkit" }),
        ("shift+←→", "agent"),
        ("alt+p", "project"),
        ("esc", if !l.studio.filter.is_empty() || tools { "back" } else { "close" }),
    ]
    .into_iter()
    .filter(|(_, what)| !what.is_empty())
    .collect();
    page::hints(ui, hints, &keys);
}

#[cfg(test)]
mod tests {
    use super::super::Data;
    use super::*;
    use crate::pane::Place;
    use crate::services::{Services, fallback_settings};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn launcher() -> Launcher {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        svc.refresh_studio(false);
        let st = svc.state();
        let data = Data {
            profiles: st.profiles.ready().cloned().unwrap_or_default(),
            providers: st.providers.ready().cloned().unwrap_or_default(),
            usage: BTreeMap::new(),
            usage_status: BTreeMap::new(),
            installed: vec![],
            recents: vec![],
            models: st.models.clone(),
            keyed: vec![],
            orchestrator_running: BTreeMap::new(),
            shared: false,
            jsh: None,
            studio: st.studio.clone(),
        };
        Launcher::new(data, Some(PathBuf::from("/films/commercial")), Place::Tab).with_studio()
    }

    #[test]
    fn a_card_casts_and_a_chip_takes_out() {
        let mut l = launcher();
        assert!(matches!(click(&mut l, CARD), Outcome::None));
        assert!(matches!(click(&mut l, CARD + 2), Outcome::None));
        assert_eq!(l.studio.picked.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Claudia", "Becky"]);
        assert_eq!(l.studio.sel, 2, "the cursor follows the click");
        click(&mut l, CARD);
        assert_eq!(l.studio.picked.len(), 1, "a second click takes them out");
        click(&mut l, UNCAST);
        assert!(l.studio.picked.is_empty());
        // a target from a roster that has since shrunk does nothing
        click(&mut l, CARD + 9_999);
        click(&mut l, UNCAST + 5);
        assert!(l.studio.picked.is_empty());
    }

    #[test]
    fn a_switch_turns_a_tool_off_and_a_row_selects_it() {
        let mut l = launcher();
        let ffmpeg = TOOLS.iter().position(|t| t.id == bro_core::studio::FFMPEG).unwrap();
        assert!(l.studio_tool_on(bro_core::studio::FFMPEG));
        click(&mut l, SWITCH + ffmpeg);
        assert!(!l.studio_tool_on(bro_core::studio::FFMPEG));
        assert!(!l.studio_kit().has_tool(bro_core::studio::FFMPEG));
        click(&mut l, TOOL + 1);
        assert_eq!((l.studio.tool_sel, l.studio.focus), (1, StudioFocus::Tools));
        click(&mut l, FIELD);
        assert_eq!(l.studio.focus, StudioFocus::Cast);
        click(&mut l, TOOL + 99);
        assert_eq!(l.studio.tool_sel, 1);
    }

    #[test]
    fn the_button_launches_with_the_kit_and_the_wheel_scrolls() {
        let mut l = launcher();
        click(&mut l, CARD);
        let Outcome::Launch(spec, Place::Tab) = click(&mut l, MAIN) else { panic!("it launches") };
        let kit = spec.studio.expect("the launch carries the kit");
        assert_eq!(kit.cast.len(), 1);
        assert!(kit.has_tool(bro_core::studio::SLOPCANNON));
        assert!(matches!(click(&mut l, INSTALL), Outcome::Install(Harness::Claude)));
        click(&mut l, REFRESH);
        assert_eq!(l.studio.ask.take(), Some(true));
        click(&mut l, RUNS_ON);
        assert_eq!(l.tab, SessionTab::New);
        wheel(&mut l, true);
        wheel(&mut l, true);
        assert_eq!(l.studio.scroll, 2.0 * WHEEL);
        for _ in 0..5 {
            wheel(&mut l, false);
        }
        assert_eq!(l.studio.scroll, 0.0);
    }

    #[test]
    fn a_tool_that_isnt_ready_gets_going_from_its_row() {
        use bro_core::quick_install::Tool as Install;
        use bro_core::studio::{ELEVENLABS, FFMPEG, Facts, JUSTIMAGINE, REMOTION, SLOPCANNON, statuses};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |l: &mut Launcher, code: KeyCode, mods: KeyModifiers| l.key(KeyEvent::new(code, mods));
        let at = |id: &str| TOOLS.iter().position(|t| t.id == id).unwrap();
        let tokens = Tokens::from_theme(&crate::theme::get("graphite"));
        // a machine with none of it
        let mut l = launcher();
        l.data.studio.tools = statuses(&Facts::default());
        assert!(matches!(click(&mut l, FIX + at(FFMPEG)), Outcome::InstallTool(Install::Ffmpeg, false)));
        assert_eq!((l.studio.tool_sel, l.studio.focus), (at(FFMPEG), StudioFocus::Tools), "its row is selected");
        assert!(matches!(click(&mut l, FIX + at(REMOTION)), Outcome::InstallTool(Install::Node, false)), "Remotion runs on Node.js");
        assert!(matches!(click(&mut l, FIX + at(JUSTIMAGINE)), Outcome::InstallTool(Install::JustImagine, false)));
        assert_eq!(fix_label(&l, FFMPEG).map(|f| f.0).as_deref(), Some("Install"));
        // installed and stopped: it starts
        l.data.studio.tools = statuses(&Facts { justimagine_installed: true, ..Facts::default() });
        assert!(matches!(click(&mut l, FIX + at(JUSTIMAGINE)), Outcome::InstallTool(Install::JustImagine, true)));
        assert_eq!(fix_label(&l, JUSTIMAGINE).map(|f| f.0).as_deref(), Some("Start"));
        // enter on the row does the same; a switch does nothing for a tool that can't be used
        click(&mut l, TOOL + at(FFMPEG));
        assert!(matches!(key(&mut l, KeyCode::Enter, KeyModifiers::NONE), Outcome::InstallTool(Install::Ffmpeg, false)));
        click(&mut l, SWITCH + at(FFMPEG));
        assert!(!l.studio_tool_on(FFMPEG));

        // ElevenLabs takes a key: the button opens its field, which has the keyboard
        assert!(matches!(click(&mut l, FIX + at(ELEVENLABS)), Outcome::None));
        assert_eq!(l.studio.key.as_ref().map(|k| k.tool), Some(ELEVENLABS));
        for ch in "sk_abx".chars() {
            key(&mut l, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        key(&mut l, KeyCode::Backspace, KeyModifiers::NONE);
        assert!(l.studio.filter.is_empty(), "typing a key doesn't look for a character");
        l.paste("ELEVENLABS_API_KEY=\"c123\"\n");
        assert_eq!(l.studio.key.as_ref().map(|k| k.text.as_str()), Some("sk_abc123"));
        assert!(masked("sk_abc123").ends_with("c123") && !masked("sk_abc123").contains("sk_") && masked("sk_abc123").chars().count() == 9);
        assert_eq!(masked("abc"), "abc");
        let Outcome::SaveKey(tool, saved) = key(&mut l, KeyCode::Enter, KeyModifiers::NONE) else { panic!("enter keeps the key") };
        assert_eq!((tool, saved.as_str()), (ELEVENLABS, "sk_abc123"));
        assert_eq!((l.studio.key.clone(), l.studio.saved), (None, Some(ELEVENLABS)));
        assert_eq!(tool_state(&l, ELEVENLABS, &tokens).2, "key saved");
        // the toolkit is looked at again with the key in it: the row says what it found
        l.data.studio.tools = statuses(&Facts { elevenlabs_key: true, ..Facts::default() });
        l.studio_sync();
        assert_eq!(l.studio.saved, None);
        assert_eq!(tool_state(&l, ELEVENLABS, &tokens).2, "Key found");

        // a tool that signs in can still be given a key (ctrl+k, or its button); esc closes the field only
        click(&mut l, TOOL + at(SLOPCANNON));
        assert!(l.studio_keyless(SLOPCANNON) && l.studio_fix(SLOPCANNON).is_none());
        key(&mut l, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(l.studio.key.as_ref().map(|k| k.tool), Some(SLOPCANNON));
        assert!(matches!(key(&mut l, KeyCode::Esc, KeyModifiers::NONE), Outcome::None));
        assert!(l.studio.key.is_none() && l.tab == SessionTab::Slop);
        click(&mut l, ADD_KEY + at(SLOPCANNON));
        assert!(matches!(click(&mut l, KEY_SAVE), Outcome::None), "an empty field just closes");
        assert!(l.studio.key.is_none() && l.studio.saved.is_none());
        click(&mut l, ADD_KEY + at(SLOPCANNON));
        click(&mut l, TOOL + at(FFMPEG));
        assert!(l.studio.key.is_none(), "another row closes it");
        click(&mut l, ADD_KEY + at(FFMPEG));
        assert!(l.studio.key.is_none(), "ffmpeg takes no key");
        // the page takes the window on this tab only
        assert!(l.fills_window());
        l.set_tab(SessionTab::New);
        assert!(!l.fills_window());
    }

    #[test]
    fn a_portrait_is_cropped_round_the_face() {
        // a landscape sheet in a portrait card: a slice round the face, clamped to the picture
        let wide = Picture { image: 1, w: 1600, h: 900 };
        let uv = cover_uv(&wide, 0.75, Some((0.9, 0.5)), 0.4);
        assert!((uv[2] - 1.0).abs() < 1e-5 && uv[0] > 0.5 && (uv[1], uv[3]) == (0.0, 1.0), "{uv:?}");
        let uv = cover_uv(&wide, 0.75, Some((0.5, 0.5)), 0.4);
        assert!((uv[0] + uv[2] - 1.0).abs() < 1e-5, "centred on the face: {uv:?}");
        // a tall picture: the band the face is in, never past the top
        let tall = Picture { image: 1, w: 600, h: 1200 };
        let uv = cover_uv(&tall, 0.75, Some((0.5, 0.1)), 0.4);
        assert_eq!((uv[0], uv[1], uv[2]), (0.0, 0.0, 1.0));
        assert!((uv[3] - 2.0 / 3.0).abs() < 1e-5, "{uv:?}");
        // no face found: the upper part
        let uv = cover_uv(&tall, 0.75, None, 0.4);
        assert!(uv[1] < 0.1, "{uv:?}");
        assert_eq!(lower_first("Draw a keyframe"), "draw a keyframe");
        assert_eq!(initials("Chinese Bone", 2), "CB");
        assert_eq!(initials("NB (Nikita Boar)", 2), "NN");
        assert_eq!(initials("burnz", 2), "B");
        assert_eq!(tile_colors("Becky"), tile_colors("Becky"));
        assert_ne!(tile_colors("Becky"), tile_colors("Astra"));
    }
}
