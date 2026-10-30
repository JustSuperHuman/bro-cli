//! The launcher in the desktop app: "New session" / "Continue a session" as a page over the body, with the
//! sidebar beside it still live.
//!
//! The project the session starts in is an address bar across the top, as File Explorer's: a folder, the path as
//! breadcrumbs, the key that changes it and a chevron; the project picker hangs from it (`folder/gui.rs`). Under
//! it, the New / Continue switch on the left and profiles on the right; close sits in the top corner. The New tab
//! shows the agents as tiles (★ orchestrator set apart in gold; on its tab, the harness it runs on), then the
//! "run on" list beside what launches and its options, and the launch button bottom-right with the keys beside
//! it. Narrow bodies stack the options under the list (the terminal UI's breakpoints). The Continue tab is
//! `continue_picker/gui.rs`.
//!
//! Keys never come here: they reach [`Launcher::key`]. This draws that state machine's state and maps each
//! click to what [`Launcher::click`] does for the same target.

use super::{AccountKind, Focus, Item, Launcher, Outcome, SessionTab};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::{self, Mark};
use crate::app::gui::kit::{self, Cursor, Style, Tokens, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::continue_picker::gui as cont;
use crate::folder::{self, gui as picker};
use crate::pane::Place;
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::catalogue::ModelRow;
use bro_core::launch::{LaunchSpec, Permission};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;

// ------------------------------------------------------------------ targets (`Action::Overlay(n)`)

const CLOSE: usize = 1;
const PROFILES: usize = 2;
const PROJECT: usize = 3;
const TAB_NEW: usize = 4;
const TAB_CONTINUE: usize = 5;
const LAUNCH: usize = 6;
const INSTALL: usize = 7;
const ORCHESTRATOR: usize = 8;
const LIST_FIELD: usize = 9;
const MODEL_FIELD: usize = 10;
/// the run-on list's background (the wheel there moves in it)
const LIST_AREA: usize = 11;
const MODEL_AREA: usize = 12;
const NO_PROJECT: usize = 13;
/// + the agent's index in `Harness::ALL`
const HARNESS: usize = 20;
/// + the index in `Harness::ALL` of the harness the orchestrator runs on
const BRAIN: usize = 30;
/// + the index in [`PERMS`]
const PERM: usize = 40;
/// + the index in [`BROWSERS`]
const BROWSER: usize = 50;
/// + 0 new session, 1 split
const PLACE: usize = 60;
/// Each list gets a block of numbers.
const SPAN: usize = 1_000_000;
/// + the index among the run-on list's selectable rows
const ROW: usize = SPAN;
/// + the index in the filtered models
const MODEL: usize = 2 * SPAN;
/// + the Continue tab's own targets (`continue_picker::gui`)
const CONTINUE: usize = 3 * SPAN;
/// + the project picker's own targets (`folder::gui`)
const PICKER: usize = 4 * SPAN;

const PERMS: [(Permission, &str); 3] = [(Permission::Default, "Ask"), (Permission::Auto, "Auto"), (Permission::Skip, "Skip all")];
const BROWSERS: [(BrowserMode, &str); 4] = [(BrowserMode::Off, "Off"), (BrowserMode::Auto, "On"), (BrowserMode::Edge, "Edge"), (BrowserMode::Chrome, "Chrome")];

// ------------------------------------------------------------------ metrics and icons

/// The page's padding.
const PAD: f32 = 24.0;
/// The address bar across the top.
const BAR_H: f32 = 36.0;
/// The row under it: New / Continue and profiles.
const TABS_H: f32 = 32.0;
const TILE_H: f32 = 40.0;
/// a run-on row
const ROW_H: f32 = 36.0;
/// a model row
const MODEL_H: f32 = 32.0;
pub(crate) const FIELD_H: f32 = 32.0;
/// The bottom row: the keys on the left, the main button on the right.
pub(crate) const FOOTER_H: f32 = 40.0;
/// Below this the options go under the list instead of beside it (the terminal UI's 84 × 11 cells).
const SIDE_MIN_W: f32 = 760.0;
const SIDE_MIN_H: f32 = 300.0;
/// The options block: its heading and three rows.
const OPTIONS_H: f32 = 30.0 + 3.0 * 40.0;
/// A breadcrumb separator: a small chevron with room either side.
const CRUMB_SEP: f32 = 22.0;
/// Your home folder's breadcrumb (a house).
const HOME_W: f32 = 16.0;
/// Where a list row's mark sits and where its text starts (the search fields above line up with them).
pub(crate) const MARK_X: f32 = 12.0;
pub(crate) const TEXT_X: f32 = 40.0;

pub(crate) const I_CLOSE: &str = "\u{F0156}";
const I_PLUS: &str = "\u{F0415}";
const I_HISTORY: &str = "\u{F02DA}";
pub(crate) const I_FOLDER: &str = "\u{F0256}";
const I_FOLDER_OPEN: &str = "\u{F0DCF}";
const I_HOME: &str = "\u{F06A1}";
const I_ACCOUNT: &str = "\u{F0004}";
pub(crate) const I_SEARCH: &str = "\u{F0349}";
const I_STAR: &str = "\u{F04CE}";
pub(crate) const I_ALERT: &str = "\u{F0026}";
const I_DOWNLOAD: &str = "\u{F01DA}";
const I_RETURN: &str = "\u{F0311}";
pub(crate) const I_CHECK: &str = "\u{F012C}";
const I_CHEVRON_RIGHT: &str = "\u{F0142}";
const I_CHEVRON_DOWN: &str = "\u{F0140}";
const I_CHEVRON_UP: &str = "\u{F0143}";
/// the orchestrator is running
const I_PULSE: &str = "\u{F0430}";

/// What's left of a login, coloured as the sidebar colours it: ≤20 % danger, ≤50 % amber (`warn`), else good.
fn left_color(left: f64, t: &Tokens) -> Color {
    if left <= 20.0 {
        t.danger
    } else if left <= 50.0 {
        t.warn
    } else {
        t.good
    }
}

/// A provider's logo, else the terminal UI's symbol for it (in its colour; unknown ones muted).
pub(crate) fn provider_mark(id: &str, ui: &Ui) -> Mark {
    brand::provider(id).unwrap_or_else(|| {
        let (g, c) = crate::ui::provider_glyph(id, ui.theme.muted, ui.theme);
        Mark::Glyph(g, Some(kit::color(c, ui.t.muted, &ui.t.ansi)))
    })
}

/// A group heading in sentence case: "your logins" → "Your logins".
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// `n` as an index into the block starting at `base`.
fn within(n: usize, base: usize) -> Option<usize> {
    n.checked_sub(base).filter(|k| *k < SPAN)
}

// ------------------------------------------------------------------ clicks and the wheel

/// Target `n` was clicked: what `Launcher::click` does for the same target (`count` 2 = a double click, which
/// the state machine already reads as "the selected row again").
pub(crate) fn click(l: &mut Launcher, n: usize, _p: Point, mods: KeyModifiers, _count: u32) -> Outcome {
    // the project picker takes every click: a row picks and opens it, as Enter would; the rest do nothing
    if let Some(p) = &mut l.project_picker {
        let outcome = match within(n, PICKER) {
            Some(picker::CLOSE) => folder::Outcome::Close,
            Some(k) if k >= picker::ROW => match picker::click_row(p, k - picker::ROW) {
                Some(folder::Outcome::None) if p.error.is_none() => p.key(key(KeyCode::Enter)),
                Some(o) => o,
                None => folder::Outcome::None,
            },
            _ => folder::Outcome::None,
        };
        return l.project_outcome(outcome);
    }
    if let Some(k) = within(n, CONTINUE) {
        return Launcher::continue_outcome(cont::click(&mut l.continue_picker, k, mods));
    }
    if let Some(i) = within(n, ROW) {
        if i >= Launcher::selectable(&l.items()).len() {
            return Outcome::None;
        }
        // the selected row again acts like Enter
        if l.focus == Focus::List && l.list_sel == i {
            return l.key(key(KeyCode::Enter));
        }
        l.focus = Focus::List;
        if l.list_sel != i {
            l.list_sel = i;
            l.reset_models();
        }
        return Outcome::None;
    }
    if let Some(k) = within(n, MODEL) {
        if k >= l.models_view().len() {
            return Outcome::None;
        }
        if l.focus == Focus::Models && l.model_sel == k {
            return l.launch();
        }
        l.focus = Focus::Models;
        l.model_sel = k;
        return Outcome::None;
    }
    match n {
        CLOSE => return Outcome::Close,
        PROFILES => return Outcome::Profiles,
        PROJECT => l.open_project_picker(),
        NO_PROJECT if l.orchestrator => l.select_no_project(),
        TAB_NEW => l.tab = SessionTab::New,
        TAB_CONTINUE => l.tab = SessionTab::Continue,
        LAUNCH => return l.key(key(KeyCode::Enter)),
        INSTALL => return Outcome::Install(l.harness),
        ORCHESTRATOR => {
            if !l.orchestrator {
                l.enter_orchestrator();
            }
        }
        // a field takes the keyboard to its list (as tab / esc do)
        LIST_FIELD => l.focus = Focus::List,
        MODEL_FIELD => {
            if l.needs_model() {
                l.focus = Focus::Models;
            }
        }
        n if (HARNESS..HARNESS + Harness::ALL.len()).contains(&n) => {
            let h = Harness::ALL[n - HARNESS];
            if l.orchestrator || l.harness != h {
                let current = if l.orchestrator { Harness::ALL.len() } else { Harness::ALL.iter().position(|candidate| *candidate == l.harness).unwrap_or(0) };
                l.set_harness((n - HARNESS) as i32 - current as i32);
            }
        }
        n if (BRAIN..BRAIN + Harness::ALL.len()).contains(&n) => {
            let h = Harness::ALL[n - BRAIN];
            if l.harness != h {
                l.set_brain(h);
            }
        }
        n if (PERM..PERM + PERMS.len()).contains(&n) => l.permission = PERMS[n - PERM].0,
        // the terminal UI cycles these with ctrl+b / ctrl+s; a choice here cycles to it
        n if (BROWSER..BROWSER + BROWSERS.len()).contains(&n) => {
            let want = BROWSERS[n - BROWSER].0;
            for _ in 0..BROWSERS.len() {
                if l.browser == want {
                    break;
                }
                l.key(ctrl('b'));
            }
        }
        // the other placement than the current one
        n if (PLACE..PLACE + 2).contains(&n) && (l.place == Place::Tab) != (n == PLACE) => {
            l.key(ctrl('s'));
        }
        _ => {}
    }
    Outcome::None
}

/// The wheel over target `n` (None: elsewhere over the launcher): like `Launcher::scroll`, it moves the selection
/// of the list under the pointer three rows.
pub(crate) fn wheel(l: &mut Launcher, n: Option<usize>, down: bool) {
    let code = if down { KeyCode::Down } else { KeyCode::Up };
    if let Some(p) = &mut l.project_picker {
        for _ in 0..3 {
            p.key(key(code));
        }
        return;
    }
    if l.tab == SessionTab::Continue {
        for _ in 0..3 {
            l.continue_picker.key(key(code));
        }
        return;
    }
    match n {
        Some(n) if n == MODEL_AREA || n == MODEL_FIELD || within(n, MODEL).is_some() => l.focus = Focus::Models,
        Some(n) if n == LIST_AREA || n == LIST_FIELD || within(n, ROW).is_some() => l.focus = Focus::List,
        _ => {}
    }
    l.move_sel(if down { 3 } else { -3 });
}

// ------------------------------------------------------------------ the page

/// Draw the launcher over `area` (the body, right of the sidebar and below the project tabs).
pub(crate) fn draw(l: &mut Launcher, ui: &mut Ui, area: Rect, cx: &Cx) -> bool {
    let t = ui.t;
    // A page on the content layer: the card colour, 8px corners, and a hairline edge. Its nearly opaque fill
    // keeps the launcher readable over session output and the desktop, including on a glass window.
    ui.panel(area, radius::OVERLAY, t.surface.with_alpha(0.97), t.hairline);
    ui.block(area);
    let inner = area.inset(PAD);
    let (mut bar, close, tabs) = head(inner);
    let mut no_project_button = None;
    if l.orchestrator && l.tab == SessionTab::New {
        let w = widgets::button_width(ui, "Orchestrator");
        let button = Rect::new(bar.right() - w, bar.y, w, bar.h);
        no_project_button = Some(button);
        bar.w = (bar.w - w - 8.0).max(0.0);
        widgets::button(ui, button, "Orchestrator", if l.no_project { widgets::Kind::Secondary } else { widgets::Kind::Ghost }, Action::Overlay(NO_PROJECT));
        if l.no_project {
            ui.outline(button, radius::CONTROL, 1.0, t.gold);
        }
        if ui.pointer.is_some_and(|p| button.contains(p)) {
            widgets::tooltip(ui, button, "Coordinate across projects · alt+g");
        }
    }
    address_bar(ui, &l.dir, l.orchestrator && l.no_project && l.tab == SessionTab::New, bar, false, Action::Overlay(PROJECT), Some("alt+p"));
    if widgets::icon_button(ui, close, I_CLOSE, t.muted, Action::Overlay(CLOSE)) {
        widgets::tooltip(ui, close, "Close · esc");
    }
    let tips = draw_tabs(l, ui, tabs);
    let body = Rect::new(inner.x, tabs.bottom() + 20.0, inner.w, (inner.bottom() - tabs.bottom() - 20.0).max(0.0));
    // while the project picker is open it has the keyboard: no caret on the page beneath
    let page_cx = Cx { focused: cx.focused && l.project_picker.is_none(), ..*cx };
    match l.tab {
        SessionTab::Continue => cont::draw(&mut l.continue_picker, ui, body, CONTINUE, &page_cx),
        SessionTab::New => draw_new(l, ui, body, &page_cx),
    }
    // the project picker hangs from the address bar over everything; the bar stays lit above the dimmed page
    match &mut l.project_picker {
        Some(p) => {
            let prev = ui.layer();
            let k = picker::opening(ui);
            let screen = ui.screen;
            widgets::scrim(ui, screen, k, Action::None);
            address_bar(ui, &l.dir, l.orchestrator && l.no_project, bar, true, Action::None, Some("alt+p"));
            picker::draw(p, ui, bar, area, &l.dir, &|n| Action::Overlay(PICKER + n), cx.focused, k);
            ui.set_layer(prev);
        }
        None => picker::closed(ui),
    }
    // alt held: the launcher's own alt keys, as key tips on what they reach
    if cx.alt && l.project_picker.is_none() {
        let prev = ui.layer();
        widgets::keytip(ui, Point::new(bar.x + 22.0, bar.bottom() - 2.0), "p");
        if let Some(button) = no_project_button {
            widgets::keytip(ui, Point::new(button.x + button.w / 2.0, button.bottom() - 2.0), "g");
        }
        for (r, k) in tips {
            widgets::keytip(ui, Point::new(r.x + r.w / 2.0, r.bottom() - 2.0), k);
        }
        ui.set_layer(prev);
    }
    true
}

/// The address bar and close across the top of `inner`, and the row under them.
fn head(inner: Rect) -> (Rect, Rect, Rect) {
    let close = Rect::new(inner.right() - BAR_H, inner.y, BAR_H, BAR_H);
    let bar = Rect::new(inner.x, inner.y, (close.x - 8.0 - inner.x).max(0.0), BAR_H);
    let tabs = Rect::new(inner.x, bar.bottom() + 12.0, inner.w, TABS_H);
    (bar, close, tabs)
}

/// The project as an address bar, as File Explorer's: a folder, the path as breadcrumbs, and on the right the key
/// that changes it and a chevron. Clicking anywhere on it opens the project picker; `open` draws it lit (an
/// outline, the chevron up) while the picker hangs from it. A coordinator can choose any project folder.
pub(crate) fn address_bar(ui: &mut Ui, dir: &Path, no_project: bool, r: Rect, open: bool, action: Action, key: Option<&str>) {
    let t = ui.t;
    if r.w < 80.0 {
        return;
    }
    let icon = Rect::new(r.x + MARK_X, r.y, 18.0, r.h);
    let hot = if open { 0.0 } else { ui.hover_amount("launcher-address", r) };
    let down = !open && ui.pressing && hot > 0.5;
    if open {
        // over the scrim: the page's colour first, so the dimmed bar beneath doesn't show through the control fill
        ui.round(r, radius::CONTROL, t.surface);
    }
    widgets::control(ui, r, if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) });
    if open {
        ui.outline(r, radius::CONTROL, 1.0, t.accent.with_alpha(0.8));
    }
    ui.icon(icon, if open { I_FOLDER_OPEN } else { I_FOLDER }, size::TITLE + 1.0, t.muted.mix(t.text, if open { 0.75 } else { 0.35 + 0.4 * hot }));
    // the right end: the key that changes it, then the chevron
    let live = open || !matches!(action, Action::None);
    let chev = Rect::new(r.right() - 34.0, r.y, 24.0, r.h);
    if live {
        ui.icon(chev, if open { I_CHEVRON_UP } else { I_CHEVRON_DOWN }, size::TITLE, t.muted.mix(t.text, 0.6 * hot));
    }
    let kw = key.map_or(0.0, |k| kbd_w(ui, k));
    let mut end = if live { chev.x - 8.0 } else { r.right() - 12.0 };
    if let Some(key) = key.filter(|_| live && r.w >= 280.0) {
        let kx = chev.x - 6.0 - kw;
        widgets::kbd(ui, Point::new(kx, r.y + (r.h - 18.0) / 2.0), key);
        end = kx - 14.0;
    }
    let x = r.x + TEXT_X;
    let text = Rect::new(x, r.y, (end - x).max(0.0), r.h);
    if no_project {
        ui.label_in(text, "Choose a project folder", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
    } else {
        crumbs(ui, text, &path_parts(dir), t.text);
    }
    if !open && live {
        ui.hit(r, action);
    }
}

