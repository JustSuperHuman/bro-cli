//! The profiles view in the desktop app: every Claude account and Codex profile with its login state, the
//! actions on them with their keys, and the name prompt and the remove confirmation as dialogs over the list.
//! It draws [`ProfilesView`]'s own state; clicks send the keys the terminal UI's buttons send.
//!
//! Also home to the few pieces the setup and quick install pages share with it: key buttons and keycaps, the
//! page column and title, page cards, the focus outline, the hint row.

use super::{Mode, Pending, ProfilesView};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand;
use crate::app::gui::kit::widgets::Kind;
use crate::app::gui::kit::{Cursor, Style, Tokens, Ui, radius, size, weight, widgets};
use crate::pane::{Cx, Pane};
use bro_core::Harness;
use bro_core::profiles::{Profile, ProfileKind};
use bro_gfx::text::Align;
use bro_gfx::{Color, Corners, Fill, Point, Quad, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::path::PathBuf;

// targets
const BG: usize = 0;
const ADD_CLAUDE: usize = 1;
const ADD_CODEX: usize = 2;
const LOGIN: usize = 3;
const REMOVE: usize = 4;
const RELOAD: usize = 5;
const CREATE: usize = 6;
const CANCEL: usize = 7;
const YES: usize = 8;
const ROW: usize = 100;

/// A login row (two lines), and the list's inner padding.
const ROW_H: f32 = 48.0;
const PAD: f32 = 4.0;

const PLUS: &str = "\u{F0415}";
const LOGIN_ICON: &str = "\u{F0342}";
const TRASH: &str = "\u{F09E7}";
const REFRESH: &str = "\u{F0450}";
const FOLDER: &str = "\u{F0256}";
const ALERT: &str = "\u{F002A}";
const CHECK: &str = "\u{F012C}";
/// not logged in: a quiet account outline
const LOGGED_OUT: &str = "\u{F0BE7}";

/// A standard control's height (WinUI's 32).
pub(crate) const BUTTON_H: f32 = 32.0;
/// A page title's size (WinUI's subtitle).
pub(crate) const TITLE: f32 = 20.0;

/// The name prompt or the remove confirmation, while either is open or saving.
enum Card {
    Add { kind: ProfileKind, name: String, saving: bool },
    Remove { id: String, dir: Option<PathBuf>, saving: bool },
}

/// Draw the view inside its card (`area`: the card's content).
pub(crate) fn draw(v: &mut ProfilesView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let id = cx.id;
    // the window only ticks panes while nothing animates, and a save in flight shows a spinner: look for its
    // result here too (the tick's own `poll`, which only takes a finished result)
    if v.pending.is_some() {
        v.poll(cx);
    }
    ui.hit_with(area, Action::View(id, BG), Cursor::Default);
    if area.w < 240.0 || area.h < 220.0 {
        ui.label_in(area.inset(12.0), "Enlarge this pane to manage profiles", Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Center);
        return true;
    }
    let col = page(area, 880.0);
    let (profiles, why) = {
        let st = cx.svc.state();
        (st.profiles.ready().cloned(), st.profiles.why())
    };
    // the terminal UI takes no clicks on the list or its buttons while a prompt is open or a save runs
    let modal = !matches!(v.mode, Mode::List) || v.pending.is_some();
    let live = move |n: usize| (!modal).then_some(Action::View(id, n));

    // ---- title, reload (just its mark and key when the pane is narrow)
    let reload = if col.w >= 360.0 { "Reload" } else { "" };
    let rw = key_button_width(ui, Some(REFRESH), reload, Some("r"));
    let rr = Rect::new(col.right() - rw, col.y - 2.0, rw, BUTTON_H);
    // just the mark: its name on hover
    if key_button(ui, rr, Some(REFRESH), reload, Some("r"), Kind::Ghost, live(RELOAD)) > 0.5 && reload.is_empty() {
        widgets::tooltip(ui, rr, "Reload");
    }
    let head_h = page_title(ui, Point::new(col.x, col.y), "Profiles", Some("Claude and Codex logins"), col.w - rw - 12.0);

    // ---- geometry: toolbar (add on the left, the selection's actions on the right; flowing onto more lines
    // when narrow), list, footer (the folder and its note), hints at the bottom
    let buttons = [
        (PLUS, "Claude", "a", ADD_CLAUDE),
        (PLUS, "Codex", "c", ADD_CODEX),
        (LOGIN_ICON, "Log in", "l", LOGIN),
        (TRASH, "Remove", "d", REMOVE),
    ];
    let widths: Vec<f32> = buttons.iter().map(|(icon, label, key, _)| key_button_width(ui, Some(*icon), label, Some(*key))).collect();
    let one_line = widths.iter().sum::<f32>() + 8.0 * 2.0 + 24.0 <= col.w;
    let bar_y = col.y + head_h + 20.0;
    let mut spots: Vec<Rect> = vec![];
    if one_line {
        let mut x = col.x;
        for w in &widths[..2] {
            spots.push(Rect::new(x, bar_y, *w, BUTTON_H));
            x += w + 8.0;
        }
        // placed again below once the selection is known (Remove hides for machine logins)
        spots.push(Rect::new(col.right() - widths[3] - 8.0 - widths[2], bar_y, widths[2], BUTTON_H));
        spots.push(Rect::new(col.right() - widths[3], bar_y, widths[3], BUTTON_H));
    } else {
        let (mut x, mut y) = (col.x, bar_y);
        for w in &widths {
            if x > col.x && x + w > col.right() {
                x = col.x;
                y += BUTTON_H + 8.0;
            }
            spots.push(Rect::new(x, y, *w, BUTTON_H));
            x += w + 8.0;
        }
    }
    let bar_bottom = spots.iter().map(|r| r.bottom()).fold(bar_y + BUTTON_H, f32::max);
    let list_y = bar_bottom + 12.0;
    let hints_y = col.bottom() - 18.0;
    let footer_h = 62.0;
    let room = (hints_y - 18.0 - footer_h - 16.0 - list_y).max(ROW_H + 2.0 * PAD);
    let fit = (((room - 2.0 * PAD) / ROW_H).floor() as usize).max(1);
    if let Some(ps) = &profiles {
        v.rows = fit;
        v.sync_selection(ps);
    }
    let selected = v.selected(cx);

    // ---- toolbar
    let removable = selected.as_ref().is_some_and(|p| !p.is_local());
    if one_line && !removable {
        spots[2].x = col.right() - widths[2];
    }
    let login_kind = if selected.as_ref().is_some_and(|p| !p.authenticated) { Kind::Primary } else { Kind::Secondary };
    for (k, ((icon, label, key, n), r)) in buttons.iter().zip(&spots).enumerate() {
        match k {
            0 | 1 => {
                key_button(ui, *r, Some(*icon), label, Some(*key), Kind::Secondary, live(*n));
            }
            2 => {
                key_button(ui, *r, Some(*icon), label, Some(*key), login_kind, live(*n));
            }
            _ if removable => {
                key_button_ink(ui, *r, Some(*icon), label, Some(*key), Kind::Secondary, Some(t.danger.mix(t.text, 0.25)), live(*n));
            }
            _ => {}
        }
    }

    // ---- the logins: a card of rows
    let mut below = list_y;
    match &profiles {
        None => {
            let st = Style::new(size::BODY, weight::MEDIUM, t.muted);
            ui.label(Point::new(col.x + 2.0, list_y + 6.0), &why, st);
            below = list_y + 30.0;
        }
        Some(ps) if !ps.is_empty() => {
            let shown = ps.len().saturating_sub(v.scroll).min(v.rows);
            let list = Rect::new(col.x, list_y, col.w, shown as f32 * ROW_H + 2.0 * PAD);
            page_card(ui, list);
            ui.clip(list, radius::OVERLAY);
            for (row, (i, p)) in ps.iter().enumerate().skip(v.scroll).take(v.rows).enumerate() {
                let r = Rect::new(list.x + PAD, list.y + PAD + row as f32 * ROW_H, list.w - 2.0 * PAD, ROW_H);
                // the fill stops a pixel short of its neighbours, as WinUI's list items
                let fill = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
                let on = i == v.sel;
                if modal {
                    if on {
                        ui.round(fill, radius::CONTROL, t.selected);
                    }
                } else {
                    ui.hit(r, Action::View(id, ROW + i));
                    widgets::row(ui, fill, ("profile", id, &p.id), on, Action::View(id, ROW + i));
                }
                profile_row(ui, r, p, on);
            }
            ui.unclip();
            if ps.len() > v.rows {
                let content = ps.len() as f32 * ROW_H;
                widgets::scrollbar(ui, list.inset(2.0), content, v.rows as f32 * ROW_H, v.scroll as f32 * ROW_H, 1.0);
            }
            below = list.bottom();
        }
        Some(_) => {}
    }

    // ---- the selected login's folder and what removing it means
    if let Some(p) = profiles.as_ref().and_then(|ps| ps.get(v.sel)) {
        let fy = below + 16.0;
        let path_st = Style::mono(size::SMALL, t.muted);
        ui.icon(Rect::new(col.x, fy, 16.0, 18.0), FOLDER, size::BODY, t.muted);
        let chars = ((col.w - 24.0) / 7.0).max(12.0) as usize;
        let pb = ui.line(&crate::util::short_path(&p.dir, chars), path_st, Some(col.w - 24.0));
        ui.draw(&pb, Point::new(col.x + 24.0, fy + (18.0 - pb.h) / 2.0));
        let note = if p.is_local() { "Machine login: can be used here; cannot be removed by Bro." } else { "Remove deletes this profile's login, settings and saved sessions." };
        ui.label_in(Rect::new(col.x + 24.0, fy + 22.0, col.w - 24.0, 18.0), note, Style::new(size::SMALL, weight::REGULAR, t.faint), Align::Left);
    }

    // ---- the keys, quietly
    hints(ui, Point::new(col.x, hints_y), col.w, &[("↑↓", "select"), ("enter", "log in"), ("del", "remove"), ("r", "reload")]);

    // ---- the name prompt or the remove confirmation, over the list
    let card = match (&v.pending, &v.mode) {
        (Some(Pending::Create(kind, name, _)), _) => Some(Card::Add { kind: *kind, name: name.clone(), saving: true }),
        (Some(Pending::Remove(pid, _)), _) => {
            let dir = profiles.as_ref().and_then(|ps| ps.iter().find(|p| &p.id == pid)).map(|p| p.dir.clone());
            Some(Card::Remove { id: pid.clone(), dir, saving: true })
        }
        (None, Mode::Add(kind, name)) => Some(Card::Add { kind: *kind, name: name.clone(), saving: false }),
        (None, Mode::Confirm(p)) => Some(Card::Remove { id: p.id.clone(), dir: Some(p.dir.clone()), saving: false }),
        (None, Mode::List) => None,
    };
    let k = ui.ease(("profiles-card", id), if card.is_some() { 1.0 } else { 0.0 }, 140.0);
    if let Some(card) = card {
        let prev = ui.layer();
        let veil = area.outset(6.0);
        ui.fill(veil, t.scrim.fade(0.66 * k));
        ui.hit_with(veil, Action::View(id, BG), Cursor::Default);
        // centred over the list (over the whole pane when it's narrow), a third of the way down
        let over = if col.w >= 520.0 { Rect::new(col.x, area.y, col.w, area.h) } else { area };
        match card {
            Card::Add { kind, name, saving } => add_card(v, ui, over, id, kind, &name, saving, cx.focused, k),
            Card::Remove { id: pid, dir, saving } => remove_card(ui, over, id, &pid, dir.as_ref(), saving, k),
        }
        ui.set_layer(prev);
    }
    true
}

/// One login: its agent's logo, its id (and "machine login"), then whether it's logged in (a check, or a quiet
/// outline) and its email.
fn profile_row(ui: &mut Ui, r: Rect, p: &Profile, on: bool) {
    let t = ui.t;
    let h = if p.is_codex() { Harness::Codex } else { Harness::Claude };
    let logo = Rect::new(r.x + 12.0, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0);
    ui.mark(brand::harness(h), logo, 20.0, None);
    let x = logo.right() + 14.0;
    let id_st = Style::new(size::BODY, weight::SEMIBOLD, if on { t.text } else { t.text.mix(t.muted, 0.12) });
    let room = r.right() - 12.0 - x;
    let id_w = ui.measure(&p.id, id_st);
    // "machine login" sits beside the id, or joins the line under it when the row is narrow
    let pill_w = if p.is_local() { widgets::pill_width(ui, "machine login") + 8.0 } else { 0.0 };
    let tag_beside = p.is_local() && id_w + pill_w <= room;
    let idb = ui.line(&p.id, id_st, Some(room.max(30.0)));
    let top = r.y + (r.h - idb.h - 2.0 - 16.0) / 2.0;
    ui.draw(&idb, Point::new(x, top));
    if tag_beside {
        widgets::pill(ui, Point::new(x + idb.w + 8.0, top + (idb.h - 16.0) / 2.0), 16.0, "machine login", t.muted, tag_fill(t));
    }
    let y2 = top + idb.h + 2.0;
    let status = if p.authenticated { p.email.as_deref().unwrap_or("Logged in") } else { "Not logged in" };
    let status = if p.is_local() && !tag_beside { format!("{status} · machine login") } else { status.to_string() };
    let (glyph, gc) = if p.authenticated { (CHECK, t.good) } else { (LOGGED_OUT, t.muted) };
    ui.icon(Rect::new(x - 1.0, y2, 14.0, 16.0), glyph, size::SMALL, gc);
    let st = Style::new(size::SMALL, weight::REGULAR, t.muted);
    ui.label_in(Rect::new(x + 17.0, y2, (r.right() - x - 28.0).max(10.0), 16.0), &status, st, Align::Left);
}

/// A neutral tag's fill ("machine login").
fn tag_fill(t: &Tokens) -> Color {
    t.text.with_alpha(if t.light { 0.07 } else { 0.08 })
}

/// The dialog for a new profile's name: the field, the rule or what went wrong, and Create.
#[allow(clippy::too_many_arguments)]
fn add_card(v: &ProfilesView, ui: &mut Ui, area: Rect, id: u64, kind: ProfileKind, name: &str, saving: bool, focused: bool, k: f32) {
    let t = ui.t;
    let w = 460.0f32.min(area.w - 24.0);
    let inner = w - 48.0;
    let family = if kind == ProfileKind::ClaudeAccount { "Claude" } else { "Codex" };
    let harness = if kind == ProfileKind::CodexProfile { Harness::Codex } else { Harness::Claude };
    let missing = v.missing.map(|h| format!("{} isn't installed: Create installs it first, then log in with l.", bro_core::agent_install::display_name(h)));
    let (message, mc) = match (&v.error, &missing) {
        (Some(e), _) => (e.clone(), t.danger),
        (None, Some(m)) => (m.clone(), t.inline),
        (None, None) => ("1–40 letters, digits, . _ - (example: work). 'local' is reserved.".to_string(), t.muted),
    };
    let msg = ui.para(&message, Style::new(size::SMALL, weight::REGULAR, mc), inner, Some(3));
    let buttons = [Btn { icon: None, label: "Create + log in", key: "enter", kind: Kind::Primary, n: CREATE }, Btn { icon: None, label: "Cancel", key: "esc", kind: Kind::Secondary, n: CANCEL }];
    let fit = fit_buttons(ui, inner, &buttons);
    let foot_h = 24.0 + fit.height() + 24.0;
    let h = 24.0 + 28.0 + 16.0 + BUTTON_H + 8.0 + msg.h + 24.0 + foot_h;
    let r = Rect::new(area.x + (area.w - w) / 2.0, area.y + (area.h * 0.34 - h / 2.0).max(16.0) + (1.0 - k) * 8.0, w, h);
    dialog(ui, r, foot_h);
    ui.hit_with(r, Action::View(id, BG), Cursor::Default);
    let x = r.x + 24.0;
    let mut y = r.y + 24.0;
    // the agent's logo before the title, when there's room for both
    let tx = if inner >= 280.0 {
        ui.mark(brand::harness(harness), Rect::new(x, y + 4.0, 20.0, 20.0), 20.0, None);
        x + 30.0
    } else {
        x
    };
    ui.label_in(Rect::new(tx, y, inner - (tx - x), 28.0), &format!("{family} profile name:"), Style::new(TITLE, weight::SEMIBOLD, t.text), Align::Left);
    y += 28.0 + 16.0;
    widgets::field(ui, Rect::new(x, y, inner, BUTTON_H), name, (!saving).then_some(name.len()), "work", focused && !saving, Action::View(id, BG));
    y += BUTTON_H + 8.0;
    ui.draw(&msg, Point::new(x, y));
    let actions = Rect::new(x, r.bottom() - 24.0 - fit.height(), inner, fit.height());
    if saving {
        saving_note(ui, actions.right(), actions.bottom() - BUTTON_H / 2.0);
        return;
    }
    draw_buttons(ui, actions, &buttons, fit, id);
}

/// The dialog that names exactly what removing a profile deletes.
fn remove_card(ui: &mut Ui, area: Rect, id: u64, pid: &str, dir: Option<&PathBuf>, saving: bool, k: f32) {
    let t = ui.t;
    let w = 480.0f32.min(area.w - 24.0);
    let inner = w - 48.0;
    // the alert, then the words beside it
    let text_x = 30.0;
    let title = ui.para(&format!("Remove {pid}?"), Style::new(TITLE, weight::SEMIBOLD, t.text), inner - text_x, Some(2));
    let detail = ui.para("Deletes login, settings and saved sessions.", Style::new(size::BODY, weight::REGULAR, t.muted), inner - text_x, Some(2));
    let path = dir.map(|d| ui.para(&d.display().to_string(), Style::mono(size::SMALL, t.muted), inner - 44.0, Some(3)));
    let well_h = path.as_ref().map_or(0.0, |p| p.h + 16.0);
    let buttons = [Btn { icon: Some(TRASH), label: "Remove", key: "y", kind: Kind::Danger, n: YES }, Btn { icon: None, label: "Cancel", key: "esc", kind: Kind::Secondary, n: CANCEL }];
    let fit = fit_buttons(ui, inner, &buttons);
    let foot_h = 24.0 + fit.height() + 24.0;
    let h = 24.0 + title.h + 4.0 + detail.h + if path.is_some() { 16.0 + well_h } else { 0.0 } + 24.0 + foot_h;
    let r = Rect::new(area.x + (area.w - w) / 2.0, area.y + (area.h * 0.34 - h / 2.0).max(16.0) + (1.0 - k) * 8.0, w, h);
    dialog(ui, r, foot_h);
    ui.hit_with(r, Action::View(id, BG), Cursor::Default);
    let x = r.x + 24.0;
    let mut y = r.y + 24.0;
    // centred on the title's first line
    let first = (TITLE * 1.42).round();
    ui.icon(Rect::new(x, y + (first - 20.0) / 2.0, 20.0, 20.0), ALERT, size::HEADING + 2.0, t.danger);
    ui.draw(&title, Point::new(x + text_x, y));
    ui.draw(&detail, Point::new(x + text_x, y + title.h + 4.0));
    y += title.h + 4.0 + detail.h + 16.0;
    if let Some(p) = &path {
        let well = Rect::new(x, y, inner, p.h + 16.0);
        ui.panel(well, radius::CONTROL, t.sunken, t.hairline);
        ui.icon(Rect::new(well.x + 10.0, well.y + 8.0, 16.0, 17.0), FOLDER, size::BODY, t.muted);
        ui.draw(p, Point::new(well.x + 34.0, well.y + 8.0));
    }
    let actions = Rect::new(x, r.bottom() - 24.0 - fit.height(), inner, fit.height());
    if saving {
        saving_note(ui, actions.right(), actions.bottom() - BUTTON_H / 2.0);
        return;
    }
    draw_buttons(ui, actions, &buttons, fit, id);
}

/// A dialog button.
struct Btn<'a> {
    icon: Option<&'a str>,
    label: &'a str,
    key: &'a str,
    kind: Kind,
    n: usize,
}

