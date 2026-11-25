//! The sidebar beside the session tabs. The tabs hold the sessions, so the sidebar keeps to what needs you, a card
//! for each question or permission (newest first), then what the tabs list: every session and the orchestrators
//! (while there are some), and the projects, each with its icon. Each says what its sessions are doing as the tabs
//! used to: how many run (a spinner while one works), how many wait on you and how many finished unseen. Projects
//! with nothing running fold into Recent, the last used first (a click on its heading opens it). The rows,
//! their order and the keys are [`crate::sidebar`]'s compact rows (every session and the orchestrators are clicks
//! and alt+0 / alt+O); a click on an ask goes to its session, on the rest lists its sessions in the tabs.

use super::{PAD, SideState, bar_geometry};
use crate::app::gui::layout::{flow_begin, key_of};
use crate::app::App;
use crate::app::SideHit;
use crate::app::gui::actions::Action;
use crate::app::gui::chrome::session_mark;
use crate::app::gui::icons::{Icons, Look, Tone};
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::app::gui::strip::{Tally, draw_tally};
use crate::app::project_tabs::StripTab;
use crate::pane::Activity;
use crate::sidebar::{LiveInfo, OpenInfo, Recent, Row};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use std::collections::HashMap;

/// A section's heading.
const HEAD: f32 = 28.0;
/// A project's row, and the room between two.
const PROJECT: f32 = 34.0;
const PROJECT_GAP: f32 = 2.0;
/// A project's icon tile.
const TILE: f32 = 24.0;
/// Inside the card: around an ask, and the tile of its kind.
const ASK_PAD: f32 = 10.0;
const KIND: f32 = 28.0;
/// The ask's text starts here, from the card's left.
const ASK_TEXT: f32 = ASK_PAD + KIND + 10.0;
/// Between what needs you and the projects.
const SECTION_GAP: f32 = 16.0;
/// Recent's heading, and how many of its projects' icons it shows while folded.
const RECENT: f32 = 30.0;
const PEEK: usize = 5;

enum Item {
    Head { label: &'static str, count: usize, plus: bool },
    /// a row of what needs you, and where it sits in the card
    Ask { i: usize, first: bool, last: bool },
    /// every session, or the orchestrators
    Scope(StripTab),
    Project { i: usize },
    /// the projects with no session running, folded away or open
    Recent { open: bool },
    Note(String),
}

/// What a session asks, in words, and a glyph for its kind: a command to run, a file to change, a question, a plan.
pub(crate) fn ask_parts(info: &LiveInfo) -> (&'static str, String) {
    let text = info.note.as_deref().filter(|n| n.starts_with('⏸')).map(|n| n.trim_start_matches('⏸').trim().to_string()).unwrap_or_default();
    if text.is_empty() {
        return ("\u{F182D}", "Waiting on your answer".into());
    }
    let after = |prefix: &str| text.strip_prefix(prefix).map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if let Some(r) = after("Bash:").or_else(|| after("Run:")) {
        return ("\u{F018D}", r);
    }
    for (prefix, verb) in [("Edit:", "Edit"), ("MultiEdit:", "Edit"), ("NotebookEdit:", "Edit"), ("Write:", "Write")] {
        if let Some(r) = after(prefix) {
            return ("\u{F0CB6}", format!("{verb} {r}"));
        }
    }
    if let Some(r) = after("Read:") {
        return ("\u{F0DCB}", format!("Read {r}"));
    }
    if let Some(r) = after("Question:") {
        return ("\u{F1739}", r);
    }
    if let Some(r) = after("MCP:") {
        return ("\u{F0A66}", format!("Use {r}"));
    }
    if text.starts_with("Approve the plan") {
        return ("\u{F08A8}", "Approve the plan".into());
    }
    if text.starts_with("WebFetch") || text.starts_with("WebSearch") {
        return ("\u{F01E7}", text);
    }
    ("\u{F173B}", text)
}

/// When it started waiting, as the header's clock reads ("14:05"); from an earlier day, how long ago ("2d").
fn since(at: u64) -> String {
    let (now, at) = (crate::util::now_secs(), at as i64);
    let off = crate::util::local_offset_secs();
    let day = |s: i64| (s + off).div_euclid(86_400);
    if day(at) != day(now) {
        return format!("{} ago", crate::util::short_dur((now - at).max(0) as u64));
    }
    let s = (at + off).rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// The session's own name, when it has one worth reading (an agent's title or yours, not its login and model).
fn session_name(info: &LiveInfo) -> Option<String> {
    if info.orchestrator {
        return Some("Orchestrator".into());
    }
    info.name.clone().filter(|n| !n.trim().is_empty())
}

/// The ask's title, laid out in `w`: two lines at most.
fn ask_title(ui: &mut Ui, text: &str, w: f32) -> crate::app::gui::kit::TextBox {
    let t = ui.t;
    ui.rich(&[(text.to_string(), Style::new(size::BODY, weight::MEDIUM, t.text))], size::BODY, Some(w.max(10.0)), true, Some(2))
}

/// An ask's height in a card `w` wide.
fn ask_height(ui: &mut Ui, info: &LiveInfo, w: f32) -> f32 {
    let (_, text) = ask_parts(info);
    let title = ask_title(ui, &text, w - ASK_TEXT - ASK_PAD);
    ASK_PAD + (title.h + 2.0 + 17.0).max(KIND) + ASK_PAD
}

/// The first letter or digit of a project's name, for its tile.
fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "·".into())
}

