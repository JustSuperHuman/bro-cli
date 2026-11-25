//! The Storyboard in the desktop app ([`super::StoryboardView`] is its state and keys).
//!
//! Across the top, the film: its title, where it stands, its treatment, and the button that tells the session
//! what you changed. Under it the timeline: every scene as a block as long as it runs, filled by how far it has
//! got, with a check once you approved it. Below, the scenes down the left as cards (the chosen keyframe, the
//! title, the time), and the selected scene on the right: the keyframe it is filmed from, large; its other
//! keyframes and its takes to choose between; then its script, its direction and what you want changed, each
//! edited where it stands in the terminal font, as a screenplay is set. A narrow pane drops the list: the
//! timeline picks the scene.
//!
//! Pictures come from the window's media cache ([`Media::thumb`]), decoded off its thread.

use super::{Editing, StoryboardView};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::app::gui::media::{Media, Thumbnail};
use crate::pane::{Cx, PaneId};
use crate::textarea::TextArea;
use crate::views::usage::gui::{hints, icon_text_button, icon_text_button_width};
use bro_core::storyboard::{self, BEAT_MAX_CHARS, Field, SCENE_MAX_MS, SCENE_MIN_MS, Scene, Shot, Stage, clock, seconds};
use bro_gfx::text::Align;
use bro_gfx::{Color, Fill, Point, Quad, Rect};
use crossterm::event::KeyModifiers;
use std::cell::RefCell;
use std::path::Path;

// ------------------------------------------------------------------ targets

/// the view's background: a click there keeps what was being typed
const BODY: usize = 0;
const SEND: usize = 1;
const APPROVE: usize = 3;
const APPROVE_ALL: usize = 4;
/// the big picture: opens it in the preview
const STAGE: usize = 7;
/// the scene list and the selected scene's side (the wheel)
const LIST: usize = 8;
const DETAIL: usize = 9;
/// + the field's place in [`FIELDS`]
const FIELD: usize = 10;
/// + the scene's place in the film: its card
const SCENE: usize = 1_000;
/// + the scene's place in the film: its block on the timeline
const BLOCK: usize = 2_000;
/// + the keyframe's place in the selected scene
const KEYFRAME: usize = 3_000;
/// + the take's place in the selected scene
const TAKE: usize = 4_000;

const FIELDS: [Field; 6] = [Field::Film, Field::Concept, Field::Title, Field::Script, Field::Direction, Field::Note];

// ------------------------------------------------------------------ metrics

const PAD: f32 = 20.0;
/// Below this the scene list goes: the timeline picks the scene.
const LIST_MIN_W: f32 = 760.0;
const LIST_W: f32 = 292.0;
const CARD_H: f32 = 68.0;
const TIMELINE_H: f32 = 30.0;
/// The terminal font in the text fields, and a row of it.
const MONO: f32 = 12.5;
const ROW: f32 = 19.0;
const FIELD_PAD_X: f32 = 12.0;
const FIELD_PAD_Y: f32 = 9.0;
/// A keyframe or a take in the strips under the big picture.
const THUMB_H: f32 = 44.0;
const WHEEL: f32 = 72.0;

const I_FILM: &str = "\u{F0FCE}";
const I_CHECK: &str = "\u{F012C}";
const I_ALERT: &str = "\u{F0026}";
const I_PLAY: &str = "\u{F040A}";
const I_IMAGE: &str = "\u{F02E9}";
const I_SEND: &str = "\u{F048A}";
const I_CHECK_ALL: &str = "\u{F012D}";

/// Create Slop's colour (the launcher's tab uses the same one).
fn tone(ui: &Ui) -> Color {
    Color::hex(if ui.t.light { 0xD6247A } else { 0xFF5FA8 })
}

/// Where a text field was drawn, for putting the caret where a click lands.
#[derive(Clone, Copy)]
struct Geo {
    pane: PaneId,
    field: Field,
    origin: Point,
    cols: usize,
    adv: f32,
}

thread_local! {
    static GEO: RefCell<Vec<Geo>> = const { RefCell::new(Vec::new()) };
}

/// The width of a character of the terminal font at `size`.
fn advance(ui: &mut Ui, size: f32) -> f32 {
    ui.measure("MMMMMMMMMMMMMMMMMMMM", Style::mono(size, ui.t.text)) / 20.0
}

/// A scene's frame as width over height, from the film's aspect ("16:9", "9:16").
fn aspect(v: &StoryboardView) -> f32 {
    let said = v.board.as_ref().map(|b| b.aspect.as_str()).unwrap_or("");
    let (w, h) = said.split_once([':', 'x', '/']).unwrap_or(("16", "9"));
    match (w.trim().parse::<f32>(), h.trim().parse::<f32>()) {
        (Ok(w), Ok(h)) if w > 0.0 && h > 0.0 => (w / h).clamp(0.4, 2.6),
        _ => 16.0 / 9.0,
    }
}

// ------------------------------------------------------------------ the view

