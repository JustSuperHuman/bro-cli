//! An installed page in the desktop app: its title and line with the page's own buttons (a repo's pull, GitHub and
//! stop watching; a project's folder), the search, then the skills (grouped by where they came from on Installed;
//! each with the agents that get it, bro's own with their switches; a repo's every skill, installed or not, with tick
//! boxes and an install button; the disabled ones with their enable) beside the selected one's details: what it
//! does, where it came from and stands, who gets it and where it lives, and what can be done with it. Every row has a
//! ⋯ (on hover) and a right-click menu; ticked skills of a repo install together from the bar over the list.

use super::super::{Entry, Page, RepoStatus, SkillsView, home_of, origin_line, origin_tag, origin_text, places_line, repo_standing, repo_status, since};
use super::{ACCOUNT, ALERT, ARCHIVE, CHECK, CHEVRON_DOWN, CROSS, DELETE_FOREVER, EARTH, EYE_OFF, FOLDER, GITHUB, INFO, MORE, OPEN, REFRESH_ICON, RESTORE, SEARCH_ICON, STAR, agent_mark};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::{Logo, Mark};
use crate::app::gui::kit::widgets::{self, Kind};
use crate::app::gui::kit::{Style, Tokens, Ui, radius, size, weight};
use crate::pane::{Cx, PaneId};
use crate::services::skills::{Bundled, and_list, plural};
use crate::views::profiles::gui::{BUTTON_H, TITLE, edge_fade, focus_ring, hints, key_button, key_button_ink, key_button_width, page_card};
use bro_core::disabled_skills::Disabled;
use bro_core::skill_inventory::{Agent, Origin, Place, Skill};
use bro_core::skill_repos::{self, RepoSkill, Target};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use std::path::PathBuf;

// targets
const I_SEARCH: usize = 8;
const I_CLEAR: usize = 9;
const REFRESH: usize = 10;
const PULL: usize = 11;
const WEB: usize = 12;
/// stop watching the repo shown: its menu
const DROP: usize = 13;
const PREVIEW: usize = 14;
const REVEAL: usize = 15;
const DISABLE_SEL: usize = 16;
const GO_DISCOVER: usize = 17;
/// the details' switch for one of bro's own
const SWITCH_SEL: usize = 18;
const ENABLE_SEL: usize = 19;
const DELETE_SEL: usize = 29;
/// install the selected skill (or the ticked ones) everywhere; where to install them: a menu
const INSTALL_SEL: usize = 50;
const TARGET_SEL: usize = 51;
/// the selected skill's menu
const MORE_SEL: usize = 52;
/// the bar over a repo's ticked skills
const BULK_INSTALL: usize = 53;
const BULK_TARGET: usize = 54;
const BULK_CLEAR: usize = 55;
/// tick every skill of the repo that isn't installed (or untick them)
const TICK_ALL: usize = 56;
/// the folder disabled skills are kept in
const OPEN_STORE: usize = 57;
/// `I_ROW + k`: selectable line `k` (select; double-click: open)
const I_ROW: usize = 1_000;
/// `I_CTL + k`: line `k`'s own control (a switch, JustImagine's write, a repo skill's install or enable, a disabled
/// one's enable)
const I_CTL: usize = 10_000;
/// `I_MORE + k`: line `k`'s ⋯ (its menu)
const I_MORE: usize = 20_000;
/// `I_TICK + k`: line `k`'s tick box
const I_TICK: usize = 30_000;

const ROW_H: f32 = 52.0;
const HEAD_H: f32 = 34.0;
const NOTE_H: f32 = 30.0;
/// the bar over a repo's list while skills are ticked
const BULK_H: f32 = 48.0;
/// the details beside the list, when the page is wide enough for both
const SIDE_W: f32 = 330.0;
const SIDE_MIN: f32 = 780.0;
/// the details under the list otherwise
const UNDER_H: f32 = 140.0;

/// The selectable line a target belongs to (a right-click on it opens that line's menu).
pub(super) fn row_of(n: usize) -> Option<usize> {
    match n {
        n if n >= I_TICK + 10_000 => None,
        n if n >= I_TICK => Some(n - I_TICK),
        n if n >= I_MORE => Some(n - I_MORE),
        n if n >= I_CTL => Some(n - I_CTL),
        n if n >= I_ROW => Some(n - I_ROW),
        _ => None,
    }
}

