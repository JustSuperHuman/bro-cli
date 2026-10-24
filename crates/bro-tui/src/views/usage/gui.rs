//! The usage view in the desktop app: a card per login with its 5h and weekly windows as segment gauges (the
//! terminal UI's ▰▰▰▱▱), reset countdowns, headroom and the large/small task picks. Clicking a signed-in login's
//! card starts a session on it (the terminal UI's click); `r`, `j`/`k` and the rest of the keys reach
//! [`super::UsageView`] as before.

use crate::alerts::Kind;
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Tokens, Ui, brand, radius, size, weight, widgets};
use crate::pane::Cx;
use crate::services::UsageEntry;
use bro_core::Harness;
use bro_core::profiles::Profile;
use bro_core::usage::Window;
use bro_gfx::text::Align;
use bro_gfx::{Color, Fill, Point, Quad, Rect};
use crossterm::event::KeyModifiers;
use std::collections::BTreeMap;

/// The view's background (wheel, focus).
const BODY: usize = 0;
const REFRESH: usize = 1;
const ADD_GEMINI: usize = 2;
const STUDIO: usize = 3;
const GOOGLE: usize = 4;
const SUBSCRIPTION_ACCOUNT: usize = 5;
/// `CARD + i`: profile `i`'s card.
const CARD: usize = 1000;

const PAD: f32 = 24.0;
const GAP: f32 = 16.0;
/// Cards narrower than this drop a column.
const MIN_CARD: f32 = 300.0;
/// Three cards this wide at most.
const MAX_W: f32 = 3.0 * 420.0 + 2.0 * GAP;
/// Inside a card.
const INSET: f32 = 16.0;
/// The agent's logo in a card's header.
const LOGO: f32 = 22.0;

/// What one frame shows, copied out of the shared state (no lock is held while drawing).
struct Snap {
    ago: String,
    profiles: Result<Vec<Profile>, String>,
    usage: BTreeMap<String, UsageEntry>,
    picks: (Option<String>, Option<String>),
}

fn snapshot(cx: &Cx) -> Snap {
    let st = cx.svc.state();
    let ago = match st.usage_at.map(|a| a.elapsed().as_secs()) {
        Some(s) if s < 5 => "just refreshed".to_string(),
        Some(s) => format!("refreshed {} ago", crate::util::short_dur(s)),
        None => "not refreshed yet".into(),
    };
    Snap { ago, profiles: st.profiles.ready().cloned().ok_or_else(|| st.profiles.why()), usage: st.usage.clone(), picks: st.picks.clone() }
}

fn harness(p: &Profile) -> Harness {
    if p.is_codex() { Harness::Codex } else { Harness::Claude }
}

/// An agent's colour: its logo's own (Claude's orange), or the text colour for marks that are black or white
/// by design (OpenAI's).
pub(crate) fn brand_color(h: Harness, t: &Tokens) -> Color {
    brand::harness(h).color(t)
}

/// Colour for "% used": the terminal UI's thresholds (≥ 85 danger, ≥ 60 amber, else good).
pub(crate) fn pct_color(pct: f32, t: &Tokens) -> Color {
    if pct >= 85.0 {
        t.danger
    } else if pct >= 60.0 {
        t.inline
    } else {
        t.good
    }
}

/// Where a segment gauge goes and how many segments it has.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gauge {
    pub r: Rect,
    pub n: usize,
}

/// Fit a segment gauge into `r`: segments 5–6 px wide with 2 px between them (about `widgets::segment_count(w,
/// 7)` of them), every one the same whole number of device pixels, the width that leaves the least over (so the
/// gauge can come out a pixel or two narrower than `r`).
pub(crate) fn gauge_fit(ui: &Ui, r: Rect) -> Gauge {
    let s = ui.scale.max(0.5);
    let (x, y) = ((r.x * s).round() / s, (r.y * s).round() / s);
    // in device pixels; `widgets::segments` puts 2 px between segments this wide
    let (room, gap) = ((r.w * s).floor(), 2.0 * s);
    let mut best: Option<(f32, usize, f32)> = None;
    for seg in (5.0 * s).floor() as u32..=(6.0 * s).floor() as u32 {
        let seg = seg as f32;
        let n = ((room + gap) / (seg + gap)).floor();
        let used = n * seg + (n - 1.0) * gap;
        if n >= 5.0 && used <= room && best.is_none_or(|(left, _, _)| room - used < left) {
            best = Some((room - used, n as usize, used / s));
        }
    }
    match best {
        Some((_, n, w)) => Gauge { r: Rect::new(x, y, w, r.h), n },
        None => Gauge { r: Rect::new(x, y, r.w, r.h), n: widgets::segment_count(r.w, 7.0) },
    }
}

/// Usage as little segments, the terminal UI's ▰▰▰▱▱: filled ones in `color`, the rest faint.
pub(crate) fn gauge(ui: &mut Ui, g: Gauge, fraction: f32, color: Color) {
    widgets::segments(ui, g.r, fraction, g.n, color);
}

