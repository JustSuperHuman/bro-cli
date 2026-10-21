//! The Continue tab (earlier sessions) in the desktop app, drawn inside the launcher page (see
//! `launcher/gui.rs`): the search, the project / All sessions scope, the sessions with their picks, the keys and
//! a Resume button. Keys reach `ContinuePicker::key` through the launcher; a click on a row does what
//! `ContinuePicker::click` does there (ctrl picks one more, shift a range, the cursor row again resumes).

use super::{ContinuePicker, Entry, Outcome};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand;
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::launcher::gui as page;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// targets, after the launcher's base for this tab
pub(crate) const FIELD: usize = 1;
pub(crate) const SCOPE_PROJECT: usize = 2;
pub(crate) const SCOPE_ALL: usize = 3;
pub(crate) const RESUME: usize = 4;
/// the list's background
pub(crate) const AREA: usize = 5;
/// + the row's position in the list
pub(crate) const ROW: usize = 1000;

const ROW_H: f32 = 40.0;
const I_ARCHIVE: &str = "\u{F120E}";

/// Target `n` (this tab's own numbering) was clicked.
pub(crate) fn click(p: &mut ContinuePicker, n: usize, mods: KeyModifiers) -> Outcome {
    match n {
        SCOPE_PROJECT => p.set_all(false),
        SCOPE_ALL => p.set_all(true),
        // the button and the ⏎ hint: what Enter does
        RESUME => return p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        n if n >= ROW => return click_row(p, n - ROW, mods),
        _ => {}
    }
    Outcome::None
}

/// A click on the row at list position `k`: ctrl picks or unpicks it (and the cursor row, when it's the first
/// pick), shift picks the range from the anchor, the cursor row resumes, another row takes the cursor.
fn click_row(p: &mut ContinuePicker, k: usize, mods: KeyModifiers) -> Outcome {
    if k >= p.view.len() {
        return Outcome::None;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        if p.picked.is_empty() && k != p.sel {
            p.toggle(p.sel);
        }
        p.toggle(k);
        return Outcome::None;
    }
    if mods.contains(KeyModifiers::SHIFT) {
        p.extend(k);
        return Outcome::None;
    }
    if k == p.sel {
        return p.resume_outcome();
    }
    p.sel = k;
    p.anchor = None;
    Outcome::None
}

/// Draw the tab in `r` (below the launcher's header); targets are `base + n`.
pub(crate) fn draw(p: &mut ContinuePicker, ui: &mut Ui, r: Rect, base: usize, cx: &Cx) {
    let t = ui.t;
    let footer = Rect::new(r.x, r.bottom() - page::FOOTER_H, r.w, page::FOOTER_H);
    // the search, and the scope on its right
    let top = Rect::new(r.x, r.y, r.w, page::FIELD_H);
    let mut field_w = r.w;
    if let Some((_, name)) = &p.project {
        let name = crate::ui::fit(name, 18);
        let segs = [page::Seg::new(&name).icon(page::I_FOLDER), page::Seg::new("All sessions")];
        let kw = page::kbd_w(ui, "Tab");
        let sw = page::seg_width(ui, &segs, size::SMALL);
        let x = r.right() - kw - 10.0 - sw;
        page::segmented(ui, Point::new(x, top.y), top.h, &segs, Some(if p.all { 1 } else { 0 }), size::SMALL, "continue-scope", |i| Action::Overlay(base + if i == 0 { SCOPE_PROJECT } else { SCOPE_ALL }));
        widgets::kbd(ui, Point::new(r.right() - kw, top.y + (top.h - 18.0) / 2.0), "Tab");
        field_w = (x - 16.0 - r.x).max(120.0);
    }
    page::input(ui, Rect::new(r.x, top.y, field_w, top.h), page::I_SEARCH, &p.filter, "type to search earlier sessions", true, cx.focused, false, Action::Overlay(base + FIELD));

    // the sessions
    let list = Rect::new(r.x, top.bottom() + 12.0, r.w, (footer.y - 18.0 - top.bottom() - 12.0).max(0.0));
    ui.hit_with(list, Action::Overlay(base + AREA), Cursor::Default);
    let view = p.view();
    if view.is_empty() {
        let msg = if p.entries.is_empty() {
            "No earlier sessions yet"
        } else if p.all {
            "Nothing matches"
        } else {
            "No sessions here. Press Tab for all sessions."
        };
        ui.label(Point::new(list.x + 12.0, list.y + 12.0), msg, Style::new(size::BODY, weight::REGULAR, t.muted));
    } else {
        draw_rows(p, ui, list, &view, base);
    }

    // the keys, and Resume
    let n = p.picked.len();
    let (label, go) = match n {
        0 => ("Resume".to_string(), !view.is_empty()),
        1 => ("Resume 1 session".to_string(), true),
        n => (format!("Resume all {n}, one at a time"), true),
    };
    let (fill, ink) = if go { (t.accent, t.on_accent) } else { (t.control, t.muted) };
    let br = page::main_button(ui, footer, &label, fill, ink, go, "continue-resume", Action::Overlay(base + RESUME));
    let archived = if p.show_archived { "hide archived" } else { "show archived" };
    let resume;
    let keys: Vec<(&str, &str)> = if n == 0 {
        vec![("⏎", "resume"), ("↑↓", "move"), ("⇧↑↓ ^space", "pick several"), ("del", "archive"), ("^z", "undo"), ("^a", archived), ("esc", "close")]
    } else {
        resume = if n == 1 { "resume 1 session".to_string() } else { format!("resume all {n}, one at a time") };
        vec![("⏎", resume.as_str()), ("⇧↑↓", "range"), ("^space ^click", "pick / unpick"), ("esc", "clear picks")]
    };
    let first = page::hints(ui, Rect::new(footer.x, footer.y, (br.x - footer.x - 24.0).max(0.0), footer.h), &keys);
    if first.w > 0.0 {
        ui.hit(first, Action::Overlay(base + RESUME));
    }
}

