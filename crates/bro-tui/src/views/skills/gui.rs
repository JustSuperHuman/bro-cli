//! The Skills view in the desktop app, laid out as settings is: the pages down the left as a navigation list (the
//! installed skills, Discover, each source, each sidebar project, and "Add repo" at its foot), the open page on the
//! right (see [`installed`] and [`discover`]). "Add repo" opens a dialog over the page: paste a repo, bro says what it
//! makes of it as you type, and the repos it features are a click away. A narrow pane has the pages as chips over
//! the page instead of the list.
//!
//! It draws [`SkillsView`]'s own state; keys do what they do in the terminal UI, and clicks reach everything: a
//! right-click (or a row's ⋯) opens the skill's menu where it was clicked, floating over the page; a click anywhere
//! else closes it.

mod discover;
mod installed;

use super::{AddCheck, MenuIcon, NavItem, NavNote, Page, SkillsView};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::{self, Logo, Mark};
use crate::app::gui::kit::widgets::{self, Kind, MenuLine};
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight};
use crate::pane::{Cx, PaneId};
use crate::views::profiles::gui::{BUTTON_H, key_button, key_button_width, page};
use bro_core::skill_inventory::Agent;
use bro_core::skill_repos;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;

#[cfg(test)]
pub(crate) use discover::{CLEAR, DOMAIN, MINE, ROW, SEARCH, SORT};

// targets the pages share (Discover's and the installed pages' are in their modules)
const BG: usize = 0;
const NAV_BG: usize = 1;
/// "Add repo": the dialog opens
const ADD: usize = 2;
const D_FIELD: usize = 3;
const D_ADD: usize = 4;
const D_CANCEL: usize = 5;
const D_SCRIM: usize = 6;
/// the repo the dialog suggests: fill it in
const D_SUGGEST: usize = 7;
/// `NAV + n`: page `n` of the list
const NAV: usize = 100;
/// the open menu: a click beside it closes it; `MENU + i` runs item `i`
pub(crate) const MENU_OUT: usize = 900_000;
pub(crate) const MENU: usize = 910_000;

/// the list down the left, as settings' is
const NAV_W: f32 = 232.0;
const NAV_ROW: f32 = 34.0;
const NAV_PITCH: f32 = 36.0;
/// below this the pages are chips over the page
const NAV_MIN: f32 = 700.0;
const CHIP_H: f32 = 28.0;

pub(super) const PLUS: &str = "\u{F0415}";
pub(super) const CHECK: &str = "\u{F012C}";
pub(super) const ALERT: &str = "\u{F0026}";
pub(super) const STAR: &str = "\u{F04CE}";
pub(super) const OPEN: &str = "\u{F03CC}";
pub(super) const GITHUB: &str = "\u{F02A4}";
pub(super) const REFRESH_ICON: &str = "\u{F0450}";
pub(super) const SEARCH_ICON: &str = "\u{F0349}";
pub(super) const CROSS: &str = "\u{F0156}";
pub(super) const FOLDER: &str = "\u{F0256}";
pub(super) const ACCOUNT: &str = "\u{F0004}";
pub(super) const INFO: &str = "\u{F02FD}";
pub(super) const ARCHIVE: &str = "\u{F003C}";
pub(super) const RESTORE: &str = "\u{F006F}";
pub(super) const DELETE_FOREVER: &str = "\u{F05E8}";
pub(super) const MORE: &str = "\u{F01D8}";
pub(super) const EARTH: &str = "\u{F01E7}";
pub(super) const CHEVRON_DOWN: &str = "\u{F0140}";
pub(super) const EYE_OFF: &str = "\u{F0209}";
pub(super) const DOWNLOAD: &str = "\u{F01DA}";

/// A menu item's icon.
fn menu_glyph(i: MenuIcon) -> &'static str {
    match i {
        MenuIcon::Open => "\u{F0214}",
        MenuIcon::Folder => "\u{F0770}",
        MenuIcon::Copy => "\u{F018F}",
        MenuIcon::Web => OPEN,
        MenuIcon::GitHub => GITHUB,
        MenuIcon::Everywhere => EARTH,
        MenuIcon::Project => FOLDER,
        MenuIcon::Disable => ARCHIVE,
        MenuIcon::Enable => RESTORE,
        MenuIcon::Delete => DELETE_FOREVER,
        MenuIcon::Pull => REFRESH_ICON,
        MenuIcon::Unwatch => EYE_OFF,
        MenuIcon::Switch => "\u{F0425}",
        MenuIcon::Tick => CHECK,
    }
}