/// How a dialog's buttons fit its width: side by side with their keys, side by side without, or stacked.
#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Keys,
    Plain,
    Stacked,
}

impl Fit {
    fn height(self) -> f32 {
        if self == Fit::Stacked { BUTTON_H * 2.0 + 8.0 } else { BUTTON_H }
    }
}

/// Side by side, a dialog's buttons share its width equally (as WinUI's ContentDialog).
fn fit_buttons(ui: &mut Ui, inner: f32, buttons: &[Btn; 2]) -> Fit {
    let half = (inner - 8.0) / 2.0;
    let mut widest = |keys: bool| buttons.iter().map(|b| key_button_width(ui, b.icon, b.label, keys.then_some(b.key))).fold(0.0, f32::max);
    if widest(true) <= half {
        Fit::Keys
    } else if widest(false) <= half {
        Fit::Plain
    } else {
        Fit::Stacked
    }
}

/// A dialog's buttons in `r`: the first (the primary) on the left, or on top when stacked.
fn draw_buttons(ui: &mut Ui, r: Rect, buttons: &[Btn; 2], fit: Fit, id: u64) {
    for (k, b) in buttons.iter().enumerate() {
        let key = (fit == Fit::Keys).then_some(b.key);
        let br = if fit == Fit::Stacked {
            Rect::new(r.x, r.y + k as f32 * (BUTTON_H + 8.0), r.w, BUTTON_H)
        } else {
            let w = (r.w - 8.0) / 2.0;
            Rect::new(r.x + k as f32 * (w + 8.0), r.y, w, BUTTON_H)
        };
        key_button(ui, br, b.icon, b.label, key, b.kind, Some(Action::View(id, b.n)));
    }
}