/// A project's colour as the tile under its drawn icon's white symbol: the same hue, saturated and deep enough for
/// the white to read on it (a pale yellow turns ochre, a pastel blue a clear blue; a grey stays grey). The phone
/// does the same with the project's hue (bro-cli-mobile's `symbolTile`).
pub(crate) fn symbol_tile(c: Color) -> Color {
    let (h, s, l) = c.to_hsl();
    let s = if s < 0.08 { s } else { s.clamp(0.6, 0.9) };
    let mut l = l.min(0.6);
    let mut tile = Color::from_hsl(h, s, l, 1.0);
    while tile.luminance() > 0.27 && l > 0.2 {
        l -= 0.01;
        tile = Color::from_hsl(h, s, l, 1.0);
    }
    tile
}

/// A project's icon in `r`: its picture on a quiet tile, its drawn symbol or its letter on a tile of its colour.
pub(crate) fn project_icon(ui: &mut Ui, r: Rect, name: &str, color: Color, look: Look) {
    let t = ui.t;
    let corner = (r.w * 0.27).round();
    match look {
        // an app icon fills its tile, with a faint edge so a dark one keeps its shape on a dark sidebar (and a white
        // one on a light sidebar)
        Look::Picture(id, _, Tone::Opaque) => {
            ui.image(id, r, [0.0, 0.0, 1.0, 1.0], corner, 1.0);
            ui.outline(r, corner, 0.5, t.text.with_alpha(0.12));
        }
        Look::Picture(id, _, Tone::Symbol) => {
            ui.round(r, corner, symbol_tile(color));
            ui.image(id, r, [0.0, 0.0, 1.0, 1.0], corner, 1.0);
        }
        Look::Picture(id, aspect, tone) => {
            // behind it, a tile it reads on: light under dark marks on a dark theme, dark under white marks on a
            // light one, else a quiet one
            match (tone, t.light) {
                (Tone::Dark, false) => ui.round(r, corner, Color::WHITE.with_alpha(0.9)),
                (Tone::Light, true) => ui.round(r, corner, Color::hex(0x24272e)),
                (_, true) => ui.panel(r, corner, Color::WHITE, t.stroke),
                (_, false) => ui.round(r, corner, t.text.with_alpha(0.09)),
            }
            let room = r.w - (r.w * 0.2).round().max(3.0);
            let (w, h) = if aspect >= 1.0 { (room, room / aspect) } else { (room * aspect, room) };
            let ir = Rect::new(r.x + (r.w - w) / 2.0, r.y + (r.h - h) / 2.0, w, h);
            ui.image(id, ir, [0.0, 0.0, 1.0, 1.0], (corner * 0.5).max(1.5), 1.0);
        }
        Look::Letter | Look::Loading => {
            let quiet = if look == Look::Loading { 0.6 } else { 1.0 };
            ui.round(r, corner, color.with_alpha(if t.light { 0.16 } else { 0.2 } * quiet));
            let ink = if t.light { color.mix(t.text, 0.35) } else { color.mix(t.text, 0.12) };
            ui.label_in(r, &initial(name), Style::new((r.h * 0.5).round(), weight::BOLD, ink.fade(quiet)), Align::Center);
        }
    }
}