/// A card inside a view, as WinUI lays cards on a page: a step up from the pane, a hairline edge, 8px corners,
/// no shadow. `hot` (0..1) steps a clickable card's fill up under the pointer and `down` presses it. Returns
/// the fill (to cover things drawn on the card).
pub(crate) fn card(ui: &mut Ui, r: Rect, hot: f32, down: bool) -> Color {
    let t = ui.t;
    let rest = t.control.over(t.surface);
    let fill = if down { t.control_pressed.over(t.surface) } else { rest.mix(t.control_hover.over(t.surface), hot) };
    // a surface of the window: as see-through as the rest of it over a material
    ui.glass_panel(r, radius::OVERLAY, fill, t.hairline);
    fill
}

/// A status at `at` (left edge, vertical middle): its icon in the status colour (no chip, no dot), then the
/// text, cut to end by `right`. Returns where the text ends.
pub(crate) fn status(ui: &mut Ui, at: Point, glyph: &str, c: Color, text: &str, st: Style, right: f32) -> f32 {
    ui.icon(Rect::new(at.x, at.y - 9.0, 16.0, 18.0), glyph, size::TITLE, c);
    let b = ui.line(text, st, Some((right - at.x - 22.0).max(1.0)));
    ui.draw(&b, Point::new(at.x + 22.0, at.y - b.h / 2.0));
    at.x + 22.0 + b.w
}

/// A neutral chip's fill (a status code, a level): coloured text goes on it, never a coloured fill.
pub(crate) fn chip_fill(t: &Tokens) -> Color {
    t.text.with_alpha(if t.light { 0.06 } else { 0.07 })
}

/// Key hints along the bottom of a view: the keys as keycaps and what they do, quietly.
pub(crate) fn hints(ui: &mut Ui, at: Point, items: &[(&str, &str)]) {
    let st = Style::new(size::SMALL, weight::REGULAR, ui.t.muted);
    let mut x = at.x;
    for (key, what) in items {
        x += widgets::kbd(ui, Point::new(x, at.y), key) + 8.0;
        let b = ui.label_in(Rect::new(x, at.y, 400.0, 18.0), what, st, Align::Left);
        x += b.w + 20.0;
    }
}

/// A button with an icon before its label, in the kit's button styles (4px corners; a standard button has the
/// control fill and edge). Returns true while hovered.
pub(crate) fn icon_text_button(ui: &mut Ui, r: Rect, glyph: &str, label: &str, kind: widgets::Kind, action: Action) -> bool {
    let hot = ui.hover_amount(("icon-text-button", label, r.x as i32, r.y as i32), r);
    let down = ui.pressing && hot > 0.5;
    let t = ui.t;
    let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
    let ink = match kind {
        widgets::Kind::Primary => {
            ui.round(r, radius::CONTROL, if down { t.accent.fade(0.8) } else { t.accent.fade(1.0 - 0.1 * hot) });
            ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, Color::rgba(0.0, 0.0, 0.0, 0.18));
            if down { t.on_accent.fade(0.8) } else { t.on_accent }
        }
        widgets::Kind::Danger => {
            ui.round(r, radius::CONTROL, if down { t.danger.fade(0.8) } else { t.danger.fade(1.0 - 0.1 * hot) });
            t.ink_on(t.danger)
        }
        widgets::Kind::Secondary => {
            widgets::control(ui, r, if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) });
            if down { t.muted } else { t.text }
        }
        widgets::Kind::Ghost => {
            if hot > 0.0 || down {
                ui.round(r, radius::CONTROL, if down { t.text.with_alpha(0.04) } else { t.text.with_alpha(0.06 * hot) });
            }
            if down { t.muted } else { t.text }
        }
    };
    let b = ui.line(label, Style::new(size::BODY, weight::MEDIUM, ink), None);
    let w = 16.0 + 8.0 + b.w;
    let x = r.x + (r.w - w) / 2.0;
    ui.icon(Rect::new(x, r.y, 16.0, r.h), glyph, size::TITLE, ink);
    ui.draw(&b, Point::new(x + 24.0, r.y + (r.h - b.h) / 2.0));
    ui.hit(r, action);
    hot > 0.5
}

/// Width an [`icon_text_button`] with `label` wants.
pub(crate) fn icon_text_button_width(ui: &mut Ui, label: &str) -> f32 {
    ui.measure(label, Style::new(size::BODY, weight::MEDIUM, ui.t.text)) + 24.0 + 24.0
}