pub(crate) fn draw(v: &mut StoryboardView, ui: &mut Ui, area: Rect, cx: &mut Cx, media: &mut Media) -> bool {
    v.watch(cx.waker());
    v.sync();
    GEO.with(|g| g.borrow_mut().retain(|x| x.pane != cx.id));
    let t = ui.t;
    ui.hit_with(area, Action::View(cx.id, BODY), Cursor::Default);
    let page = Rect::new(area.x + PAD, area.y + 14.0, (area.w - 2.0 * PAD).max(0.0), (area.h - 14.0).max(0.0));
    let foot_y = area.bottom() - 28.0;
    let mut y = head(v, ui, page, cx);

    if v.scenes().is_empty() {
        empty(v, ui, Rect::new(page.x, y, page.w, (foot_y - y).max(0.0)));
        return true;
    }
    y = timeline(v, ui, Rect::new(page.x, y + 4.0, page.w, TIMELINE_H), cx) + 8.0;
    y = problems(v, ui, Rect::new(page.x, y, page.w, 20.0));
    if let Some(e) = v.error() {
        ui.icon(Rect::new(page.x, y, 16.0, 20.0), I_ALERT, 13.0, t.danger);
        ui.label_in(Rect::new(page.x + 22.0, y, page.w - 22.0, 20.0), &e, Style::new(size::SMALL, weight::MEDIUM, t.danger), Align::Left);
        y += 24.0;
    }
    let body = Rect::new(page.x, y + 6.0, page.w, (foot_y - 10.0 - y - 6.0).max(0.0));
    let beside = body.w >= LIST_MIN_W;
    let detail = if beside {
        list(v, ui, Rect::new(body.x, body.y, LIST_W, body.h), cx, media);
        ui.vline(body.x + LIST_W + 12.0, body.y, body.h, t.hairline);
        Rect::new(body.x + LIST_W + 24.0, body.y, body.w - LIST_W - 24.0, body.h)
    } else {
        body
    };
    scene_side(v, ui, detail, cx, media);

    // as many of the keys as the pane has room for
    let keys: &[(&str, &str)] = if v.edit.is_some() {
        &[("ctrl+s", "save"), ("tab", "next text"), ("esc", "cancel")]
    } else if page.w >= 760.0 {
        &[("j/k", "scene"), ("s", "script"), ("d", "direction"), ("c", "ask for a change"), ("a", "approve"), ("1-9", "keyframe"), ("g", "send changes")]
    } else if page.w >= 520.0 {
        &[("j/k", "scene"), ("s", "script"), ("d", "direction"), ("c", "change"), ("a", "approve"), ("g", "send")]
    } else {
        &[("j/k", "scene"), ("s", "script"), ("a", "approve")]
    };
    hints(ui, Point::new(page.x, foot_y), keys);
    true
}