/// A dialog's card, as WinUI's ContentDialog: the floating card, its buttons on a quieter band (`foot_h` high)
/// under a hairline.
fn dialog(ui: &mut Ui, r: Rect, foot_h: f32) {
    let t = ui.t;
    widgets::card(ui, r, radius::OVERLAY);
    // inside the card's edge, which is a whole number of device pixels
    let edge = ui.scale.round().max(1.0) / ui.scale;
    let foot = Rect::new(r.x + edge, r.bottom() - foot_h, r.w - 2.0 * edge, foot_h - edge);
    ui.round_corners(foot, Corners::bottom(radius::OVERLAY - edge), t.raised.mix(t.bg, if t.light { 0.55 } else { 0.45 }));
    ui.hline(foot.x, foot.y, foot.w, t.hairline);
}

/// "Saving profile…" with a spinner, right-aligned at (`right`, `mid`).
fn saving_note(ui: &mut Ui, right: f32, mid: f32) {
    let t = ui.t;
    let st = Style::new(size::BODY, weight::MEDIUM, t.muted);
    let b = ui.line("Saving profile…", st, None);
    ui.draw(&b, Point::new(right - b.w, mid - b.h / 2.0));
    ui.spinner(Point::new(right - b.w - 14.0, mid), 5.0, 1.7, t.accent);
}

