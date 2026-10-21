//! The project picker in the desktop app: a dropdown hanging from the launcher's address bar (see
//! `launcher/gui.rs`), as wide as it, with the search or typed path, its completions and your projects, and what
//! Enter will do. Keys reach `FolderPicker::key` through the launcher; a click on a row does what
//! `FolderPicker::click` does there (and the launcher then opens it, as in the terminal UI). Clicks beside it do
//! nothing, as in the terminal UI; esc or × closes it.

use super::{FolderPicker, Kind, Outcome, looks_like_path, same_path};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Style, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::launcher::gui as page;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

// targets, after the launcher's base for the picker
pub(crate) const CLOSE: usize = 1;
pub(crate) const FIELD: usize = 2;
/// + the row's position in the list
pub(crate) const ROW: usize = 1000;

const ROW_H: f32 = 36.0;
/// The dropdown's padding.
const PAD: f32 = 20.0;
const I_FILE: &str = "\u{F0224}";
const I_FOLDER_PLUS: &str = "\u{F0B9D}";
const I_CHEVRON: &str = "\u{F0142}";
/// The check before "current" and the room after it.
const CHECK_W: f32 = 17.0;
/// The open animation's key.
const OPEN: &str = "launcher-picker-open";

/// A click on row `i`: what `FolderPicker::click` does for it (select it; open it when it's already selected).
/// None when there's no such row.
pub(crate) fn click_row(p: &mut FolderPicker, i: usize) -> Option<Outcome> {
    if p.pending.is_some() {
        return Some(Outcome::None);
    }
    if i >= p.view.len() {
        return None;
    }
    if i == p.sel {
        return Some(p.open_selected());
    }
    p.sel = i;
    p.error = None;
    Some(Outcome::None)
}

/// What the disk says about the typed path, looked up on a thread so a slow drive never holds up a frame.
struct Typed {
    path: PathBuf,
    /// (is a file, exists), once known
    state: Option<(bool, bool)>,
    at: Instant,
    pending: Option<Receiver<(bool, bool)>>,
}

thread_local! {
    static TYPED: RefCell<Option<Typed>> = const { RefCell::new(None) };
}

/// Is the typed `path` a file, and does it exist? Asked again when the path changes and every couple of seconds
/// (the terminal UI asks on every draw); until the first answer it reads as an existing folder.
fn typed_state(ui: &mut Ui, path: &Path) -> (bool, bool) {
    TYPED.with(|c| {
        let mut c = c.borrow_mut();
        let stale = c.as_ref().is_none_or(|t| t.path != path || (t.pending.is_none() && t.at.elapsed() > Duration::from_secs(2)));
        if stale {
            let (tx, rx) = channel();
            let p = path.to_path_buf();
            let asked = std::thread::Builder::new().name("bro-folder-stat".into()).spawn(move || {
                let meta = std::fs::metadata(&p);
                let _ = tx.send((meta.as_ref().is_ok_and(|m| m.is_file()), meta.is_ok()));
            });
            let known = c.as_ref().filter(|t| t.path == path).and_then(|t| t.state);
            *c = Some(Typed { path: path.to_path_buf(), state: known, at: Instant::now(), pending: asked.ok().map(|_| rx) });
        }
        let Some(t) = c.as_mut() else { return (false, true) };
        if let Some(rx) = &t.pending {
            match rx.try_recv() {
                Ok(s) => {
                    t.state = Some(s);
                    t.pending = None;
                    t.at = Instant::now();
                }
                // draw again soon to pick the answer up
                Err(TryRecvError::Empty) => ui.ticking = true,
                Err(TryRecvError::Disconnected) => t.pending = None,
            }
        }
        t.state.unwrap_or((false, true))
    })
}

/// The picker isn't open: the next time it opens, it fades in again.
pub(crate) fn closed(ui: &mut Ui) {
    ui.ease(OPEN, 0.0, 1.0);
}

/// How far the picker has opened (0..1, eased over ~140 ms): the launcher fades its scrim in with it.
pub(crate) fn opening(ui: &mut Ui) -> f32 {
    ui.ease(OPEN, 1.0, 140.0)
}