/// The page's own buttons: (icon, label, key, kind, ink, target).
type PageButton = (Option<&'static str>, &'static str, Option<&'static str>, Kind, bool, usize);

fn page_buttons(page: &Page) -> Vec<PageButton> {
    match page {
        Page::Repo(spec) => vec![
            (Some(REFRESH_ICON), "Pull now", Some("r"), Kind::Secondary, false, PULL),
            (Some(if skill_repos::is_github(spec) { GITHUB } else { OPEN }), if skill_repos::is_github(spec) { "GitHub" } else { "Open" }, Some("l"), Kind::Ghost, false, WEB),
            (Some(EYE_OFF), "Stop watching", Some("w"), Kind::Ghost, true, DROP),
        ],
        Page::Picks => vec![(Some("\u{F018C}"), "Find more", None, Kind::Ghost, false, GO_DISCOVER)],
        Page::Project(_) => vec![(Some(FOLDER), "Show folder", Some("o"), Kind::Ghost, false, REVEAL)],
        Page::Disabled => vec![(Some(FOLDER), "Open the folder", None, Kind::Ghost, false, OPEN_STORE), (Some(REFRESH_ICON), "Look again", Some("r"), Kind::Ghost, false, REFRESH)],
        _ => vec![(Some(REFRESH_ICON), "Look again", Some("r"), Kind::Ghost, false, REFRESH)],
    }
}

/// Draw an installed page in `col`.
pub(super) fn draw(v: &mut SkillsView, ui: &mut Ui, col: Rect, cx: &mut Cx) {
    let t = ui.t;
    let id = cx.id;
    let (title, sub) = v.heading(cx);

    // ---- title, the page's buttons beside it, its line under them
    let buttons = page_buttons(&v.page);
    let widths: Vec<f32> = buttons.iter().map(|(i, l, k, ..)| key_button_width(ui, *i, l, *k)).collect();
    let buttons_w = widths.iter().sum::<f32>() + 8.0 * widths.len().saturating_sub(1) as f32;
    let tb = ui.line(&title, Style::new(TITLE, weight::SEMIBOLD, t.text), Some((col.w - buttons_w - 24.0).max(60.0)));
    ui.draw(&tb, Point::new(col.x, col.y));
    let mut bx = col.right() - buttons_w;
    for ((icon, label, key, kind, danger, n), w) in buttons.iter().zip(&widths) {
        let r = Rect::new(bx, col.y - 4.0, *w, BUTTON_H);
        key_button_ink(ui, r, *icon, label, *key, *kind, danger.then_some(t.danger), Some(Action::View(id, *n)));
        bx += w + 8.0;
    }
    let sb = ui.para(&sub, Style::new(size::BODY, weight::REGULAR, t.muted), col.w, Some(2));
    let mut y = col.y + tb.h.max(BUTTON_H - 6.0) + 4.0;
    ui.draw(&sb, Point::new(col.x, y));
    y += sb.h + 14.0;

    // ---- a repo whose last pull didn't work says so
    if let Page::Repo(spec) = &v.page
        && let Some(e) = cx.svc.state().skills.repos.get(spec).and_then(|r| r.error.clone())
    {
        let bw = key_button_width(ui, Some(REFRESH_ICON), "Try again", None);
        let text = ui.para(&format!("Couldn't pull it: {e}"), Style::new(size::SMALL, weight::MEDIUM, t.danger), col.w - 60.0 - bw, Some(2));
        let h = (text.h + 20.0).max(BUTTON_H + 12.0);
        let r = Rect::new(col.x, y, col.w, h);
        ui.panel(r, radius::CONTROL, t.danger.with_alpha(0.08), t.danger.with_alpha(0.35));
        ui.icon(Rect::new(r.x + 12.0, r.y, 18.0, h), ALERT, size::TITLE, t.danger);
        ui.draw(&text, Point::new(r.x + 38.0, r.y + (h - text.h) / 2.0));
        key_button(ui, Rect::new(r.right() - 8.0 - bw, r.y + (h - BUTTON_H) / 2.0, bw, BUTTON_H), Some(REFRESH_ICON), "Try again", None, Kind::Secondary, Some(Action::View(id, PULL)));
        y += h + 12.0;
    }

    // ---- the search
    let fw = col.w.min(460.0);
    let field = Rect::new(col.x, y, fw, BUTTON_H);
    let typing = v.installed.typing && cx.focused;
    widgets::field(ui, field, &v.installed.query, Some(v.installed.query.len()), "Search these skills", typing, Action::View(id, I_SEARCH));
    let end = Rect::new(field.right() - 30.0, y + 2.0, 28.0, BUTTON_H - 4.0);
    if !v.installed.query.is_empty() {
        widgets::icon_button(ui, end, CROSS, t.muted, Action::View(id, I_CLEAR));
    } else if !typing {
        ui.icon(end, SEARCH_ICON, size::BODY, t.faint);
    }
    y += BUTTON_H + 14.0;

    // ---- a repo's ticked skills: install them together. The bar is there while any can be ticked, ticked or not, so
    // ticking one doesn't move the rest from under the pointer
    if let Page::Repo(spec) = &v.page
        && (!v.ticked.is_empty() || tickable(v, spec, cx) > 0)
    {
        bulk_bar(v, ui, Rect::new(col.x, y, col.w, BULK_H), id);
        y += BULK_H + 12.0;
    }

    // ---- the list, and the selected one's details beside it (under it when narrow)
    let hints_y = col.bottom() - 18.0;
    let body = Rect::new(col.x, y, col.w, (hints_y - 14.0 - y).max(ROW_H));
    let side = col.w >= SIDE_MIN;
    let (list, detail) = if side {
        (Rect::new(body.x, body.y, body.w - SIDE_W - 20.0, body.h), Rect::new(body.right() - SIDE_W, body.y, SIDE_W, body.h))
    } else {
        let dh = UNDER_H.min(body.h * 0.45);
        (Rect::new(body.x, body.y, body.w, body.h - dh - 12.0), Rect::new(body.x, body.bottom() - dh, body.w, dh))
    };
    let entries = v.installed_list(cx);
    let at = SkillsView::selectable(&entries);
    v.installed.sel = v.installed.sel.min(at.len().saturating_sub(1));
    if at.is_empty() && !entries.iter().any(|e| matches!(e, Entry::Note(_))) {
        empty(v, ui, list, cx);
    } else {
        draw_list(v, ui, list, &entries, cx);
    }
    details(v, ui, detail, side, cx);

    hints(ui, Point::new(col.x, hints_y), col.w, &v.hints());
}

/// How many skills of watched repo `spec` can be ticked (aren't installed or disabled).
fn tickable(v: &SkillsView, spec: &str, cx: &Cx) -> usize {
    let Some(inv) = cx.svc.state().skills.inventory.clone() else { return 0 };
    v.repo_skills(spec, cx).iter().filter(|f| repo_status(spec, &f.name, &inv).tickable()).count()
}

/// The ticked skills of the repo shown, and their install: everywhere, into a project, or untick them. With none
/// ticked yet, how it works and ticking them all.
fn bulk_bar(v: &SkillsView, ui: &mut Ui, r: Rect, id: PaneId) {
    let t = ui.t;
    if v.ticked.is_empty() {
        ui.panel(r, radius::CONTROL, t.control.with_alpha(0.35), t.stroke);
        let w = key_button_width(ui, Some(CHECK), "Tick all", Some("*"));
        key_button(ui, Rect::new(r.right() - 8.0 - w, r.y + (r.h - BUTTON_H) / 2.0, w, BUTTON_H), Some(CHECK), "Tick all", Some("*"), Kind::Ghost, Some(Action::View(id, TICK_ALL)));
        ui.icon(Rect::new(r.x + 12.0, r.y, 18.0, r.h), INFO, size::BODY, t.faint);
        let text = "Tick the skills you want, then install them together: everywhere, or into one project";
        ui.label_in(Rect::new(r.x + 38.0, r.y, (r.w - 38.0 - w - 20.0).max(0.0), r.h), text, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        return;
    }
    ui.panel(r, radius::CONTROL, t.accent.with_alpha(0.07), t.accent.with_alpha(0.35));
    let clear_w = key_button_width(ui, None, "Clear", Some("esc"));
    let to_w = key_button_width(ui, Some(CHEVRON_DOWN), "Into a project", Some("p"));
    let all_w = key_button_width(ui, Some(EARTH), "Install everywhere", Some("i"));
    let by = r.y + (r.h - BUTTON_H) / 2.0;
    let mut bx = r.right() - 8.0 - clear_w;
    key_button(ui, Rect::new(bx, by, clear_w, BUTTON_H), None, "Clear", Some("esc"), Kind::Ghost, Some(Action::View(id, BULK_CLEAR)));
    bx -= 8.0 + to_w;
    key_button(ui, Rect::new(bx, by, to_w, BUTTON_H), Some(CHEVRON_DOWN), "Into a project", Some("p"), Kind::Secondary, Some(Action::View(id, BULK_TARGET)));
    bx -= 8.0 + all_w;
    key_button(ui, Rect::new(bx, by, all_w, BUTTON_H), Some(EARTH), "Install everywhere", Some("i"), Kind::Primary, Some(Action::View(id, BULK_INSTALL)));
    ui.icon(Rect::new(r.x + 12.0, r.y, 18.0, r.h), CHECK, size::TITLE, t.accent);
    let n = ui.line(&format!("{} ticked", v.ticked.len()), Style::new(size::BODY, weight::SEMIBOLD, t.text), None);
    ui.draw(&n, Point::new(r.x + 38.0, r.y + (r.h - n.h) / 2.0));
    let names_x = r.x + 38.0 + n.w + 10.0;
    ui.label_in(Rect::new(names_x, r.y, (bx - 12.0 - names_x).max(0.0), r.h), &and_list(&v.ticked), Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
}

/// Why there's nothing to show, and the way on.
fn empty(v: &SkillsView, ui: &mut Ui, list: Rect, cx: &Cx) {
    let t = ui.t;
    let id = cx.id;
    let (looking, ways): (bool, Vec<(&str, usize)>) = {
        let st = cx.svc.state();
        let looking = st.skills.inventory.is_none() || matches!(&v.page, Page::Repo(spec) if st.skills.repos.get(spec).is_some_and(|r| r.syncing.is_some()));
        let ways = if !v.installed.query.trim().is_empty() {
            vec![("Clear the search", I_CLEAR)]
        } else {
            match &v.page {
                Page::Installed => vec![("Add repo", super::ADD), ("Discover", GO_DISCOVER)],
                Page::Repo(spec) if !looking && st.skills.repos.get(spec).is_none_or(|r| r.skills.is_none()) => vec![("Pull now", PULL)],
                _ => vec![],
            }
        };
        (looking, ways)
    };
    let mut x = list.x + 16.0;
    let mut y = list.y + 18.0;
    if looking {
        ui.spinner(Point::new(x + 7.0, y + 10.0), 6.0, 1.7, t.muted);
        x += 24.0;
    } else if v.page == Page::Disabled {
        ui.icon(Rect::new(x, y, 18.0, 20.0), ARCHIVE, size::TITLE, t.faint);
        x += 26.0;
    }
    let b = ui.para(&empty_message(v, cx), Style::new(size::BODY, weight::REGULAR, t.muted), list.w - 32.0, Some(3));
    ui.draw(&b, Point::new(x, y));
    y += b.h + 12.0;
    for (label, n) in ways {
        let w = key_button_width(ui, None, label, None);
        key_button(ui, Rect::new(x, y, w, BUTTON_H), None, label, None, if n == super::ADD || n == PULL { Kind::Primary } else { Kind::Secondary }, Some(Action::View(id, n)));
        x += w + 8.0;
    }
}

/// Why an installed page has nothing to show, in the desktop app's words.
fn empty_message(v: &SkillsView, cx: &Cx) -> String {
    let st = cx.svc.state();
    if st.skills.inventory.is_none() {
        return "Looking at your skills…".into();
    }
    let q = v.installed.query.trim();
    if !q.is_empty() {
        return format!("Nothing here matches “{q}”.");
    }
    match &v.page {
        Page::Repo(spec) => match st.skills.repos.get(spec) {
            Some(r) if r.syncing.is_some() => format!("Pulling {}…", skill_repos::label(spec)),
            Some(r) if r.error.is_some() => "Its last pull didn't work, so bro can't list its skills yet.".into(),
            Some(r) if r.skills.is_some() => "No skills in it: bro looks for a folder with a SKILL.md at its root, one folder down, or under skills/.".into(),
            _ => "Not pulled yet: pull it to see the skills in it.".into(),
        },
        Page::Installed => "No skills yet. Add a repo, or find some on Discover.".into(),
        Page::Disabled => "Nothing disabled. Disabling a skill (d, or its menu) moves every copy of it here, out of your agents' reach, until you enable it again.".into(),
        _ => "Nothing here yet.".into(),
    }
}

/// The lines, scrolled: headings, notes, skills, bro's own, a repo's skills, the disabled ones.
fn draw_list(v: &mut SkillsView, ui: &mut Ui, list: Rect, entries: &[Entry], cx: &Cx) {
    let t = ui.t;
    let id = cx.id;
    let (inv, pending) = {
        let st = cx.svc.state();
        (st.skills.inventory.clone(), st.skills.pending.clone())
    };
    let found = match &v.page {
        Page::Repo(spec) => v.repo_skills(spec, cx),
        _ => vec![],
    };
    // lay out: (entry, top, height, selectable index)
    let mut items: Vec<(&Entry, f32, f32, Option<usize>)> = vec![];
    let (mut y, mut k) = (0.0, 0);
    for (n, e) in entries.iter().enumerate() {
        let h = match e {
            Entry::Head(_) if n == 0 => HEAD_H - 8.0,
            Entry::Head(_) => HEAD_H,
            Entry::Note(_) => NOTE_H,
            _ => ROW_H,
        };
        items.push((e, y, h, e.selectable().then_some(k)));
        if e.selectable() {
            k += 1;
        }
        y += h;
    }
    let content_h = y;
    let view = Rect::new(list.x - 8.0, list.y, list.w + 8.0, list.h);
    // the page scrolls freely; it follows the selection when that moves
    let mut scroll = v.installed.scroll as f32;
    if v.installed.shown_sel != Some(v.installed.sel) {
        if let Some(at) = items.iter().position(|i| i.3 == Some(v.installed.sel)) {
            // a group's heading comes into view with its first skill
            let top = if at > 0 && matches!(items[at - 1].0, Entry::Head(_)) { items[at - 1].1 } else { items[at].1 };
            let bottom = items[at].1 + items[at].2;
            if top - 4.0 < scroll {
                scroll = top - 4.0;
            } else if bottom + 18.0 > scroll + view.h {
                scroll = bottom + 18.0 - view.h;
            }
        }
        v.installed.shown_sel = Some(v.installed.sel);
    }
    scroll = scroll.clamp(0.0, (content_h - view.h).max(0.0));
    v.installed.scroll = scroll.round() as usize;
    let shown = ui.ease(("skills-installed-scroll", id, format!("{:?}", v.page)), scroll, 150.0);
    v.sel_at = None;
    let spin = |name: &str| pending.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
    ui.clip(view, 0.0);
    for (e, top, h, k) in &items {
        let ry = view.y + top - shown;
        if ry + h < view.y || ry > view.bottom() {
            continue;
        }
        let r = Rect::new(view.x, ry, view.w - 8.0, *h);
        let fill = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
        let on = k.is_some_and(|k| v.installed.sel == k);
        if on {
            // a menu opened with keys opens here
            v.sel_at = Some((r.x + 48.0, (r.bottom() - 6.0).min(view.bottom() - 12.0)));
        }
        match (e, k) {
            (Entry::Head(text), _) => widgets::section(ui, Rect::new(r.x + 8.0, r.bottom() - 24.0, r.w - 8.0, 18.0), text),
            (Entry::Note(text), _) => {
                ui.icon(Rect::new(r.x + 8.0, r.y + 5.0, 16.0, 18.0), INFO, size::SMALL, t.faint);
                ui.label_in(Rect::new(r.x + 30.0, r.y + 5.0, r.w - 36.0, 18.0), text, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
            }
            (Entry::Skill(i), Some(k)) => {
                let Some(s) = inv.as_ref().and_then(|inv| inv.skills.get(*i)) else { continue };
                let hot = widgets::row(ui, fill, ("skills-installed", id, *k), on, Action::View(id, I_ROW + k));
                skill_row(ui, r, s, &v.page, on, hot, spin(&s.name), *k, id);
            }
            (Entry::Bro(b), Some(k)) => {
                widgets::row(ui, fill, ("skills-installed", id, *k), on, Action::View(id, I_ROW + k));
                bro_row(ui, r, *b, on, *k, cx);
            }
            (Entry::Repo(n), Some(k)) => {
                let (Some(f), Some(inv), Page::Repo(spec)) = (found.get(*n), inv.as_deref(), &v.page) else { continue };
                let st = repo_status(spec, &f.name, inv);
                let hot = widgets::row(ui, fill, ("skills-installed", id, *k), on, Action::View(id, I_ROW + k));
                let copy = st.everywhere.or_else(|| st.projects.first().copied()).and_then(|i| inv.skills.get(i));
                let place = place_words(&st, inv, cx);
                repo_row(ui, r, f, &st, copy, &place, v.ticked.contains(&f.name), on, hot, spin(&f.name), *k, id);
            }
            (Entry::Disabled(i), Some(k)) => {
                let Some(d) = inv.as_ref().and_then(|inv| inv.disabled.get(*i)) else { continue };
                let hot = widgets::row(ui, fill, ("skills-installed", id, *k), on, Action::View(id, I_ROW + k));
                disabled_row(ui, r, d, on, hot, spin(&d.name), *k, id);
            }
            _ => {}
        }
    }
    if shown > 0.5 {
        edge_fade(ui, Rect::new(view.x, view.y, view.w, 18.0), t.surface, true);
    }
    if shown + view.h < content_h - 0.5 {
        edge_fade(ui, Rect::new(view.x, view.bottom() - 24.0, view.w, 24.0), t.surface, false);
    }
    ui.unclip();
    let fade = ui.ease(("skills-installed-bar", id), if ui.hovered(view) { 1.0 } else { 0.35 }, 200.0);
    widgets::scrollbar(ui, view, content_h, view.h, shown, fade);
}

/// A row's ⋯ (its menu), at the right end, while it's hovered or selected; returns where the row's content ends.
fn more_button(ui: &mut Ui, r: Rect, lit: bool, k: usize, id: PaneId) -> f32 {
    let right = r.right() - 8.0 - BUTTON_H;
    if lit {
        let b = Rect::new(right, r.y + (r.h - BUTTON_H) / 2.0, BUTTON_H, BUTTON_H);
        if widgets::icon_button(ui, b, MORE, ui.t.muted, Action::View(id, I_MORE + k)) {
            widgets::tooltip(ui, b, "More (or right-click)");
        }
    }
    right - 6.0
}

/// The agents' marks, right to left from `right`; returns where they start.
fn agent_marks(ui: &mut Ui, mut right: f32, mid: f32, agents: &[Agent], hot: f32) -> f32 {
    let t = ui.t;
    for a in agents.iter().rev() {
        right -= 18.0;
        let mr = Rect::new(right, mid - 9.0, 18.0, 18.0);
        let m = agent_mark(*a);
        match m {
            Mark::Glyph(..) => ui.mark(m, mr, 13.0, Some(t.muted.mix(t.text, hot * 0.5))),
            _ => ui.mark(m, mr, 14.0, None),
        }
        right -= 4.0;
    }
    right
}

/// A row's name (and a pill after it) and what it does under it, from `x` to `right`.
#[allow(clippy::too_many_arguments)]
fn name_and_about(ui: &mut Ui, r: Rect, x: f32, right: f32, name: &str, about: Option<&str>, pill: Option<(&str, Color)>, on: bool, ink: Color) {
    let t = ui.t;
    let text_w = (right - 12.0 - x).max(40.0);
    let name_st = Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::MEDIUM }, ink);
    let nb = ui.line(name, name_st, Some(text_w * 0.75));
    ui.draw(&nb, Point::new(x, r.y + 8.0));
    if let Some((tag, c)) = pill {
        let pw = widgets::pill_width(ui, tag);
        if nb.w + 10.0 + pw < text_w {
            widgets::pill(ui, Point::new(x + nb.w + 8.0, r.y + 9.0), 17.0, tag, c, c.with_alpha(0.13));
        }
    }
    // one line, whatever the SKILL.md wraps its description over
    let desc = about.unwrap_or("no description in its SKILL.md");
    ui.label_in(Rect::new(x, r.y + 29.0, text_w, 18.0), desc, Style::new(size::SMALL, weight::REGULAR, if about.is_some() { t.muted } else { t.faint }), Align::Left);
}

/// One skill: its name (and where it came from, on a page that mixes them), what it's for; on the right the agents
/// that get it, and its ⋯.
#[allow(clippy::too_many_arguments)]
fn skill_row(ui: &mut Ui, r: Rect, s: &Skill, page: &Page, on: bool, hot: f32, busy: bool, k: usize, id: PaneId) {
    let t = ui.t;
    let mid = r.y + r.h / 2.0;
    let mut right = more_button(ui, r, on || hot > 0.05, k, id);
    if busy {
        ui.spinner(Point::new(right - 10.0, mid), 6.0, 1.7, t.shine);
        right -= 26.0;
    }
    right = agent_marks(ui, right, mid, &s.agents(), hot);
    // a page that mixes sources names each one's
    let pill = matches!(page, Page::Project(_)).then(|| if s.project.is_some() { ("this project".to_string(), t.accent) } else { (origin_tag(&s.origin), t.muted) });
    name_and_about(ui, r, r.x + 14.0, right, &s.name, s.summary().as_deref(), pill.as_ref().map(|(p, c)| (p.as_str(), *c)), on, t.text);
}

/// Where a skill of the repo shown is installed, in a few words.
fn place_words(st: &RepoStatus, inv: &bro_core::skill_inventory::Inventory, cx: &Cx) -> String {
    let projects: Vec<String> = st.projects.iter().filter_map(|&i| inv.skills.get(i)?.project.as_deref().map(|p| super::super::project_name(p, cx))).collect();
    match (st.everywhere.is_some(), &projects[..]) {
        (true, []) => "Everywhere".into(),
        (true, p) => format!("Everywhere + {}", plural(p.len(), "project")),
        (false, [one]) => format!("In {one}"),
        (false, p) if !p.is_empty() => format!("In {}", plural(p.len(), "project")),
        _ => String::new(),
    }
}

/// One skill of the repo shown: its tick box (or a check when it's installed, the disabled mark when it's disabled),
/// its name and what it's for; on the right where it's installed and the agents that get it, or its install (or
/// enable) button, and its ⋯.
#[allow(clippy::too_many_arguments)]
fn repo_row(ui: &mut Ui, r: Rect, f: &RepoSkill, st: &RepoStatus, copy: Option<&Skill>, place: &str, ticked: bool, on: bool, hot: f32, busy: bool, k: usize, id: PaneId) {
    let t = ui.t;
    let mid = r.y + r.h / 2.0;
    let lit = on || hot > 0.05;
    // the tick box, or where it stands
    let bx = Rect::new(r.x + 12.0, mid - 10.0, 20.0, 20.0);
    if busy {
        ui.spinner(bx.center(), 6.0, 1.7, t.shine);
    } else if st.installed() {
        ui.icon(bx, CHECK, size::HEADING, t.good);
    } else if st.disabled.is_some() {
        ui.icon(bx, ARCHIVE, size::TITLE, t.faint);
    } else {
        let hb = Rect::new(bx.x - 6.0, bx.y - 6.0, 32.0, 32.0);
        let h = ui.hover_amount(("skills-tick", id, k), hb);
        widgets::checkbox(ui, bx, ticked, h.max(if on { 0.5 } else { 0.0 }));
        ui.hit(hb, Action::View(id, I_TICK + k));
    }
    let mut right = more_button(ui, r, lit, k, id);
    // its button: install it (it isn't), enable it (it's disabled)
    let control = match (st.installed(), st.disabled.is_some()) {
        (false, false) => Some((Some(super::DOWNLOAD), "Install", if lit { Kind::Primary } else { Kind::Secondary })),
        (false, true) => Some((Some(RESTORE), "Enable", Kind::Secondary)),
        _ => None,
    };
    if let Some((icon, label, kind)) = control.filter(|_| !busy) {
        let w = key_button_width(ui, icon, label, None).max(92.0);
        right -= w;
        key_button(ui, Rect::new(right, mid - BUTTON_H / 2.0, w, BUTTON_H), icon, label, None, kind, Some(Action::View(id, I_CTL + k)));
        right -= 12.0;
    }
    if let Some(s) = copy {
        right = agent_marks(ui, right, mid, &s.agents(), hot) - 8.0;
    }
    if !place.is_empty() {
        let st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
        let w = ui.measure(place, st).min(160.0);
        right -= w;
        ui.label_in(Rect::new(right, r.y, w, r.h), place, st, Align::Left);
        right -= 10.0;
    }
    // a skill of yours of that name: installing it everywhere leaves it out
    let pill = (st.taken.is_some() && !st.installed()).then_some(("name taken", t.inline));
    let ink = if st.disabled.is_some() && !st.installed() { t.text.mix(t.muted, 0.45) } else { t.text };
    let about = f.description.as_deref().map(bro_core::skill_inventory::one_line);
    name_and_about(ui, r, r.x + 44.0, right, &f.name, about.as_deref(), pill, on, ink);
}

/// One disabled skill: its name and what it was for; on the right when it was disabled, the agents that had it,
/// its enable and its ⋯.
#[allow(clippy::too_many_arguments)]
fn disabled_row(ui: &mut Ui, r: Rect, d: &Disabled, on: bool, hot: f32, busy: bool, k: usize, id: PaneId) {
    let t = ui.t;
    let mid = r.y + r.h / 2.0;
    let lit = on || hot > 0.05;
    let mut right = more_button(ui, r, lit, k, id);
    if busy {
        ui.spinner(Point::new(right - 10.0, mid), 6.0, 1.7, t.shine);
        right -= 26.0;
    } else {
        let w = key_button_width(ui, Some(RESTORE), "Enable", None);
        right -= w;
        key_button(ui, Rect::new(right, mid - BUTTON_H / 2.0, w, BUTTON_H), Some(RESTORE), "Enable", None, if lit { Kind::Primary } else { Kind::Secondary }, Some(Action::View(id, I_CTL + k)));
        right -= 12.0;
    }
    right = agent_marks(ui, right, mid, &d.agents(), hot) - 8.0;
    let when = since(d.at);
    let st = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let w = ui.measure(&when, st);
    right -= w;
    ui.label_in(Rect::new(right, r.y, w, r.h), &when, st, Align::Left);
    right -= 10.0;
    let about = d.description.as_deref().map(bro_core::skill_inventory::one_line);
    let pill = d.project.as_ref().map(|_| ("a project's", t.muted));
    name_and_about(ui, r, r.x + 14.0, right, &d.name, about.as_deref(), pill, on, t.text);
}

/// One of bro's own: its name and the folders it writes, what it does; its switch on the right (JustImagine's: a
/// button that writes them; the orchestrator's and the browser's: written when needed).
fn bro_row(ui: &mut Ui, r: Rect, b: Bundled, on: bool, k: usize, cx: &Cx) {
    let t = ui.t;
    let id = cx.id;
    let mid = r.y + r.h / 2.0;
    let mut right = r.right() - 12.0;
    match cx.svc.bundled_on(b) {
        Some(state) => {
            right -= widgets::SWITCH_W;
            widgets::switch(ui, Point::new(right, mid - widgets::SWITCH_H / 2.0), state, Action::View(id, I_CTL + k));
            let label = if state { "On" } else { "Off" };
            let st = Style::new(size::BODY, weight::MEDIUM, if state { t.text } else { t.muted });
            let w = ui.measure(label, st);
            right -= 10.0 + w;
            ui.label_in(Rect::new(right, r.y, w, r.h), label, st, Align::Left);
        }
        None if b == Bundled::JustImagine => {
            let written = cx.svc.state().skills.imagine == Some(true);
            let label = if written { "Write again" } else { "Install" };
            let w = key_button_width(ui, None, label, None);
            right -= w;
            key_button(ui, Rect::new(right, mid - BUTTON_H / 2.0, w, BUTTON_H), None, label, None, Kind::Secondary, Some(Action::View(id, I_CTL + k)));
        }
        None => {
            let st = Style::new(size::SMALL, weight::REGULAR, t.faint);
            let w = ui.measure("when it's needed", st);
            right -= w;
            ui.label_in(Rect::new(right, r.y, w, r.h), "when it's needed", st, Align::Left);
        }
    }
    let x = r.x + 14.0;
    let text_w = (right - 16.0 - x).max(40.0);
    let nb = ui.line(b.title(), Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::MEDIUM }, t.text), None);
    ui.draw(&nb, Point::new(x, r.y + 8.0));
    let names = ui.line(&b.names().join(", "), Style::mono(size::CAPTION, t.faint), Some((text_w - nb.w - 10.0).max(10.0)));
    ui.draw(&names, Point::new(x + nb.w + 10.0, r.y + 8.0 + (nb.h - names.h) / 2.0 + 1.0));
    ui.label_in(Rect::new(x, r.y + 29.0, text_w, 18.0), b.about(), Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
}

/// A button of the details.
struct Btn {
    icon: Option<&'static str>,
    label: &'static str,
    key: Option<&'static str>,
    kind: Kind,
    danger: bool,
    target: usize,
}

const fn btn(icon: &'static str, label: &'static str, key: Option<&'static str>, kind: Kind, target: usize) -> Btn {
    Btn { icon: Some(icon), label, key, kind, danger: false, target }
}

/// What the details say about the selected line, laid out the same for every kind of line.
struct Card {
    name: String,
    /// where it came from, and its mark
    origin: String,
    mark: Mark,
    about: Option<String>,
    /// where it stands: (icon, words, colour)
    standing: Option<(&'static str, String, Color)>,
    /// who gets it (or got it), under a heading
    places: Option<(&'static str, Vec<Place>)>,
    /// its folder, under a heading
    folder: Option<(&'static str, PathBuf)>,
    /// the first is the main one (full width beside the list); the rest go two to a row
    buttons: Vec<Btn>,
}

/// The selected line's details: beside the list, or under it in a narrow pane.
fn details(v: &SkillsView, ui: &mut Ui, r: Rect, side: bool, cx: &Cx) {
    let t = ui.t;
    page_card(ui, r);
    ui.clip(r, radius::OVERLAY);
    let card = match v.chosen(cx) {
        Some(Entry::Skill(i)) => v.installed_skill(i, cx).map(|s| skill_card(&s, t, cx)),
        Some(Entry::Repo(k)) => v.repo_skill(k, cx).map(|(spec, f, st)| repo_card(v, &spec, &f, &st, t, cx)),
        Some(Entry::Disabled(i)) => v.disabled_skill(i, cx).map(|d| disabled_card(&d, t, cx)),
        Some(Entry::Bro(b)) => {
            bro_details(ui, r, b, side, cx);
            None
        }
        _ => {
            ui.label_in(Rect::new(r.x + 18.0, r.y + 18.0, r.w - 36.0, 20.0), "Pick a skill to see what it does", Style::new(size::BODY, weight::REGULAR, t.faint), Align::Left);
            None
        }
    };
    if let Some(c) = card {
        if side { card_side(ui, r, &c, cx.id) } else { card_under(ui, r, &c, cx.id) }
    }
    ui.unclip();
}

/// Where a skill came from, as a mark: bro's, its repo's, Skill Issue's, Codex's (OpenAI's logo), yours.
fn origin_mark(o: &Origin) -> Mark {
    match o {
        Origin::Bro => Mark::Glyph("\u{F140B}", None),
        Origin::Repo(spec) if skill_repos::pick(spec).is_some() => Mark::Glyph(STAR, None),
        Origin::Repo(spec) if skill_repos::is_github(spec) => Mark::Glyph(GITHUB, None),
        Origin::Repo(_) => Mark::Glyph("\u{F062C}", None),
        Origin::Codex => Mark::Logo(Logo::OpenAi),
        Origin::Yours => Mark::Glyph(ACCOUNT, None),
    }
}

/// An installed skill's details: open it, its folder, disable it (or what keeps it), its repo.
fn skill_card(s: &Skill, t: &Tokens, cx: &Cx) -> Card {
    let mut buttons = vec![btn(OPEN, "Open SKILL.md", Some("enter"), Kind::Primary, PREVIEW), btn(FOLDER, "Folder", Some("o"), Kind::Secondary, REVEAL)];
    if s.can_disable() {
        buttons.push(btn(ARCHIVE, "Disable", Some("d"), Kind::Secondary, DISABLE_SEL));
    }
    if let Origin::Repo(spec) = &s.origin {
        let github = skill_repos::is_github(spec);
        buttons.push(btn(if github { GITHUB } else { OPEN }, if github { "GitHub" } else { "Repo" }, Some("l"), Kind::Secondary, WEB));
    }
    buttons.push(btn(MORE, "More", Some("m"), Kind::Ghost, MORE_SEL));
    let standing = match s.origin {
        Origin::Yours if s.project.is_none() => Some((ACCOUNT, "Yours: bro never changes it. Disabling moves every copy to Disabled, out of reach, until you enable it again.".to_string(), t.muted)),
        Origin::Codex => Some((INFO, "Codex ships it and keeps it up to date.".to_string(), t.muted)),
        Origin::Bro => Some((INFO, "bro writes it for a feature of its own: switch it on or off on the bro page.".to_string(), t.muted)),
        _ => None,
    };
    Card { name: s.name.clone(), origin: origin_line(s, cx), mark: origin_mark(&s.origin), about: s.description.clone(), standing, places: Some(("Available to", s.places.clone())), folder: Some(("Folder", s.dir().to_path_buf())), buttons }
}

/// A watched repo's skill: installed (where, and its folder), disabled, or waiting to be picked; its install.
fn repo_card(v: &SkillsView, spec: &str, f: &RepoSkill, st: &RepoStatus, t: &Tokens, cx: &Cx) -> Card {
    let inv = cx.svc.state().skills.inventory.clone().unwrap_or_default();
    let copy = st.everywhere.or_else(|| st.projects.first().copied()).and_then(|i| inv.skills.get(i).cloned());
    let words = repo_standing(st, &inv, cx);
    let (good, muted, inline) = (t.good, t.muted, t.inline);
    let standing = match (&copy, st.disabled.is_some(), st.taken.is_some()) {
        (Some(_), ..) => Some((CHECK, format!("{}{}", words[..1].to_uppercase(), &words[1..]), good)),
        (None, true, _) => Some((ARCHIVE, "Disabled: enable it to put it back where it was.".to_string(), muted)),
        (None, false, true) => Some((ALERT, "Another skill in your agents' homes has its name: installing it everywhere leaves it out. A project can still have it.".to_string(), inline)),
        _ => Some((INFO, "Not installed. Install it everywhere (every agent, every project) or into one project; bro keeps it in step with the repo.".to_string(), muted)),
    };
    let github = skill_repos::is_github(spec);
    let web = btn(if github { GITHUB } else { OPEN }, if github { "GitHub" } else { "Repo" }, Some("l"), Kind::Secondary, WEB);
    let buttons = match (&copy, st.disabled.is_some()) {
        (Some(_), _) => vec![btn(OPEN, "Open SKILL.md", Some("enter"), Kind::Primary, PREVIEW), btn(FOLDER, "Folder", Some("o"), Kind::Secondary, REVEAL), btn(ARCHIVE, "Disable", Some("d"), Kind::Secondary, DISABLE_SEL), web, btn(MORE, "More", Some("m"), Kind::Ghost, MORE_SEL)],
        (None, true) => vec![btn(RESTORE, "Enable", Some("e"), Kind::Primary, ENABLE_SEL), Btn { danger: true, ..btn(DELETE_FOREVER, "Delete forever", Some("x"), Kind::Secondary, DELETE_SEL) }, btn(OPEN, "Open SKILL.md", Some("enter"), Kind::Secondary, PREVIEW), web],
        (None, false) => {
            let ticked = v.ticked.contains(&f.name);
            vec![
                btn(EARTH, "Install everywhere", Some("i"), Kind::Primary, INSTALL_SEL),
                btn(CHEVRON_DOWN, "Into a project", Some("p"), Kind::Secondary, TARGET_SEL),
                btn(CHECK, if ticked { "Untick" } else { "Tick" }, Some("space"), Kind::Secondary, I_TICK + v.installed.sel),
                btn(OPEN, "SKILL.md", Some("enter"), Kind::Secondary, PREVIEW),
                web,
            ]
        }
    };
    Card {
        name: f.name.clone(),
        origin: format!("in {}", skill_repos::label(spec)),
        mark: origin_mark(&Origin::Repo(spec.to_string())),
        about: f.description.clone(),
        standing,
        places: copy.as_ref().map(|s| ("Available to", s.places.clone())),
        folder: Some(match &copy {
            Some(s) => ("Folder", s.dir().to_path_buf()),
            None => ("In the clone", f.dir.clone()),
        }),
        buttons,
    }
}

/// A disabled skill: enable it, delete it for good, open it where it's kept.
fn disabled_card(d: &Disabled, t: &Tokens, cx: &Cx) -> Card {
    let muted = t.muted;
    let mut buttons = vec![btn(RESTORE, "Enable", Some("e"), Kind::Primary, ENABLE_SEL), Btn { danger: true, ..btn(DELETE_FOREVER, "Delete forever", Some("x"), Kind::Secondary, DELETE_SEL) }, btn(OPEN, "SKILL.md", Some("enter"), Kind::Secondary, PREVIEW), btn(FOLDER, "Folder", Some("o"), Kind::Secondary, REVEAL)];
    if matches!(d.origin, Origin::Repo(_)) {
        buttons.push(btn(GITHUB, "GitHub", Some("l"), Kind::Secondary, WEB));
    }
    Card {
        name: d.name.clone(),
        origin: format!("disabled {} · {}", since(d.at), origin_text(&d.origin, d.project.as_deref(), cx)),
        mark: Mark::Glyph(ARCHIVE, None),
        about: d.description.clone(),
        standing: Some((INFO, "Out of every agent's reach. Enabling it puts each copy back where it was.".to_string(), muted)),
        places: Some(("Was in", d.places.clone())),
        folder: Some(("Kept in", d.dir())),
        buttons,
    }
}

/// The details beside the list: name, where it came from, what it does, where it stands, who gets it from where,
/// its folder; the buttons at the foot (from the bottom up when there's room).
fn card_side(ui: &mut Ui, r: Rect, c: &Card, id: PaneId) {
    let t = ui.t;
    let pad = 18.0;
    let x = r.x + pad;
    let w = r.w - 2.0 * pad;
    let mut y = r.y + pad;
    let nb = ui.para(&c.name, Style::new(size::HEADING + 2.0, weight::SEMIBOLD, t.text), w, Some(2));
    ui.draw(&nb, Point::new(x, y));
    y += nb.h + 4.0;
    let ob = ui.para(&c.origin, Style::new(size::SMALL, weight::MEDIUM, t.muted), w - 22.0, Some(2));
    let ms = if matches!(c.mark, Mark::Logo(_)) { 11.0 } else { size::SMALL };
    ui.mark(c.mark, Rect::new(x, y, 14.0, 18.0), ms, Some(t.muted));
    ui.draw(&ob, Point::new(x + 20.0, y + 1.0));
    y += ob.h.max(18.0) + 14.0;
    // what it does: the lines its SKILL.md breaks by hand run on, its paragraphs and lists stay
    let desc = c.about.as_deref().map(bro_core::skill_inventory::reflow).unwrap_or_else(|| "Its SKILL.md has no description.".into());
    let db = ui.para(&desc, Style::new(size::BODY, weight::REGULAR, if c.about.is_some() { t.text.mix(t.muted, 0.15) } else { t.faint }), w, Some(9));
    ui.draw(&db, Point::new(x, y));
    y += db.h + 14.0;
    if let Some((glyph, text, ink)) = &c.standing {
        let b = ui.para(text, Style::new(size::SMALL, weight::MEDIUM, *ink), w - 22.0, Some(4));
        ui.icon(Rect::new(x, y, 16.0, 18.0), glyph, size::BODY, *ink);
        ui.draw(&b, Point::new(x + 22.0, y + 1.0));
        y += b.h.max(18.0) + 14.0;
    }
    // who gets it, and from where
    if let Some((head, places)) = c.places.as_ref().filter(|(_, p)| !p.is_empty()) {
        widgets::section(ui, Rect::new(x, y, w, 18.0), head);
        y += 24.0;
        let mut agents: Vec<Agent> = places.iter().map(|p| p.agent).collect();
        agents.sort();
        agents.dedup();
        for a in agents {
            let dirs: Vec<_> = places.iter().filter(|p| p.agent == a).map(|p| home_of(&p.dir)).collect();
            let first = dirs.first().map(|d| crate::folder::shorten_home(d)).unwrap_or_default();
            let more = match dirs.len().saturating_sub(1) {
                0 => String::new(),
                n => format!(" + {n}"),
            };
            let m = agent_mark(a);
            match m {
                Mark::Glyph(..) => ui.mark(m, Rect::new(x, y, 16.0, 20.0), 13.0, Some(t.muted)),
                _ => ui.mark(m, Rect::new(x, y, 16.0, 20.0), 14.0, None),
            }
            let lb = ui.line(a.label(), Style::new(size::SMALL, weight::SEMIBOLD, t.text), None);
            ui.draw(&lb, Point::new(x + 24.0, y + (20.0 - lb.h) / 2.0));
            ui.label_in(Rect::new(x + 24.0 + lb.w + 8.0, y + 1.0, w - 32.0 - lb.w, 18.0), &format!("{first}{more}"), Style::mono(size::CAPTION, t.muted), Align::Left);
            y += 24.0;
        }
        y += 10.0;
    }
    if let Some((head, dir)) = &c.folder {
        widgets::section(ui, Rect::new(x, y, w, 18.0), head);
        y += 24.0;
        let pb = ui.para(&crate::folder::shorten_home(dir), Style::mono(size::CAPTION, t.muted), w, Some(3));
        ui.draw(&pb, Point::new(x, y));
        y += pb.h + 16.0;
    }
    // the buttons: the main one full width, the rest two to a row
    let rows = 1 + c.buttons.len().saturating_sub(1).div_ceil(2);
    let rows_h = rows as f32 * BUTTON_H + (rows - 1) as f32 * 8.0;
    let by = (r.bottom() - pad - rows_h).max(y);
    buttons_grid(ui, Rect::new(x, by, w, rows_h), &c.buttons, true, id);
}

/// The details under the list (a narrow pane): name, where it came from and what it does on the left, the buttons on
/// the right.
fn card_under(ui: &mut Ui, r: Rect, c: &Card, id: PaneId) {
    let t = ui.t;
    let pad = 16.0;
    let bw = 280.0f32.min(r.w * 0.46);
    let x = r.x + pad;
    let w = r.w - 3.0 * pad - bw;
    let mut y = r.y + pad - 2.0;
    let nb = ui.line(&c.name, Style::new(size::TITLE, weight::SEMIBOLD, t.text), Some(w * 0.6));
    ui.draw(&nb, Point::new(x, y));
    let ob = ui.line(&c.origin, Style::new(size::SMALL, weight::REGULAR, t.muted), Some((w - nb.w - 10.0).max(10.0)));
    ui.draw(&ob, Point::new(x + nb.w + 10.0, y + (nb.h - ob.h) / 2.0 + 1.0));
    y += nb.h + 6.0;
    let db = ui.para(c.about.as_deref().unwrap_or("Its SKILL.md has no description."), Style::new(size::SMALL, weight::REGULAR, t.text.mix(t.muted, 0.2)), w, Some(2));
    ui.draw(&db, Point::new(x, y));
    y += db.h + 6.0;
    let line = match (&c.standing, &c.places) {
        (Some((_, text, ink)), _) => Some((text.clone(), *ink)),
        (None, Some((_, places))) => Some((places_line(places), t.faint)),
        _ => None,
    };
    if let Some((text, ink)) = line {
        ui.label_in(Rect::new(x, y, w, 18.0), &text, Style::new(size::SMALL, weight::REGULAR, ink), Align::Left);
    }
    // two rows of buttons, the main one first
    let shown = &c.buttons[..c.buttons.len().min(3)];
    let rows_h = BUTTON_H * 2.0 + 8.0;
    buttons_grid(ui, Rect::new(r.right() - pad - bw, r.y + (r.h - rows_h).max(0.0) / 2.0, bw, rows_h), shown, false, id);
}

/// Buttons in `r`: the first full width, the rest two to a row; `keys`: with their keys.
fn buttons_grid(ui: &mut Ui, r: Rect, buttons: &[Btn], keys: bool, id: PaneId) {
    let half = (r.w - 8.0) / 2.0;
    let mut y = r.y;
    for (n, b) in buttons.iter().enumerate() {
        let rect = match n {
            0 => Rect::new(r.x, y, r.w, BUTTON_H),
            n if n % 2 == 1 => Rect::new(r.x, y, half, BUTTON_H),
            _ => Rect::new(r.x + half + 8.0, y, half, BUTTON_H),
        };
        if n == 0 || n % 2 == 0 {
            y += BUTTON_H + 8.0;
        }
        if n == 0 {
            focus_ring(ui, rect);
        }
        // a key only where the label still fits beside it
        let key = b.key.filter(|k| keys && key_button_width(ui, b.icon, b.label, Some(k)) <= rect.w);
        key_button_ink(ui, rect, b.icon, b.label, key, b.kind, b.danger.then_some(ui.t.danger), Some(Action::View(id, b.target)));
    }
}

/// One of bro's own in the details: what it does, the folders it writes, where it is now, and its switch.
fn bro_details(ui: &mut Ui, r: Rect, b: Bundled, side: bool, cx: &Cx) {
    let t = ui.t;
    let id = cx.id;
    let pad = 18.0;
    let x = r.x + pad;
    let w = if side { r.w - 2.0 * pad } else { r.w * 0.58 };
    let mut y = r.y + pad;
    let nb = ui.line(b.title(), Style::new(size::HEADING + 2.0, weight::SEMIBOLD, t.text), Some(w));
    ui.draw(&nb, Point::new(x, y));
    y += nb.h + 4.0;
    let ob = ui.line(&format!("bundled with bro · writes {}", b.names().join(", ")), Style::new(size::SMALL, weight::MEDIUM, t.muted), Some(w));
    ui.draw(&ob, Point::new(x, y));
    y += ob.h + 12.0;
    let ab = ui.para(&format!("{}{}", b.about()[..1].to_uppercase(), &b.about()[1..]), Style::new(size::BODY, weight::REGULAR, t.text.mix(t.muted, 0.15)), w, Some(5));
    ui.draw(&ab, Point::new(x, y));
    y += ab.h + 14.0;
    let homes = cx.svc.state().skills.inventory.as_ref().map_or(0, |inv| inv.everywhere().filter(|s| s.origin == Origin::Bro && b.names().contains(&s.name.as_str())).map(|s| s.places.len()).max().unwrap_or(0));
    let (glyph, line, ink) = if homes > 0 { (CHECK, format!("In {} now", plural(homes, "agent home")), t.good) } else { (INFO, "In no agent home yet: new sessions get it when it's on".to_string(), t.muted) };
    ui.icon(Rect::new(x, y, 16.0, 18.0), glyph, size::BODY, ink);
    ui.label_in(Rect::new(x + 22.0, y, w - 22.0, 18.0), &line, Style::new(size::SMALL, weight::MEDIUM, ink), Align::Left);
    // the switch, as a button that says what it does
    let (label, kind, act) = match cx.svc.bundled_on(b) {
        Some(true) => ("Turn off", Kind::Secondary, true),
        Some(false) => ("Turn on", Kind::Primary, true),
        None if b == Bundled::JustImagine => (if cx.svc.state().skills.imagine == Some(true) { "Write them again" } else { "Install them" }, Kind::Primary, true),
        None => ("Written when it's needed", Kind::Ghost, false),
    };
    let bw = if side { r.w - 2.0 * pad } else { 200.0f32.min(r.w * 0.36) };
    let button = if side { Rect::new(x, r.bottom() - pad - BUTTON_H, bw, BUTTON_H) } else { Rect::new(r.right() - pad - bw, r.y + (r.h - BUTTON_H) / 2.0, bw, BUTTON_H) };
    if act {
        focus_ring(ui, button);
    }
    key_button(ui, button, None, label, act.then_some("enter"), kind, act.then_some(Action::View(id, SWITCH_SEL)));
}

/// Target `n` was clicked on an installed page (at `p`: a menu it opens opens there).
pub(super) fn click(v: &mut SkillsView, n: usize, p: Point, count: u32, cx: &mut Cx) {
    if n != I_SEARCH && n != I_CLEAR {
        v.installed.typing = false;
    }
    let at = Some((p.x, p.y));
    match n {
        I_SEARCH => v.installed.typing = true,
        I_CLEAR => {
            v.installed.query.clear();
            v.installed.reset();
            v.installed.typing = true;
        }
        REFRESH => {
            cx.svc.scan_skills();
            cx.svc.load_skill_repos();
        }
        PULL => {
            if let Page::Repo(spec) = &v.page {
                cx.svc.sync_skill_repo(spec, true);
            }
        }
        WEB => v.open_web(cx),
        DROP => v.stop_menu(at, cx),
        PREVIEW | SWITCH_SEL => v.activate(cx),
        REVEAL => v.reveal(cx),
        OPEN_STORE => {
            let dir = bro_core::disabled_skills::root();
            let _ = std::fs::create_dir_all(&dir);
            super::super::reveal(&dir);
        }
        DISABLE_SEL => v.disable_chosen(cx),
        ENABLE_SEL => v.enable_chosen(cx),
        DELETE_SEL => {
            v.delete_chosen(cx);
            if let Some(m) = v.menu.as_mut() {
                m.at = at;
            }
        }
        GO_DISCOVER => v.go(Page::Discover),
        INSTALL_SEL | BULK_INSTALL => v.install_here(Target::Everywhere, cx),
        TARGET_SEL | BULK_TARGET => v.choose_target(at, cx),
        MORE_SEL => v.open_menu(at, cx),
        BULK_CLEAR => v.ticked.clear(),
        TICK_ALL => v.tick_all(cx),
        n if n >= I_TICK => {
            v.installed.sel = n - I_TICK;
            v.tick_chosen(cx);
        }
        n if n >= I_MORE => {
            v.installed.sel = n - I_MORE;
            v.open_menu(at, cx);
        }
        n if n >= I_CTL => {
            v.installed.sel = n - I_CTL;
            match v.chosen(cx) {
                Some(Entry::Bro(_)) => v.activate(cx),
                Some(Entry::Skill(_)) => v.disable_chosen(cx),
                Some(Entry::Disabled(_)) => v.enable_chosen(cx),
                Some(Entry::Repo(k)) => match v.repo_skill(k, cx) {
                    Some((_, _, st)) if st.disabled.is_some() && !st.installed() => v.enable_chosen(cx),
                    // its Install: where to (everywhere straight away when there's no project to choose)
                    Some(_) if cx.svc.projects().is_empty() => v.install_here(Target::Everywhere, cx),
                    Some(_) => {
                        // the row's own skill, not the ticked ones
                        let ticked = std::mem::take(&mut v.ticked);
                        v.choose_target(at, cx);
                        v.ticked = ticked;
                    }
                    None => {}
                },
                _ => {}
            }
        }
        n if n >= I_ROW => {
            let k = n - I_ROW;
            if count >= 2 && v.installed.sel == k {
                v.activate(cx);
            }
            v.installed.sel = k;
        }
        _ => {}
    }
}