/// Target `n` was clicked: what the terminal UI does for the same button or row.
pub(crate) fn click(v: &mut ProfilesView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    if v.pending.is_some() {
        return;
    }
    if n >= ROW {
        if matches!(v.mode, Mode::List) {
            let i = n - ROW;
            v.sel = i;
            v.selected_id = cx.svc.state().profiles.ready().and_then(|ps| ps.get(i).map(|p| p.id.clone()));
        }
        return;
    }
    let (code, ok) = match n {
        ADD_CLAUDE => (KeyCode::Char('a'), matches!(v.mode, Mode::List)),
        ADD_CODEX => (KeyCode::Char('c'), matches!(v.mode, Mode::List)),
        LOGIN => (KeyCode::Char('l'), matches!(v.mode, Mode::List)),
        REMOVE => (KeyCode::Char('d'), matches!(v.mode, Mode::List)),
        RELOAD => (KeyCode::Char('r'), matches!(v.mode, Mode::List)),
        CREATE => (KeyCode::Enter, matches!(v.mode, Mode::Add(..))),
        YES => (KeyCode::Char('y'), matches!(v.mode, Mode::Confirm(_))),
        CANCEL => (KeyCode::Esc, !matches!(v.mode, Mode::List)),
        _ => return,
    };
    // a button from the last frame only acts in the mode it was drawn for (never types into the name)
    if ok {
        v.key(KeyEvent::new(code, KeyModifiers::NONE), cx);
    }
}

