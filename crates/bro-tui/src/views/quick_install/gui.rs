//! The quick install view in the desktop app: the tools a coding machine wants, grouped, each with its tick,
//! its mark, what's installed and a line on what it is, then the install bar pinned under them with the notes
//! and how the selected tool installs. It draws [`QuickInstallView`]'s own state; a click on a row selects and
//! activates it (ticks it, or installs from the install button), as in the terminal UI.

use super::{QuickInstallView, Row, rows};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::{self, Mark};
use crate::app::gui::kit::widgets::Kind;
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::pane::{Cx, Pane};
use crate::services::justtools::JobKind;
use crate::views::profiles::gui::{BUTTON_H, TITLE, edge_fade, focus_ring, hints, key_button, key_button_width, page, page_card};
use bro_core::quick_install::{Os, Tool};
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

// targets
const BG: usize = 0;
const TICK_ALL: usize = 1;
const TICK_NONE: usize = 2;
const LOOK: usize = 3;
/// `ROW + i`: selectable row `i` (the tools, then the install button)
const ROW: usize = 100;

const ROW_H: f32 = 36.0;
const HEAD_H: f32 = 32.0;
const GROUP_GAP: f32 = 12.0;
const NOTE_H: f32 = 24.0;
/// columns: the tick (in line with the headings), the tool's mark, the name (and ★), the detail, then the blurb
const TICK_X: f32 = 8.0;
const MARK_X: f32 = 38.0;
const NAME_X: f32 = 66.0;
const NAME_W: f32 = 196.0;
const DETAIL_W: f32 = 156.0;
/// WinUI's checkbox
const BOX: f32 = 20.0;

const PLAY: &str = "\u{F040A}";
const REFRESH: &str = "\u{F0450}";
const CHECK: &str = "\u{F012C}";
const CROSS: &str = "\u{F0156}";
const STAR: &str = "\u{F04CE}";
const INFO: &str = "\u{F02FD}";