/// Soft fades in the pane's colour where a scrolled list runs under its top and bottom edges, drawn over the
/// list. `off` is how far it's scrolled, `content` and `r.h` its height and the view's.
pub(crate) fn edge_fades(ui: &mut Ui, r: Rect, content: f32, off: f32) {
    const H: f32 = 22.0;
    let above = (off / H).clamp(0.0, 1.0);
    let below = ((content - r.h - off) / H).clamp(0.0, 1.0);
    if above <= 0.0 && below <= 0.0 {
        return;
    }
    let bg = ui.t.surface;
    let prev = ui.layer();
    if above > 0.0 {
        let p = ui.px(Rect::new(r.x, r.y, r.w, H));
        ui.scene.glass_quad(Quad::new(p, Fill::Vertical(bg.fade(above), bg.with_alpha(0.0))));
    }
    if below > 0.0 {
        let p = ui.px(Rect::new(r.x, r.bottom() - H, r.w, H));
        ui.scene.glass_quad(Quad::new(p, Fill::Vertical(bg.with_alpha(0.0), bg.fade(below))));
    }
    ui.set_layer(prev);
}

/// A view's title with a quiet line after it, on one baseline. Returns the title's width.
pub(crate) fn title(ui: &mut Ui, r: Rect, text: &str, meta: &[(String, Style)]) -> f32 {
    let t = ui.t;
    let tb = ui.line(text, Style::new(20.0, weight::SEMIBOLD, t.text), None);
    let ty = r.y + (r.h - tb.h) / 2.0;
    ui.draw(&tb, Point::new(r.x, ty));
    if !meta.is_empty() {
        let x = r.x + tb.w + 14.0;
        let mb = ui.rich(meta, size::SMALL, Some((r.right() - x).max(1.0)), false, Some(1));
        // share the title's baseline
        ui.draw(&mb, Point::new(x, ty + tb.h * 0.735 - mb.h * 0.735));
    }
    tb.w
}

pub(crate) fn draw(v: &mut super::UsageView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let snap = snapshot(cx);
    ui.hit_with(area, Action::View(cx.id, BODY), Cursor::Default);
    let page = Rect::new(area.x + PAD, area.y, (area.w - 2.0 * PAD).clamp(0.0, MAX_W), area.h);

    // header: title, when it was read, refresh
    let head = Rect::new(page.x, area.y + 18.0, page.w, 36.0);
    let label = "Refresh";
    let bw = icon_text_button_width(ui, label);
    let br = Rect::new(head.right() - bw, head.y + 2.0, bw, 32.0);
    let aw = icon_text_button_width(ui, "API key");
    let ar = Rect::new(br.x - 8.0 - aw, br.y, aw, 32.0);
    icon_text_button(ui, ar, "\u{F0415}", "API key", widgets::Kind::Secondary, Action::View(cx.id, ADD_GEMINI));
    icon_text_button(ui, br, "\u{F0450}", label, widgets::Kind::Secondary, Action::View(cx.id, REFRESH));
    let gw = icon_text_button_width(ui, "Google");
    let gr = Rect::new(ar.x - 8.0 - gw, ar.y, gw, ar.h);
    icon_text_button(ui, gr, "\u{F0415}", "Google", widgets::Kind::Secondary, Action::View(cx.id, GOOGLE));
    let meta = vec![(format!("{} · every 60s", snap.ago), Style::new(size::SMALL, weight::REGULAR, t.muted))];
    title(ui, Rect { w: gr.x - 16.0 - head.x, ..head }, "Usage", &meta);

    // key hints along the bottom
    let foot_y = area.bottom() - 30.0;
    hints(ui, Point::new(page.x, foot_y), &[("r", "refresh"), ("j/k", "scroll"), ("alt+o", "profiles")]);

    let grid = Rect::new(page.x, head.bottom() + 22.0, page.w, (foot_y - 14.0 - head.bottom() - 22.0).max(0.0));
    let profiles = match &snap.profiles {
        Ok(p) => p,
        Err(why) => {
            empty(ui, grid, "\u{F029A}", &format!("profiles {why}"));
            return true;
        }
    };
    if profiles.is_empty() {
        empty(ui, grid, "\u{F0004}", "no profiles yet — alt+o to add a Claude account or Codex profile");
        return true;
    }

    // a grid of cards, two or three to a row when there's room
    let cols = (((grid.w + GAP) / (MIN_CARD + GAP)).floor() as usize).clamp(1, 3);
    let cw = (grid.w - GAP * (cols - 1) as f32) / cols as f32;
    let heights: Vec<f32> = profiles.iter().map(|p| card_height(ui, p, snap.usage.get(&p.id), cw)).collect();
    let rows = profiles.len().div_ceil(cols);
    let mut tops = Vec::with_capacity(rows);
    let mut row_h = Vec::with_capacity(rows);
    let mut acc = 0.0;
    for r in 0..rows {
        let h = heights[r * cols..((r + 1) * cols).min(heights.len())].iter().copied().fold(0.0, f32::max);
        tops.push(acc);
        row_h.push(h);
        acc += h + GAP;
    }
    let content = (acc - GAP).max(0.0);
    // j/k and the wheel move a row at a time, never past the last row (the terminal UI could scroll into blank)
    let max_off = (content - grid.h).max(0.0);
    let max_row = tops.iter().position(|&y| y >= max_off - 0.5).unwrap_or(rows.saturating_sub(1));
    v.scroll = v.scroll.min(max_row);
    let target = tops.get(v.scroll).copied().unwrap_or(0.0).min(max_off);
    let off = ui.ease(("usage-scroll", cx.id), target, 180.0);

    let view = Rect::new(grid.x - 18.0, grid.y - 4.0, grid.w + 36.0, grid.h + 8.0);
    ui.clip(view, 0.0);
    for (i, p) in profiles.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let card = Rect::new(grid.x + c as f32 * (cw + GAP), grid.y + tops[r] - off, cw, row_h[r]);
        if card.bottom() < view.y || card.y > view.bottom() {
            continue;
        }
        draw_card(ui, card, i, p, snap.usage.get(&p.id), &snap.picks, cx);
    }
    ui.unclip();
    edge_fades(ui, view, content + 8.0, off);
    let vis = ui.ease(("usage-bar", cx.id), if ui.hovered(grid) { 1.0 } else { 0.0 }, 200.0);
    widgets::scrollbar(ui, Rect::new(grid.x, grid.y, grid.w + 14.0, grid.h), content, grid.h, off, vis);
    true
}