/// The wheel moves the selection, as in the terminal UI (only while the list is in charge).
pub(crate) fn wheel(v: &mut ProfilesView, _n: Option<usize>, down: bool, cx: &mut Cx) {
    let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
    v.mouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE }, ratatui::layout::Rect::default(), cx);
}

// ------------------------------------------------------------------ pieces the view pages share

/// The page column inside a view's area: generous margins, at most `max` wide, left-aligned.
pub(crate) fn page(area: Rect, max: f32) -> Rect {
    let mx = if area.w >= 760.0 {
        32.0
    } else if area.w >= 480.0 {
        24.0
    } else {
        16.0
    };
    let top = if area.h >= 560.0 { 24.0 } else { 16.0 };
    Rect::new(area.x + mx, area.y + top, (area.w - 2.0 * mx).min(max).max(0.0), (area.h - top - 16.0).max(0.0))
}

/// A page's title and the line under it. Returns their height.
pub(crate) fn page_title(ui: &mut Ui, at: Point, title: &str, sub: Option<&str>, max_w: f32) -> f32 {
    let t = ui.t;
    let tb = ui.line(title, Style::new(TITLE, weight::SEMIBOLD, t.text), Some(max_w.max(40.0)));
    ui.draw(&tb, at);
    let mut h = tb.h;
    if let Some(s) = sub {
        let sb = ui.line(s, Style::new(size::BODY, weight::REGULAR, t.muted), Some(max_w.max(40.0)));
        ui.draw(&sb, Point::new(at.x, at.y + h + 2.0));
        h += 2.0 + sb.h;
    }
    h
}