/// Something in the scrolling list.
enum Item {
    Head(&'static str),
    Tool(usize, Tool),
    /// the last JustTools job's outcome under its row: (text, went wrong)
    Note(String, bool),
}

/// Draw the view inside its card (`area`: the card's content).
pub(crate) fn draw(v: &mut QuickInstallView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let id = cx.id;
    // what the terminal UI's render does besides drawing: the first look, and the busy flag its tick reads
    if v.checked_at.is_none() {
        v.check(cx);
    }
    // the window only ticks panes while nothing animates (and installs show spinners): keep the watch going
    // here too (the tick's own `poll`, timed by the view itself)
    if v.watching.is_some() || !v.started.is_empty() {
        v.poll(cx);
    }
    let jt = cx.svc.state().justtools.clone();
    v.busy = jt.job.is_some() || jt.paste_busy.is_some();
    ui.hit_with(area, Action::View(id, BG), Cursor::Default);
    let col = page(area, 880.0);
    let all = rows();
    let last = all.len() - 1;
    v.sel = v.sel.min(last);
    let checking = v.found.is_none();
    let picked = v.picked.clone().unwrap_or_default();

    // ---- title, the machine, the quick actions
    let actions: [(Option<&str>, &str, &str, Kind, usize); 3] = [
        (None, "Tick all missing", "a", Kind::Secondary, TICK_ALL),
        (None, "Tick none", "n", Kind::Secondary, TICK_NONE),
        (Some(REFRESH), "Look again", "r", Kind::Ghost, LOOK),
    ];
    let widths: Vec<f32> = actions.iter().map(|(i, l, k, ..)| key_button_width(ui, *i, l, Some(*k))).collect();
    let actions_w = widths.iter().sum::<f32>() + 8.0 * (widths.len() - 1) as f32;
    let beside = col.w >= actions_w + 260.0;
    let tb = ui.line("Quick install", Style::new(TITLE, weight::SEMIBOLD, t.text), None);
    ui.draw(&tb, Point::new(col.x, col.y));
    let sub_y = col.y + tb.h + 2.0;
    let sub_w = if beside { col.w - actions_w - 24.0 } else { col.w };
    match &v.found {
        Some(f) => {
            ui.icon(Rect::new(col.x, sub_y, 16.0, 18.0), os_icon(&f.os), size::BODY, t.muted);
            ui.label_in(Rect::new(col.x + 22.0, sub_y, sub_w - 22.0, 18.0), f.os.describe(), Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
        }
        None => {
            ui.spinner(Point::new(col.x + 7.0, sub_y + 9.0), 5.0, 1.6, t.muted);
            ui.label_in(Rect::new(col.x + 22.0, sub_y, sub_w - 22.0, 18.0), "looking at this machine…", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
        }
    }
    let head_bottom = sub_y + 18.0;
    // beside the title, or under it (flowing onto more lines when narrow)
    let (mut ax, mut ay) = if beside { (col.right() - actions_w, col.y - 2.0) } else { (col.x, head_bottom + 12.0) };
    for ((icon, label, key, kind, n), w) in actions.iter().zip(&widths) {
        if !beside && ax > col.x && ax + w > col.right() {
            ax = col.x;
            ay += BUTTON_H + 8.0;
        }
        key_button(ui, Rect::new(ax, ay, *w, BUTTON_H), *icon, label, Some(*key), *kind, Some(Action::View(id, *n)));
        ax += w + 8.0;
    }
    let list_top = if beside { head_bottom + 16.0 } else { ay + BUTTON_H + 16.0 };

    // ---- the install bar, pinned at the bottom (measured first)
    let todo = v.to_install(cx);
    let how = match (v.row(), &v.found) {
        (Row::Tool(tool), Some(found)) => found.how.get(&tool).map(|h| (tool, h.clone())),
        _ => None,
    };
    let bar_pad = 16.0;
    let inner_w = col.w - 2.0 * 16.0;
    let how_box = how.as_ref().map(|(tool, h)| {
        let lead = format!("{} · how: ", tool.name());
        ui.rich(&[(lead, Style::new(size::SMALL, weight::MEDIUM, t.muted)), (h.clone(), Style::mono(size::SMALL, t.text.mix(t.muted, 0.2)))], size::SMALL, Some(inner_w), true, Some(2))
    });
    let notes: Vec<&str> = std::iter::once("each installer runs in a new tab so you can follow it and answer its prompts")
        .chain(v.watching.is_some().then_some("open a new terminal or restart bro afterwards so new sessions find the new tools on PATH"))
        .collect();
    let hints_y = col.bottom() - 18.0;
    let bar_full = bar_pad + BUTTON_H + 12.0 + notes.len() as f32 * 20.0 + how_box.as_ref().map_or(0.0, |b| 10.0 + 1.0 + 10.0 + b.h) + bar_pad;
    // it grows and shrinks (the how line comes and goes with the selection) instead of jumping
    let bar_h = ui.ease(("quick-install-bar-h", id), bar_full, 140.0);
    let bar = Rect::new(col.x, hints_y - 16.0 - bar_h, col.w, bar_h);

    // ---- the list: layout, keep the selection in view, draw what shows
    let mut items: Vec<(Item, f32, f32)> = vec![];
    let mut y = 0.0;
    let mut group = None;
    for (i, row) in all.iter().enumerate() {
        let Row::Tool(tool) = *row else { continue };
        if group != Some(tool.group()) {
            if group.is_some() {
                y += GROUP_GAP;
            }
            group = Some(tool.group());
            items.push((Item::Head(tool.group().label()), y, HEAD_H));
            y += HEAD_H;
        }
        items.push((Item::Tool(i, tool), y, ROW_H));
        y += ROW_H;
        // the last outcome shows unless JustTools itself is being worked on (a skill download doesn't hide it)
        if tool == Tool::JustTools && jt.job.as_ref().is_none_or(|j| j.kind == JobKind::Skill) {
            if let Some(e) = &jt.error {
                items.push((Item::Note(e.clone(), true), y, NOTE_H));
                y += NOTE_H;
            } else if let Some(done) = &jt.done {
                items.push((Item::Note(done.clone(), false), y, NOTE_H));
                y += NOTE_H;
            }
        }
    }
    let content_h = y;
    let view = Rect::new(col.x - 8.0, list_top, col.w + 16.0, (bar.y - 12.0 - list_top).max(ROW_H));
    // `scroll` holds the list's offset in pixels here (the terminal UI keeps lines in it)
    let mut scroll = v.scroll as f32;
    if let Some(at) = items.iter().position(|(it, ..)| matches!(it, Item::Tool(i, _) if *i == v.sel)) {
        let (_, top, h) = &items[at];
        // the group's heading comes into view with its first row
        let top = if at > 0 && matches!(items[at - 1].0, Item::Head(_)) { items[at - 1].1 } else { *top };
        let bottom = items[at].1 + h + if items.get(at + 1).is_some_and(|(it, ..)| matches!(it, Item::Note(..))) { NOTE_H } else { 0.0 };
        // clear of the faded edges
        if top - 4.0 < scroll {
            scroll = top - 4.0;
        } else if bottom + 22.0 > scroll + view.h {
            scroll = bottom + 22.0 - view.h;
        }
    }
    scroll = scroll.clamp(0.0, (content_h - view.h).max(0.0));
    v.scroll = scroll.round() as usize;
    let shown = ui.ease(("quick-install-scroll", id), scroll, 150.0);
    ui.clip(view, 0.0);
    let x0 = col.x;
    for (it, top, h) in &items {
        let y = view.y + top - shown;
        if y + h < view.y || y > view.bottom() {
            continue;
        }
        match it {
            Item::Head(label) => widgets::section(ui, Rect::new(x0, y + 6.0, col.w, HEAD_H - 6.0), &sentence(label)),
            Item::Tool(i, tool) => {
                let r = Rect::new(x0 - 8.0, y, col.w + 16.0, ROW_H);
                // the whole row clicks; its fill stops a pixel short of its neighbours, as WinUI's list items
                ui.hit(r, Action::View(id, ROW + i));
                let fill = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
                let hot = widgets::row(ui, fill, ("quick-install", id, *i), v.sel == *i, Action::View(id, ROW + i));
                tool_row(v, ui, r, *tool, v.sel == *i, hot, picked.contains(tool), checking, &jt, cx);
            }
            Item::Note(text, bad) => {
                let c = if *bad { t.danger } else { t.good };
                let nx = x0 - 8.0 + NAME_X;
                ui.icon(Rect::new(nx, y + 3.0, 14.0, 18.0), if *bad { CROSS } else { CHECK }, size::SMALL, c);
                ui.label_in(Rect::new(nx + 20.0, y + 3.0, col.right() - nx - 20.0, 18.0), text, Style::new(size::SMALL, weight::MEDIUM, c), Align::Left);
            }
        }
    }
    // the list's cut edges fade out
    if shown > 0.5 {
        edge_fade(ui, Rect::new(view.x, view.y, view.w, 18.0), t.surface, true);
    }
    if shown + view.h < content_h - 0.5 {
        edge_fade(ui, Rect::new(view.x, view.bottom() - 24.0, view.w, 24.0), t.surface, false);
    }
    ui.unclip();
    let fade = ui.ease(("quick-install-bar", id), if ui.hovered(view) { 1.0 } else { 0.35 }, 200.0);
    widgets::scrollbar(ui, view, content_h, view.h, shown, fade);

    // ---- the install bar
    page_card(ui, bar);
    ui.clip(bar, radius::OVERLAY);
    let bx = bar.x + 16.0;
    let mut by = bar.y + bar_pad;
    let on = v.sel == last;
    let (label, kind) = if todo.is_empty() { ("Install".to_string(), Kind::Secondary) } else { (format!("Install {}", todo.len()), Kind::Primary) };
    let bw = key_button_width(ui, Some(PLAY), &label, Some("i"));
    let button = Rect::new(bx, by, bw, BUTTON_H);
    if on {
        focus_ring(ui, button);
    }
    key_button(ui, button, Some(PLAY), &label, Some("i"), kind, Some(Action::View(id, ROW + last)));
    let what = if todo.is_empty() {
        "nothing ticked to install".to_string()
    } else {
        let mut names: Vec<&str> = todo.iter().take(6).map(|t| t.name()).collect();
        let more = format!("{} more", todo.len().saturating_sub(6));
        if todo.len() > 6 {
            names.push(&more);
        }
        names.join(", ")
    };
    let wst = Style::new(size::BODY, if todo.is_empty() { weight::REGULAR } else { weight::MEDIUM }, if todo.is_empty() { t.muted } else { t.text.mix(t.muted, 0.2) });
    ui.label_in(Rect::new(button.right() + 14.0, by, bar.right() - 16.0 - button.right() - 14.0, BUTTON_H), &what, wst, Align::Left);
    by += BUTTON_H + 12.0;
    for (k, note) in notes.iter().enumerate() {
        let st = Style::new(size::SMALL, weight::REGULAR, if k == 0 { t.muted } else { t.inline });
        ui.icon(Rect::new(bx, by, 14.0, 18.0), INFO, size::SMALL, if k == 0 { t.faint } else { t.inline });
        ui.label_in(Rect::new(bx + 20.0, by, inner_w - 20.0, 18.0), note, st, Align::Left);
        by += 20.0;
    }
    if let Some(b) = &how_box {
        by += 10.0;
        ui.hline(bx, by, inner_w, t.hairline);
        by += 11.0;
        ui.draw(b, Point::new(bx, by));
    }
    ui.unclip();

    // ---- the keys, quietly
    hints(ui, Point::new(col.x, hints_y), col.w, &[("j/k", "move"), ("space", "tick"), ("i", "install"), ("a/n", "all / none"), ("r", "look again")]);
    true
}

/// One tool: its tick (✓ installed, a spinner while JustTools works on it), its mark, name and ★, what's
/// there, what it is. The detail reads exactly as in the terminal UI.
#[allow(clippy::too_many_arguments)]
fn tool_row(v: &QuickInstallView, ui: &mut Ui, r: Rect, tool: Tool, on: bool, hot: f32, ticked: bool, checking: bool, jt: &crate::services::justtools::JustToolsState, cx: &Cx) {
    let t = ui.t;
    let installed = v.installed(tool, cx);
    let jt_job = jt.job.as_ref().filter(|j| tool == Tool::JustTools && j.kind != JobKind::Skill);
    let paste_busy = jt.paste_busy.filter(|_| tool == Tool::JustPaste);
    let busy = jt_job.is_some() || paste_busy.is_some();
    let mid = r.y + r.h / 2.0;

    // tick
    let bx = Rect::new(r.x + TICK_X, mid - BOX / 2.0, BOX, BOX);
    if busy {
        ui.spinner(bx.center(), 5.5, 1.7, t.shine);
    } else if installed {
        // done: the check alone, nothing behind it
        ui.icon(bx, CHECK, size::HEADING, t.good);
    } else {
        checkbox(ui, bx, ticked, if on { 1.0 } else { hot });
    }

    // its mark: an agent's own logo, else a quiet monochrome icon
    let mr = Rect::new(r.x + MARK_X, mid - 9.0, 18.0, 18.0);
    match tool_mark(tool) {
        m @ Mark::Logo(_) => ui.mark(m, mr, 16.0, None),
        m => ui.mark(m, mr, size::TITLE + 1.0, Some(t.muted)),
    }

    // name and ★
    let x = r.x + NAME_X;
    let name_st = match () {
        _ if tool == Tool::JustTools => Style::new(size::BODY, weight::SEMIBOLD, t.shine),
        _ if on => Style::new(size::BODY, weight::SEMIBOLD, t.text),
        _ => Style::new(size::BODY, weight::MEDIUM, t.text.mix(t.muted, 0.1)),
    };
    // the columns give way in a narrow pane: the blurb goes first
    let name_w = NAME_W.min((r.right() - 12.0 - x) * 0.56);
    let nb = ui.line(tool.name(), name_st, Some((name_w - 26.0).max(20.0)));
    ui.draw(&nb, Point::new(x, mid - nb.h / 2.0));
    if tool.recommended() {
        ui.icon(Rect::new(x + nb.w + 5.0, mid - 8.0, 14.0, 16.0), STAR, size::CAPTION, t.shine);
    }

    // detail
    let detail = match jt_job {
        Some(j) => super::step_label(j.step).to_string(),
        None if tool == Tool::JustTools && jt.installed.is_some() => jt.installed.as_ref().and_then(|i| i.version.clone()).unwrap_or_else(|| "installed".into()),
        None if paste_busy.is_some() => "switching on".into(),
        None if tool == Tool::JustPaste && installed => jt.paste.as_ref().map_or("on", bro_core::justpaste::describe).into(),
        None if installed => match v.found.as_ref().and_then(|f| f.status.get(&tool)) {
            // a version reads better than a path
            Some(d) if d.starts_with(|c: char| c.is_ascii_digit()) => d.clone(),
            _ => "installed".into(),
        },
        None if checking => "checking…".into(),
        None => "not installed".into(),
    };
    let dc = if busy {
        t.shine
    } else if installed {
        t.good.mix(t.text, 0.15)
    } else {
        t.faint
    };
    let dx = x + name_w;
    let detail_w = DETAIL_W.min(r.right() - 12.0 - dx);
    ui.label_in(Rect::new(dx, r.y, (detail_w - 10.0).max(8.0), r.h), &detail, Style::new(size::SMALL, if installed || busy { weight::MEDIUM } else { weight::REGULAR }, dc), Align::Left);

    // what it is
    let blurb_x = dx + DETAIL_W;
    let blurb_w = r.right() - 12.0 - blurb_x;
    if blurb_w >= 80.0 {
        ui.label_in(Rect::new(blurb_x, r.y, blurb_w, r.h), tool.blurb(), Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
    }
}

/// WinUI's checkbox: 4px corners, a muted edge when off, the accent with a check when on (`hot`: hovered).
fn checkbox(ui: &mut Ui, r: Rect, on: bool, hot: f32) {
    let t = ui.t;
    if on {
        ui.round(r, radius::CONTROL, t.accent.fade(1.0 - 0.1 * hot));
        ui.icon(r, CHECK, size::TITLE, t.on_accent);
    } else {
        ui.panel(r, radius::CONTROL, t.text.with_alpha(0.03 + 0.04 * hot), t.muted);
    }
}

/// A tool's mark: the agents' logos (Pi and omp have none, so their glyph), else an icon for what it is.
fn tool_mark(tool: Tool) -> Mark {
    let glyph = match tool {
        Tool::Agent(h) => return brand::harness(h),
        Tool::JustTools => "\u{F1323}",
        Tool::JustPaste => "\u{F0192}",
        Tool::JustWatch => "\u{F13B4}",
        Tool::JustImagine => "\u{F0E0C}",
        Tool::Git => "\u{F02A2}",
        Tool::GitHubCli => "\u{F02A4}",
        Tool::Node => "\u{F0399}",
        Tool::Bun => "\u{E76F}",
        Tool::Python => "\u{F0320}",
        Tool::Uv => "\u{F03D7}",
        Tool::Dotnet9 | Tool::Dotnet10 | Tool::Dotnet11 => "\u{E77F}",
        Tool::Ripgrep => "\u{F13B8}",
        Tool::Ffmpeg => "\u{F384}",
        Tool::YtDlp => "\u{F05C3}",
        Tool::Tailscale => "\u{F15FC}",
        Tool::PowerToys => "\u{F09AD}",
        Tool::ShareX => "\u{F0E51}",
    };
    Mark::Glyph(glyph, None)
}

/// A group's heading in sentence case ("essentials" → "Essentials").
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The machine's mark beside its description.
fn os_icon(os: &Os) -> &'static str {
    match os {
        Os::Windows { .. } => "\u{F05B3}",
        Os::Mac { .. } => "\u{F0035}",
        Os::Linux { .. } => "\u{F033D}",
    }
}

/// Target `n` was clicked: a row selects and activates (ticks, or installs from the install button), the
/// quick actions send their keys, as in the terminal UI.
pub(crate) fn click(v: &mut QuickInstallView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    let code = match n {
        TICK_ALL => KeyCode::Char('a'),
        TICK_NONE => KeyCode::Char('n'),
        LOOK => KeyCode::Char('r'),
        n if n >= ROW => {
            v.sel = (n - ROW).min(rows().len() - 1);
            v.activate(cx);
            return;
        }
        _ => return,
    };
    v.key(KeyEvent::new(code, KeyModifiers::NONE), cx);
}

/// The wheel moves the selection, as in the terminal UI.
pub(crate) fn wheel(v: &mut QuickInstallView, _n: Option<usize>, down: bool, cx: &mut Cx) {
    let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
    v.mouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE }, ratatui::layout::Rect::default(), cx);
}