/// A quiet line where the cards would be.
fn empty(ui: &mut Ui, r: Rect, glyph: &str, text: &str) {
    let t = ui.t;
    let row = Rect::new(r.x, r.y, r.w, 24.0);
    ui.icon(Rect::new(row.x, row.y, 18.0, row.h), glyph, size::TITLE, t.faint);
    ui.label_in(Rect::new(row.x + 28.0, row.y, row.w - 28.0, row.h), text, Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Left);
}

/// What a card's lower part shows when there's no reading.
fn why_not(p: &Profile, e: Option<&UsageEntry>) -> String {
    match e {
        Some(UsageEntry { error: Some(err), .. }) => err.clone(),
        Some(UsageEntry { fetching: true, .. }) => "fetching…".into(),
        _ if !p.authenticated => "not logged in — alt+o to log in".into(),
        _ => "no reading yet".into(),
    }
}

/// The scoped windows a card shows (the terminal UI shows up to two).
fn scoped(e: Option<&UsageEntry>) -> Vec<(String, Window)> {
    e.and_then(|e| e.usage.as_ref()).map(|u| u.scoped.iter().take(2).cloned().collect()).unwrap_or_default()
}

/// The headroom line, "headroom now ≈X% · week Y%", and "capped by the weekly ratio" when it is.
fn headroom_spans(e: Option<&UsageEntry>, t: &Tokens) -> (Vec<(String, Style)>, bool) {
    let quiet = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let dot = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let mut s = vec![("headroom  ".to_string(), quiet)];
    match e.and_then(|e| e.headroom.as_ref()) {
        Some(hr) => {
            let approx = if hr.capped { "≈" } else { "" };
            s.push((format!("now {approx}{:.0}%", hr.now), Style::new(size::SMALL, weight::SEMIBOLD, pct_color(100.0 - hr.now, t))));
            s.push(("  ·  ".into(), dot));
            s.push((format!("week {:.0}%", hr.week), Style::new(size::SMALL, weight::MEDIUM, pct_color(100.0 - hr.week, t))));
            (s, hr.capped)
        }
        None => {
            s.push(("—".into(), quiet));
            (s, false)
        }
    }
}

const CAPPED: &str = "capped by the weekly ratio";

/// The footer's lines: headroom, the capped note, the last refresh's failure.
struct Foot {
    headroom: crate::app::gui::kit::TextBox,
    capped: Option<crate::app::gui::kit::TextBox>,
    error: Option<crate::app::gui::kit::TextBox>,
}

impl Foot {
    fn lay(ui: &mut Ui, e: Option<&UsageEntry>, inner: f32) -> Foot {
        let t = ui.t;
        let (spans, capped) = headroom_spans(e, t);
        let headroom = ui.rich(&spans, size::SMALL, Some(inner), false, Some(1));
        let capped = capped.then(|| ui.line(CAPPED, Style::new(size::SMALL, weight::REGULAR, t.faint), Some(inner)));
        let error = e.and_then(|e| e.error.as_ref()).map(|err| ui.para(&format!("last refresh failed: {err}"), Style::new(size::SMALL, weight::MEDIUM, t.danger), inner - 18.0, Some(2)));
        Foot { headroom, capped, error }
    }
    fn height(&self) -> f32 {
        self.headroom.h + self.capped.as_ref().map(|b| b.h).unwrap_or(0.0) + self.error.as_ref().map(|b| 8.0 + b.h).unwrap_or(0.0)
    }
}

/// One window: its label row, then the gauge.
const METER_H: f32 = 32.0;
const METER_GAP: f32 = 14.0;