/// Count `l` into what its sessions are doing: waiting on you, else working (its subagents and background tasks
/// too), else finished unseen.
fn count_in(tally: &mut Tally, l: &crate::sidebar::LiveInfo) {
    tally.count += 1;
    if crate::sidebar::needs_you(l) {
        tally.asks += 1;
    } else if l.activity == Some(Activity::Working) || l.background.0 + l.background.1 > 0 {
        tally.working = true;
    } else if l.done {
        tally.done += 1;
    }
}

pub(super) fn draw_list(app: &mut App, ui: &mut Ui, list: Rect, s: &mut SideState, focused: bool, icons: &mut Icons) {
    let t = ui.t;
    let model = app.sidebar_model_at(Some(ui.now));
    let rows = &model.rows;
    app.side_sel = app.side_sel.min(rows.len().saturating_sub(1));
    s.rows.clear();
    s.list = list;
    if s.filter != app.side.filter {
        s.filter = app.side.filter.clone();
        s.scroll = 0.0;
        s.manual = false;
    }
    // what each project's sessions are doing, every session's and the orchestrators'
    let mut busy: HashMap<String, Tally> = HashMap::new();
    let (mut every, mut orchestrators) = (Tally::default(), Tally::default());
    let lives = &model.live;
    // the projects with nothing running: folded into Recent while some run
    let (idle, recent) = &model.recent;
    let fold = match recent {
        Recent::Folded { open } if !idle.is_empty() => Some(*open),
        _ => None,
    };
    for l in lives {
        count_in(busy.entry(l.project_key.clone()).or_default(), l);
        count_in(&mut every, l);
        if l.orchestrator {
            count_in(&mut orchestrators, l);
        }
    }
    let tally_of = |scope: &StripTab| match scope {
        StripTab::All => every,
        StripTab::Orchestrator => orchestrators,
        StripTab::Project(key) => busy.get(key).copied().unwrap_or_default(),
    };
    // the layout: (item, top, height) from the list's top
    let card_w = list.w - 2.0 * PAD;
    let asks: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Live { .. })).map(|(i, _)| i).collect();
    let projects: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Project { .. })).map(|(i, _)| i).collect();
    // under Projects what runs, then Recent; with nothing folded, every project
    let (running, resting): (Vec<usize>, Vec<usize>) = match fold {
        Some(_) => projects.iter().partition(|&&i| matches!(&rows[i], Row::Project { live, .. } if *live > 0)),
        None => (projects.clone(), vec![]),
    };
    let mut items: Vec<(Item, f32, f32)> = vec![];
    let mut y = 2.0;
    if !asks.is_empty() {
        items.push((Item::Head { label: "Needs you", count: asks.len(), plus: false }, y, HEAD));
        y += HEAD;
        for (k, &i) in asks.iter().enumerate() {
            let Row::Live { info, .. } = &rows[i] else { continue };
            let h = ask_height(ui, info, card_w);
            items.push((Item::Ask { i, first: k == 0, last: k + 1 == asks.len() }, y, h));
            y += h;
        }
        y += SECTION_GAP;
    }
    // every session (once there are two to list) and the orchestrators (while one runs), over the projects; the
    // filter leaves them out
    if app.side.filter.trim().is_empty() {
        let mut scopes = vec![];
        if every.count >= 2 {
            scopes.push(StripTab::All);
        }
        if orchestrators.count > 0 {
            scopes.push(StripTab::Orchestrator);
        }
        if !scopes.is_empty() {
            for scope in scopes {
                items.push((Item::Scope(scope), y, PROJECT));
                y += PROJECT + PROJECT_GAP;
            }
            y += SECTION_GAP - PROJECT_GAP;
        }
    }
    items.push((Item::Head { label: "Projects", count: running.len(), plus: true }, y, HEAD));
    y += HEAD;
    if projects.is_empty() {
        let note = if app.side.filter.is_empty() { "Open a folder or start a session and your projects line up here, each wearing its own icon." } else { "No project or question matches that" };
        items.push((Item::Note(note.into()), y, 52.0));
        y += 52.0;
    }
    for &i in &running {
        items.push((Item::Project { i }, y, PROJECT));
        y += PROJECT + PROJECT_GAP;
    }
    if let Some(open) = fold {
        y += 6.0;
        items.push((Item::Recent { open }, y, RECENT));
        y += RECENT + PROJECT_GAP;
        for &i in &resting {
            items.push((Item::Project { i }, y, PROJECT));
            y += PROJECT + PROJECT_GAP;
        }
    }
    s.content = y + 8.0;
    let view = list;
    s.view = view.h;
    // keep the keyboard's selection in view
    if s.last_sel != Some(app.side_sel) || s.last_focus != app.side_focus {
        s.manual = false;
    }
    let keep = (app.side_focus && !s.manual).then_some(app.side_sel);
    let at = |i: usize| items.iter().find(|(it, ..)| matches!(it, Item::Ask { i: k, .. } | Item::Project { i: k } if *k == i)).map(|(_, top, h)| (*top, *h));
    if let Some((top, h)) = keep.and_then(at) {
        // a first ask, project or recent one brings its heading along
        let top = if keep == asks.first().copied() || keep == running.first().copied() {
            top - HEAD
        } else if keep == resting.first().copied() {
            top - RECENT - PROJECT_GAP
        } else {
            top
        };
        if top < s.scroll {
            s.scroll = top.max(0.0);
        } else if top + h > s.scroll + view.h {
            s.scroll = top + h - view.h;
        }
    }
    s.last_sel = Some(app.side_sel);
    s.last_focus = app.side_focus;
    s.scroll = s.scroll.clamp(0.0, (s.content - view.h).max(0.0));

    ui.clip(view, 0.0);
    ui.hit_with(view, Action::SideList, Cursor::Default);
    let x = list.x + PAD;
    // every item glides to its place as asks and projects come, go and reorder; a new one opens from nothing in its
    // slot while the rest slide over (a filter, a width change or a drag snaps)
    let snap = flow_begin("side-compact", ui.now, key_of((list.w.round() as i32, &s.filter, crate::app::gui::layout::resizing())));
    let placed: Vec<(f32, f32)> = items
        .iter()
        .map(|(item, top, _)| {
            let key = match item {
                Item::Head { label, .. } => key_of(("head", label)),
                Item::Note(_) => key_of("note"),
                Item::Recent { .. } => key_of("recent"),
                Item::Scope(scope) => key_of(("scope", format!("{scope:?}"))),
                Item::Ask { i, .. } => match &rows[*i] {
                    Row::Live { info, .. } => key_of(("ask", info.pane)),
                    Row::Project { key, .. } => key_of(("project", key)),
                },
                Item::Project { i } => match &rows[*i] {
                    Row::Project { key, .. } => key_of(("project", key)),
                    Row::Live { info, .. } => key_of(("ask", info.pane)),
                },
            };
            ui.flow_row("side-compact", key, *top, snap)
        })
        .collect();
    // the card behind what needs you
    let at = |pred: fn(&Item) -> bool| items.iter().position(|(it, ..)| pred(it));
    let card_top = at(|it| matches!(it, Item::Ask { first: true, .. })).map(|n| placed[n].0);
    let card_bottom = at(|it| matches!(it, Item::Ask { last: true, .. })).map(|n| placed[n].0 + items[n].2 * placed[n].1);
    if let (Some(top), Some(bottom)) = (card_top, card_bottom) {
        let card = Rect::new(x, view.y + top - s.scroll, card_w, bottom - top);
        if t.light {
            ui.shadow(card, radius::OVERLAY, 10.0, 2.0, t.shadow.fade(0.22));
        }
        ui.panel(card, radius::OVERLAY, if t.light { Color::WHITE.with_alpha(0.86) } else { t.text.with_alpha(0.045) }, t.stroke);
    }
    // the one the session tabs list
    let showing = match &app.scope {
        StripTab::Project(k) => Some(k.clone()),
        _ => None,
    };
    let current = app.current_project().map(|r| app.svc.project_for(&r).key);
    for (n, (item, _, h)) in items.iter().enumerate() {
        let (sy, reveal) = placed[n];
        let ry = view.y + sy - s.scroll;
        if ry + h < view.y || ry > view.bottom() {
            continue;
        }
        let r = Rect::new(x, ry, card_w, *h);
        let opened = Rect { h: h * reveal, ..r }.intersect(&view);
        if opened.is_empty() {
            continue;
        }
        let growing = reveal < 0.999;
        if growing {
            ui.clip(opened, 0.0);
        }
        match item {
            Item::Head { label, count, plus } => head(ui, r, label, *count, *plus),
            Item::Note(text) => {
                let p = ui.para(text, Style::new(size::SMALL, weight::REGULAR, t.faint), r.w - 16.0, Some(3));
                ui.draw(&p, Point::new(r.x + 8.0, r.y + 4.0));
            }
            Item::Scope(scope) => draw_scope(app, ui, r, scope, tally_of(scope)),
            Item::Recent { open } => draw_recent(app, ui, r, idle, *open, icons),
            Item::Ask { i, first, last } => {
                let Row::Live { info, .. } = &rows[*i] else { if growing { ui.unclip(); } continue };
                let selected = focused && *i == app.side_sel;
                let global = info.orchestrator && info.project_key == bro_core::orchestrator::GLOBAL_KEY;
                let look = (!global).then(|| icons.look(&info.project_key, &info.project_root));
                draw_ask(app, ui, r, *i, info, selected, (*first, *last), look);
                s.rows.push((*i, r));
            }
            Item::Project { i } => {
                let Row::Project { key, name, root, live, last_age, .. } = &rows[*i] else { if growing { ui.unclip(); } continue };
                let selected = focused && *i == app.side_sel;
                let look = icons.look(key, root);
                let state = Tally { count: *live, ..busy.get(key).copied().unwrap_or_default() };
                let p = ProjectRow { i: *i, key, name, root, live: *live, last_age: last_age.map(|age| age + model.history_at.elapsed().as_secs()), selected, showing: showing.as_deref() == Some(key.as_str()), current: current.as_deref() == Some(key.as_str()), busy: state, look, changing: icons.busy(key) };
                draw_project(app, ui, r, &p);
                s.rows.push((*i, r));
            }
        }
        if growing {
            ui.unclip();
        }
    }
    ui.unclip();
    let moving = app.side_nav_at.is_some_and(|at| at.elapsed().as_millis() < 1050);
    let vis = ui.ease("side-bar", if moving || ui.hovered(view) || s.bar_drag.is_some() { 1.0 } else { 0.0 }, 200.0);
    widgets::scrollbar(ui, view, s.content, view.h, s.scroll, vis);
    s.bar = bar_geometry(view, s.content, s.scroll);
    if s.bar.is_some() {
        ui.hit_with(Rect::new(view.right() - 12.0, view.y, 9.0, view.h), Action::SideScroll, Cursor::Default);
    }
}