/// Draw the picker hanging from `anchor` (the launcher's address bar), as wide as it and inside `page`, sliding
/// down into place as it opens (`k`, 0..1); the launcher has dimmed the window behind it. Targets are `base + n`;
/// `current` is the launcher's folder (checked in the list).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(p: &mut FolderPicker, ui: &mut Ui, anchor: Rect, page: Rect, current: &Path, base: usize, cx: &Cx, k: f32) {
    let t = ui.t;
    // as tall as its rows need (room for six at least), growing downwards from just under the bar
    let w = anchor.w.max(320f32.min(page.w - 24.0));
    let x = anchor.x.min(page.right() - 12.0 - w).max(page.x + 12.0);
    let top = anchor.bottom() + 4.0;
    let max_h = (page.bottom() - 12.0 - top).max(220.0);
    let status = if p.error.is_some() || p.pending.is_some() { 28.0 } else { 0.0 };
    let rows = (p.view().len().max(6) as f32) * ROW_H;
    let h = (PAD + 28.0 + 12.0 + page::FIELD_H + 10.0 + status + rows + 12.0 + 24.0 + PAD - 4.0).min(max_h);
    let r = Rect::new(x, top - (1.0 - k) * 8.0, w, h);
    widgets::card(ui, r, radius::OVERLAY);
    let inner = Rect::new(r.x + PAD, r.y + PAD - 4.0, r.w - 2.0 * PAD, r.h - 2.0 * PAD + 4.0);

    // title and close
    let title = Rect::new(inner.x, inner.y, inner.w, 28.0);
    ui.label_in(Rect { w: inner.w - 40.0, ..title }, "Choose project", Style::new(size::HEADING, weight::SEMIBOLD, t.text), Align::Left);
    let close = Rect::new(title.right() - 32.0 + 6.0, title.y - 2.0, 32.0, 32.0);
    if widgets::icon_button(ui, close, page::I_CLOSE, t.muted, Action::Overlay(base + CLOSE)) {
        widgets::tooltip(ui, close, "Close · esc");
    }

    // the search or path
    let field = Rect::new(inner.x, title.bottom() + 12.0, inner.w, page::FIELD_H);
    let path_like = looks_like_path(p.filter.trim());
    let icon = if path_like { page::I_FOLDER } else { page::I_SEARCH };
    page::input(ui, field, icon, &p.filter, "find a project, or type a path (ctrl+v to paste)", true, cx.focused, path_like, Action::Overlay(base + FIELD));
    let mut y = field.bottom() + 10.0;
    // what went wrong, or that the folder is being made
    if let Some(error) = &p.error {
        let line = Rect::new(inner.x, y, inner.w, 22.0);
        ui.icon(Rect::new(line.x + page::MARK_X, line.y, 18.0, line.h), page::I_ALERT, size::BODY, t.danger);
        ui.label_in(Rect::new(line.x + page::TEXT_X, line.y, line.w - page::TEXT_X, line.h), error, Style::new(size::SMALL, weight::MEDIUM, t.danger), Align::Left);
        y += 28.0;
    } else if p.pending.is_some() {
        let line = Rect::new(inner.x, y, inner.w, 22.0);
        ui.spinner(Point::new(line.x + page::MARK_X + 9.0, line.y + line.h / 2.0), 5.5, 2.0, t.muted);
        ui.label_in(Rect::new(line.x + page::TEXT_X, line.y, line.w - page::TEXT_X, line.h), "creating folder…", Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Left);
        y += 28.0;
    }

    // the folders
    let footer = Rect::new(inner.x, inner.bottom() - 24.0, inner.w, 24.0);
    let list = Rect::new(inner.x, y, inner.w, (footer.y - 12.0 - y).max(0.0));
    let view = p.view();
    if view.is_empty() {
        ui.label(Point::new(inner.x + 12.0, list.y + 10.0), "nothing here — type a path (~/, C:\\, /…)", Style::new(size::BODY, weight::REGULAR, t.muted));
    } else {
        draw_rows(p, ui, list, view, current, base);
    }

    // the keys
    let keys: Vec<(&str, &str)> = if p.pending.is_some() {
        vec![("esc", "cancel")]
    } else {
        let creates = view.get(p.sel).is_some_and(|(path, kind)| *kind == Kind::Typed && !typed_state(ui, path).1);
        vec![("⏎", if creates { "create and use" } else { "use project" }), ("ctrl+v", "paste path"), ("tab", "complete"), ("↑↓", "move"), ("esc", "cancel")]
    };
    page::hints(ui, Rect { x: footer.x + 4.0, w: footer.w - 4.0, ..footer }, &keys);
}