/// A path as the parts an address bar shows: "~" for your home, then each folder ("F:\code\bro" → F:, code,
/// bro; "/opt/x" → /, opt, x).
fn path_parts(p: &Path) -> Vec<String> {
    use std::path::Component;
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Prefix(x) => Some(x.as_os_str().to_string_lossy().into_owned()),
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                Component::ParentDir => Some("..".into()),
                Component::RootDir | Component::CurDir => None,
            })
            .collect()
    };
    let all = parts(p);
    let same = |a: &String, b: &String| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    if let Some(home) = dirs::home_dir() {
        let h = parts(&home);
        if !h.is_empty() && all.len() >= h.len() && all.iter().zip(&h).all(|(a, b)| same(a, b)) {
            return std::iter::once("~".to_string()).chain(all[h.len()..].iter().cloned()).collect();
        }
    }
    // a Unix root shows as "/"
    if p.has_root() && !matches!(p.components().next(), Some(Component::Prefix(_))) {
        return std::iter::once("/".to_string()).chain(all).collect();
    }
    all
}

/// `parts` as breadcrumbs in `r` ("F:  ›  code  ›  bro-cli-v2"): the earlier ones muted, the last semibold in
/// `last`. When they don't fit, the middle gives way to "…" (then the first part too; then the last is cut short).
fn crumbs(ui: &mut Ui, r: Rect, parts: &[String], last: Color) {
    let t = ui.t;
    let n = parts.len();
    if n == 0 || r.w < 24.0 {
        return;
    }
    let head = Style::new(size::BODY, weight::REGULAR, t.muted);
    let tail = Style::new(size::BODY, weight::SEMIBOLD, last);
    let style = |i: usize| if i + 1 == n { tail } else { head };
    // your home is a house rather than a "~" (easier to take in at a glance)
    let home = |i: usize| parts[i] == "~";
    let widths: Vec<f32> = parts.iter().enumerate().map(|(i, s)| if home(i) { HOME_W } else { ui.measure(s, style(i)) }).collect();
    let ell = ui.measure("…", head);
    let plan = crumb_plan(&widths, ell, r.w);
    let mut x = r.x;
    for (j, part) in plan.iter().enumerate() {
        if j > 0 {
            ui.icon(Rect::new(x, r.y, CRUMB_SEP, r.h), I_CHEVRON_RIGHT, size::SMALL, t.faint);
            x += CRUMB_SEP;
        }
        let (text, st) = match part {
            Some(i) if home(*i) => {
                ui.icon(Rect::new(x, r.y, HOME_W, r.h), I_HOME, size::TITLE + 1.0, style(*i).color);
                x += HOME_W;
                continue;
            }
            Some(i) => (parts[*i].as_str(), style(*i)),
            None => ("…", head),
        };
        let b = ui.line(text, st, Some((r.right() - x).max(1.0)));
        ui.draw(&b, Point::new(x, r.y + (r.h - b.h) / 2.0));
        x += b.w;
    }
}