/// A section's heading: its name, how many, and for the projects a + that opens one.
fn head(ui: &mut Ui, r: Rect, label: &str, count: usize, plus: bool) {
    let t = ui.t;
    let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.35));
    let b = ui.line(label, st, None);
    let base = r.y + (r.h - b.h) / 2.0 + 1.0;
    ui.draw(&b, Point::new(r.x + 6.0, base));
    if count > 0 {
        ui.label(Point::new(r.x + 6.0 + b.w + 6.0, base), &count.to_string(), Style::new(size::SMALL, weight::MEDIUM, t.faint));
    }
    if plus {
        let pr = Rect::new(r.right() - 26.0, r.y + 3.0, 24.0, 24.0);
        if widgets::icon_button(ui, pr, "\u{F0415}", t.muted, Action::OpenProject) {
            widgets::tooltip(ui, pr, "Open a project folder · it gets its icon from the favicon inside");
        }
    }
}

/// Recent's heading, a fold: how many projects rest in it and (folded) the icons of the last used, then a chevron
/// that turns down as it opens.
fn draw_recent(app: &App, ui: &mut Ui, r: Rect, idle: &[OpenInfo], open: bool, icons: &mut Icons) {
    let t = ui.t;
    let hot = ui.hover_amount("side-recent", r);
    let k = ui.ease("side-recent-open", if open { 1.0 } else { 0.0 }, 160.0);
    row_fill(ui, r, false, false, hot, false);
    ui.hit(r, Action::Side(SideHit::Recent));
    let ink = t.text.mix(t.muted, 0.35).mix(t.text, hot * 0.6);
    let b = ui.line("Recent", Style::new(size::SMALL, weight::SEMIBOLD, ink), None);
    let base = r.y + (r.h - b.h) / 2.0 + 1.0;
    ui.draw(&b, Point::new(r.x + 6.0, base));
    let count = idle.len().to_string();
    let cst = Style::new(size::SMALL, weight::MEDIUM, t.faint);
    let cx = r.x + 6.0 + b.w + 6.0;
    ui.label(Point::new(cx, base), &count, cst);
    let chev = Rect::new(r.right() - 24.0, r.y + (r.h - 16.0) / 2.0, 16.0, 16.0);
    let cink = t.faint.mix(t.text, hot * 0.5);
    ui.icon(chev, "\u{F0142}", 14.0, cink.fade(1.0 - k));
    if k > 0.0 {
        ui.icon(chev, "\u{F0140}", 14.0, cink.fade(k));
    }
    // folded, a peek at what's inside: the icons of the last used, as many as fit, wiped away as it opens
    let peek = 1.0 - k;
    if peek > 0.01 {
        const ICON: f32 = 16.0;
        const STEP: f32 = 20.0;
        let x0 = cx + ui.measure(&count, cst) + 10.0;
        let fits = ((chev.x - 6.0 - x0 + (STEP - ICON)) / STEP).floor().max(0.0) as usize;
        let shown = fits.min(PEEK).min(idle.len());
        ui.clip(Rect::new(x0, r.y, shown as f32 * STEP * peek, r.h), 0.0);
        for (n, o) in idle.iter().take(shown).enumerate() {
            let look = icons.look(&o.key, &o.root);
            let ir = Rect::new(x0 + n as f32 * STEP, r.y + (r.h - ICON) / 2.0, ICON, ICON);
            project_icon(ui, ir, &o.name, t.project(&o.key, &app.theme), look);
        }
        ui.unclip();
    }
}