/// The film across the top: its title and where it stands, its treatment, and on the right what you changed and
/// the button that tells the session. Returns the y under it.
fn head(v: &mut StoryboardView, ui: &mut Ui, page: Rect, cx: &mut Cx) -> f32 {
    let t = ui.t;
    let pink = tone(ui);
    let board = v.board.clone().unwrap_or_default();
    // on the right: send changes, approve all
    let mut right = page.right();
    let unsent = v.unsent();
    // what you changed, and the button that tells the session: at the end of the title's line, or on a line of
    // its own under the film in a narrow pane
    let stacked = page.w < 520.0;
    let send = (v.session.is_some() && !board.edits.is_empty()).then(|| {
        let n = board.edits.len();
        let changes = format!("{n} change{}", if n == 1 { "" } else { "s" });
        if !unsent {
            "Sent to the agent".to_string()
        } else if page.w >= 700.0 || stacked {
            format!("Send {changes} to the agent")
        } else {
            format!("Send {changes}")
        }
    });
    let send_button = |ui: &mut Ui, r: Rect, label: &str| {
        let kind = if unsent { widgets::Kind::Primary } else { widgets::Kind::Ghost };
        if icon_text_button(ui, r, if unsent { I_SEND } else { I_CHECK }, label, kind, Action::View(cx.id, SEND)) {
            let said = board.edits_said();
            let mut tip = said.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
            if said.len() > 5 {
                tip.push_str(&format!("\n+{} more", said.len() - 5));
            }
            widgets::tooltip_under(ui, r, &tip, 380.0);
        }
    };
    if let Some(label) = send.as_ref().filter(|_| !stacked) {
        let w = icon_text_button_width(ui, label);
        right -= w;
        send_button(ui, Rect::new(right, page.y, w, 32.0), label);
        right -= 8.0;
    }
    let waiting = board.scenes.iter().filter(|s| !s.approved && s.note.is_empty()).count();
    if waiting > 1 && page.w >= 620.0 {
        let label = format!("Approve all {waiting}");
        let w = icon_text_button_width(ui, &label);
        right -= w;
        icon_text_button(ui, Rect::new(right, page.y, w, 32.0), I_CHECK_ALL, &label, widgets::Kind::Secondary, Action::View(cx.id, APPROVE_ALL));
        right -= 8.0;
    }

    // the title: a field while it is edited, else the words (a click edits them)
    let icon = Rect::new(page.x, page.y, 26.0, 32.0);
    ui.icon(icon, I_FILM, 20.0, pink);
    let x = page.x + 34.0;
    let room = (right - 12.0 - x).max(80.0);
    let title_r = Rect::new(x, page.y, room, 32.0);
    if v.edit.as_ref().is_some_and(|e| e.field == Field::Film) {
        let h = text_field(v, ui, Rect::new(x, page.y, room.min(520.0), 0.0), Field::Film, "", "Name the film", 1, cx);
        let _ = h;
    } else {
        let title = if board.title.is_empty() { "Untitled film" } else { &board.title };
        let st = Style::new(20.0, weight::SEMIBOLD, if board.title.is_empty() { t.muted } else { t.text });
        let tb = ui.line(title, st, Some(room));
        ui.draw(&tb, Point::new(x, page.y + (32.0 - tb.h) / 2.0));
        let hit = Rect::new(x - 4.0, page.y, tb.w + 8.0, 32.0);
        let hot = ui.hover_amount(("storyboard-title", cx.id), hit);
        if hot > 0.0 {
            ui.round(hit, radius::CONTROL, t.text.with_alpha(0.05 * hot));
        }
        ui.hit_with(hit.intersect(&title_r), Action::View(cx.id, FIELD), Cursor::Text);
    }
    let mut y = page.y + 36.0;
    let sum = v.summary();
    if !sum.is_empty() {
        ui.label_in(Rect::new(x, y, page.right() - x, 18.0), &sum, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        y += 22.0;
    }
    if let Some(label) = send.as_ref().filter(|_| stacked) {
        let w = icon_text_button_width(ui, label).min(page.right() - x);
        send_button(ui, Rect::new(x, y + 2.0, w, 32.0), label);
        y += 42.0;
    }
    // the treatment: two lines of it, all of it while it is edited
    if v.edit.as_ref().is_some_and(|e| e.field == Field::Concept) {
        y += text_field(v, ui, Rect::new(x, y, (page.right() - x).min(760.0), 0.0), Field::Concept, "", "The visual world, who is who, the arc", 2, cx) + 8.0;
    } else if !board.scenes.is_empty() || !board.concept.is_empty() {
        let said = if board.concept.is_empty() { "Add a treatment: the visual world, who is who, the arc" } else { &board.concept };
        let st = Style::new(size::BODY, weight::REGULAR, if board.concept.is_empty() { t.faint } else { t.text.mix(t.muted, 0.45) });
        let b = ui.para(said, st, (page.right() - x).min(760.0), Some(2));
        let r = Rect::new(x - 4.0, y - 2.0, b.w + 8.0, b.h + 4.0);
        let hot = ui.hover_amount(("storyboard-concept", cx.id), r);
        if hot > 0.0 {
            ui.round(r, radius::CONTROL, t.text.with_alpha(0.05 * hot));
        }
        ui.draw(&b, Point::new(x, y));
        ui.hit_with(r, Action::View(cx.id, FIELD + 1), Cursor::Text);
        y += b.h + 10.0;
    }
    y
}

/// Nothing on the board yet: what it is for, and what happens next.
fn empty(v: &StoryboardView, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let w = r.w.min(560.0);
    let x = r.x + (r.w - w) / 2.0;
    let mid = r.y + (r.h * 0.3).min(160.0);
    ui.icon(Rect::new(x, mid, w, 44.0), I_FILM, 38.0, t.faint);
    ui.label_in(Rect::new(x, mid + 56.0, w, 26.0), "The film goes here", Style::new(size::HEADING + 2.0, weight::SEMIBOLD, t.text), Align::Center);
    let what = if v.session.is_some() {
        "Tell the agent what you want to make. It writes the film here a scene at a time: what is heard, what is seen and how long it runs. You approve the scenes before anything is drawn, and choose the keyframes before anything is filmed."
    } else {
        "This project has no storyboard yet. Start a Create Slop session in it and the agent writes the film here a scene at a time, for you to approve before anything is drawn or filmed."
    };
    let b = ui.rich(&[(what.to_string(), Style::new(size::BODY, weight::REGULAR, t.muted))], size::BODY, Some(w), true, Some(5));
    ui.draw(&b, Point::new(x + (w - b.w) / 2.0, mid + 92.0));
}

/// The film as a strip: a block a scene, as long as it runs, filled by how far it has got. A click picks the
/// scene. Returns the y under it.
fn timeline(v: &mut StoryboardView, ui: &mut Ui, r: Rect, cx: &Cx) -> f32 {
    let t = ui.t;
    let pink = tone(ui);
    let Some(board) = &v.board else { return r.y };
    let total = board.runs_ms().max(board.end_ms()).max(1) as f32;
    let slop = v.slopcannon;
    let mut tip = None;
    for (i, s) in board.scenes.iter().enumerate() {
        let x0 = r.x + r.w * (s.start_ms as f32 / total);
        let x1 = r.x + r.w * (s.end_ms as f32 / total);
        let block = Rect::new(x0 + 1.0, r.y, (x1 - x0 - 2.0).max(3.0), r.h);
        let hot = ui.hover_amount(("storyboard-block", cx.id, i), block);
        let fill = match s.stage() {
            Stage::Filmed => t.good.with_alpha(0.34),
            Stage::Keyframed => pink.with_alpha(0.26),
            Stage::Written => t.text.with_alpha(0.07),
        };
        ui.round(block, radius::CONTROL, fill.mix(t.text.with_alpha(0.2), 0.35 * hot));
        let off = slop && !(SCENE_MIN_MS..=SCENE_MAX_MS).contains(&s.ms());
        if i == v.sel {
            ui.outline(block, radius::CONTROL, 1.5, pink);
        } else if off {
            ui.outline(block, radius::CONTROL, 1.0, t.warn.with_alpha(0.8));
        }
        if block.w >= 22.0 {
            let ink = if i == v.sel { t.text } else { t.muted };
            let n = Style::new(size::SMALL, weight::SEMIBOLD, ink);
            // the number, with what the scene is waiting on beside it when there is room
            let mark = if !s.note.is_empty() { Some((I_ALERT, t.warn)) } else if s.approved { Some((I_CHECK, t.good)) } else { None };
            match mark.filter(|_| block.w >= 44.0) {
                Some((glyph, c)) => {
                    let nb = ui.line(&(i + 1).to_string(), n, None);
                    let w = nb.w + 4.0 + 13.0;
                    let x = block.x + (block.w - w) / 2.0;
                    ui.draw(&nb, Point::new(x, block.y + (block.h - nb.h) / 2.0));
                    ui.icon(Rect::new(x + nb.w + 4.0, block.y, 13.0, block.h), glyph, 12.0, c);
                }
                None => {
                    ui.label_in(block, &(i + 1).to_string(), n, Align::Center);
                }
            }
        }
        ui.hit(block, Action::View(cx.id, BLOCK + i));
        if ui.hovered(block) {
            let name = if s.title.is_empty() { "Untitled".to_string() } else { s.title.clone() };
            let mut said = format!("{} · {name}\n{}–{} · {}", i + 1, clock(s.start_ms), clock(s.end_ms), seconds(s.ms()));
            if off {
                said.push_str("\nSLOPCANNON films scenes of 5 to 15 seconds");
            }
            tip = Some((block, said));
        }
    }
    // the clock under the strip: the start, the end, and the selected scene's own start
    let cs = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
    let y = r.bottom() + 3.0;
    ui.label_in(Rect::new(r.x, y, 60.0, 14.0), "0:00", cs, Align::Left);
    let end = clock(total as u64);
    ui.label_in(Rect::new(r.right() - 60.0, y, 60.0, 14.0), &end, cs, Align::Right);
    if let Some(s) = board.scenes.get(v.sel).filter(|s| s.start_ms > 0) {
        let x = r.x + r.w * (s.start_ms as f32 / total);
        if x > r.x + 44.0 && x < r.right() - 90.0 {
            ui.label_in(Rect::new(x, y, 60.0, 14.0), &clock(s.start_ms), cs.color(t.muted), Align::Left);
        }
    }
    if let Some((block, said)) = tip {
        widgets::tooltip_under(ui, block, &said, 300.0);
    }
    y + 14.0
}

/// What stands between the board and a film, in a line (all of it when pointed at). Returns the y under it.
fn problems(v: &StoryboardView, ui: &mut Ui, r: Rect) -> f32 {
    let t = ui.t;
    let list = v.board.as_ref().map(|b| b.problems(v.slopcannon)).unwrap_or_default();
    let Some(first) = list.first() else { return r.y };
    ui.icon(Rect::new(r.x, r.y, 16.0, r.h), I_ALERT, 13.0, t.warn);
    let more = if list.len() > 1 { format!("  +{} more", list.len() - 1) } else { String::new() };
    let st = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let line = format!("{}{more}", sentence(first));
    let b = ui.line(&line, st, Some(r.w - 22.0));
    ui.draw(&b, Point::new(r.x + 22.0, r.y + (r.h - b.h) / 2.0));
    let hit = Rect::new(r.x, r.y, b.w + 22.0, r.h);
    if list.len() > 1 && ui.hovered(hit) {
        let all = list.iter().take(6).map(|p| sentence(p)).collect::<Vec<_>>().join("\n");
        widgets::tooltip_under(ui, hit, &all, 420.0);
    }
    r.bottom() + 4.0
}

fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ------------------------------------------------------------------ pictures

/// A tile's two colours from a scene's id: the same scene is always the same colour.
fn tile_colors(seed: &str) -> (Color, Color) {
    let h = seed.bytes().fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    (Color::from_hsl(hue, 0.34, 0.34, 1.0), Color::from_hsl((hue + 0.08).fract(), 0.42, 0.16, 1.0))
}

/// A keyframe or a take in `r`, covering it: the picture once it is read, else a tile in the scene's colour.
/// `video`: a play mark over it.
fn frame(ui: &mut Ui, media: &mut Media, project: &Path, shot: Option<&Shot>, r: Rect, rad: f32, seed: &str, video: bool) {
    let t = ui.t;
    let (top, bottom) = tile_colors(seed);
    let px = ui.px(r);
    ui.scene.quad(Quad::new(px, Fill::Vertical(top, bottom)).radius(rad * ui.scale));
    let mark = (r.h * 0.34).clamp(12.0, 34.0);
    match shot.map(|s| media.thumb(&storyboard::full_path(project, &s.path))) {
        Some(Thumbnail::Ready(tex, w, h)) => {
            let (ra, ia) = (r.w / r.h.max(1.0), w.max(1) as f32 / h.max(1) as f32);
            let uv = if ia > ra {
                let k = ra / ia;
                [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, 1.0]
            } else {
                let k = ia / ra;
                [0.0, (1.0 - k) / 2.0, 1.0, (1.0 + k) / 2.0]
            };
            ui.image(tex, r, uv, rad, 1.0);
        }
        Some(Thumbnail::Loading) => ui.spinner(r.center(), (mark * 0.3).max(4.0), 1.8, Color::WHITE.with_alpha(0.7)),
        // a file that isn't there (yet), or no picture at all: the tile says which
        Some(Thumbnail::Failed) if !video => ui.icon(r, I_IMAGE, mark, Color::WHITE.with_alpha(0.42)),
        _ => {}
    }
    if video {
        let c = r.center();
        let k = (mark * 0.62).max(8.0);
        ui.circle_over(c, k, Color::BLACK.with_alpha(0.5));
        ui.icon(Rect::new(c.x - k, c.y - k, 2.0 * k, 2.0 * k), I_PLAY, k * 1.1, Color::WHITE);
    }
    let _ = t;
}

// ------------------------------------------------------------------ the scenes

/// The scenes as cards down the left: the chosen keyframe, the number and title, the time, where it stands.
fn list(v: &mut StoryboardView, ui: &mut Ui, view: Rect, cx: &Cx, media: &mut Media) {
    let t = ui.t;
    ui.hit_with(view, Action::View(cx.id, LIST), Cursor::Default);
    let n = v.scenes().len();
    let pitch = CARD_H + 4.0;
    let content = n as f32 * pitch;
    let max = (content - view.h).max(0.0);
    if std::mem::take(&mut v.reveal) {
        let top = v.sel as f32 * pitch;
        if top < v.scroll {
            v.scroll = top;
        } else if top + CARD_H > v.scroll + view.h {
            v.scroll = top + CARD_H - view.h;
        }
    }
    v.scroll = v.scroll.clamp(0.0, max);
    let off = ui.ease(("storyboard-list", cx.id), v.scroll, 130.0).clamp(0.0, max);
    let ratio = aspect(v);
    let project = v.project.clone();
    ui.clip(view, 0.0);
    for (i, s) in v.scenes().iter().enumerate() {
        let card = Rect::new(view.x, view.y + i as f32 * pitch - off, view.w - 8.0, CARD_H);
        if card.bottom() < view.y || card.y > view.bottom() {
            continue;
        }
        widgets::row(ui, card, ("storyboard-scene", cx.id, i), i == v.sel, Action::View(cx.id, SCENE + i));
        // the picture keeps the film's shape inside a box as tall as the card allows
        let (bw, bh) = (92.0, CARD_H - 16.0);
        let (pw, ph) = if ratio >= bw / bh { (bw, bw / ratio) } else { (bh * ratio, bh) };
        let pic = Rect::new(card.x + 8.0 + (bw - pw) / 2.0, card.y + 8.0 + (bh - ph) / 2.0, pw, ph);
        let shot = s.chosen_take().or(s.chosen_keyframe());
        frame(ui, media, &project, shot, pic, 4.0, &s.id, s.chosen_take().is_some());
        if shot.is_none() {
            ui.label_in(pic, &(i + 1).to_string(), Style::new(size::HEADING, weight::BOLD, Color::WHITE.with_alpha(0.55)), Align::Center);
        }
        let x = card.x + 8.0 + bw + 12.0;
        let room = (card.right() - 10.0 - x).max(20.0);
        let name = if s.title.is_empty() { "Untitled" } else { &s.title };
        let head = ui.line(&format!("{}  {name}", i + 1), Style::new(size::BODY, weight::SEMIBOLD, t.text), Some(room));
        ui.draw(&head, Point::new(x, card.y + 9.0));
        ui.label_in(Rect::new(x, card.y + 27.0, room, 16.0), &format!("{} · {}", clock(s.start_ms), seconds(s.ms())), Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        let (glyph, ink, said) = standing(ui, s);
        let mut sx = x;
        if let Some(glyph) = glyph {
            ui.icon(Rect::new(x - 1.0, card.y + 44.0, 14.0, 16.0), glyph, 12.0, ink);
            sx += 17.0;
        }
        ui.label_in(Rect::new(sx, card.y + 44.0, (card.right() - 10.0 - sx).max(10.0), 16.0), &said, Style::new(size::SMALL, weight::REGULAR, if glyph.is_some() { ink.mix(t.muted, 0.35) } else { t.faint }), Align::Left);
    }
    ui.unclip();
    let vis = 0.5 + 0.5 * ui.hover_amount(("storyboard-list-bar", cx.id), view);
    widgets::scrollbar(ui, view, content, view.h, off, vis);
}

/// Where a scene stands, as an icon (when one says it) and a few words.
fn standing(ui: &Ui, s: &Scene) -> (Option<&'static str>, Color, String) {
    let t = ui.t;
    let made = match s.stage() {
        Stage::Filmed => format!("{} take{}", s.takes.len(), if s.takes.len() == 1 { "" } else { "s" }),
        Stage::Keyframed => format!("{} keyframe{}", s.keyframes.len(), if s.keyframes.len() == 1 { "" } else { "s" }),
        Stage::Written => "Written".to_string(),
    };
    if !s.note.is_empty() {
        (Some(I_ALERT), t.warn, "Change asked for".to_string())
    } else if s.approved {
        (Some(I_CHECK), t.good, format!("Approved · {}", made.to_lowercase()))
    } else {
        (None, t.muted, made)
    }
}

/// The selected scene: its heading and sign-off, the picture it is filmed from, its keyframes and takes, then
/// its texts. It scrolls as one.
fn scene_side(v: &mut StoryboardView, ui: &mut Ui, view: Rect, cx: &mut Cx, media: &mut Media) {
    let t = ui.t;
    let pink = tone(ui);
    let Some(s) = v.scene().cloned() else { return };
    let i = v.sel;
    ui.hit_with(view, Action::View(cx.id, DETAIL), Cursor::Default);
    let project = v.project.clone();
    let ratio = aspect(v);
    let w = (view.w - 10.0).min(820.0);
    let x = view.x;
    ui.clip(view, 0.0);
    let top = view.y - v.detail_scroll;
    let mut y = top;

    // the heading: its number, its title (a click edits it), and the sign-off at the end of the line
    let (label, kind, glyph) = if s.approved { ("Approved", widgets::Kind::Secondary, I_CHECK) } else { ("Approve", widgets::Kind::Primary, I_CHECK) };
    let bw = icon_text_button_width(ui, label);
    let approve = Rect::new(x + w - bw, y, bw, 32.0);
    if icon_text_button(ui, approve, glyph, label, kind, Action::View(cx.id, APPROVE)) {
        widgets::tooltip(ui, approve, if s.approved { "Take the approval back · a" } else { "This scene is ready to be drawn and filmed · a" });
    }
    let nb = ui.line(&format!("{}", i + 1), Style::new(20.0, weight::BOLD, pink), None);
    ui.draw(&nb, Point::new(x, y + (32.0 - nb.h) / 2.0));
    let tx = x + nb.w + 12.0;
    let room = (approve.x - 14.0 - tx).max(60.0);
    if v.edit.as_ref().is_some_and(|e| e.field == Field::Title) {
        text_field(v, ui, Rect::new(tx, y, room.min(460.0), 0.0), Field::Title, "", "Name the scene", 1, cx);
    } else {
        let name = if s.title.is_empty() { "Untitled scene" } else { &s.title };
        let tb = ui.line(name, Style::new(size::HEADING + 2.0, weight::SEMIBOLD, if s.title.is_empty() { t.muted } else { t.text }), Some(room));
        let hit = Rect::new(tx - 4.0, y, tb.w + 8.0, 32.0);
        let hot = ui.hover_amount(("storyboard-scene-title", cx.id), hit);
        if hot > 0.0 {
            ui.round(hit, radius::CONTROL, t.text.with_alpha(0.05 * hot));
        }
        ui.draw(&tb, Point::new(tx, y + (32.0 - tb.h) / 2.0));
        ui.hit_with(hit, Action::View(cx.id, FIELD + 2), Cursor::Text);
    }
    y += 38.0;
    // when it runs, how long, who is in it
    let mut meta = format!("{}–{} · {}", clock(s.start_ms), clock(s.end_ms), seconds(s.ms()));
    if let Some(c) = s.credits {
        meta.push_str(&format!(" · {c:.0} credits quoted"));
    }
    let mb = ui.line(&meta, Style::new(size::SMALL, weight::REGULAR, t.muted), None);
    ui.draw(&mb, Point::new(x, y + (20.0 - mb.h) / 2.0));
    let mut cx_x = x + mb.w + 12.0;
    for name in &s.cast {
        let pw = widgets::pill_width(ui, name);
        if cx_x + pw > x + w {
            break;
        }
        widgets::pill(ui, Point::new(cx_x, y + 1.0), 18.0, name, t.text.mix(t.muted, 0.3), t.control);
        cx_x += pw + 6.0;
    }
    y += 30.0;

    // the picture it is filmed from, in the film's shape: big enough to judge, small enough that the script under
    // it is on screen with it
    let (max_w, max_h) = if ratio < 1.0 { (w * 0.42, 340.0) } else { ((w * 0.58).clamp(220.0, 460.0), 270.0) };
    let stage_h = (max_w / ratio).min(max_h);
    let stage_w = stage_h * ratio;
    let stage = Rect::new(x, y, stage_w, stage_h);
    let shown = s.chosen_take().or(s.chosen_keyframe());
    frame(ui, media, &project, shown, stage, radius::OVERLAY, &s.id, s.chosen_take().is_some());
    match shown {
        Some(shot) => {
            ui.hit(stage, Action::View(cx.id, STAGE));
            if ui.hovered(stage) {
                let what = if shot.note.is_empty() { shot.path.clone() } else { format!("{}\n{}", shot.note, shot.path) };
                widgets::tooltip(ui, stage, &what);
            }
        }
        None => {
            let said = if s.approved { "Approved. Its keyframes are next." } else { "No keyframe yet. Approve the scene and the agent draws it." };
            ui.label_in(Rect::new(stage.x + 16.0, stage.y + stage.h / 2.0 - 10.0, stage.w - 32.0, 20.0), said, Style::new(size::BODY, weight::MEDIUM, Color::WHITE.with_alpha(0.78)), Align::Center);
        }
    }
    // beside the picture when there is room, else under it: the keyframes and takes to choose between
    let side_x = stage.right() + 18.0;
    let beside = x + w - side_x >= 150.0;
    let (sx, mut sy, sw) = if beside { (side_x, y, x + w - side_x) } else { (x, stage.bottom() + 14.0, w) };
    sy = strip(ui, media, &project, cx, &s, false, Rect::new(sx, sy, sw, 0.0), ratio);
    sy = strip(ui, media, &project, cx, &s, true, Rect::new(sx, sy, sw, 0.0), ratio);
    y = if beside { stage.bottom().max(sy) + 18.0 } else { sy + 6.0 };

    // the texts
    y = labelled(v, ui, x, y, w, "Script", "what is heard", None, Field::Script, &s.script, "The lyric, the lines, the voice-over", 2, cx);
    let count = s.direction.chars().count();
    let over = v.slopcannon && count > BEAT_MAX_CHARS;
    let counted = v.slopcannon.then(|| (format!("{} / {}", thousands(count), thousands(BEAT_MAX_CHARS)), over));
    y = labelled(v, ui, x, y, w, "Direction", "what is seen", counted, Field::Direction, &s.direction, "The staging, the camera, the performance", 3, cx);
    y = labelled(v, ui, x, y, w, "Ask for a change", "", None, Field::Note, &s.note, "Tell the agent what to change in this scene", 1, cx);
    ui.unclip();

    // how far there is to scroll; the caret of a text being edited stays in view
    let content = y - top + 12.0;
    let max = (content - view.h).max(0.0);
    if let Some(row) = caret_y(v, cx.id) {
        let at = row - top;
        if at < v.detail_scroll + 8.0 {
            v.detail_scroll = (at - 8.0).max(0.0);
        } else if at + ROW + 16.0 > v.detail_scroll + view.h {
            v.detail_scroll = at + ROW + 16.0 - view.h;
        }
    }
    v.detail_scroll = v.detail_scroll.clamp(0.0, max);
    widgets::scrollbar(ui, Rect::new(view.right() - 8.0, view.y, 8.0, view.h), content, view.h, v.detail_scroll, 0.8);
}

/// 2800 → "2,800".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A scene's keyframes (or its takes) as a strip of small pictures under their heading; the chosen one carries a
/// ring and a check, and a click chooses another. Returns the y under it.
#[allow(clippy::too_many_arguments)]
fn strip(ui: &mut Ui, media: &mut Media, project: &Path, cx: &Cx, s: &Scene, takes: bool, at: Rect, ratio: f32) -> f32 {
    let t = ui.t;
    let pink = tone(ui);
    let (shots, chosen, base, name) = if takes { (&s.takes, s.take, TAKE, "Takes") } else { (&s.keyframes, s.keyframe, KEYFRAME, "Keyframes") };
    if shots.is_empty() && takes {
        return at.y;
    }
    widgets::section(ui, Rect::new(at.x, at.y, at.w, 18.0), name);
    if shots.len() > 1 {
        let said = if takes { "click to put one in the cut" } else { "click to film from one" };
        let sb = ui.line(said, Style::new(size::CAPTION, weight::MEDIUM, t.faint), None);
        if sb.w + 90.0 < at.w {
            ui.draw(&sb, Point::new(at.right() - sb.w, at.y + (18.0 - sb.h) / 2.0));
        }
    }
    let mut y = at.y + 24.0;
    if shots.is_empty() {
        ui.label_in(Rect::new(at.x, y, at.w, 18.0), "None drawn yet", Style::new(size::SMALL, weight::REGULAR, t.faint), Align::Left);
        return y + 26.0;
    }
    let tw = (THUMB_H * ratio).clamp(30.0, 110.0);
    let (mut x, gap) = (at.x, 8.0);
    let chosen = chosen.min(shots.len() - 1);
    let mut tip = None;
    for (n, shot) in shots.iter().enumerate() {
        if x + tw > at.right() + 0.5 {
            x = at.x;
            y += THUMB_H + gap;
        }
        let r = Rect::new(x, y, tw, THUMB_H);
        frame(ui, media, project, Some(shot), r, 4.0, &format!("{}{n}", s.id), takes);
        let hot = ui.hover_amount(("storyboard-shot", cx.id, takes, n), r);
        if n == chosen {
            ui.outline_over(r.outset(2.0), 6.0, 2.0, pink);
            let c = Point::new(r.right() - 9.0, r.y + 9.0);
            ui.circle_over(c, 7.0, pink);
            ui.icon(Rect::new(c.x - 7.0, c.y - 7.0, 14.0, 14.0), I_CHECK, 9.0, ui.t.on_bold());
        } else if hot > 0.0 {
            ui.outline_over(r, 4.0, 1.0, Color::WHITE.with_alpha(0.6 * hot));
        }
        // its number, so the 1-9 keys can be read off the strip
        if !takes && n < 9 {
            let tag = Rect::new(r.x + 4.0, r.bottom() - 18.0, 14.0, 14.0);
            ui.round_over(tag, 3.0, Color::BLACK.with_alpha(0.55));
            ui.label_in(tag, &(n + 1).to_string(), Style::new(size::CAPTION, weight::SEMIBOLD, Color::WHITE), Align::Center);
        }
        ui.hit(r, Action::View(cx.id, base + n));
        if ui.hovered(r) {
            let what = if shot.note.is_empty() { shot.path.clone() } else { format!("{}\n{}", shot.note, shot.path) };
            tip = Some((r, format!("{what}\nDouble-click opens it")));
        }
        x += tw + gap;
    }
    if let Some((r, said)) = tip {
        widgets::tooltip(ui, r, &said);
    }
    y + THUMB_H + 16.0
}

// ------------------------------------------------------------------ the texts

/// A text field under its heading (`about`: what it holds, quietly; `counted`: a count at the right, in the
/// warning colour when it is over). Returns the y under it.
#[allow(clippy::too_many_arguments)]
fn labelled(v: &mut StoryboardView, ui: &mut Ui, x: f32, y: f32, w: f32, name: &str, about: &str, counted: Option<(String, bool)>, field: Field, text: &str, placeholder: &str, min_rows: usize, cx: &mut Cx) -> f32 {
    let t = ui.t;
    let nb = ui.line(name, Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.35)), None);
    ui.draw(&nb, Point::new(x, y + (18.0 - nb.h) / 2.0));
    if !about.is_empty() {
        ui.label_in(Rect::new(x + nb.w + 8.0, y, 200.0, 18.0), about, Style::new(size::SMALL, weight::REGULAR, t.faint), Align::Left);
    }
    if let Some((count, over)) = counted {
        ui.label_in(Rect::new(x + w - 140.0, y, 140.0, 18.0), &count, Style::new(size::CAPTION, weight::MEDIUM, if over { t.warn } else { t.faint }), Align::Right);
    }
    let h = text_field(v, ui, Rect::new(x, y + 24.0, w, 0.0), field, text, placeholder, min_rows, cx);
    y + 24.0 + h + 16.0
}