/// Which breadcrumbs fit `room` (each part `widths[i]` wide, the "…" `ell`, a separator between each): the parts
/// to show in order, None for the "…". All of them when they fit; else the first, "…" and as many of the last
/// ones as fit; else "…" and the last; else the last alone (cut short as it's drawn).
fn crumb_plan(widths: &[f32], ell: f32, room: f32) -> Vec<Option<usize>> {
    let n = widths.len();
    if n == 0 {
        return vec![];
    }
    let width = |plan: &[Option<usize>]| plan.iter().map(|p| p.map(|i| widths[i]).unwrap_or(ell)).sum::<f32>() + CRUMB_SEP * plan.len().saturating_sub(1) as f32;
    let mut plans: Vec<Vec<Option<usize>>> = vec![(0..n).map(Some).collect()];
    for k in (1..n.saturating_sub(1)).rev() {
        plans.push([Some(0), None].into_iter().chain((n - k..n).map(Some)).collect());
    }
    if n > 1 {
        plans.push(vec![None, Some(n - 1)]);
    }
    plans.into_iter().find(|p| width(p) <= room).unwrap_or_else(|| vec![Some(n - 1)])
}

/// Under the address bar: New / Continue on the left, profiles on the right.
/// The New session / Continue switch and Profiles; returns where their alt keys reach (for key tips).
fn draw_tabs(l: &Launcher, ui: &mut Ui, r: Rect) -> Vec<(Rect, &'static str)> {
    let segs = [Seg::new("New session").icon(I_PLUS), Seg::new("Continue").icon(I_HISTORY)];
    let on = if l.tab == SessionTab::New { 0 } else { 1 };
    let sw = segmented(ui, Point::new(r.x, r.y), r.h, &segs, Some(on), size::BODY, "launcher-tab", |i| Action::Overlay(if i == 0 { TAB_NEW } else { TAB_CONTINUE }));
    let w0 = seg_one(ui, &segs[0], size::BODY);
    let w1 = seg_one(ui, &segs[1], size::BODY);
    let profiles = profiles_button(ui, r, r.w - sw - 16.0);
    vec![(Rect::new(r.x + 2.0, r.y, w0, r.h), "n"), (Rect::new(r.x + 2.0 + w0, r.y, w1, r.h), "r"), (profiles, "o")]
}

/// "Profiles alt+o" at the right end of `r` as a subtle button (a fill only under the pointer), shrinking to its
/// icon when the row is tight.
fn profiles_button(ui: &mut Ui, r: Rect, room: f32) -> Rect {
    let t = ui.t;
    let st = Style::new(size::BODY, weight::MEDIUM, t.text);
    let lw = ui.measure("Profiles", st);
    let kw = kbd_w(ui, "alt+o");
    let full = 10.0 + 16.0 + 8.0 + lw + 12.0 + kw + 8.0;
    let short = 10.0 + 16.0 + 8.0 + lw + 12.0;
    if room < short {
        let b = Rect::new(r.right() - r.h, r.y, r.h, r.h);
        if widgets::icon_button(ui, b, I_ACCOUNT, t.muted, Action::Overlay(PROFILES)) {
            widgets::tooltip(ui, b, "Profiles · alt+o");
        }
        return b;
    }
    let w = if room >= full { full } else { short };
    let b = Rect::new(r.right() - w, r.y, w, r.h);
    let hot = ui.hover_amount("launcher-profiles", b);
    let down = ui.pressing && hot > 0.5;
    if hot > 0.0 {
        ui.round(b, radius::CONTROL, t.text.with_alpha(if down { 0.04 } else { 0.06 * hot }));
    }
    let ink = t.muted.mix(t.text, 0.45 + 0.55 * hot);
    ui.icon(Rect::new(b.x + 10.0, b.y, 16.0, b.h), I_ACCOUNT, size::TITLE, ink);
    ui.label_in(Rect::new(b.x + 34.0, b.y, lw + 2.0, b.h), "Profiles", st.color(ink), Align::Left);
    if w >= full {
        widgets::kbd(ui, Point::new(b.x + 34.0 + lw + 12.0, b.y + (b.h - 18.0) / 2.0), "alt+o");
    }
    ui.hit(b, Action::Overlay(PROFILES));
    b
}

/// The longest short form of `p` (`~` for home, middle parts elided) that fits `max_w`; empty when none does.
pub(crate) fn fit_path(ui: &mut Ui, p: &std::path::Path, st: Style, max_w: f32) -> String {
    for n in [60, 48, 40, 32, 26, 20, 14] {
        let s = crate::util::short_path(p, n);
        if ui.measure(&s, st) <= max_w {
            return s;
        }
    }
    String::new()
}

// ------------------------------------------------------------------ the New tab

/// What the New tab shows this frame, asked of the state machine once (its lists are rebuilt on every call, and
/// the window redraws often while a caret blinks).
struct Shown<'a> {
    l: &'a Launcher,
    items: Vec<Item>,
    /// the cursor's position in `items`
    cursor: Option<usize>,
    needs: bool,
    /// the models the chosen row can run, and the filtered view of them
    models: &'a [ModelRow],
    view: Vec<usize>,
    spec: Option<LaunchSpec>,
    installed: bool,
    sign_in: bool,
}

impl<'a> Shown<'a> {
    fn of(l: &'a Launcher) -> Shown<'a> {
        let items = l.items();
        let cursor = l.cursor_row(&items);
        let needs = l.needs_model();
        Shown {
            l,
            cursor,
            needs,
            models: l.models(),
            view: if needs { l.models_view() } else { vec![] },
            spec: l.spec(),
            installed: l.installed(l.harness),
            sign_in: l.sign_in_needed(),
            items,
        }
    }

    /// The row under the cursor.
    fn chosen(&self) -> Option<&Item> {
        self.cursor.and_then(|k| self.items.get(k))
    }
}

fn draw_new(l: &Launcher, ui: &mut Ui, r: Rect, cx: &Cx) {
    let s = Shown::of(l);
    let mut y = r.y;
    y += draw_agents(l, ui, Rect::new(r.x, y, r.w, TILE_H));
    if l.orchestrator {
        y += 12.0;
        y += draw_brains(l, ui, Rect::new(r.x, y, r.w, 28.0));
    }
    if !s.installed {
        y += 12.0;
        draw_install(l, ui, Rect::new(r.x, y, r.w, 36.0));
        y += 36.0;
    }
    y += 24.0;
    let footer = Rect::new(r.x, r.bottom() - FOOTER_H, r.w, FOOTER_H);
    let body = Rect::new(r.x, y, r.w, (footer.y - 22.0 - y).max(0.0));
    if body.w >= SIDE_MIN_W && body.h >= SIDE_MIN_H {
        draw_side(&s, ui, body, cx);
    } else {
        draw_stacked(&s, ui, body, cx);
    }
    draw_footer(&s, ui, footer);
}

/// A choice tile's box (the agents, the harness the orchestrator runs on): a standard control, or once chosen
/// the selected fill inside a 1px outline in `tone`. Never colour behind the mark.
fn choice(ui: &mut Ui, r: Rect, on: bool, hot: f32, tone: Color) {
    let t = ui.t;
    if on {
        ui.panel(r, radius::CONTROL, t.selected, tone);
    } else {
        let down = ui.pressing && hot > 0.5;
        widgets::control(ui, r, if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) });
    }
}

/// The agents as tiles, their real logos on the tile's own fill; ★ orchestrator set apart in gold. Wraps on
/// narrow pages; returns the height used.
fn draw_agents(l: &Launcher, ui: &mut Ui, r: Rect) -> f32 {
    let t = ui.t;
    let ls = Style::new(size::BODY, weight::SEMIBOLD, t.text);
    let gap = 8.0;
    let mut x = r.x;
    let mut y = r.y;
    for (i, h) in Harness::ALL.into_iter().enumerate() {
        let on = h == l.harness && !l.orchestrator;
        let installed = l.installed(h);
        let lw = ui.measure(h.label(), ls);
        let w = (14.0 + 20.0 + 10.0 + lw + 18.0 + if installed { 0.0 } else { 22.0 }).max(108.0);
        if x + w > r.right() && x > r.x {
            x = r.x;
            y += TILE_H + gap;
        }
        let tile = Rect::new(x, y, w, TILE_H);
        let hot = ui.hover_amount(("launcher-agent", i), tile);
        choice(ui, tile, on, hot, t.accent.with_alpha(0.85));
        ui.mark(brand::harness(h), Rect::new(tile.x + 14.0, tile.y, 20.0, TILE_H), 18.0, (!installed).then_some(t.faint));
        let ink = if on {
            t.text
        } else if installed {
            t.text.mix(t.muted, 0.3 - 0.3 * hot)
        } else {
            t.muted
        };
        ui.label_in(Rect::new(tile.x + 44.0, tile.y, lw + 4.0, TILE_H), h.label(), ls.color(ink), Align::Left);
        if !installed {
            let d = Rect::new(tile.right() - 30.0, tile.y, 18.0, TILE_H);
            ui.icon(d, I_DOWNLOAD, size::BODY, t.faint);
            if ui.hovered(tile) {
                widgets::tooltip(ui, tile, &format!("{} isn't installed yet", h.label()));
            }
        }
        ui.hit(tile, Action::Overlay(HARNESS + i));
        x += w + gap;
    }
    // ★ orchestrator, a little apart
    let running = l.running_orchestrator().is_some();
    let lw = ui.measure("orchestrator", ls);
    let w = 14.0 + 20.0 + 10.0 + lw + 18.0 + if running { 20.0 } else { 0.0 };
    let apart = 25.0;
    if x + apart + w > r.right() && x > r.x {
        x = r.x;
        y += TILE_H + gap;
    } else if x > r.x {
        ui.vline(x - gap + apart / 2.0, y + 10.0, TILE_H - 20.0, t.border);
        x += apart - gap + 8.0;
    }
    let tile = Rect::new(x, y, w, TILE_H);
    let hot = ui.hover_amount("launcher-orchestrator", tile);
    if l.orchestrator {
        ui.panel(tile, radius::CONTROL, t.gold.with_alpha(0.1), t.gold.with_alpha(0.75));
    } else {
        choice(ui, tile, false, hot, t.gold);
    }
    ui.icon(Rect::new(tile.x + 14.0, tile.y, 20.0, TILE_H), I_STAR, 18.0, t.gold);
    ui.label_in(Rect::new(tile.x + 44.0, tile.y, lw + 4.0, TILE_H), "orchestrator", ls.color(t.gold), Align::Left);
    if running {
        let at = Rect::new(tile.right() - 32.0, tile.y, 18.0, TILE_H);
        ui.icon(at, I_PULSE, size::TITLE, t.gold);
        if ui.hovered(tile) {
            widgets::tooltip(ui, tile, "running");
        }
    }
    ui.hit(tile, Action::Overlay(ORCHESTRATOR));
    y + TILE_H - r.y
}