/// One thing that needs you, in the card: the kind's tile, what it asks (two lines at most), then its project (its
/// icon, `look`; the orchestrator's mark for the one over every project), the session and since when.
#[allow(clippy::too_many_arguments)]
fn draw_ask(app: &App, ui: &mut Ui, r: Rect, i: usize, info: &LiveInfo, selected: bool, (first, last): (bool, bool), look: Option<Look>) {
    let t = ui.t;
    let hot = ui.hover_amount(("side-ask", info.pane), r);
    // hover and the keys' pick: a pill inside the card, clear of its edge
    let pill = Rect::new(r.x + 3.0, r.y + if first { 3.0 } else { 1.5 }, r.w - 6.0, r.h - if first { 3.0 } else { 1.5 } - if last { 3.0 } else { 1.5 });
    if selected {
        ui.round(pill, radius::OVERLAY - 2.0, t.selected);
        ui.outline(pill, radius::OVERLAY - 2.0, 1.0, t.accent.with_alpha(0.5));
    } else if hot > 0.0 {
        ui.round(pill, radius::OVERLAY - 2.0, t.hover.fade(hot));
    }
    if !first {
        ui.hline(r.x + ASK_PAD, r.y, r.w - 2.0 * ASK_PAD, t.hairline);
    }
    ui.hit(r, Action::Side(SideHit::Row(i)));
    let (glyph, text) = ask_parts(info);
    // its kind, on a warm tile
    let tile = Rect::new(r.x + ASK_PAD, r.y + ASK_PAD, KIND, KIND);
    ui.round(tile, 7.0, t.warn.with_alpha(if t.light { 0.12 } else { 0.16 }));
    ui.icon(tile, glyph, 15.0, t.warn);
    // what it asks
    let tx = r.x + ASK_TEXT;
    let title = ask_title(ui, &text, r.right() - ASK_PAD - tx);
    let ty = r.y + ASK_PAD - 1.0;
    ui.draw(&title, Point::new(tx, ty));
    // its project's icon, then where and since when
    let sy = ty + title.h + 2.0;
    match look {
        Some(look) => project_icon(ui, Rect::new(tx, sy + 2.0, 13.0, 13.0), &info.project_name, t.project(&info.project_key, &app.theme), look),
        None => ui.mark(session_mark(app, info.pane, t), Rect::new(tx - 1.0, sy, 15.0, 17.0), 12.0, None),
    }
    let mut parts = vec![];
    if look.is_some() {
        parts.push(info.project_name.clone());
    }
    parts.extend(session_name(info));
    if let Some(at) = info.waiting_since {
        parts.push(since(at));
    }
    let sub = parts.join(" · ");
    ui.label_in(Rect::new(tx + 17.0, sy, r.right() - ASK_PAD - tx - 17.0, 17.0), &sub, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
}

/// A project's row as drawn.
struct ProjectRow<'a> {
    i: usize,
    key: &'a str,
    name: &'a str,
    root: &'a std::path::Path,
    live: usize,
    last_age: Option<u64>,
    /// the keys' pick
    selected: bool,
    /// its tab is the one showing
    showing: bool,
    /// new sessions start here
    current: bool,
    busy: Tally,
    look: Look,
    /// its icon is being changed
    changing: bool,
}