/// A text of the board in a field at `at` (its height is the text's): the terminal font, wrapped as the editor
/// wraps it. A click starts editing it there; while it is edited it carries the caret and the accent. Returns
/// its height.
#[allow(clippy::too_many_arguments)]
fn text_field(v: &mut StoryboardView, ui: &mut Ui, at: Rect, field: Field, text: &str, placeholder: &str, min_rows: usize, cx: &Cx) -> f32 {
    let t = ui.t;
    let adv = advance(ui, MONO);
    let cols = (((at.w - 2.0 * FIELD_PAD_X) / adv).floor() as usize).max(8);
    let editing = v.edit.as_mut().filter(|e| e.field == field);
    let focused = editing.is_some() && cx.focused;
    let (body, rows, cursor) = match editing {
        Some(Editing { area, cols: c, .. }) => {
            *c = cols;
            (area.text().to_string(), area.rows(cols), Some(area.cursor()))
        }
        None => (text.to_string(), TextArea::new(text).rows(cols), None),
    };
    let shown = rows.len().max(min_rows);
    let h = shown as f32 * ROW + 2.0 * FIELD_PAD_Y;
    let r = Rect::new(at.x, at.y, at.w, h);
    let fk = ui.ease(("storyboard-field", cx.id, FIELDS.iter().position(|f| *f == field)), if cursor.is_some() { 1.0 } else { 0.0 }, 130.0);
    let hot = if cursor.is_some() { 0.0 } else { ui.hover_amount(("storyboard-field-hover", cx.id, field.key(), at.y as i32), r) };
    ui.panel(r, radius::CONTROL, t.control.mix(t.control_hover, hot).mix(t.field_focus, fk), t.stroke.mix(t.accent.with_alpha(0.8), fk));
    let origin = Point::new(r.x + FIELD_PAD_X, r.y + FIELD_PAD_Y);
    GEO.with(|g| g.borrow_mut().push(Geo { pane: cx.id, field, origin, cols, adv }));
    let clip = ui.scene.clip().rect.scale(1.0 / ui.scale);
    if body.is_empty() {
        let st = Style { italic: true, ..Style::new(size::BODY, weight::REGULAR, t.faint) };
        let pb = ui.line(placeholder, st, Some(at.w - 2.0 * FIELD_PAD_X));
        ui.draw(&pb, Point::new(origin.x, origin.y + (ROW - pb.h) / 2.0));
        if focused {
            widgets::caret(ui, ("storyboard-caret", cx.id), Point::new(origin.x, origin.y + 2.0), ROW - 4.0);
        }
    }
    // screenplay marks stand out a little: a shot's tag, a timestamp
    let ink = Style::mono(MONO, t.text.mix(t.muted, if cursor.is_some() { 0.0 } else { 0.12 }));
    let crow = cursor.map(|c| rows.iter().rposition(|&(s, e)| c >= s && c <= e).unwrap_or(0));
    for (n, &(start, end)) in rows.iter().enumerate() {
        let ry = origin.y + n as f32 * ROW;
        if ry + ROW < clip.y || ry > clip.bottom() {
            continue;
        }
        let line = body[start..end].trim_end_matches('\n');
        let b = ui.line(line, ink, None);
        ui.draw(&b, Point::new(origin.x, ry + (ROW - b.h) / 2.0));
        if focused && crow == Some(n) {
            let cx_x = b.layout.caret_x((cursor.unwrap_or(start) - start).min(line.len())) / ui.scale;
            widgets::caret(ui, ("storyboard-caret", cx.id), Point::new(origin.x + cx_x, ry + 2.0), ROW - 4.0);
        }
    }
    ui.hit_with(r, Action::View(cx.id, FIELD + FIELDS.iter().position(|f| *f == field).unwrap_or(0)), Cursor::Text);
    h
}