/// The orchestrator tab's second row: the harness it runs on, and how it reaches your sessions.
fn draw_brains(l: &Launcher, ui: &mut Ui, r: Rect) -> f32 {
    let t = ui.t;
    let lead = Style::new(size::SMALL, weight::SEMIBOLD, t.gold);
    let lw = ui.measure("Runs on", lead);
    ui.label_in(Rect::new(r.x, r.y, lw + 2.0, r.h), "Runs on", lead, Align::Left);
    let cs = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let start = r.x + lw + 14.0;
    let mut x = start;
    let mut y = r.y;
    for (i, h) in Harness::ALL.into_iter().enumerate() {
        let on = h == l.harness;
        let installed = l.installed(h);
        let w = 10.0 + 16.0 + 6.0 + ui.measure(h.label(), cs) + 12.0;
        if x + w > r.right() && x > start {
            x = start;
            y += r.h + 6.0;
        }
        let c = Rect::new(x, y, w, r.h);
        let hot = ui.hover_amount(("launcher-brain", i), c);
        if on {
            ui.panel(c, radius::CONTROL, t.selected, t.accent.with_alpha(0.85));
        } else if hot > 0.0 {
            ui.round(c, radius::CONTROL, t.hover.fade(hot));
        }
        ui.mark(brand::harness(h), Rect::new(c.x + 10.0, c.y, 16.0, c.h), 14.0, (!installed).then_some(t.faint));
        let ink = if on {
            t.text
        } else if installed {
            t.muted.mix(t.text, 0.5 * hot)
        } else {
            t.faint
        };
        ui.label_in(Rect::new(c.x + 32.0, c.y, w - 38.0, c.h), h.label(), cs.color(ink), Align::Left);
        ui.hit(c, Action::Overlay(BRAIN + i));
        x += w + 6.0;
    }
    let note = match l.harness {
        Harness::Claude | Harness::Codex => "tools built in",
        Harness::Pi | Harness::Omp => "tools through bro ctl",
    };
    let ns = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let nw = ui.measure(note, ns);
    if x + 8.0 + nw <= r.right() {
        ui.label_in(Rect::new(x + 8.0, y, nw + 2.0, r.h), note, ns, Align::Left);
    }
    y + r.h - r.y
}

/// The agent isn't on PATH: say so, and offer to install it (the whole bar takes the click). A standard control
/// with a red edge, the alert icon on it in red: no red tile behind the icon.
fn draw_install(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let text = format!("{} isn't installed yet · enter or click here to install it", l.harness.label());
    let st = Style::new(size::BODY, weight::MEDIUM, t.danger);
    let w = (MARK_X + 18.0 + 10.0 + ui.measure(&text, st) + 16.0).min(r.w);
    let c = Rect::new(r.x, r.y, w, r.h);
    let hot = ui.hover_amount("launcher-install", c);
    ui.panel(c, radius::CONTROL, t.control.mix(t.control_hover, hot), t.danger.with_alpha(0.55));
    ui.icon(Rect::new(c.x + MARK_X, c.y, 18.0, c.h), I_ALERT, size::TITLE, t.danger);
    ui.label_in(Rect::new(c.x + TEXT_X, c.y, c.w - TEXT_X - 12.0, c.h), &text, st, Align::Left);
    ui.hit(c, Action::Overlay(INSTALL));
}

/// Wide pages: the run-on list on the left; what launches (or the models) and the options on the right.
fn draw_side(s: &Shown, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    // a model list wants the room more than the run-on list does
    let panel_w = if s.needs { (r.w * 0.52).clamp(400.0, 580.0) } else { (r.w * 0.38).clamp(340.0, 440.0) };
    let gap = 40.0;
    let list = Rect::new(r.x, r.y, r.w - panel_w - gap, r.h);
    let panel = Rect::new(list.right() + gap, r.y, panel_w, r.h);
    ui.vline(list.right() + gap / 2.0, r.y, r.h, t.hairline);
    draw_run_on(s, ui, list, cx);
    if s.needs {
        let models = Rect::new(panel.x, panel.y, panel.w, (panel.h - OPTIONS_H - 28.0).max(0.0));
        draw_models(s, ui, models, cx);
        draw_options(s.l, ui, Rect::new(panel.x, models.bottom() + 28.0, panel.w, OPTIONS_H));
    } else {
        // the heading sits level with the run-on field
        let top = panel.y + (FIELD_H - 20.0) / 2.0;
        let used = draw_summary(s, ui, Rect { y: top, h: panel.bottom() - top, ..panel });
        let y = (top + used + 30.0).min(panel.bottom() - OPTIONS_H).max(panel.y);
        draw_options(s.l, ui, Rect::new(panel.x, y, panel.w, OPTIONS_H));
    }
}

/// Narrow pages: the list (with the models beside it when needed), then what launches and the options.
fn draw_stacked(s: &Shown, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    let rows = r.h >= 440.0;
    let opts_h = if rows { OPTIONS_H } else { options_flow_h(s.l, ui, r.w) };
    let lists = Rect::new(r.x, r.y, r.w, (r.h - opts_h - 28.0 - 36.0).max(0.0));
    if lists.h > 0.0 {
        if s.needs {
            let lw = (lists.w * 0.45).floor();
            draw_run_on(s, ui, Rect { w: lw, ..lists }, cx);
            draw_models(s, ui, Rect::new(lists.x + lw + 20.0, lists.y, lists.w - lw - 20.0, lists.h), cx);
        } else {
            draw_run_on(s, ui, lists, cx);
        }
    }
    // what Enter runs, in one line
    let y = lists.bottom() + 14.0;
    ui.hline(r.x, y, r.w, t.hairline);
    let line = Rect::new(r.x, y + 8.0, r.w, 24.0);
    match &s.spec {
        Some(spec) => {
            let lead = Style::new(size::BODY, weight::MEDIUM, t.muted);
            let lw = ui.measure("→", lead);
            ui.label_in(Rect::new(line.x, line.y, lw + 2.0, line.h), "→", lead, Align::Left);
            ui.mark(brand::harness(spec.harness), Rect::new(line.x + lw + 8.0, line.y, 18.0, line.h), 15.0, None);
            ui.label_in(Rect::new(line.x + lw + 32.0, line.y, line.w - lw - 32.0, line.h), &crate::services::launch::label_for(spec), Style::new(size::BODY, weight::SEMIBOLD, t.text), Align::Left);
        }
        None if s.needs => {
            ui.label_in(line, "→ pick a model", Style::new(size::BODY, weight::MEDIUM, t.shine), Align::Left);
        }
        None => {
            ui.label_in(line, "→ pick where to run", Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Left);
        }
    }
    let opts = Rect::new(r.x, line.bottom() + 12.0, r.w, opts_h);
    if rows {
        draw_options(s.l, ui, opts);
    } else {
        draw_options_flow(s.l, ui, opts);
    }
}

/// A list row's selection and hover look (4px corners, no side marker): the selected fill while its list has the
/// keyboard, a quieter one while it doesn't. Returns the hover amount.
pub(crate) fn row_fill(ui: &mut Ui, r: Rect, key: impl std::hash::Hash, on: bool, focused: bool) -> f32 {
    let t = ui.t;
    let hot = ui.hover_amount(("launcher-row", key), r);
    if on {
        ui.round(r, radius::CONTROL, if focused { t.selected } else { t.text.with_alpha(0.05) });
    } else if hot > 0.0 {
        ui.round(r, radius::CONTROL, t.hover.fade(hot));
    }
    hot
}

/// How far a list `content` high scrolls in a view `view` high: the selected row (`top`, `h`) sits near the
/// middle (eased). The state machine keeps no scroll; the selection decides, as in the terminal UI.
pub(crate) fn scroll(ui: &mut Ui, key: &'static str, sel: Option<(f32, f32)>, content: f32, view: f32) -> f32 {
    let max = (content - view).max(0.0);
    let target = sel.map(|(y, h)| y + h / 2.0 - view / 2.0).unwrap_or(0.0).clamp(0.0, max);
    ui.ease(("launcher-scroll", key), target, 120.0).clamp(0.0, max)
}

/// The mark for a run-on row: Claude / Codex logins show their agent's logo, providers theirs (or the terminal
/// UI's symbol for them), the pool Claude's (it's Claude accounts), a harness's own login its own.
fn account_mark(kind: &AccountKind, harness: Harness, ui: &Ui) -> Mark {
    match kind {
        AccountKind::Profile(p) if bro_core::profiles::is_gemini_id(&p) => brand::provider("gemini").unwrap(),
        AccountKind::Profile(p) if p.starts_with("codex:") => brand::harness(Harness::Codex),
        AccountKind::Profile(_) | AccountKind::Pool | AccountKind::Shared(true) => brand::harness(Harness::Claude),
        AccountKind::Shared(false) => brand::harness(Harness::Codex),
        AccountKind::Provider(id) => provider_mark(id, ui),
        AccountKind::Native => brand::harness(harness),
    }
}