/// A pick box (WinUI's check box, outlined): an empty box, or once picked an accent edge with the check in the
/// accent. No colour behind the check.
fn pick_box(ui: &mut Ui, center: Point, picked: bool) {
    let t = ui.t;
    let r = Rect::new(center.x - 9.0, center.y - 9.0, 18.0, 18.0);
    if picked {
        ui.panel(r, radius::CONTROL, t.control, t.accent);
        ui.icon(r, page::I_CHECK, size::SMALL + 1.0, t.accent);
    } else {
        ui.panel(r, radius::CONTROL, t.control, t.muted.with_alpha(0.55));
    }
}

/// The rows: pick, agent logo, title on the left; id, project (all sessions), login, archived and age in columns
/// on the right.
fn draw_rows(p: &ContinuePicker, ui: &mut Ui, list: Rect, view: &[&Entry], base: usize) {
    let t = ui.t;
    let content = view.len() as f32 * ROW_H;
    let off = page::scroll(ui, "continue", Some((p.sel as f32 * ROW_H, ROW_H)), content, list.h);
    let first = (off / ROW_H).floor().max(0.0) as usize;
    let last = (((off + list.h) / ROW_H).ceil() as usize + 1).min(view.len());
    let shown = &view[first..last];
    // column widths from what's on screen
    let ids = Style::mono(size::SMALL, t.faint);
    let ps = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let ls = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let ages = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let tail = |e: &Entry| format!("[{}]", e.id.rsplit('-').next().unwrap_or(&e.id));
    let mut id_w: f32 = 0.0;
    let mut proj_w: f32 = 0.0;
    let mut login_w: f32 = 0.0;
    let mut age_w: f32 = 0.0;
    for e in shown {
        id_w = id_w.max(ui.measure(&tail(e), ids));
        if p.all {
            proj_w = proj_w.max(ui.measure(&crate::ui::fit(&e.project_name, 16), ps));
        }
        if let Some(l) = &e.login {
            login_w = login_w.max(ui.measure(&crate::ui::fit(l, 12), ls));
        }
        age_w = age_w.max(ui.measure(&crate::util::short_dur(e.age_secs), ages));
    }
    // projects are a folder and a name; archived ones an archive box and the word
    let folder_w = if proj_w > 0.0 { 18.0 } else { 0.0 };
    let arch = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let arch_w = if shown.iter().any(|e| e.archived) { 18.0 + ui.measure("archived", arch) } else { 0.0 };
    ui.clip(list, 0.0);
    for (k, e) in view.iter().enumerate().take(last).skip(first) {
        let rr = Rect::new(list.x, list.y + k as f32 * ROW_H - off, list.w, ROW_H);
        let on = k == p.sel;
        let picked = p.is_picked(&e.id);
        let hot = ui.hover_amount(("continue-row", k), rr);
        if on {
            ui.round(rr, radius::CONTROL, t.selected);
        } else if picked {
            ui.round(rr, radius::CONTROL, t.accent.with_alpha(0.07 + 0.03 * hot));
        } else if hot > 0.0 {
            ui.round(rr, radius::CONTROL, t.hover.fade(hot));
        }
        ui.hit(rr, Action::Overlay(base + ROW + k));
        let mid = rr.y + rr.h / 2.0;
        // once something is picked, a column of pick boxes opens on the left
        let lead = if p.picked.is_empty() { 0.0 } else { 28.0 };
        if lead > 0.0 {
            pick_box(ui, Point::new(rr.x + 21.0, mid), picked);
        }
        ui.mark(brand::harness(e.harness), Rect::new(rr.x + page::MARK_X + lead, rr.y, 18.0, rr.h), 15.0, None);
        // the columns, right to left: age, archived, login, project, id
        let mut x = rr.right() - 12.0;
        let age = crate::util::short_dur(e.age_secs);
        let aw = ui.measure(&age, ages);
        ui.label_in(Rect::new(x - aw, rr.y, aw + 1.0, rr.h), &age, ages, Align::Left);
        x -= age_w + 16.0;
        if arch_w > 0.0 {
            if e.archived {
                ui.icon(Rect::new(x - arch_w, rr.y, 14.0, rr.h), I_ARCHIVE, size::SMALL + 1.0, t.faint);
                ui.label_in(Rect::new(x - arch_w + 18.0, rr.y, arch_w - 17.0, rr.h), "archived", arch, Align::Left);
            }
            x -= arch_w + 14.0;
        }
        if login_w > 0.0 {
            if let Some(l) = &e.login {
                ui.label_in(Rect::new(x - login_w, rr.y, login_w + 1.0, rr.h), &crate::ui::fit(l, 12), ls, Align::Left);
            }
            x -= login_w + 16.0;
        }
        if proj_w > 0.0 {
            let px = x - proj_w - folder_w;
            ui.icon(Rect::new(px, rr.y, 14.0, rr.h), page::I_FOLDER, size::SMALL + 1.0, t.faint);
            ui.label_in(Rect::new(px + folder_w, rr.y, proj_w + 1.0, rr.h), &crate::ui::fit(&e.project_name, 16), ps, Align::Left);
            x -= proj_w + folder_w + 16.0;
        }
        ui.label_in(Rect::new(x - id_w, rr.y, id_w + 1.0, rr.h), &tail(e), ids, Align::Left);
        x -= id_w + 20.0;
        // the title (archived ones muted and struck through)
        let ink = if e.archived { t.muted } else { t.text };
        let ts = Style::new(size::BODY, if on || picked { weight::SEMIBOLD } else { weight::MEDIUM }, ink);
        let tx = rr.x + page::TEXT_X + lead;
        let b = ui.line(&e.title, ts, Some((x - tx).max(10.0)));
        ui.draw(&b, Point::new(tx, rr.y + (rr.h - b.h) / 2.0));
        if e.archived {
            ui.top_fill(Rect::new(tx, mid, b.w, 1.0), 0.0, t.muted);
        }
    }
    ui.unclip();
    let vis = 0.5 + 0.5 * ui.hover_amount("continue-bar", list);
    widgets::scrollbar(ui, list, content, list.h, off, vis);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::Harness;

    fn e(idx: usize, project: &str) -> Entry {
        Entry { idx, id: format!("id{idx}"), harness: Harness::Claude, title: format!("task {idx}"), login: None, age_secs: 60, project_key: project.into(), project_name: project.into(), archived: false }
    }

    #[test]
    fn clicks_pick_range_and_resume_as_in_the_terminal() {
        let mut p = ContinuePicker::new((0..4).map(|i| e(i, "a")).collect(), None);
        // ctrl+click picks the cursor row too, so two rows end up picked
        assert!(matches!(click(&mut p, ROW + 2, KeyModifiers::CONTROL), Outcome::None));
        assert_eq!(p.picked, ["id0", "id2"]);
        click(&mut p, ROW + 3, KeyModifiers::SHIFT);
        assert_eq!(p.picked, ["id0", "id2", "id3"]);
        // the cursor row again resumes every pick; so does the button
        assert!(matches!(click(&mut p, ROW + 3, KeyModifiers::NONE), Outcome::ResumeMany(ref v) if v == &[0, 2, 3]));
        assert!(matches!(click(&mut p, RESUME, KeyModifiers::NONE), Outcome::ResumeMany(ref v) if v == &[0, 2, 3]));
        // a plain click elsewhere moves the cursor; a row that's gone does nothing
        assert!(matches!(click(&mut p, ROW + 1, KeyModifiers::NONE), Outcome::None));
        assert_eq!(p.sel, 1);
        assert!(matches!(click(&mut p, ROW + 9, KeyModifiers::NONE), Outcome::None));
        assert_eq!(p.sel, 1);
    }

    #[test]
    fn the_scope_buttons_switch_between_the_project_and_everything() {
        let mut p = ContinuePicker::new(vec![e(0, "a"), e(1, "b")], Some(("a".into(), "a".into())));
        assert_eq!(p.view().len(), 1);
        click(&mut p, SCOPE_ALL, KeyModifiers::NONE);
        assert!(p.all);
        assert_eq!(p.view().len(), 2);
        click(&mut p, SCOPE_PROJECT, KeyModifiers::NONE);
        assert!(!p.all);
    }
}