/// How tall a card wants to be at width `w`.
fn card_height(ui: &mut Ui, p: &Profile, e: Option<&UsageEntry>, w: f32) -> f32 {
    if p.is_gemini_subscription() { return subscription_card_height(ui, e, w); }
    if p.is_gemini_api() { return api_card_height(ui, e, w); }
    let inner = w - 2.0 * INSET;
    let mut h = INSET + 36.0;
    match e.and_then(|e| e.usage.as_ref()) {
        Some(_) => {
            let meters = 2 + scoped(e).len();
            h += 20.0 + meters as f32 * METER_H + (meters - 1) as f32 * METER_GAP + 18.0 + 12.0;
            h += Foot::lay(ui, e, inner).height() + INSET;
        }
        None => {
            let st = Style::new(size::SMALL, weight::MEDIUM, ui.t.muted);
            h += 18.0 + ui.para(&why_not(p, e), st, inner - 24.0, Some(3)).h.max(20.0) + INSET;
        }
    }
    h
}

fn draw_card(ui: &mut Ui, r: Rect, i: usize, p: &Profile, e: Option<&UsageEntry>, picks: &(Option<String>, Option<String>), cx: &Cx) {
    let t = ui.t;
    if p.is_gemini_subscription() { draw_subscription_card(ui, r, i, p, e, cx); return; }
    if p.is_gemini_api() { draw_api_card(ui, r, i, p, e, cx); return; }
    let h = harness(p);
    let launchable = p.authenticated;
    let hot = if launchable { ui.hover_amount(("usage-card", cx.id, i), r) } else { 0.0 };
    // a card that starts a session: its fill steps up under the pointer, down while pressed
    let fill = card(ui, r, hot, launchable && ui.pressing && hot > 0.5);
    if launchable {
        ui.hit(r, Action::View(cx.id, CARD + i));
    }

    // header: the agent's logo, the login, its picks; plan · email under it
    let x0 = r.x + INSET;
    let right = r.right() - INSET;
    let head = Rect::new(x0, r.y + INSET, right - x0, 36.0);
    let mark = brand::harness(h);
    let dim = if p.authenticated { 1.0 } else { 0.5 };
    ui.mark(mark, Rect::new(x0, head.y, LOGO, head.h), LOGO, Some(mark.color(t).fade(dim)));
    let tx = x0 + LOGO + 14.0;
    let row1 = Rect::new(tx, head.y - 1.0, right - tx, 20.0);
    let mut badges: Vec<(&str, Color)> = vec![];
    if picks.0.as_deref() == Some(p.id.as_str()) {
        badges.push(("large task", t.good));
    }
    if picks.1.as_deref() == Some(p.id.as_str()) {
        badges.push(("small task", t.shine));
    }
    let fetching = e.is_some_and(|e| e.fetching);
    let badges_w: f32 = badges.iter().map(|(b, _)| widgets::pill_width(ui, b) + 6.0).sum::<f32>() + if fetching { 20.0 } else { 0.0 };
    let name_st = Style::new(size::TITLE, weight::SEMIBOLD, if p.authenticated { t.text } else { t.text.mix(t.muted, 0.4) });
    let name = ui.line(&p.id, name_st, Some((right - tx - badges_w - 4.0).max(24.0)));
    ui.draw(&name, Point::new(tx, row1.y + (row1.h - name.h) / 2.0));
    let mut bx = tx + name.w + 8.0;
    for (b, c) in badges {
        let pr = widgets::pill(ui, Point::new(bx, row1.y), 20.0, b, c, c.with_alpha(0.12));
        bx = pr.right() + 6.0;
    }
    if fetching {
        ui.spinner(Point::new(bx + 7.0, row1.y + 10.0), 4.6, 1.6, t.accent);
    }
    // "+ session" shows at the top right while the pointer is over the card (above whatever is there)
    if hot > 0.01 {
        let st = Style::new(size::SMALL, weight::SEMIBOLD, t.accent);
        let b = ui.line("+ session", st, None);
        let chip = Rect::new(right - b.w - 16.0, row1.y, b.w + 16.0, 20.0);
        let prev = ui.layer();
        // painted onto the card (glass), so it covers what's there without making the card any less see-through
        ui.glass(Rect::new(chip.x - 8.0, chip.y - 2.0, right - chip.x + 8.0, chip.h + 4.0), bro_gfx::Corners::ZERO, fill.fade(hot));
        ui.glass_round(chip, radius::CONTROL, t.accent.with_alpha(0.12).over(fill).fade(hot));
        ui.draw_faded(&b, Point::new(chip.x + 8.0, chip.y + (chip.h - b.h) / 2.0), hot);
        ui.set_layer(prev);
    }
    let plan = e.and_then(|e| e.usage.as_ref()).and_then(|u| u.plan.clone()).or_else(|| p.plan.clone()).unwrap_or_else(|| "—".into());
    let mut sub = vec![(plan, Style::new(size::SMALL, weight::SEMIBOLD, t.shine))];
    if let Some(m) = &p.email {
        sub.push((format!("  ·  {m}"), Style::new(size::SMALL, weight::REGULAR, t.muted)));
    }
    let sb = ui.rich(&sub, size::SMALL, Some(right - tx), false, Some(1));
    ui.draw(&sb, Point::new(tx, head.bottom() - sb.h + 1.0));

    let inner = right - x0;
    let mut y = head.bottom();
    match e.and_then(|e| e.usage.as_ref()) {
        Some(u) => {
            y += 20.0;
            meter(ui, Rect::new(x0, y, inner, METER_H), "5h", u.five_hour.as_ref(), 8.0);
            y += METER_H + METER_GAP;
            meter(ui, Rect::new(x0, y, inner, METER_H), "week", u.weekly.as_ref(), 8.0);
            for (name, w) in scoped(e) {
                y += METER_H + METER_GAP;
                meter(ui, Rect::new(x0, y, inner, METER_H), &name, Some(&w), 6.0);
            }
            // the footer sits on the card's bottom edge so it lines up across a row
            let foot = Foot::lay(ui, e, inner);
            let mut fy = r.bottom() - INSET - foot.height();
            ui.hline(x0, fy - 12.0, inner, t.hairline);
            ui.draw(&foot.headroom, Point::new(x0, fy));
            fy += foot.headroom.h;
            if let Some(b) = &foot.capped {
                ui.draw(b, Point::new(x0, fy));
                fy += b.h;
            }
            if let Some(b) = &foot.error {
                fy += 8.0;
                ui.icon(Rect::new(x0 - 1.0, fy, 14.0, 16.0), "\u{F0026}", size::SMALL, t.danger);
                ui.draw(b, Point::new(x0 + 18.0, fy));
            }
        }
        None => {
            y += 18.0;
            let why = why_not(p, e);
            let erred = e.is_some_and(|e| e.error.is_some());
            let c = if erred { t.danger } else { t.muted };
            let st = Style::new(size::SMALL, weight::MEDIUM, c);
            let b = ui.para(&why, st, inner - 24.0, Some(3));
            let icon_r = Rect::new(x0, y, 16.0, 18.0);
            if fetching && !erred {
                ui.spinner(Point::new(icon_r.x + 7.0, icon_r.y + 9.0), 4.6, 1.6, t.accent);
            } else {
                let glyph = if erred {
                    "\u{F0026}"
                } else if !p.authenticated {
                    "\u{F0004}"
                } else {
                    "\u{F029A}"
                };
                ui.icon(icon_r, glyph, size::BODY, if erred { t.danger } else { t.faint });
            }
            ui.draw(&b, Point::new(x0 + 24.0, y + 1.0));
        }
    }
}