/// Draw the view inside its card (`area`: the card's content).
pub(crate) fn draw(v: &mut SkillsView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let nav = v.prepare(cx);
    if v.fetching || v.busy {
        ui.ticking = true;
    }
    let id = cx.id;
    ui.hit_with(area, Action::View(id, BG), Cursor::Default);
    let col = if area.w >= NAV_MIN {
        nav_list(v, ui, Rect { w: NAV_W, ..area }, &nav, id);
        let left = area.x + NAV_W + 20.0;
        let top = if area.h >= 560.0 { 22.0 } else { 14.0 };
        Rect::new(left, area.y + top, (area.right() - left - 28.0).min(1180.0), (area.h - top - 16.0).max(0.0))
    } else {
        page_chips(v, ui, page(area, 1180.0), &nav, id)
    };
    if v.page == Page::Discover {
        discover::draw(v, ui, col, cx);
    } else {
        installed::draw(v, ui, col, cx);
    }
    if v.adding.is_some() {
        dialog(v, ui, area, cx);
    }
    // a menu left open by a pane that's no longer the one in use goes
    if !cx.focused {
        v.menu = None;
    }
    draw_menu(v, ui, area, id);
    true
}

/// The open menu, floating where it was opened (by the selected row when it was opened with keys).
fn draw_menu(v: &SkillsView, ui: &mut Ui, area: Rect, id: PaneId) {
    let Some(m) = &v.menu else { return };
    let prev = ui.layer();
    // a click anywhere else closes it
    ui.hit_with(area, Action::View(id, MENU_OUT), Cursor::Default);
    let labels: Vec<String> = (0..m.items.len()).map(|i| m.label(i)).collect();
    let lines: Vec<MenuLine> = m.items.iter().zip(&labels).map(|(it, label)| MenuLine { icon: it.icon.map(menu_glyph), label, key: it.key, heading: it.act.is_none(), danger: it.danger }).collect();
    let at = match (m.at, v.sel_at) {
        (Some((x, y)), _) => Point::new(x, y),
        (None, Some((x, y))) => Point::new(x, y),
        _ => Point::new(area.x + area.w / 2.0 - 120.0, area.y + area.h / 3.0),
    };
    let bounds = Rect::new(area.x + 6.0, area.y + 6.0, (area.w - 12.0).max(1.0), (area.h - 12.0).max(1.0));
    widgets::menu(ui, at, bounds, Some(&m.title), &lines, Some(m.sel), |i| Action::View(id, MENU + i));
    ui.set_layer(prev);
}

/// A page's mark in the list.
fn page_mark(p: &Page) -> Mark {
    match p {
        Page::Installed => Mark::Glyph("\u{F05E1}", None),
        Page::Discover => Mark::Glyph("\u{F018C}", None),
        Page::Disabled => Mark::Glyph(ARCHIVE, None),
        Page::Bro => Mark::Glyph("\u{F140B}", None),
        Page::Repo(spec) if skill_repos::is_github(spec) => Mark::Glyph(GITHUB, None),
        Page::Repo(_) => Mark::Glyph("\u{F062C}", None),
        Page::Picks => Mark::Glyph(STAR, None),
        Page::Yours => Mark::Glyph(ACCOUNT, None),
        Page::Codex => Mark::Logo(Logo::OpenAi),
        Page::Project(_) => Mark::Glyph(FOLDER, None),
    }
}