/// The "run on" list: recent combos, your logins, other logins through the proxy, providers.
fn draw_run_on(s: &Shown, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    let l = s.l;
    let focused = l.focus == Focus::List;
    let field = Rect::new(r.x, r.y, r.w, FIELD_H);
    input(ui, field, I_SEARCH, &l.list_filter, "run on… ↑↓ to pick · type a project", focused, cx.focused, false, Action::Overlay(LIST_FIELD));
    let view = Rect::new(r.x, field.bottom() + 10.0, r.w, (r.bottom() - field.bottom() - 10.0).max(0.0));
    ui.hit_with(view, Action::Overlay(LIST_AREA), Cursor::Default);
    let items = &s.items;
    if items.is_empty() {
        ui.label(Point::new(view.x + 12.0, view.y + 10.0), "nothing matches · esc clears the filter", Style::new(size::BODY, weight::REGULAR, t.muted));
        return;
    }
    let cursor = s.cursor;
    // where each row goes: headers are a heading with room above, rows a line each
    let mut tops = Vec::with_capacity(items.len());
    let mut y = 0.0;
    for (k, it) in items.iter().enumerate() {
        let h = match it {
            Item::Header(_) if k == 0 => 26.0,
            Item::Header(_) => 38.0,
            _ => ROW_H,
        };
        tops.push((y, h));
        y += h;
    }
    let content = y;
    let off = scroll(ui, "run-on", cursor.map(|k| tops[k]), content, view.h);
    let pct = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let pct_w = ui.measure("100% left", pct);
    // what's left as the terminal UI's ▰▰▱ segments, as room allows
    let seg_w = if r.w >= 560.0 {
        72.0
    } else if r.w >= 440.0 {
        44.0
    } else {
        0.0
    };
    let now = crate::util::now_secs();
    ui.clip(view, 0.0);
    let mut idx = 0;
    for (k, it) in items.iter().enumerate() {
        let (top, h) = tops[k];
        let rr = Rect::new(view.x, view.y + top - off, view.w, h);
        let this = idx;
        if !matches!(it, Item::Header(_)) {
            idx += 1;
        }
        if rr.bottom() < view.y || rr.y > view.bottom() {
            continue;
        }
        let on = cursor == Some(k);
        let mark = Rect::new(rr.x + MARK_X, rr.y, 18.0, rr.h);
        match it {
            Item::Header(name) => widgets::section(ui, Rect::new(rr.x + 12.0, rr.bottom() - 22.0, rr.w - 24.0, 18.0), &sentence(name)),
            Item::Recent(i) => {
                let Some(rc) = l.data.recents.get(*i) else { continue };
                row_fill(ui, rr, ("run-on", this), on, focused);
                ui.hit(rr, Action::Overlay(ROW + this));
                let (kind, who) = l.recent_row(rc);
                let model = rc.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m)));
                ui.mark(account_mark(&kind, l.harness, ui), mark, 15.0, None);
                let age = format!("{} ago", crate::util::short_dur((now - rc.at).max(0) as u64));
                let ast = Style::new(size::SMALL, weight::REGULAR, t.faint);
                let aw = ui.measure(&age, ast);
                ui.label_in(Rect::new(rr.right() - 12.0 - aw, rr.y, aw + 1.0, rr.h), &age, ast, Align::Left);
                let ls = Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::MEDIUM }, t.text);
                let room = rr.right() - 12.0 - aw - 16.0 - (rr.x + TEXT_X);
                let lb = ui.line(&who, ls, Some(room.max(10.0)));
                ui.draw(&lb, Point::new(rr.x + TEXT_X, rr.y + (rr.h - lb.h) / 2.0));
                if let Some(m) = model {
                    let ms = Style::mono(size::SMALL, t.muted);
                    let left = room - lb.w - 10.0;
                    if left > 24.0 {
                        let mb = ui.line(&m, ms, Some(left));
                        ui.draw(&mb, Point::new(rr.x + TEXT_X + lb.w + 10.0, rr.y + (rr.h - mb.h) / 2.0));
                    }
                }
            }
            Item::Account(a) => {
                row_fill(ui, rr, ("run-on", this), on, focused);
                ui.hit(rr, Action::Overlay(ROW + this));
                ui.mark(account_mark(&a.kind, l.harness, ui), mark, 15.0, (!a.ready).then_some(t.faint));
                // what's left on the right, its segments beside it
                let mut right = rr.right() - 12.0;
                if let Some(left) = a.left {
                    let s = format!("{left:.0}% left");
                    let col = left_color(left, t);
                    let w = ui.measure(&s, pct);
                    ui.label_in(Rect::new(right - w, rr.y, w + 1.0, rr.h), &s, pct.color(col), Align::Left);
                    right -= pct_w + 12.0;
                    if seg_w > 0.0 {
                        let bar = Rect::new(right - seg_w, rr.y + (rr.h - 6.0) / 2.0, seg_w, 6.0);
                        widgets::segments(ui, bar, (left / 100.0) as f32, widgets::segment_count(seg_w, 7.0), col);
                        right -= seg_w + 14.0;
                    }
                } else if !a.ready {
                    let ns = Style::new(size::SMALL, weight::REGULAR, t.faint);
                    let w = ui.measure("not signed in", ns);
                    ui.label_in(Rect::new(right - w, rr.y, w + 1.0, rr.h), "not signed in", ns, Align::Left);
                    right -= w + 14.0;
                }
                let ls = Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::MEDIUM }, if a.ready || on { t.text } else { t.muted });
                let room = right - (rr.x + TEXT_X);
                let lb = ui.line(&a.label, ls, Some(room.max(10.0)));
                ui.draw(&lb, Point::new(rr.x + TEXT_X, rr.y + (rr.h - lb.h) / 2.0));
                let left = room - lb.w - 10.0;
                if !a.detail.is_empty() && left > 24.0 {
                    let db = ui.line(&a.detail, Style::new(size::SMALL, weight::REGULAR, t.muted), Some(left));
                    ui.draw(&db, Point::new(rr.x + TEXT_X + lb.w + 10.0, rr.y + (rr.h - db.h) / 2.0));
                }
            }
        }
    }
    ui.unclip();
    let vis = 0.5 + 0.5 * ui.hover_amount("launcher-run-on-bar", view);
    widgets::scrollbar(ui, view, content, view.h, off, vis);
}

/// 262144 → "262k", 1048576 → "1M".
fn short_ctx(c: u64) -> String {
    if c >= 1_000_000 { format!("{}M", c / 1_000_000) } else { format!("{}k", c / 1000) }
}