/// Where the caret of the text being edited was drawn (its row's top), when one is.
fn caret_y(v: &StoryboardView, pane: PaneId) -> Option<f32> {
    let e = v.edit.as_ref().filter(|e| !matches!(e.field, Field::Film | Field::Concept))?;
    let geo = GEO.with(|g| g.borrow().iter().find(|x| x.pane == pane && x.field == e.field).copied())?;
    let rows = e.area.rows(geo.cols);
    let row = rows.iter().rposition(|&(s, end)| e.area.cursor() >= s && e.area.cursor() <= end).unwrap_or(0);
    Some(geo.origin.y + row as f32 * ROW)
}

// ------------------------------------------------------------------ clicks and the wheel

/// Target `n` was clicked at `p`.
pub(crate) fn click(v: &mut StoryboardView, n: usize, p: Point, _mods: KeyModifiers, count: u32, cx: &mut Cx) {
    match n {
        SEND => v.send_changes(cx),
        APPROVE => v.toggle_approved(cx),
        APPROVE_ALL => v.approve_all(cx),
        STAGE => {
            if let Some(path) = v.scene().and_then(|s| s.chosen_take().or(s.chosen_keyframe())).map(|s| s.path.clone()) {
                v.preview(&path, cx);
            }
        }
        BODY | LIST | DETAIL => v.commit(cx),
        n if (FIELD..FIELD + FIELDS.len()).contains(&n) => {
            let field = FIELDS[n - FIELD];
            v.begin(field, cx);
            // the caret goes where the click landed
            let geo = GEO.with(|g| g.borrow().iter().find(|x| x.pane == cx.id && x.field == field).copied());
            if let (Some(geo), Some(e)) = (geo, v.edit.as_mut().filter(|e| e.field == field)) {
                let rows = e.area.rows(geo.cols);
                if rows.is_empty() {
                    return;
                }
                let row = (((p.y - geo.origin.y).max(0.0) / ROW) as usize).min(rows.len() - 1);
                let (start, end) = rows[row];
                let col = ((p.x - geo.origin.x).max(0.0) / geo.adv).round() as usize;
                let text = &e.area.text()[start..end];
                let mut used = 0;
                let at = text
                    .char_indices()
                    .find_map(|(pos, c)| {
                        used += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                        (used > col).then_some(start + pos)
                    })
                    .unwrap_or(start + text.trim_end_matches('\n').len());
                e.area.set_cursor(at);
            }
        }
        n if n >= TAKE => {
            let k = n - TAKE;
            if count >= 2 {
                if let Some(path) = v.scene().and_then(|s| s.takes.get(k)).map(|s| s.path.clone()) {
                    v.preview(&path, cx);
                }
            } else {
                v.pick_take(k, cx);
            }
        }
        n if n >= KEYFRAME => {
            let k = n - KEYFRAME;
            if count >= 2 {
                if let Some(path) = v.scene().and_then(|s| s.keyframes.get(k)).map(|s| s.path.clone()) {
                    v.preview(&path, cx);
                }
            } else {
                v.pick_keyframe(k, cx);
            }
        }
        n if n >= BLOCK => v.select(n - BLOCK, cx),
        n if n >= SCENE => v.select(n - SCENE, cx),
        _ => {}
    }
}