/// The pages down the left: headings over the sources and the projects, each page with what it holds, and "Add repo"
/// at the foot.
fn nav_list(v: &SkillsView, ui: &mut Ui, r: Rect, nav: &[NavItem], id: PaneId) {
    let t = ui.t;
    ui.hit_with(r, Action::View(id, NAV_BG), Cursor::Default);
    widgets::section(ui, Rect::new(r.x + 14.0, r.y + 14.0, r.w - 28.0, 18.0), "Skills");
    let add = Rect::new(r.x + 10.0, r.bottom() - 14.0 - BUTTON_H, r.w - 24.0, BUTTON_H);
    let mut y = r.y + 14.0 + 28.0;
    let mut n = 0;
    for item in nav {
        match item {
            NavItem::Head(h) => {
                y += 10.0;
                if y + 22.0 > add.y - 8.0 {
                    break;
                }
                ui.label_in(Rect::new(r.x + 14.0, y, r.w - 28.0, 18.0), h, Style::new(size::CAPTION, weight::SEMIBOLD, t.faint), Align::Left);
                y += 22.0;
            }
            NavItem::Page(p, label, note) => {
                let row = Rect::new(r.x + 4.0, y, r.w - 12.0, NAV_ROW);
                if row.bottom() > add.y - 8.0 {
                    break;
                }
                let on = *p == v.page;
                let hot = widgets::row(ui, row, ("skills-nav", id, n), on, Action::View(id, NAV + n));
                // marks stand alone in the text's colour, a touch quieter until hovered or chosen
                let ink = if on { t.text } else { t.text.mix(t.muted, 0.3 * (1.0 - hot)) };
                let ir = Rect::new(row.x + 10.0, row.y, 20.0, row.h);
                match page_mark(p) {
                    m @ Mark::Logo(_) => ui.mark(m, ir, 14.0, Some(ink)),
                    Mark::Glyph(g, _) => ui.icon(ir, g, 16.0, ink),
                }
                let right = row.right() - 12.0;
                let note_w = match note {
                    NavNote::Nothing => 0.0,
                    NavNote::Busy | NavNote::Failed => 16.0,
                    NavNote::Count(_) | NavNote::Of(..) => ui.measure(&note.text(), Style::new(size::SMALL, weight::REGULAR, t.muted)),
                };
                let lx = row.x + 40.0;
                let lb = ui.line(label, Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::REGULAR }, t.text), Some((right - note_w - 8.0 - lx).max(20.0)));
                ui.draw(&lb, Point::new(lx, row.y + (row.h - lb.h) / 2.0));
                let mid = row.y + row.h / 2.0;
                match note {
                    NavNote::Nothing => {}
                    NavNote::Count(_) | NavNote::Of(..) => {
                        let st = Style::new(size::SMALL, weight::REGULAR, if on { t.text.mix(t.muted, 0.4) } else { t.muted });
                        let b = ui.line(&note.text(), st, None);
                        ui.draw(&b, Point::new(right - b.w, mid - b.h / 2.0));
                    }
                    NavNote::Busy => ui.spinner(Point::new(right - 6.0, mid), 5.0, 1.8, t.shine),
                    NavNote::Failed => ui.icon(Rect::new(right - 14.0, row.y, 14.0, row.h), ALERT, size::BODY, t.danger),
                }
                y += NAV_PITCH;
                n += 1;
            }
        }
    }
    key_button(ui, add, Some(PLUS), "Add repo", Some("a"), Kind::Secondary, Some(Action::View(id, ADD)));
}

/// The pages as chips over the page in a narrow pane (every page, wrapping): returns what's left for the page.
fn page_chips(v: &SkillsView, ui: &mut Ui, col: Rect, nav: &[NavItem], id: PaneId) -> Rect {
    let t = ui.t;
    let (mut x, mut y) = (col.x, col.y);
    let pages: Vec<(&Page, &String)> = nav.iter().filter_map(|n| if let NavItem::Page(p, l, _) = n { Some((p, l)) } else { None }).collect();
    for (n, (p, label)) in pages.iter().enumerate() {
        let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
        let w = ui.measure(label, st) + 24.0;
        if x > col.x && x + w > col.right() {
            x = col.x;
            y += CHIP_H + 6.0;
        }
        let r = Rect::new(x, y, w, CHIP_H);
        let on = **p == v.page;
        let hot = ui.hover_amount(("skills-chip", id, n), r);
        let ink = if on {
            ui.round(r, r.h / 2.0, t.accent.fade(1.0 - 0.1 * hot));
            t.on_accent
        } else {
            ui.panel(r, r.h / 2.0, t.control.mix(t.control_hover, hot), t.stroke);
            t.text.mix(t.muted, 0.3 - 0.3 * hot)
        };
        ui.label_in(r, label, st.color(ink), Align::Center);
        ui.hit(r, Action::View(id, NAV + n));
        x += w + 6.0;
    }
    // and adding a repo, after them
    let w = key_button_width(ui, Some(PLUS), "Add repo", None);
    if x > col.x && x + w > col.right() {
        x = col.x;
        y += CHIP_H + 6.0;
    }
    key_button(ui, Rect::new(x, y - 2.0, w, BUTTON_H), Some(PLUS), "Add repo", None, Kind::Secondary, Some(Action::View(id, ADD)));
    let top = y + CHIP_H + 16.0;
    Rect::new(col.x, top, col.w, (col.bottom() - top).max(0.0))
}