/// 0.6 → "0.6", 2.5 → "2.5", 10.0 → "10".
fn short_price(p: f64) -> String {
    let s = format!("{p:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The models the chosen login or provider can run: filterable, with context size and $ per million in / out.
fn draw_models(s: &Shown, ui: &mut Ui, r: Rect, cx: &Cx) {
    let t = ui.t;
    let l = s.l;
    let focused = l.focus == Focus::Models;
    let models = s.models;
    let idx = &s.view;
    let field = Rect::new(r.x, r.y, r.w, FIELD_H);
    let prompt = format!("model… {} to pick from", models.len());
    input(ui, field, I_SEARCH, &l.model_filter, &prompt, focused, cx.focused, false, Action::Overlay(MODEL_FIELD));
    let view = Rect::new(r.x, field.bottom() + 10.0, r.w, (r.bottom() - field.bottom() - 10.0).max(0.0));
    ui.hit_with(view, Action::Overlay(MODEL_AREA), Cursor::Default);
    if idx.is_empty() {
        let msg = if models.is_empty() { "no models known for this" } else { "nothing matches" };
        ui.label(Point::new(view.x + 12.0, view.y + 10.0), msg, Style::new(size::BODY, weight::REGULAR, t.muted));
        return;
    }
    let content = idx.len() as f32 * MODEL_H;
    let off = scroll(ui, "models", Some((l.model_sel as f32 * MODEL_H, MODEL_H)), content, view.h);
    let first = (off / MODEL_H).floor().max(0.0) as usize;
    let last = (((off + view.h) / MODEL_H).ceil() as usize + 1).min(idx.len());
    // the price and context columns line up on the right
    let ps = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let cs = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let price = |m: &bro_core::catalogue::ModelRow| m.pricing.map(|(a, b)| format!("${}/{}", short_price(a), short_price(b)));
    let mut price_w: f32 = 0.0;
    let mut ctx_w: f32 = 0.0;
    for &i in &idx[first..last] {
        if let Some(p) = price(&models[i]) {
            price_w = price_w.max(ui.measure(&p, ps));
        }
        if let Some(c) = models[i].context {
            ctx_w = ctx_w.max(ui.measure(&short_ctx(c), cs));
        }
    }
    ui.clip(view, 0.0);
    for (k, &i) in idx.iter().enumerate().take(last).skip(first) {
        let m = &models[i];
        let rr = Rect::new(view.x, view.y + k as f32 * MODEL_H - off, view.w, MODEL_H);
        let on = k == l.model_sel;
        row_fill(ui, rr, ("model", k), on, focused);
        ui.hit(rr, Action::Overlay(MODEL + k));
        let mut right = rr.right() - 12.0;
        if price_w > 0.0 {
            if let Some(p) = price(m) {
                let w = ui.measure(&p, ps);
                ui.label_in(Rect::new(right - w, rr.y, w + 1.0, rr.h), &p, ps, Align::Left);
            }
            right -= price_w + 16.0;
        }
        if ctx_w > 0.0 {
            if let Some(c) = m.context {
                let ctx = short_ctx(c);
                let w = ui.measure(&ctx, cs);
                ui.label_in(Rect::new(right - w, rr.y, w + 1.0, rr.h), &ctx, cs, Align::Left);
            }
            right -= ctx_w + 16.0;
        }
        let ms = Style::mono(size::BODY, if on { t.text } else { t.text.mix(t.muted, 0.25) }).weight(if on { weight::SEMIBOLD } else { weight::REGULAR });
        let b = ui.line(&m.id, ms, Some((right - rr.x - 12.0).max(10.0)));
        ui.draw(&b, Point::new(rr.x + 12.0, rr.y + (rr.h - b.h) / 2.0));
    }
    ui.unclip();
    let vis = 0.5 + 0.5 * ui.hover_amount("launcher-models-bar", view);
    widgets::scrollbar(ui, view, content, view.h, off, vis);
}

/// What will run: the agent's logo and the launch label, the login's plan and what's left of it. Returns the
/// height used.
fn draw_summary(s: &Shown, ui: &mut Ui, r: Rect) -> f32 {
    let t = ui.t;
    let l = s.l;
    widgets::section(ui, Rect::new(r.x, r.y, r.w, 20.0), &sentence("launches"));
    let mut y = r.y + 36.0;
    let chosen = s.chosen();
    // what's left: the terminal UI's ▰▰▱ segments and the figure
    let usage = |ui: &mut Ui, x: f32, y: f32, w: f32, left: f64| {
        let col = left_color(left, t);
        let mw = (w - 90.0).clamp(60.0, 200.0);
        widgets::segments(ui, Rect::new(x, y + 7.0, mw, 6.0), (left / 100.0) as f32, widgets::segment_count(mw, 10.0), col);
        ui.label_in(Rect::new(x + mw + 12.0, y, 90.0, 20.0), &format!("{left:.0}% left"), Style::new(size::SMALL, weight::SEMIBOLD, col), Align::Left);
    };
    if l.orchestrator {
        let w = r.w - 30.0;
        if let Some(running) = l.running_orchestrator() {
            ui.icon(Rect::new(r.x, y, 22.0, 26.0), I_STAR, 18.0, t.gold);
            ui.label_in(Rect::new(r.x + 30.0, y, w, 26.0), running, Style::new(size::HEADING, weight::SEMIBOLD, t.gold), Align::Left);
            y += 34.0;
            ui.label_in(Rect::new(r.x + 30.0, y, w, 20.0), "Already running for this project.", Style::new(size::BODY, weight::REGULAR, t.text), Align::Left);
            y += 22.0;
            ui.label_in(Rect::new(r.x + 30.0, y, w, 20.0), "⏎ takes you to it.", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
            return y + 20.0 - r.y;
        }
        let what = match &s.spec {
            Some(spec) => crate::services::launch::label_for(spec),
            None => format!("orchestrator · {}", l.harness.label()),
        };
        ui.icon(Rect::new(r.x, y, 22.0, 26.0), I_STAR, 18.0, t.gold);
        ui.label_in(Rect::new(r.x + 30.0, y, w, 26.0), &what, Style::new(size::HEADING, weight::SEMIBOLD, t.gold), Align::Left);
        y += 34.0;
        let lines = if l.no_project { ["Coordinates project orchestrators.", "New ones use this model."] } else { ["Coordinates sessions in this project.", "Find it in the Orchestrators tab."] };
        for line in lines {
            ui.label_in(Rect::new(r.x + 30.0, y, w, 20.0), line, Style::new(size::BODY, weight::REGULAR, t.text), Align::Left);
            y += 20.0;
        }
        y += 10.0;
        if let Some(Item::Account(a)) = chosen
            && let Some(left) = a.left
        {
            usage(ui, r.x + 30.0, y, w, left);
            y += 24.0;
        }
        return y - r.y;
    }
    let Some(spec) = &s.spec else {
        let msg = if s.installed { "pick where it runs on the left" } else { "install it first" };
        ui.label_in(Rect::new(r.x, y, r.w, 22.0), msg, Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
        return y + 22.0 - r.y;
    };
    let w = r.w - 30.0;
    ui.mark(brand::harness(spec.harness), Rect::new(r.x, y, 22.0, 26.0), 18.0, None);
    ui.label_in(Rect::new(r.x + 30.0, y, w, 26.0), &crate::services::launch::label_for(spec), Style::new(size::HEADING, weight::SEMIBOLD, t.text), Align::Left);
    y += 32.0;
    match chosen {
        Some(Item::Account(a)) => {
            if !a.detail.is_empty() {
                ui.label_in(Rect::new(r.x + 30.0, y, w, 20.0), &a.detail, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
                y += 26.0;
            }
            if let Some(left) = a.left {
                usage(ui, r.x + 30.0, y, w, left);
                y += 24.0;
            }
        }
        Some(Item::Recent(_)) => {
            ui.label_in(Rect::new(r.x + 30.0, y, w, 20.0), "the way you last ran it here", Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
            y += 24.0;
        }
        _ => {}
    }
    y - r.y
}

/// One of the launcher's options.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opt {
    Perm,
    Browser,
    Place,
}

impl Opt {
    const ALL: [Opt; 3] = [Opt::Perm, Opt::Browser, Opt::Place];
    fn label(self) -> &'static str {
        match self {
            Opt::Perm => "Permissions",
            Opt::Browser => "Browser",
            Opt::Place => "Opens as",
        }
    }
    fn chord(self) -> &'static str {
        match self {
            Opt::Perm => "ctrl+e",
            Opt::Browser => "ctrl+b",
            Opt::Place => "ctrl+s",
        }
    }
}

/// An option's choices, the chosen one and its first target; `long` spells out where a split goes.
fn opt_choices(l: &Launcher, o: Opt, long: bool, t: &Tokens) -> (Vec<Seg<'static>>, Option<usize>, usize) {
    match o {
        Opt::Perm => (
            PERMS.iter().map(|(p, s)| Seg { label: s, icon: None, ink: (*p == Permission::Skip).then_some(t.danger) }).collect(),
            PERMS.iter().position(|(p, _)| *p == l.permission),
            PERM,
        ),
        Opt::Browser => (BROWSERS.iter().map(|(_, s)| Seg::new(s)).collect(), BROWSERS.iter().position(|(b, _)| *b == l.browser), BROWSER),
        Opt::Place => (
            vec![Seg::new("New session"), Seg::new(if long { "Split beside this one" } else { "Split" })],
            Some(if l.place == Place::Tab { 0 } else { 1 }),
            PLACE,
        ),
    }
}

/// A coordinator always opens as its own session.
const PINNED: &str = "its own session";

/// Permissions, browser and placement as labelled rows of choices with their keys, under an "Options" heading.
fn draw_options(l: &Launcher, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    widgets::section(ui, Rect::new(r.x, r.y, r.w, 20.0), &sentence("options"));
    let label_w = 104.0;
    let ls = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    for (i, o) in Opt::ALL.into_iter().enumerate() {
        let row = Rect::new(r.x, r.y + 30.0 + i as f32 * 40.0, r.w, 32.0);
        ui.label_in(Rect::new(row.x, row.y, label_w - 8.0, row.h), o.label(), ls, Align::Left);
        let x = row.x + label_w;
        if o == Opt::Place && l.orchestrator {
            ui.label_in(Rect::new(x, row.y, row.right() - x, row.h), PINNED, Style::new(size::SMALL, weight::SEMIBOLD, t.gold), Align::Left);
            continue;
        }
        let kw = kbd_w(ui, o.chord());
        let (segs, on, base) = {
            let long = opt_choices(l, o, true, t);
            if x + seg_width(ui, &long.0, size::SMALL) <= row.right() { long } else { opt_choices(l, o, false, t) }
        };
        let sw = segmented(ui, Point::new(x, row.y), row.h, &segs, on, size::SMALL, ("launcher-opt", i), |k| Action::Overlay(base + k));
        if x + sw + 12.0 + kw <= row.right() {
            widgets::kbd(ui, Point::new(row.right() - kw, row.y + (row.h - 18.0) / 2.0), o.chord());
        }
    }
}

/// Height the options take flowed into one or more lines `w` wide.
fn options_flow_h(l: &Launcher, ui: &mut Ui, w: f32) -> f32 {
    let lines = options_flow(l, ui, Rect::new(0.0, 0.0, w, 0.0), false);
    lines as f32 * 40.0 - 8.0
}

fn draw_options_flow(l: &Launcher, ui: &mut Ui, r: Rect) {
    options_flow(l, ui, r, true);
}

/// The options side by side ("Permissions [Ask Auto Skip all]   Browser [Off On …]   …"), wrapping when they
/// don't fit. Returns the number of lines.
fn options_flow(l: &Launcher, ui: &mut Ui, r: Rect, paint: bool) -> usize {
    let t = ui.t;
    let ls = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let mut x = r.x;
    let mut line = 0;
    for (i, o) in Opt::ALL.into_iter().enumerate() {
        let lw = ui.measure(o.label(), ls);
        let (segs, on, base) = opt_choices(l, o, false, t);
        let cw = if o == Opt::Place && l.orchestrator { ui.measure(PINNED, Style::new(size::SMALL, weight::SEMIBOLD, t.gold)) } else { seg_width(ui, &segs, size::SMALL) };
        let w = lw + 10.0 + cw;
        if x > r.x && x + w > r.right() {
            x = r.x;
            line += 1;
        }
        if paint {
            let y = r.y + line as f32 * 40.0;
            ui.label_in(Rect::new(x, y, lw + 2.0, 32.0), o.label(), ls, Align::Left);
            if o == Opt::Place && l.orchestrator {
                ui.label_in(Rect::new(x + lw + 10.0, y, cw + 2.0, 32.0), PINNED, Style::new(size::SMALL, weight::SEMIBOLD, t.gold), Align::Left);
            } else {
                segmented(ui, Point::new(x + lw + 10.0, y), 32.0, &segs, on, size::SMALL, ("launcher-opt", i), |k| Action::Overlay(base + k));
            }
        }
        x += w + 24.0;
    }
    line + 1
}

/// The main button's look and what it does.
struct Main {
    label: String,
    fill: Color,
    ink: Color,
    /// it runs something on click / Enter (a ⏎ mark), rather than pointing at what's missing
    go: bool,
    target: usize,
}

/// What the main button says: install, sign in, go to / start the orchestrator, launch, or what's still missing.
fn main_action(s: &Shown, t: &Tokens) -> Main {
    let l = s.l;
    let go = |label: String, fill: Color, ink: Color, target: usize| Main { label, fill, ink, go: true, target };
    if l.orchestrator && l.running_orchestrator().is_some() {
        return go("Go to the orchestrator".into(), t.gold, t.ink_on(t.gold), LAUNCH);
    }
    // Enter installs first (the terminal UI's button says "Launch …" there while it installs)
    if !s.installed {
        return go(format!("Install {}", l.harness.label()), t.danger, t.ink_on(t.danger), INSTALL);
    }
    // ... and signs in to Google before anything runs on a signed-out Gemini (the row says "enter signs in with Google")
    if s.sign_in {
        return go("Sign in with Google".into(), t.accent, t.on_accent, LAUNCH);
    }
    match &s.spec {
        Some(spec) if l.orchestrator => go(format!("Start it on {}", crate::services::launch::label_for(spec).trim_start_matches("orchestrator · ")), t.gold, t.ink_on(t.gold), LAUNCH),
        Some(spec) => go(format!("Launch {}", crate::services::launch::label_for(spec)), t.accent, t.on_accent, LAUNCH),
        None => {
            let label = if s.needs { "pick a model to launch" } else { "pick where it runs" };
            Main { label: label.into(), fill: t.control, ink: t.muted, go: false, target: LAUNCH }
        }
    }
}

/// The keys on the left (the first one, Enter, takes a click too) and the main button on the right.
fn draw_footer(s: &Shown, ui: &mut Ui, r: Rect) {
    let l = s.l;
    let m = main_action(s, ui.t);
    let br = main_button(ui, r, &m.label, m.fill, m.ink, m.go, "launcher-main", Action::Overlay(m.target));
    let enter = match l.focus {
        Focus::List if s.needs => "pick a model",
        _ => "launch",
    };
    let enter = if l.orchestrator && l.running_orchestrator().is_some() { "go to it" } else { enter };
    // Enter installs or signs in first, as the button says
    let enter = if !s.installed {
        "install"
    } else if s.sign_in {
        "sign in"
    } else {
        enter
    };
    let mut keys = vec![("⏎", enter), ("↑↓", "move"), ("←→", "agent"), ("type", if l.focus == Focus::List { "project" } else { "filter" })];
    if l.orchestrator {
        keys.insert(3, ("shift+←→", "runs on"));
    }
    if s.needs {
        keys.push(("tab", "list ⇄ models"));
    }
    keys.push(("alt+p", "project"));
    keys.push(("esc", if l.focus == Focus::List { "close" } else { "back" }));
    let first = hints(ui, Rect::new(r.x, r.y, (br.x - r.x - 24.0).max(0.0), r.h), &keys);
    if first.w > 0.0 {
        ui.hit(first, Action::Overlay(LAUNCH));
    }
}

// ------------------------------------------------------------------ pieces the Continue tab and the project picker share

/// The page's main button at the right end of `r` (36 high, centred): WinUI's accent button in `fill` with `ink`
/// text and a ⏎ mark when it runs something; a disabled-looking standard button when it only says what's missing.
/// Returns its rect.
#[allow(clippy::too_many_arguments)]
pub(crate) fn main_button(ui: &mut Ui, r: Rect, label: &str, fill: Color, ink: Color, go: bool, key: &'static str, action: Action) -> Rect {
    let t = ui.t;
    let st = Style::new(size::BODY, weight::SEMIBOLD, ink);
    let lw = ui.measure(label, st);
    let mark_w = if go { 24.0 } else { 0.0 };
    let w = (lw + 40.0 + mark_w).clamp(160.0, (r.w * 0.6).max(160.0)).min(r.w);
    let h = 36f32.min(r.h);
    let b = Rect::new(r.right() - w, r.y + (r.h - h) / 2.0, w, h);
    let hot = ui.hover_amount(key, b);
    let down = ui.pressing && hot > 0.5;
    let ink = if go {
        ui.round(b, radius::CONTROL, if down { fill.fade(0.8) } else { fill.fade(1.0 - 0.1 * hot) });
        let edge = Rect::new(b.x + radius::CONTROL, b.y, (b.w - 2.0 * radius::CONTROL).max(0.0), b.h);
        ui.hline(edge.x, b.bottom() - ui.hair(), edge.w, Color::rgba(0.0, 0.0, 0.0, 0.18));
        if down { ink.fade(0.8) } else { ink }
    } else {
        ui.panel(b, radius::CONTROL, t.control_pressed, t.stroke);
        ink
    };
    let text = Rect::new(b.x + 20.0, b.y, b.w - 40.0 - mark_w, b.h);
    ui.label_in(text, label, st.color(ink), Align::Center);
    if go {
        ui.icon(Rect::new(b.right() - 20.0 - mark_w + 4.0, b.y, mark_w, b.h), I_RETURN, size::TITLE, ink.fade(0.65));
    }
    ui.hit(b, action);
    b
}

/// The keys, quietly: "key what · key what …", cut before the first one that doesn't fit. Returns the first
/// hint's rect (empty when nothing fit).
pub(crate) fn hints(ui: &mut Ui, r: Rect, items: &[(&str, &str)]) -> Rect {
    let t = ui.t;
    let ks = Style::new(size::SMALL, weight::SEMIBOLD, t.muted);
    let ds = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let mut x = r.x;
    let mut first = Rect::ZERO;
    for (i, (k, d)) in items.iter().enumerate() {
        let kb = ui.line(k, ks, None);
        let db = ui.line(d, ds, None);
        let dot = if i > 0 { 20.0 } else { 0.0 };
        let w = dot + kb.w + 5.0 + db.w;
        if x + w > r.right() {
            break;
        }
        if i > 0 {
            ui.circle(Point::new(x + dot / 2.0, r.y + r.h / 2.0), 1.4, t.faint.fade(0.8));
        }
        let kx = x + dot;
        ui.draw(&kb, Point::new(kx, r.y + (r.h - kb.h) / 2.0));
        ui.draw(&db, Point::new(kx + kb.w + 5.0, r.y + (r.h - db.h) / 2.0));
        if i == 0 {
            first = Rect::new(kx - 4.0, r.y + 6.0, kb.w + 5.0 + db.w + 8.0, r.h - 12.0);
        }
        x += w;
    }
    first
}

/// Width of a [`widgets::kbd`] key hint (one keycap per key, 3px apart).
pub(crate) fn kbd_w(ui: &mut Ui, chord: &str) -> f32 {
    let st = Style::new(size::CAPTION, weight::MEDIUM, ui.t.muted);
    let mut w = 0.0;
    for (i, k) in widgets::chord_keys(chord).iter().enumerate() {
        if i > 0 {
            w += 3.0;
        }
        w += (ui.measure(k, st) + 10.0).max(18.0);
    }
    w
}

/// A one-line input in WinUI's text box look: a leading icon, the text or its placeholder; at rest a standard
/// control with its darker bottom edge, while it has the keyboard a full 1px accent outline and (when the window
/// has focus too) a caret at the end, where filters grow and shrink. Clicking runs `action`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn input(ui: &mut Ui, r: Rect, icon: &str, value: &str, placeholder: &str, focused: bool, window_focused: bool, mono: bool, action: Action) {
    let t = ui.t;
    if focused {
        ui.panel(r, radius::CONTROL, t.field_focus, t.accent.with_alpha(0.8));
    } else {
        let hot = ui.hover_amount(("launcher-input", r.x as i32, r.y as i32), r);
        ui.panel(r, radius::CONTROL, t.control.mix(t.control_hover, hot), t.stroke);
        let edge = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
        ui.hline(edge.x, r.bottom() - ui.hair(), edge.w, t.muted.with_alpha(0.45));
    }
    ui.icon(Rect::new(r.x + MARK_X, r.y, 18.0, r.h), icon, size::TITLE, if focused { t.muted } else { t.faint });
    let inner = Rect::new(r.x + TEXT_X, r.y, (r.w - TEXT_X - 10.0).max(0.0), r.h);
    let caret = focused && window_focused;
    ui.clip(Rect { x: inner.x - 4.0, w: inner.w + 4.0, ..inner }, 0.0);
    if value.is_empty() {
        // the caret waits just before the placeholder
        ui.label_in(inner, placeholder, Style::new(size::BODY, weight::REGULAR, t.faint), Align::Left);
        if caret {
            widgets::draw_caret(ui, Point::new(inner.x - 3.0, r.y + (r.h - 16.0) / 2.0), 16.0);
        }
    } else {
        let st = if mono { Style::mono(size::BODY, t.text) } else { Style::new(size::BODY, weight::MEDIUM, t.text) };
        let b = ui.line(value, st, None);
        let shift = (b.w - inner.w + 6.0).max(0.0);
        ui.draw(&b, Point::new(inner.x - shift, r.y + (r.h - b.h) / 2.0));
        if caret {
            widgets::draw_caret(ui, Point::new(inner.x - shift + b.w + 1.0, r.y + (r.h - 16.0) / 2.0), 16.0);
        }
    }
    ui.unclip();
    ui.hit_with(r, action, Cursor::Text);
}

/// One choice of a [`segmented`] control.
pub(crate) struct Seg<'a> {
    pub label: &'a str,
    pub icon: Option<&'a str>,
    /// its text colour while chosen (the default: the text colour)
    pub ink: Option<Color>,
}

impl<'a> Seg<'a> {
    pub(crate) fn new(label: &'a str) -> Seg<'a> {
        Seg { label, icon: None, ink: None }
    }
    pub(crate) fn icon(mut self, icon: &'a str) -> Seg<'a> {
        self.icon = Some(icon);
        self
    }
}

fn seg_pad(sz: f32) -> f32 {
    if sz >= size::BODY { 14.0 } else { 11.0 }
}

fn seg_one(ui: &mut Ui, s: &Seg, sz: f32) -> f32 {
    let w = ui.measure(s.label, Style::new(sz, weight::SEMIBOLD, ui.t.text));
    w + if s.icon.is_some() { sz + 8.0 } else { 0.0 } + 2.0 * seg_pad(sz)
}

/// Width a [`segmented`] control takes.
pub(crate) fn seg_width(ui: &mut Ui, segs: &[Seg], sz: f32) -> f32 {
    segs.iter().map(|s| seg_one(ui, s, sz)).sum::<f32>() + 4.0
}

/// A row of choices with one chosen (or none), each as wide as its label, at `at`, `h` high: the kit's segmented
/// look (a standard control; the chosen choice raised inside it with a hairline edge). `act(i)` gives each
/// choice's action. Returns its width.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented(ui: &mut Ui, at: Point, h: f32, segs: &[Seg], chosen: Option<usize>, sz: f32, key: impl std::hash::Hash + Copy, act: impl Fn(usize) -> Action) -> f32 {
    let t = ui.t;
    let total = seg_width(ui, segs, sz);
    let r = Rect::new(at.x, at.y, total, h);
    widgets::control(ui, r, t.control);
    let mut x = r.x + 2.0;
    for (i, s) in segs.iter().enumerate() {
        let w = seg_one(ui, s, sz);
        let c = Rect::new(x, r.y + 2.0, w, h - 4.0);
        let on = chosen == Some(i);
        let hot = ui.hover_amount(("launcher-seg", key, i), c);
        if on {
            ui.panel(c, radius::CONTROL - 1.0, if t.light { Color::WHITE } else { t.text.with_alpha(0.09) }, t.stroke);
        } else if hot > 0.0 {
            ui.round(c, radius::CONTROL - 1.0, t.text.with_alpha(0.045 * hot));
        }
        let ink = if on { s.ink.unwrap_or(t.text) } else { t.muted.mix(t.text, 0.45 * hot) };
        let st = Style::new(sz, if on { weight::SEMIBOLD } else { weight::MEDIUM }, ink);
        let mut tx = c.x + seg_pad(sz);
        if let Some(icon) = s.icon {
            ui.icon(Rect::new(tx, c.y, sz + 2.0, c.h), icon, sz + 1.0, ink);
            tx += sz + 8.0;
        }
        ui.label_in(Rect::new(tx, c.y, c.right() - tx, c.h), s.label, st, Align::Left);
        ui.hit(c, act(i));
        x += w;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::super::Data;
    use super::*;
    use crate::services::{Services, fallback_settings};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn launcher() -> Launcher {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        let st = svc.state();
        let data = Data {
            profiles: st.profiles.ready().cloned().unwrap_or_default(),
            providers: st.providers.ready().cloned().unwrap_or_default(),
            usage: BTreeMap::new(),
            usage_status: BTreeMap::new(),
            installed: vec![],
            recents: vec![],
            models: st.models.clone(),
            keyed: vec!["openrouter".into(), "openai".into()],
            orchestrator_running: BTreeMap::new(),
            shared: false,
        };
        Launcher::new(data, Some(PathBuf::from("/code/bro")), Place::Tab)
    }

    fn tap(l: &mut Launcher, n: usize) -> Outcome {
        click(l, n, Point::new(0.0, 0.0), KeyModifiers::NONE, 1)
    }

    fn row_of(l: &Launcher, kind: AccountKind) -> usize {
        let items = l.items();
        Launcher::selectable(&items).iter().position(|&i| matches!(&items[i], Item::Account(a) if a.kind == kind)).unwrap()
    }

    #[test]
    fn choices_go_to_what_was_clicked() {
        let mut l = launcher();
        tap(&mut l, BROWSER + 2);
        assert_eq!(l.browser, BrowserMode::Edge);
        tap(&mut l, BROWSER);
        assert_eq!(l.browser, BrowserMode::Off);
        tap(&mut l, PLACE + 1);
        assert_eq!(l.place, Place::Split);
        tap(&mut l, PLACE + 1);
        assert_eq!(l.place, Place::Split, "the chosen one again changes nothing");
        tap(&mut l, PLACE);
        assert_eq!(l.place, Place::Tab);
        tap(&mut l, PERM);
        assert_eq!(l.permission, Permission::Default);
    }

    #[test]
    fn a_row_again_acts_like_enter_and_a_model_again_launches() {
        let mut l = launcher();
        let k = row_of(&l, AccountKind::Provider("openrouter".into()));
        assert!(matches!(tap(&mut l, ROW + k), Outcome::None));
        assert_eq!((l.list_sel, l.focus), (k, Focus::List));
        assert!(l.needs_model());
        // Enter on a row that needs a model moves to the models
        assert!(matches!(tap(&mut l, ROW + k), Outcome::None));
        assert_eq!(l.focus, Focus::Models);
        tap(&mut l, MODEL + 1);
        assert_eq!(l.model_sel, 1);
        assert!(matches!(tap(&mut l, MODEL + 1), Outcome::Launch(ref s, Place::Tab) if s.provider_id.as_deref() == Some("openrouter") && s.model.is_some()));
        // a target from a list that has since shrunk does nothing
        assert!(matches!(tap(&mut l, ROW + 999), Outcome::None));
        assert!(matches!(tap(&mut l, MODEL + 999), Outcome::None));
    }

    #[test]
    fn tabs_agents_and_the_orchestrator() {
        let mut l = launcher();
        tap(&mut l, HARNESS + 1);
        assert_eq!(l.harness, Harness::Codex);
        tap(&mut l, ORCHESTRATOR);
        assert!(l.orchestrator);
        tap(&mut l, BRAIN + 2);
        assert_eq!((l.harness, l.orchestrator), (Harness::Pi, true));
        tap(&mut l, HARNESS);
        assert_eq!((l.harness, l.orchestrator), (Harness::Claude, false));
        tap(&mut l, TAB_CONTINUE);
        assert_eq!(l.tab, SessionTab::Continue);
        tap(&mut l, TAB_NEW);
        assert_eq!(l.tab, SessionTab::New);
        assert!(matches!(tap(&mut l, CLOSE), Outcome::Close));
        assert!(matches!(tap(&mut l, PROFILES), Outcome::Profiles));
        assert!(matches!(tap(&mut l, INSTALL), Outcome::Install(Harness::Claude)));
    }

    #[test]
    fn orchestrator_project_selection_and_no_project_use_their_own_running_status() {
        let mut l = launcher().with_orchestrator();
        let project = std::env::temp_dir().join("orchestrator-project");
        l.select_project(project.clone());
        l.data.orchestrator_running.insert(bro_core::orchestrator::scope_key(&project), "project brain".into());
        assert_eq!(l.running_orchestrator().map(String::as_str), Some("project brain"));
        assert_eq!(l.spec().unwrap().cwd, project);
        tap(&mut l, NO_PROJECT);
        assert!(l.no_project && l.running_orchestrator().is_none());
        assert_eq!(l.spec().unwrap().cwd, bro_core::orchestrator::workspace());
        l.data.orchestrator_running.insert(bro_core::orchestrator::GLOBAL_KEY.into(), "workspace brain".into());
        assert_eq!(l.running_orchestrator().map(String::as_str), Some("workspace brain"));
        tap(&mut l, PROJECT);
        assert!(l.project_picker.is_some(), "the path stays selectable on the orchestrator tab");
        l.select_project(project.clone());
        assert!(!l.no_project);
        assert_eq!(l.running_orchestrator().map(String::as_str), Some("project brain"));
        l.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
        assert!(l.no_project);
    }

    #[test]
    fn gemini_survives_clicking_each_tool_tile() {
        let mut l = launcher().with_profile("gemini:work").unwrap();
        for (i, harness) in Harness::ALL.iter().enumerate() {
            tap(&mut l, HARNESS + i);
            let spec = l.spec().unwrap();
            assert_eq!(spec.harness, *harness);
            assert_eq!(spec.profile_id.as_deref(), Some("gemini:work"));
            assert_eq!(spec.model.as_deref(), Some("gemini-3.8-flash"));
        }
    }

    #[test]
    fn the_wheel_moves_the_list_under_the_pointer() {
        let mut l = launcher();
        let k = row_of(&l, AccountKind::Provider("openrouter".into()));
        tap(&mut l, ROW + k);
        tap(&mut l, ROW + k);
        assert_eq!(l.focus, Focus::Models);
        wheel(&mut l, Some(MODEL_AREA), true);
        assert_eq!((l.focus, l.model_sel), (Focus::Models, 3));
        wheel(&mut l, Some(ROW), false);
        assert_eq!((l.focus, l.list_sel), (Focus::List, k - 3));
    }

    #[test]
    fn the_project_picker_takes_every_click() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("proj");
        std::fs::create_dir(&dir).unwrap();
        let mut l = launcher().with_projects(vec![dir.clone()]);
        tap(&mut l, PROJECT);
        assert!(l.project_picker.is_some());
        // the page beneath doesn't react
        assert!(matches!(tap(&mut l, CLOSE), Outcome::None));
        assert!(l.project_picker.is_some());
        // a row opens it at once, as in the terminal UI
        let i = l.project_picker.as_ref().unwrap().view().iter().position(|(p, _)| *p == dir).unwrap();
        assert!(matches!(tap(&mut l, PICKER + picker::ROW + i), Outcome::Project(ref p) if *p == dir));
        assert!(l.project_picker.is_none());
        assert_eq!(l.dir, dir);
        // × closes it
        tap(&mut l, PROJECT);
        tap(&mut l, PICKER + picker::CLOSE);
        assert!(l.project_picker.is_none());
    }

    #[test]
    fn the_address_bar_elides_the_middle_first() {
        let w = [20.0, 40.0, 30.0, 60.0];
        let all = 150.0 + 3.0 * CRUMB_SEP;
        assert_eq!(crumb_plan(&w, 10.0, all), [Some(0), Some(1), Some(2), Some(3)]);
        // first, "…", the last two
        assert_eq!(crumb_plan(&w, 10.0, all - 1.0), [Some(0), None, Some(2), Some(3)]);
        // first, "…", the last
        assert_eq!(crumb_plan(&w, 10.0, 90.0 + 2.0 * CRUMB_SEP), [Some(0), None, Some(3)]);
        // "…" and the last, then the last alone
        assert_eq!(crumb_plan(&w, 10.0, 70.0 + CRUMB_SEP), [None, Some(3)]);
        assert_eq!(crumb_plan(&w, 10.0, 30.0), [Some(3)]);
        assert_eq!(crumb_plan(&[50.0], 10.0, 10.0), [Some(0)]);
        assert!(crumb_plan(&[], 10.0, 100.0).is_empty());
    }

    #[test]
    fn paths_become_breadcrumbs() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(path_parts(&home.join("code").join("bro")), ["~", "code", "bro"]);
        assert_eq!(path_parts(&home), ["~"]);
        if cfg!(windows) {
            assert_eq!(path_parts(std::path::Path::new(r"Q:\code\bro-cli-v2")), ["Q:", "code", "bro-cli-v2"]);
            assert_eq!(path_parts(std::path::Path::new(r"Q:\")), ["Q:"]);
        } else {
            assert_eq!(path_parts(std::path::Path::new("/opt/x")), ["/", "opt", "x"]);
            assert_eq!(path_parts(std::path::Path::new("/")), ["/"]);
        }
    }
}