/// The row's fill: the keys' pick, the one the session tabs list, under the pointer, or (quietly) where new
/// sessions start.
fn row_fill(ui: &mut Ui, r: Rect, selected: bool, showing: bool, hot: f32, current: bool) {
    let t = ui.t;
    let corner = 6.0;
    if selected {
        ui.round(r, corner, t.selected);
        ui.outline(r, corner, 1.0, t.accent.with_alpha(0.5));
    } else if showing {
        ui.round(r, corner, t.text.with_alpha(if t.light { 0.065 } else { 0.075 }).mix(t.hover, hot * 0.5));
    } else if hot > 0.0 {
        ui.round(r, corner, t.hover.fade(hot));
    } else if current {
        ui.round(r, corner, t.text.with_alpha(0.025));
    }
}

/// Every session, or the orchestrators: its mark on a tile, its name, and what its sessions are doing.
fn draw_scope(app: &App, ui: &mut Ui, r: Rect, scope: &StripTab, tally: Tally) {
    let t = ui.t;
    let id = format!("side-{scope:?}");
    let hot = ui.hover_amount(("side-scope", &id), r);
    row_fill(ui, r, false, app.scope == *scope, hot, false);
    ui.hit(r, Action::Side(SideHit::Scope(scope.clone())));
    let (glyph, ink, name) = match scope {
        StripTab::Orchestrator => ("\u{F04CE}", t.gold, "Orchestrators"),
        _ => ("\u{F05AF}", t.text.mix(t.muted, 0.2), "All sessions"),
    };
    let tile = Rect::new(r.x + 6.0, r.y + (r.h - TILE) / 2.0, TILE, TILE);
    ui.round(tile, (TILE * 0.27).round(), ink.with_alpha(if t.light { 0.12 } else { 0.14 }));
    ui.icon(tile, glyph, 15.0, ink);
    let right = r.right() - 8.0 - tally.width();
    draw_tally(ui, right, r.y + r.h / 2.0, &id, tally);
    let nx = tile.right() + 10.0;
    let strong = app.scope == *scope;
    let st = Style::new(size::BODY, if strong { weight::SEMIBOLD } else { weight::MEDIUM }, t.text);
    ui.label_in(Rect::new(nx, r.y, (right - nx - 4.0).max(10.0), r.h), name, st, Align::Left);
}