/// The rows: what the folder is (typed, to create, a file, a completion, one of yours), its name, a check on the
/// launcher's folder, and where it is.
fn draw_rows(p: &FolderPicker, ui: &mut Ui, list: Rect, view: &[(PathBuf, Kind)], current: &Path, base: usize) {
    let t = ui.t;
    let content = view.len() as f32 * ROW_H;
    let off = page::scroll(ui, "picker", Some((p.sel as f32 * ROW_H, ROW_H)), content, list.h);
    let first = (off / ROW_H).floor().max(0.0) as usize;
    let last = (((off + list.h) / ROW_H).ceil() as usize + 1).min(view.len());
    let ns = Style::new(size::BODY, weight::SEMIBOLD, t.text);
    let ps = Style::mono(size::SMALL, t.faint);
    // the launcher's folder says "current" (the terminal UI's word), after a check, as the palette marks it
    let cs = Style::new(size::SMALL, weight::MEDIUM, t.accent);
    let current_w = CHECK_W + ui.measure("current", cs);
    ui.clip(list, 0.0);
    for (k, (path, kind)) in view.iter().enumerate().take(last).skip(first) {
        let rr = Rect::new(list.x, list.y + k as f32 * ROW_H - off, list.w, ROW_H);
        let on = k == p.sel;
        page::row_fill(ui, rr, ("picker", k), on, true);
        ui.hit(rr, Action::Overlay(base + ROW + k));
        let mid = rr.y + rr.h / 2.0;
        // what kind of row it is, as the terminal UI tags it
        let (file, exists) = if *kind == Kind::Typed { typed_state(ui, path) } else { (false, true) };
        let (icon, ink, tag) = match kind {
            Kind::Typed if file => (I_FILE, t.danger, Some(("file", t.danger))),
            Kind::Typed if !exists => (I_FOLDER_PLUS, t.shine, Some(("create", t.shine))),
            Kind::Typed => ("↳", t.shine, None),
            Kind::Completion => (I_CHEVRON, t.faint, None),
            Kind::Suggested => (page::I_FOLDER, t.muted, None),
        };
        ui.icon(Rect::new(rr.x + page::MARK_X, rr.y, 18.0, rr.h), icon, size::TITLE, ink);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
        let is_current = same_path(current, path);
        // where it is, on the right
        let parent = path.parent().unwrap_or(path);
        let mut tags_w = 0.0;
        if let Some((s, _)) = tag {
            tags_w += widgets::pill_width(ui, s) + 8.0;
        }
        if is_current {
            tags_w += current_w + 8.0;
        }
        let nx = rr.x + page::TEXT_X;
        let nb = ui.line(&name, ns, Some((rr.w * 0.55).max(80.0)));
        let room = rr.right() - 12.0 - (nx + nb.w + 10.0 + tags_w + 16.0);
        let where_ = if room > 40.0 { page::fit_path(ui, parent, ps, room) } else { String::new() };
        if !where_.is_empty() {
            let wb = ui.line(&where_, ps, None);
            ui.draw(&wb, Point::new(rr.right() - 12.0 - wb.w, rr.y + (rr.h - wb.h) / 2.0));
        }
        ui.draw(&nb, Point::new(nx, rr.y + (rr.h - nb.h) / 2.0));
        let mut x = nx + nb.w + 10.0;
        if let Some((s, c)) = tag {
            x += widgets::pill(ui, Point::new(x, mid - 9.0), 18.0, s, c, c.with_alpha(0.14)).w + 8.0;
        }
        // the launcher's folder: a check and "current", as a menu marks the current choice
        if is_current {
            ui.icon(Rect::new(x, rr.y, 14.0, rr.h), page::I_CHECK, 14.0, t.accent);
            let b = ui.line("current", cs, None);
            ui.draw(&b, Point::new(x + CHECK_W, mid - b.h / 2.0));
        }
    }
    ui.unclip();
    let vis = 0.5 + 0.5 * ui.hover_amount("picker-bar", list);
    widgets::scrollbar(ui, list, content, list.h, off, vis);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_click_selects_then_opens_and_waits_while_creating() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let mut p = FolderPicker::new(vec![a, b.clone()]);
        assert!(matches!(click_row(&mut p, 1), Some(Outcome::None)));
        assert_eq!(p.sel, 1);
        assert!(matches!(click_row(&mut p, 1), Some(Outcome::Open(ref x)) if *x == b));
        assert!(click_row(&mut p, 5).is_none(), "no such row");
        p.pending = Some(uuid::Uuid::new_v4());
        assert!(matches!(click_row(&mut p, 0), Some(Outcome::None)));
        assert_eq!(p.sel, 1, "nothing moves while a folder is being made");
    }
}