/// A card in a page (a list, a step, the install bar): a step up from the pane it's on, 8px corners and a
/// hairline; no shadow (only floating things cast one).
pub(crate) fn page_card(ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let fill = if t.light { Color::WHITE.mix(t.surface, 0.25) } else { t.surface.mix(t.text, 0.03) };
    ui.panel(r, radius::OVERLAY, fill, t.hairline);
}

/// The terminal UI's key hints for a view, quietly: each key as keycaps and what it does, as many as fit in
/// `max_w`.
pub(crate) fn hints(ui: &mut Ui, at: Point, max_w: f32, items: &[(&str, &str)]) {
    let st = Style::new(size::SMALL, weight::REGULAR, ui.t.faint);
    let mut x = at.x;
    for (key, what) in items {
        let w = keys_width(ui, key);
        let b = ui.line(what, st, None);
        if x + w + 6.0 + b.w > at.x + max_w {
            break;
        }
        keys(ui, Point::new(x, at.y), key, None, 1.0);
        x += w + 6.0;
        ui.draw(&b, Point::new(x, at.y + (18.0 - b.h) / 2.0));
        x += b.w + 16.0;
    }
}

/// Soften where a scrolled list is cut: `r` fades into `bg` toward its top (`top`) or bottom edge, over the
/// text of this layer.
pub(crate) fn edge_fade(ui: &mut Ui, r: Rect, bg: Color, top: bool) {
    let (a, b) = if top { (bg, bg.with_alpha(0.0)) } else { (bg.with_alpha(0.0), bg) };
    let p = ui.px(r);
    ui.scene.top_quad(Quad::new(p, Fill::Vertical(a, b)));
}