/// A project: its icon, its name, what its sessions are doing (how many run, a spinner while one works, how many
/// wait on you and how many finished unseen); × to close it under the pointer.
fn draw_project(app: &App, ui: &mut Ui, r: Rect, p: &ProjectRow) {
    let t = ui.t;
    let hot = ui.hover_amount(("side-project", p.key), r);
    row_fill(ui, r, p.selected, p.showing, hot, p.current && !app.side_focus);
    ui.hit(r, Action::Side(SideHit::Row(p.i)));
    let color = t.project(p.key, &app.theme);
    let tile = Rect::new(r.x + 6.0, r.y + (r.h - TILE) / 2.0, TILE, TILE);
    project_icon(ui, tile, p.name, color, p.look);
    if p.changing {
        ui.round(tile, (TILE * 0.27).round(), t.bg.with_alpha(0.55));
        ui.spinner(tile.center(), 5.0, 1.6, t.accent);
    }
    // right: × under the pointer, else what its sessions are doing (or when it was last used)
    let mut right = r.right() - 8.0;
    if hot > 0.3 || p.selected {
        let close = Rect::new(right - 20.0, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0);
        if widgets::icon_button(ui, close, "\u{F0156}", if p.selected { t.danger } else { t.muted }, Action::Side(SideHit::CloseProject(p.root.to_path_buf(), p.live))) {
            let what = match p.live {
                0 => format!("Close {} · the folder stays as it is, and New session opens it again", p.name),
                n => format!("Close {} and stop its {n} session{} · their conversations stay resumable", p.name, if n == 1 { "" } else { "s" }),
            };
            widgets::tooltip(ui, close, &what);
        }
        right = close.x - 4.0;
    } else if p.live > 0 {
        right -= p.busy.width();
        draw_tally(ui, right, r.y + r.h / 2.0, &format!("side-project-{}", p.key), p.busy);
        right -= 4.0;
    } else if let Some(age) = p.last_age {
        let a = super::age_label(ui, age);
        let st = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
        let w = ui.measure(&a, st);
        ui.label(Point::new(right - w, r.y + (r.h - 15.0) / 2.0), &a, st);
        right -= w + 6.0;
    }
    let nx = tile.right() + 10.0;
    let strong = p.showing || p.selected;
    let st = Style::new(size::BODY, if strong { weight::SEMIBOLD } else { weight::MEDIUM }, if strong || p.live > 0 { t.text } else { t.text.mix(t.muted, 0.35) });
    ui.label_in(Rect::new(nx, r.y, (right - nx - 4.0).max(10.0), r.h), p.name, st, Align::Left);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawn_symbol_reads_on_its_projects_colour() {
        // GitHub Dark's project colours, a pastel from the hue circle, a light theme's and a grey
        for c in [0x58a6ff, 0xbc8cff, 0x39c5cf, 0xd29922, 0x3fb950, 0x56d4dd, 0xa3e8a3, 0x2f6b9e, 0x9aa0a6] {
            let (color, tile) = (Color::hex(c), symbol_tile(Color::hex(c)));
            assert!(tile.contrast(Color::WHITE) >= 3.0, "white on {c:06x}'s tile");
            let hue = |c: Color| c.to_hsl().0 * 360.0;
            if color.to_hsl().1 > 0.08 {
                let off = (hue(tile) - hue(color)).abs();
                assert!(off.min(360.0 - off) < 2.0, "{c:06x} keeps its hue");
            } else {
                assert!(tile.to_hsl().1 < 0.08, "a grey stays grey");
            }
        }
    }

    fn asking(note: Option<&str>) -> LiveInfo {
        LiveInfo {
            pane: 1,
            harness: Some(bro_core::Harness::Claude),
            name: None,
            profile: None,
            model: None,
            project_key: "a".into(),
            project_name: "a".into(),
            project_root: "a".into(),
            activity: Some(Activity::Blocked),
            done: false,
            background: (0, 0),
            note: note.map(str::to_string),
            waiting_since: None,
            age_secs: 0,
            seq: 1,
            orchestrator: false,
            team: None,
        }
    }

    #[test]
    fn asks_read_as_what_they_want_done() {
        let parts = |n: &str| ask_parts(&asking(Some(n)));
        assert_eq!(parts("⏸ Bash: Push to atlas, branch v1"), ("\u{F018D}", "Push to atlas, branch v1".into()));
        assert_eq!(parts("⏸ Run: cargo publish"), ("\u{F018D}", "cargo publish".into()));
        assert_eq!(parts("⏸ Edit: main.rs").1, "Edit main.rs");
        assert_eq!(parts("⏸ Write: notes.md").1, "Write notes.md");
        assert_eq!(parts("⏸ Question: Which database?").1, "Which database?");
        assert_eq!(parts("⏸ Approve the plan?").1, "Approve the plan");
        assert_eq!(parts("⏸ MCP: send_email").1, "Use send_email");
        assert_eq!(parts("⏸ Should I also update the docs?").1, "Should I also update the docs?");
        assert_eq!(ask_parts(&asking(None)).1, "Waiting on your answer");
        assert_eq!(parts("↳ a recap is no question").1, "Waiting on your answer");
        assert_eq!(initial("bro-cli-v2"), "B");
        assert_eq!(initial(".dotfiles"), "D");
        assert_eq!(initial("ünïcode"), "Ü");
    }
}