fn subscription_models(e: Option<&UsageEntry>) -> Vec<bro_core::gemini::ModelQuota> {
    e.and_then(|e| e.usage.as_ref()).and_then(|u| u.subscription.as_ref()).map(|q| bro_core::gemini::quota_groups(q)).unwrap_or_default()
}

const SUBSCRIPTION_NOTE: &str = "Live quota from Google. Shared across this subscription's sessions.";

fn subscription_card_height(ui: &mut Ui, e: Option<&UsageEntry>, w: f32) -> f32 {
    let inner = (w - 2.0 * INSET).max(1.0);
    let note = ui.para(SUBSCRIPTION_NOTE, Style::new(size::SMALL, weight::REGULAR, ui.t.muted), inner, None);
    let error = e.and_then(|e| e.error.as_ref()).map(|error| ui.para(error, Style::new(size::SMALL, weight::REGULAR, ui.t.danger), inner, None).h + 8.0).unwrap_or(0.0);
    144.0 + subscription_models(e).len().max(1) as f32 * 54.0 + note.h + error
}

fn draw_subscription_card(ui: &mut Ui, r: Rect, i: usize, p: &Profile, e: Option<&UsageEntry>, cx: &Cx) {
    let t = ui.t;
    let hot = ui.hover_amount(("gemini-subscription", cx.id, i), r);
    card(ui, r, hot, false);
    let x = r.x + INSET;
    let inner = r.w - 2.0 * INSET;
    ui.mark(brand::provider("gemini").unwrap(), Rect::new(x, r.y + INSET, 28.0, 36.0), 28.0, None);
    ui.label_in(Rect::new(x + 40.0, r.y + INSET, inner - 40.0, 22.0), "Gemini subscription", Style::new(size::TITLE, weight::SEMIBOLD, t.text), Align::Left);
    let subtitle = [p.plan.as_deref(), p.email.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    ui.label_in(Rect::new(x + 40.0, r.y + INSET + 23.0, inner - 40.0, 18.0), &subtitle, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
    let mut y = r.y + 74.0;
    let models = subscription_models(e);
    if models.is_empty() {
        ui.label_in(Rect::new(x, y, inner, 36.0), if e.is_some_and(|e| e.fetching) { "Reading Google's model quota…" } else { "No model quota reported yet" }, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        y += 54.0;
    }
    for model in models {
        let color = model.remaining.map(|left| pct_color((1.0 - left) as f32 * 100.0, t)).unwrap_or(t.faint);
        let value = model.remaining.map(|left| format!("{:.0}% left", left * 100.0)).unwrap_or_else(|| "Unavailable".into());
        let value_style = Style::new(size::BODY, weight::SEMIBOLD, color);
        let value_w = ui.measure(&value, value_style);
        let name = model.name.as_deref().unwrap_or(&model.id);
        ui.label_in(Rect::new(x, y, (inner - value_w - 10.0).max(1.0), 18.0), name, Style::new(size::SMALL, weight::SEMIBOLD, t.text), Align::Left);
        ui.label_in(Rect::new(x, y, inner, 18.0), &value, value_style, Align::Right);
        let reset = model.reset.as_deref().and_then(bro_core::util::parse_iso_secs).map(|at| format!("resets in {}", crate::util::countdown(at - crate::util::now_secs()))).unwrap_or_else(|| "reset time not reported".into());
        ui.label_in(Rect::new(x, y + 20.0, inner, 16.0), &reset, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        let bar = gauge_fit(ui, Rect::new(x, y + 40.0, inner, 6.0));
        gauge(ui, bar, model.remaining.unwrap_or(0.0) as f32, color);
        y += 54.0;
    }
    let note = ui.para(SUBSCRIPTION_NOTE, Style::new(size::SMALL, weight::REGULAR, t.muted), inner, None);
    ui.draw(&note, Point::new(x, y + 8.0));
    if let Some(error) = e.and_then(|e| e.error.as_ref()) {
        let message = ui.para(error, Style::new(size::SMALL, weight::REGULAR, t.danger), inner, None);
        ui.draw(&message, Point::new(x, y + 16.0 + note.h));
    }
    let by = r.bottom() - INSET - 32.0;
    let bw = (inner - 8.0) * 0.5;
    icon_text_button(ui, Rect::new(x, by, bw, 32.0), "\u{F0415}", "Launch", widgets::Kind::Primary, Action::View(cx.id, CARD + i));
    icon_text_button(ui, Rect::new(x + bw + 8.0, by, bw, 32.0), "\u{F03CC}", "Google AI", widgets::Kind::Secondary, Action::View(cx.id, SUBSCRIPTION_ACCOUNT));
}

fn api_note(e: Option<&UsageEntry>) -> String {
    let mut note = "Recorded through Bro on this computer. Account totals and limits are in AI Studio.".to_string();
    if let Some(api) = e.and_then(|e| e.usage.as_ref()).and_then(|u| u.api.as_ref()) {
        if api.month.requests > api.month.metered { note.push_str(" Some requests did not report tokens."); }
    }
    note
}

fn api_card_height(ui: &mut Ui, e: Option<&UsageEntry>, w: f32) -> f32 {
    let inner = (w - 2.0 * INSET).max(1.0);
    let note = ui.para(&api_note(e), Style::new(size::SMALL, weight::REGULAR, ui.t.muted), inner, None);
    let error = e.and_then(|e| e.error.as_ref()).map(|error| ui.para(error, Style::new(size::SMALL, weight::REGULAR, ui.t.danger), inner, None).h + 8.0).unwrap_or(0.0);
    252.0 + note.h + error
}

fn draw_api_card(ui: &mut Ui, r: Rect, i: usize, p: &Profile, e: Option<&UsageEntry>, cx: &Cx) {
    let t = ui.t;
    let hot = ui.hover_amount(("gemini-usage", cx.id, i), r);
    card(ui, r, hot, false);
    let x = r.x + INSET;
    let inner = r.w - 2.0 * INSET;
    let mark = brand::provider("gemini").unwrap();
    ui.mark(mark, Rect::new(x, r.y + INSET, 28.0, 36.0), 28.0, None);
    let title = if p.name == "local" { "Gemini".to_string() } else { format!("Gemini · {}", p.name) };
    ui.label_in(Rect::new(x + 40.0, r.y + INSET, inner - 40.0, 22.0), &title, Style::new(size::TITLE, weight::SEMIBOLD, t.text), Align::Left);
    ui.label_in(Rect::new(x + 40.0, r.y + INSET + 23.0, inner - 40.0, 18.0), if p.name == "local" { "Detected API key" } else { "API key profile" }, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
    let api = e.and_then(|e| e.usage.as_ref()).and_then(|u| u.api.as_ref());
    let metric = |ui: &mut Ui, y: f32, label: &str, total: Option<&bro_core::gemini_api::Totals>| {
        ui.label_in(Rect::new(x, y, inner, 18.0), label, Style::new(size::SMALL, weight::SEMIBOLD, t.muted), Align::Left);
        let value = match total { Some(total) => format!("{} requests · {} tokens", total.requests, crate::views::stats::compact(total.tokens())), None => "No usage recorded yet".into() };
        ui.label_in(Rect::new(x, y + 20.0, inner, 24.0), &value, Style::new(size::BODY, weight::SEMIBOLD, t.text), Align::Left);
    };
    metric(ui, r.y + 78.0, "Today", api.map(|u| &u.today));
    metric(ui, r.y + 134.0, "This month", api.map(|u| &u.month));
    let note = ui.para(&api_note(e), Style::new(size::SMALL, weight::REGULAR, t.muted), inner, None);
    ui.draw(&note, Point::new(x, r.y + 190.0));
    if let Some(error) = e.and_then(|e| e.error.as_ref()) {
        let message = ui.para(error, Style::new(size::SMALL, weight::REGULAR, t.danger), inner, None);
        ui.draw(&message, Point::new(x, r.y + 198.0 + note.h));
    }
    let by = r.bottom() - INSET - 32.0;
    let first = Rect::new(x, by, (inner - 8.0) * 0.55, 32.0);
    icon_text_button(ui, first, "\u{F0415}", if p.authenticated { "Launch" } else { "Add key" }, widgets::Kind::Primary, Action::View(cx.id, CARD + i));
    icon_text_button(ui, Rect::new(first.right() + 8.0, by, inner - first.w - 8.0, 32.0), "\u{F03CC}", "AI Studio", widgets::Kind::Secondary, Action::View(cx.id, STUDIO));
}

/// One window: its name and reset countdown, the % used, and the segments coloured by how full it is.
fn meter(ui: &mut Ui, r: Rect, name: &str, w: Option<&Window>, bar_h: f32) {
    let t = ui.t;
    // the gauge first: the row above ends where its last segment does
    let bar = gauge_fit(ui, Rect::new(r.x, r.y + 24.0, r.w, bar_h));
    let row = Rect::new(r.x, r.y, bar.r.w, 18.0);
    let nb = ui.label_in(row, name, Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.25)), Align::Left);
    match w {
        Some(w) => {
            let c = pct_color(w.used_pct, t);
            let pb = ui.label_in(row, &format!("{:.0}%", w.used_pct), Style::new(size::TITLE, weight::SEMIBOLD, c), Align::Right);
            if let Some(at) = w.resets_at {
                let text = format!("resets in {}", crate::util::countdown(at - crate::util::now_secs()));
                let x = row.x + nb.w + 10.0;
                ui.label_in(Rect::new(x, row.y, (row.right() - pb.w - 12.0 - x).max(1.0), row.h), &text, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
            }
            gauge(ui, bar, w.used_pct / 100.0, c);
        }
        None => {
            ui.label_in(row, "—", Style::new(size::TITLE, weight::SEMIBOLD, t.faint), Align::Right);
            gauge(ui, bar, 0.0, t.faint);
        }
    }
}

/// Target `n` was clicked.
pub(crate) fn click(_v: &mut super::UsageView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    match n {
        GOOGLE => cx.act(crate::pane::Action::GeminiSignIn),
        SUBSCRIPTION_ACCOUNT => { if let Err(error) = crate::panes::links::open("https://gemini.google/subscriptions/") { cx.toast(Kind::Error, error); } }
        ADD_GEMINI => cx.act(crate::pane::Action::Open(Box::new(crate::views::profiles::ProfilesView::adding(bro_core::profiles::ProfileKind::GeminiApi)), crate::pane::Place::Tab)),
        STUDIO => { if let Err(error) = crate::panes::links::open(bro_core::gemini_api::STUDIO) { cx.toast(Kind::Error, error); } }
        REFRESH => {
            cx.svc.refresh_usage();
            cx.toast(Kind::Usage, "refreshing usage…");
        }
        n if n >= CARD => {
            // the same as the terminal UI's click on a signed-in login's block
            let pick = {
                let st = cx.svc.state();
                st.profiles.ready().and_then(|ps| ps.get(n - CARD)).cloned()
            };
            if let Some(p) = pick {
                if p.is_gemini_subscription() && !p.authenticated {
                    cx.act(crate::pane::Action::GeminiSignIn);
                } else if p.is_gemini_api() && !p.authenticated {
                    cx.act(crate::pane::Action::Open(Box::new(crate::views::profiles::ProfilesView::editing_key(&p)), crate::pane::Place::Tab));
                } else if p.authenticated {
                    cx.act(crate::pane::Action::LaunchUsage(harness(&p), Some(p.id)));
                }
            }
        }
        _ => {}
    }
}

/// The wheel over target `n` (None: elsewhere in the view): a row of cards at a time (drawing clamps it to
/// the last row that still fills the view).
pub(crate) fn wheel(v: &mut super::UsageView, _n: Option<usize>, down: bool, cx: &mut Cx) {
    if down {
        v.scroll += 1;
    } else {
        v.scroll = v.scroll.saturating_sub(1);
    }
    let n = cx.svc.state().profiles.ready().map(|p| p.len()).unwrap_or(0);
    v.scroll = v.scroll.min(n.saturating_sub(1));
}