/// The agents' marks: their logos (Pi and omp their glyphs), the shared folder a diamond.
pub(super) fn agent_mark(a: Agent) -> Mark {
    match a.harness() {
        Some(h) => brand::harness(h),
        None => Mark::Glyph("◇", None),
    }
}

/// "Add repo": paste a repo and bro says what it makes of it; the repo it features is a click away.
fn dialog(v: &SkillsView, ui: &mut Ui, area: Rect, cx: &mut Cx) {
    let t = ui.t;
    let id = cx.id;
    let prev = ui.layer();
    widgets::scrim(ui, area, 1.0, Action::View(id, D_SCRIM));
    let w = 540.0f32.min(area.w - 32.0);
    let pad = 22.0;
    let iw = w - 2.0 * pad;
    let check = v.add_check(cx);
    let text = v.adding.clone().unwrap_or_default();
    let suggestion = v.suggestion(cx).filter(|f| !matches!(&check, AddCheck::Ready(s) if skill_repos::same(s, f.repo)));
    let blurb = ui.para("Paste a GitHub link, owner/repo or any git URL. bro lists the skills in it: install the ones you want, everywhere or into a project, and bro keeps them up to date.", Style::new(size::BODY, weight::REGULAR, t.muted), iw, Some(3));
    // what bro makes of it, as it's typed
    let (glyph, line, ink) = match &check {
        AddCheck::Empty => (INFO, "A skill is a folder with a SKILL.md: the repo itself, a folder at its root, or one under skills/.".to_string(), t.faint),
        AddCheck::NotARepo => (ALERT, "Not a repo yet: github.com/owner/repo, owner/repo or a git URL".to_string(), t.danger),
        AddCheck::Watched(spec) => (CHECK, format!("Already watching {}", skill_repos::label(spec)), t.inline),
        AddCheck::Ready(spec) => (CHECK, format!("{} · ready to watch", skill_repos::label(spec)), t.good),
    };
    let status = ui.para(&line, Style::new(size::SMALL, weight::MEDIUM, ink), iw - 22.0, Some(2));
    let suggest_h = if suggestion.is_some() { 14.0 + 18.0 + 6.0 + 60.0 } else { 0.0 };
    let h = pad + 26.0 + 6.0 + blurb.h + 16.0 + BUTTON_H + 10.0 + status.h.max(18.0) + suggest_h + 22.0 + BUTTON_H + pad;
    let r = Rect::new(area.x + (area.w - w) / 2.0, area.y + ((area.h - h) / 3.0).max(12.0), w, h);
    widgets::card(ui, r, radius::OVERLAY);
    let x = r.x + pad;
    let mut y = r.y + pad;
    let tb = ui.line("Add a skill repo", Style::new(size::HEADING + 2.0, weight::SEMIBOLD, t.text), None);
    ui.draw(&tb, Point::new(x, y));
    y += 26.0 + 6.0;
    ui.draw(&blurb, Point::new(x, y));
    y += blurb.h + 16.0;
    widgets::field(ui, Rect::new(x, y, iw, BUTTON_H), &text, Some(text.len()), "github.com/owner/repo", cx.focused, Action::View(id, D_FIELD));
    y += BUTTON_H + 10.0;
    ui.icon(Rect::new(x, y, 16.0, 18.0), glyph, size::BODY, ink);
    ui.draw(&status, Point::new(x + 22.0, y + 1.0));
    y += status.h.max(18.0);
    // the repo bro suggests
    if let Some(feat) = suggestion {
        y += 14.0;
        widgets::section(ui, Rect::new(x, y, iw, 18.0), "Suggested");
        y += 18.0 + 6.0;
        let s = Rect::new(x, y, iw, 60.0);
        let hot = ui.hover_amount(("skills-suggest", id), s);
        ui.panel(s, radius::CONTROL, t.control.mix(t.control_hover, hot), t.stroke);
        ui.icon(Rect::new(s.x + 12.0, s.y + 10.0, 18.0, 20.0), STAR, size::TITLE, t.shine);
        let name = ui.line(feat.title, Style::new(size::BODY, weight::SEMIBOLD, t.text), None);
        ui.draw(&name, Point::new(s.x + 40.0, s.y + 11.0));
        let by = ui.line(&format!("by {} · {}", feat.by, feat.repo), Style::new(size::SMALL, weight::REGULAR, t.faint), Some((s.w - 52.0 - name.w).max(20.0)));
        ui.draw(&by, Point::new(s.x + 40.0 + name.w + 8.0, s.y + 11.0 + (name.h - by.h) / 2.0 + 1.0));
        let more = feat.skills.len().saturating_sub(4);
        let list = format!("{}{}", feat.skills[..feat.skills.len().min(4)].join(", "), if more > 0 { format!(" and {more} more") } else { String::new() });
        ui.label_in(Rect::new(s.x + 40.0, s.y + 32.0, s.w - 52.0, 18.0), &list, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        ui.hit(s, Action::View(id, D_SUGGEST));
        y += 60.0;
    }
    // Cancel, and the add (or show) when there's something to watch
    let by = r.bottom() - pad - BUTTON_H;
    let (label, act) = match &check {
        AddCheck::Watched(_) => ("Show it", true),
        AddCheck::Ready(_) => ("Add repo", true),
        _ => ("Add repo", false),
    };
    let aw = key_button_width(ui, Some(PLUS), label, Some("enter"));
    let add = Rect::new(r.right() - pad - aw, by, aw, BUTTON_H);
    key_button(ui, add, Some(PLUS), label, Some("enter"), Kind::Primary, act.then_some(Action::View(id, D_ADD)));
    let cw = key_button_width(ui, None, "Cancel", Some("esc"));
    key_button(ui, Rect::new(add.x - 8.0 - cw, by, cw, BUTTON_H), None, "Cancel", Some("esc"), Kind::Secondary, Some(Action::View(id, D_CANCEL)));
    let _ = y;
    ui.set_layer(prev);
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut SkillsView, n: usize, p: Point, mods: KeyModifiers, count: u32, cx: &mut Cx) {
    // an open menu first: an item runs, anywhere else closes it
    match n {
        MENU_OUT => return v.menu = None,
        n if n >= MENU => return v.run_menu(n - MENU, cx),
        _ => v.menu = None,
    }
    // the dialog: a click outside it closes it
    if v.adding.is_some() {
        match n {
            D_ADD => v.add_repo(cx),
            D_CANCEL | D_SCRIM => v.adding = None,
            D_SUGGEST => {
                if let Some(f) = v.suggestion(cx) {
                    v.adding = Some(f.repo.to_string());
                }
            }
            _ => {}
        }
        return;
    }
    match n {
        BG | NAV_BG => {
            v.installed.typing = false;
            v.market.typing = false;
        }
        ADD => v.adding = Some(String::new()),
        n if (NAV..NAV + 900).contains(&n) => {
            let pages: Vec<Page> = v.nav(cx).into_iter().filter_map(|i| if let NavItem::Page(p, ..) = i { Some(p) } else { None }).collect();
            if let Some(p) = pages.get(n - NAV) {
                v.go(p.clone());
            }
        }
        _ if v.page == Page::Discover => discover::click(v, n, p, mods, count, cx),
        _ => installed::click(v, n, p, count, cx),
    }
}

/// Target `n` was right-clicked at `p`: the menu of the line under it, there (anywhere else, nothing).
pub(crate) fn context(v: &mut SkillsView, n: usize, p: Point, cx: &mut Cx) {
    v.menu = None;
    if v.adding.is_some() || n >= MENU_OUT {
        return;
    }
    let row = if v.page == Page::Discover { discover::row_of(n) } else { installed::row_of(n) };
    if let Some(k) = row {
        if v.page == Page::Discover { v.market.sel = k } else { v.installed.sel = k }
        v.open_menu(Some((p.x, p.y)), cx);
    }
}

/// The wheel scrolls the page's list (the selection stays where it is); over an open menu it moves in it.
pub(crate) fn wheel(v: &mut SkillsView, _n: Option<usize>, down: bool, _cx: &mut Cx) {
    if v.menu.is_some() {
        return;
    }
    let step = 116usize;
    let b = if v.page == Page::Discover { &mut v.market } else { &mut v.installed };
    b.scroll = if down { b.scroll + step } else { b.scroll.saturating_sub(step) };
}