/// Keyboard focus on `r` (the selected row when it's a button): a full 1px outline in the accent, just outside
/// it.
pub(crate) fn focus_ring(ui: &mut Ui, r: Rect) {
    ui.outline(r.outset(3.0), radius::CONTROL + 3.0, 1.0, ui.t.accent);
}

/// A key's text: a single character in the terminal font, so `l` reads as a letter.
fn key_style(key: &str, c: Color) -> Style {
    if key.chars().count() == 1 { Style::mono(size::CAPTION, c) } else { Style::new(size::CAPTION, weight::MEDIUM, c) }
}

fn cap_width(ui: &mut Ui, key: &str) -> f32 {
    let st = key_style(key, ui.t.muted);
    (ui.measure(key, st) + 10.0).max(18.0)
}

/// Width [`keys`] takes for `chord`.
pub(crate) fn keys_width(ui: &mut Ui, chord: &str) -> f32 {
    let caps = widgets::chord_keys(chord);
    caps.iter().map(|k| cap_width(ui, k)).sum::<f32>() + 3.0 * caps.len().saturating_sub(1) as f32
}

/// `chord` as keycaps 18 high at `at` (top-left), one per key, as the kit's `kbd` draws them; on a filled
/// button (`on`: its ink) quiet translucent caps instead. `alpha` fades them (a disabled button). Returns the
/// width.
pub(crate) fn keys(ui: &mut Ui, at: Point, chord: &str, on: Option<Color>, alpha: f32) -> f32 {
    let t = ui.t;
    let mut x = at.x;
    for (i, k) in widgets::chord_keys(chord).iter().enumerate() {
        if i > 0 {
            x += 3.0;
        }
        let w = cap_width(ui, k);
        let r = Rect::new(x, at.y, w, 18.0);
        let ink = match on {
            Some(ink) => {
                ui.round(r, radius::CONTROL, ink.with_alpha(0.18 * alpha));
                ink.fade(0.85 * alpha)
            }
            None => {
                ui.panel(r, radius::CONTROL, t.control.fade(alpha), t.stroke.fade(alpha));
                // the heavier bottom edge of a key
                let edge = if t.light { Color::rgba(0.0, 0.0, 0.0, 0.18) } else { Color::rgba(0.0, 0.0, 0.0, 0.45) };
                ui.hline(r.x + 2.0, r.bottom() - ui.hair(), r.w - 4.0, edge.fade(alpha));
                t.muted.fade(alpha)
            }
        };
        let b = ui.line(k, key_style(k, ink), None);
        ui.draw(&b, Point::new(r.x + (w - b.w) / 2.0, r.y + (18.0 - b.h) / 2.0 - 0.5));
        x += w;
    }
    x - at.x
}

/// A key button's padding: left and right, with and without keys (the keycaps sit as far from the edge as from
/// the top).
const PAD_KEYS: (f32, f32) = (12.0, 7.0);
const PAD_PLAIN: (f32, f32) = (14.0, 14.0);
/// An icon's box, and the gaps after it and before the keys.
const ICON_W: f32 = 16.0;
const ICON_GAP: f32 = 8.0;
const KEYS_GAP: f32 = 10.0;