/// The wheel over target `n`: the scene list scrolls under the pointer, the selected scene's side elsewhere.
pub(crate) fn wheel(v: &mut StoryboardView, n: Option<usize>, down: bool, _cx: &mut Cx) {
    let d = if down { WHEEL } else { -WHEEL };
    match n {
        Some(n) if n == LIST || (SCENE..BLOCK).contains(&n) => {
            v.scroll = (v.scroll + d).max(0.0);
            v.reveal = false;
        }
        _ => v.detail_scroll = (v.detail_scroll + d).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{Services, fallback_settings};
    use std::path::PathBuf;

    fn tap(v: &mut StoryboardView, n: usize, p: Point, count: u32) -> Vec<crate::pane::Action> {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx.clone());
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        let mut cx = Cx { id: 7, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        click(v, n, p, KeyModifiers::NONE, count, &mut cx);
        actions
    }

    #[test]
    fn clicks_pick_scenes_pictures_and_texts() {
        let mut v = StoryboardView::demo(PathBuf::from("/films/night"), Some("s1".into()));
        tap(&mut v, BLOCK + 1, Point::default(), 1);
        assert_eq!(v.sel, 1);
        // a keyframe: one click films from it, two open it
        assert_eq!(v.scene().unwrap().keyframe, 1);
        tap(&mut v, KEYFRAME + 2, Point::default(), 1);
        assert_eq!(v.scene().unwrap().keyframe, 2);
        let opened = tap(&mut v, KEYFRAME, Point::default(), 2);
        assert!(matches!(opened.as_slice(), [crate::pane::Action::Preview(p)] if p.ends_with("slop/keyframes/s2-a.png")), "the first keyframe opens");
        assert_eq!(v.scene().unwrap().keyframe, 2, "opening one doesn't choose it");
        tap(&mut v, SCENE, Point::default(), 1);
        tap(&mut v, TAKE, Point::default(), 1);
        assert_eq!((v.sel, v.scene().unwrap().take), (0, 0));
        assert!(matches!(tap(&mut v, STAGE, Point::default(), 1).as_slice(), [crate::pane::Action::Preview(p)] if p.ends_with("slop/clips/s1-take1.mp4")));

        // a click in a text starts editing it with the caret where it landed
        GEO.with(|g| g.borrow_mut().push(Geo { pane: 7, field: Field::Script, origin: Point::new(100.0, 200.0), cols: 80, adv: 8.0 }));
        tap(&mut v, FIELD + 3, Point::new(100.0 + 8.0 * 5.0, 205.0), 1);
        let e = v.edit.as_ref().unwrap();
        assert_eq!((e.field, e.area.cursor()), (Field::Script, 5));
        // a click on the background keeps what was typed
        v.edit.as_mut().unwrap().area.insert("XX");
        tap(&mut v, BODY, Point::default(), 1);
        assert!(v.edit.is_none() && v.scene().unwrap().script.contains("XX"));
        tap(&mut v, APPROVE, Point::default(), 1);
        assert!(!v.scene().unwrap().approved, "it was approved");
        assert!(matches!(tap(&mut v, SEND, Point::default(), 1).as_slice(), [crate::pane::Action::Tell(sid, _)] if sid == "s1"));
        tap(&mut v, APPROVE_ALL, Point::default(), 1);
        assert_eq!(v.board.as_ref().unwrap().approved(), 5);
        // the wheel: the list under the pointer, the scene's side elsewhere
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx.clone());
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        let mut cx = Cx { id: 7, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        wheel(&mut v, Some(SCENE + 2), true, &mut cx);
        wheel(&mut v, Some(DETAIL), true, &mut cx);
        wheel(&mut v, None, true, &mut cx);
        assert_eq!((v.scroll, v.detail_scroll), (WHEEL, 2.0 * WHEEL));
        assert_eq!((thousands(2800), thousands(12), thousands(1234567)), ("2,800".into(), "12".into(), "1,234,567".into()));
    }
}