/// Width a [`key_button`] wants.
pub(crate) fn key_button_width(ui: &mut Ui, icon: Option<&str>, label: &str, key: Option<&str>) -> f32 {
    let (l, r) = if key.is_some() { PAD_KEYS } else { PAD_PLAIN };
    let mut w = l + r;
    if icon.is_some() {
        w += ICON_W + if label.is_empty() { 0.0 } else { ICON_GAP };
    }
    if !label.is_empty() {
        w += ui.measure(label, Style::new(size::BODY, weight::MEDIUM, ui.t.text));
    }
    if let Some(k) = key {
        w += KEYS_GAP + keys_width(ui, k);
    }
    w
}

/// A button (WinUI's: 4px corners; accent, standard with its edge, or subtle) with an optional icon before its
/// label and its key as keycaps after it. No action: drawn faded and takes no clicks. Returns the hover amount.
#[allow(clippy::too_many_arguments)]
pub(crate) fn key_button(ui: &mut Ui, r: Rect, icon: Option<&str>, label: &str, key: Option<&str>, kind: Kind, action: Option<Action>) -> f32 {
    key_button_ink(ui, r, icon, label, key, kind, None, action)
}

/// [`key_button`] with its icon and label in `ink` (a quiet destructive action).
#[allow(clippy::too_many_arguments)]
pub(crate) fn key_button_ink(ui: &mut Ui, r: Rect, icon: Option<&str>, label: &str, key: Option<&str>, kind: Kind, ink: Option<Color>, action: Option<Action>) -> f32 {
    let t = ui.t;
    let enabled = action.is_some();
    let hot = if enabled { ui.hover_amount(("key-button", label, key, r.x as i32, r.y as i32), r) } else { 0.0 };
    let down = enabled && ui.pressing && hot > 0.5;
    let alpha = if enabled { 1.0 } else { 0.4 };
    let base_ink = match kind {
        Kind::Primary => {
            let fill = if down { t.accent.fade(0.8) } else { t.accent.fade(1.0 - 0.1 * hot) };
            ui.round(r, radius::CONTROL, fill.fade(alpha));
            // the accent button's darker lower edge
            let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
            ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, Color::rgba(0.0, 0.0, 0.0, 0.18 * alpha));
            if down { t.on_accent.fade(0.8) } else { t.on_accent }
        }
        Kind::Danger => {
            let fill = if down { t.danger.fade(0.8) } else { t.danger.fade(1.0 - 0.1 * hot) };
            ui.round(r, radius::CONTROL, fill.fade(alpha));
            t.ink_on(t.danger)
        }
        Kind::Secondary => {
            let fill = if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) };
            widgets::control(ui, r, fill.fade(alpha));
            if down { t.muted } else { t.text }
        }
        Kind::Ghost => {
            if hot > 0.0 {
                ui.round(r, radius::CONTROL, t.text.with_alpha(if down { 0.04 } else { 0.06 * hot }));
            }
            t.text.mix(t.muted, 0.3 * (1.0 - hot))
        }
    };
    let ink = ink.unwrap_or(base_ink).fade(alpha);
    let (pad_l, pad_r) = if key.is_some() { PAD_KEYS } else { PAD_PLAIN };
    let keys_w = key.map(|k| keys_width(ui, k));
    let icon_w = icon.map_or(0.0, |_| ICON_W + if label.is_empty() { 0.0 } else { ICON_GAP });
    let keys_room = keys_w.map_or(0.0, |w| KEYS_GAP + w);
    // a button narrower than it wants cuts its label short
    let room = r.w - pad_l - pad_r - icon_w - keys_room;
    let lb = (!label.is_empty()).then(|| ui.line(label, Style::new(size::BODY, weight::MEDIUM, ink), Some(room.max(8.0))));
    let content = icon_w + lb.as_ref().map_or(0.0, |b| b.w) + keys_room;
    let mid = r.y + r.h / 2.0;
    // centred (a dialog's wide buttons), never nearer the edge than the padding
    let mut x = r.x + ((r.w - content) / 2.0).max(pad_l);
    if let Some(g) = icon {
        ui.icon(Rect::new(x, mid - 9.0, ICON_W, 18.0), g, size::TITLE, ink);
        x += icon_w;
    }
    if let Some(b) = &lb {
        ui.draw(b, Point::new(x, mid - b.h / 2.0));
        x += b.w;
    }
    if let Some(k) = key {
        x += KEYS_GAP;
        let on = matches!(kind, Kind::Primary | Kind::Danger).then_some(base_ink);
        keys(ui, Point::new(x, mid - 9.0), k, on, alpha);
    }
    if let Some(a) = action {
        ui.hit(r, a);
    }
    hot
}
