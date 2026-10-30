//! Session cards: the frame around tiles that share a project and login, each card's header (logo, title,
//! login and model, badges, status, buttons), the terminal or view inside, and the special faces a card can
//! show (a queued resume, a failed start, a folded sliver, the back of a turned card).

use super::actions::{Action, PaneAct};
use super::kit::brand::{self, Mark};
use super::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use super::layout::{GroupBox, Layout, PaneBox};
use super::term::draw::{self as tdraw, Palette};
use super::term::mirror::Mirror;
use super::tui::{Grid, TuiCanvas};
use super::PaneState;
use crate::app::App;
use crate::pane::{Activity, Cx, Pane, PaneId};
use bro_core::Harness;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use std::collections::HashMap;
use std::time::Instant;

/// An agent's mark, or (None) the default shell's.
pub fn harness_mark(h: Option<Harness>) -> Mark {
    match h {
        Some(h) => brand::harness(h),
        None => brand::shell(&crate::util::default_shell(None).0),
    }
}

/// How a session is marked: the orchestrator's star, its agent's logo, or its shell's.
pub fn session_mark(app: &App, id: PaneId, t: &super::kit::Tokens) -> Mark {
    let Some(term) = app.panes.get(&id).and_then(|p| p.as_term_ref()) else { return Mark::Glyph("\u{F018D}", None) };
    if term.meta.orchestrator {
        return Mark::Glyph("\u{F04CE}", Some(t.gold));
    }
    match term.meta.harness {
        Some(h) => brand::harness(h),
        None => match term.launch_spawn() {
            Some(s) => brand::shell(&s.program),
            None => harness_mark(None),
        },
    }
}

/// The maker of the model a session runs, when it isn't its agent's own lab (Codex on Claude, Claude on Gemini):
/// tucked into the corner of [`session_mark`] (see [`crate::maker`]).
pub fn session_badge(app: &App, id: PaneId) -> Option<Mark> {
    let m = &app.panes.get(&id)?.as_term_ref()?.meta;
    if m.orchestrator {
        return None;
    }
    crate::maker::badge(m.harness, m.profile.as_deref(), m.model.as_deref()).map(brand::maker)
}

/// A card turning between its terminal and its conversation fades the new face in this long (seconds).
const FACE_FADE: f32 = 0.26;

/// Where views and card backs were painted this frame (for the mouse).
#[derive(Default)]
pub struct Painted {
    pub views: Vec<(PaneId, Grid)>,
    /// clickable rows on turned cards: (rect, pane, uri)
    pub card_links: Vec<(Rect, PaneId, String)>,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_panes(app: &mut App, ui: &mut Ui, layout: &Layout, panes: &mut HashMap<PaneId, PaneState>, canvas: &mut TuiCanvas, focused_window: bool, media: &mut super::media::Media, apps: &mut super::apps::Apps) -> Painted {
    let mut out = Painted::default();
    if layout.all_minimized {
        ui.label_in(layout.body, "All sessions minimized · show minimized in the sidebar", Style::new(size::BODY, weight::MEDIUM, ui.t.muted), Align::Center);
    }
    for g in &layout.groups {
        draw_group(app, ui, g);
    }
    let pal = Palette::from_tokens(ui.t, ui.theme);
    let now = Instant::now();
    app.flips.retain(|id, flip| app.panes.contains_key(id) && (flip.back || flip.turning(now)));
    // the card with the keyboard last, so its edge stays whole where tiles share a line
    for b in layout.panes.iter().filter(|b| !b.focused).chain(layout.panes.iter().filter(|b| b.focused)) {
        draw_card(app, ui, layout, b, panes, canvas, &pal, focused_window, &mut out, media, apps);
    }
    // where tiles share a line there's no gutter: the line itself is grabbed to move the edge (the mouse finds the
    // tile edge under it, as in a gutter)
    for b in &layout.panes {
        for (r, vertical) in b.edge_strips() {
            ui.hit_with(r, Action::None, if vertical { Cursor::ResizeH } else { Cursor::ResizeV });
        }
    }
    out
}

fn draw_group(app: &App, ui: &mut Ui, g: &GroupBox) {
    let t = ui.t;
    let r = g.rect;
    // the frame in the project's colour, as the terminal UI's group frame
    let color = t.project(&g.key, ui.theme);
    ui.glass_panel(r, radius::OVERLAY, color.with_alpha(if g.focused { 0.05 } else { 0.03 }), color.with_alpha(if g.focused { 0.75 } else { 0.4 }));
    // the label sits in the frame's top band: what its sessions share (the project, the agent by its logo, the
    // login and model, a folder that isn't the project's root), then how many there are
    let ink = if g.focused { t.text } else { t.muted };
    let st = Style::new(size::SMALL, weight::SEMIBOLD, ink);
    let quiet = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let lr = Rect::new(r.x + 16.0, r.y + 3.0, r.w - 32.0, 16.0);
    let count = format!("{} session{}", g.count, if g.count == 1 { "" } else { "s" });
    let mut x = lr.x;
    let end = lr.right();
    let Some((id, term)) = g.pane.and_then(|id| app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|term| (id, term))) else {
        ui.label_in(Rect::new(x, lr.y, (end - x).max(0.0), lr.h), &format!("{}  ·  {count}", g.label), st, Align::Left);
        return;
    };
    let (who, cwd) = who_line(term, true);
    let put = |ui: &mut Ui, x: &mut f32, text: &str, st: Style| {
        if *x >= end {
            return;
        }
        let b = ui.line(text, st, Some(end - *x));
        ui.draw(&b, Point::new(*x, lr.y + (lr.h - b.h) / 2.0));
        *x += b.w;
    };
    put(ui, &mut x, &term.meta.project.name, st);
    put(ui, &mut x, "  ·  ", quiet);
    if x + 16.0 < end {
        ui.mark_badged(session_mark(app, id, t), session_badge(app, id), Rect::new(x, lr.y, 14.0, lr.h), 12.0, None);
        x += 18.0;
    }
    put(ui, &mut x, &who, st);
    if !cwd.is_empty() {
        put(ui, &mut x, &format!(" — {cwd}"), quiet);
    }
    put(ui, &mut x, &format!("  ·  {count}"), quiet);
}

#[allow(clippy::too_many_arguments)]
fn draw_card(app: &mut App, ui: &mut Ui, _layout: &Layout, b: &PaneBox, panes: &mut HashMap<PaneId, PaneState>, canvas: &mut TuiCanvas, pal: &Palette, focused_window: bool, out: &mut Painted, media: &mut super::media::Media, apps: &mut super::apps::Apps) {
    let id = b.id;
    let Some(p) = app.panes.get(&id) else { return };
    let t = ui.t;
    let term = p.as_term_ref();
    let notice = app.session_notice(id);
    let notice_ink = notice.filter(|n| n.unread).map(|n| match n.kind {
        bro_bridge::insights::NoticeKind::Input => t.warn,
        bro_bridge::insights::NoticeKind::Finished => t.good,
    });
    let orchestrator = term.is_some_and(|t| t.meta.orchestrator);
    // the project's colour, as the terminal UI's session frame; the orchestrator stands out in gold
    let accent = match term {
        Some(_) if orchestrator => t.gold,
        Some(term) => t.project(&term.meta.project.key, ui.theme),
        None => t.accent,
    };
    let r = b.card;
    if r.w < 4.0 || r.h < 4.0 {
        return;
    }
    let hover = ui.hover_amount(("card", id), r);
    // flip: the card narrows to its edge and widens again showing the other face
    let flip = app.flips.get(&id).copied();
    let (back, cover) = flip.map(|f| f.face(Instant::now())).unwrap_or((false, 1.0));
    if flip.is_some_and(|f| f.turning(Instant::now())) {
        // a turn is a transition: every frame until it lands
        ui.anim.moving = true;
    }
    let shown = if cover < 0.999 {
        let w = (r.w * cover).max(2.0);
        Rect { x: r.x + (r.w - w) / 2.0, w, ..r }
    } else {
        r
    };
    // card: a surface on the content layer, its edge the accent when it has the keyboard; the orchestrator keeps a
    // gold edge when it doesn't
    let border = match (b.focused, orchestrator) {
        (true, _) => accent.with_alpha(0.7),
        (false, true) => t.gold.with_alpha(0.32 + 0.18 * hover),
        (false, false) if term.is_some() => accent.with_alpha(0.3 + 0.2 * hover),
        (false, false) => t.hairline.mix(t.border, hover),
    };
    // square where it shares a line with the tile beside it (shared borders), rounded elsewhere
    let corners = b.joined.corners(radius::OVERLAY);
    {
        // the card is a surface of the window (as see-through as the rest of it over a material), its edge isn't
        let px = ui.px(shown);
        let w = ui.scale.round().max(1.0);
        let surface = notice_ink.map_or(t.surface, |ink| t.surface.mix(ink, if t.light { 0.045 } else { 0.055 }));
        ui.scene.glass_quad(bro_gfx::Quad::new(px, surface).corners(corners.scale(ui.scale)));
        ui.scene.quad(bro_gfx::Quad::new(px, bro_gfx::Color::TRANSPARENT).corners(corners.scale(ui.scale)).border(w, border));
    }
    ui.clip(shown, if b.joined.any() { 0.0 } else { radius::OVERLAY });
    // anywhere on the card is a click on it (focus, double-click zooms, alt+double-click turns it): the frame, a
    // view's margins, a folded sliver; what's inside takes its own clicks over this
    ui.hit_with(shown, Action::Pane(id, PaneAct::Header), Cursor::Default);
    // header
    let header = Rect { h: b.header.h, ..shown };
    if b.focused {
        ui.round_corners(header, bro_gfx::Corners { tl: corners.tl, tr: corners.tr, br: 0.0, bl: 0.0 }, t.text.with_alpha(if t.light { 0.025 } else { 0.03 }));
    }
    ui.hline(shown.x, header.bottom(), shown.w, t.hairline);
    ui.hit_with(header, Action::Pane(id, PaneAct::Header), Cursor::Default);
    draw_header(app, ui, b, header, accent, hover, panes.get(&id).map(|st| &st.focus));
    // content
    let content = Rect { x: shown.x + 1.0, w: (shown.w - 2.0).max(0.0), ..b.content };
    if b.folded {
        // folded out of the way of a grown session: just its name, down the sliver when it's narrow (the program
        // keeps its size until it unfolds)
        draw_folded(app, ui, b, content);
        ui.unclip();
        return;
    }
    let waker = app_waker(app, id);
    let is_term = app.panes.get(&id).is_some_and(|p| p.as_term_ref().is_some());
    if back {
        draw_back(app, ui, id, content, accent, canvas, out);
    } else if is_term {
        // "working" rain also falls inside a session while its agent is mid-turn, behind the text
        let (bd, inside) = super::backdrop::choice(app);
        let rain = match inside {
            super::backdrop::Inside::All => true,
            super::backdrop::Inside::Busy => app.panes.get(&id).is_some_and(|p| p.busy()),
            super::backdrop::Inside::No => false,
        };
        if rain && bd == crate::skin::Backdrop::Rain {
            super::backdrop::draw(ui, content, bd, ui.t.surface);
        }
        // turning between the terminal and the conversation (focus mode), the new face fades in
        let focus_face = app.focus_view(id);
        {
            let st = panes.entry(id).or_default();
            if st.face.is_some_and(|f| f != focus_face) {
                st.face_at = Some(ui.now);
            }
            st.face = Some(focus_face);
        }
        if focus_face {
            // focus mode: the conversation, the session running on under it at the card's size
            let started = Instant::now();
            if let Some(term) = app.panes.get_mut(&id).and_then(|p| p.as_term()) {
                keep_sized(waker, ui, term, content);
            }
            let st = panes.entry(id).or_default();
            st.drawn = Default::default();
            super::focus::draw(app, ui, id, content, b.focused && focused_window, &mut st.focus, media, apps);
            app.perf.pane_rendered(id, started.elapsed());
        } else {
            let tags = super::focus::terminal_quips_height(app, id);
            if let Some(term) = app.panes.get_mut(&id).and_then(|p| p.as_term()) {
                let started = Instant::now();
                draw_terminal(waker, ui, term, b, Rect { h: (content.h - tags).max(40.0), ..content }, panes, pal, focused_window, accent);
                app.perf.pane_rendered(id, started.elapsed());
            }
            let state = panes.entry(id).or_default();
            let input = state.drawn.cursor.map(|(row, col)| Rect::new(state.drawn.origin.x + col as f32 * state.drawn.cell_w, state.drawn.origin.y + row as f32 * state.drawn.cell_h, state.drawn.cell_w, state.drawn.cell_h));
            super::focus::draw_terminal_quips(app, ui, id, content, b.focused && focused_window, input, &mut state.focus);
        }
    } else if super::views::draw(app, ui, id, super::layout::inset(content, 6.0), b.focused) {
        // drawn natively by the view's own module
    } else {
        if let Some(p) = app.panes.get_mut(&id) {
            // views not drawn natively yet: the terminal UI's own drawing, straight onto the card (filling it again
            // would stack over a see-through window)
            let fill = None;
            let theme = app.theme.clone();
            let svc = app.svc.clone();
            let tx = app.tx.clone();
            let mut actions = vec![];
            let area = super::layout::inset(content, 6.0);
            let grid = canvas.paint(ui, area, fill, |f, a| {
                let mut cx = Cx { id, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: b.focused };
                p.render(f, a, &mut cx);
            });
            ui.hit_with(Rect::new(grid.origin.x, grid.origin.y, grid.cols as f32 * grid.cell_w, grid.rows as f32 * grid.cell_h), Action::Pane(id, PaneAct::Body), Cursor::Default);
            out.views.push((id, grid));
            if !actions.is_empty() {
                app.apply(id, actions);
                let _ = app.tx.send(crate::pane::Event::Tick);
            }
        }
    }
    // nearly edge-on while it turns: the face dims, as the terminal UI's squash does
    if cover < 0.6 {
        ui.top_fill(Rect { y: header.y, h: shown.h, ..shown }, 0.0, t.surface.with_alpha(0.45));
    }
    if let Some(st) = panes.get_mut(&id)
        && let Some(at) = st.face_at
    {
        let k = (ui.now.saturating_duration_since(at).as_secs_f32() / FACE_FADE).min(1.0);
        if k < 1.0 {
            ui.anim.moving = true;
            let r = ui.px(content);
            ui.scene.top_glass_quad(bro_gfx::Quad::new(r, t.surface.with_alpha((1.0 - k).powi(2))));
        } else {
            st.face_at = None;
        }
    }
    if let Some(ink) = notice_ink {
        ui.top_fill(content, 0.0, ink.with_alpha(if t.light { 0.025 } else { 0.035 }));
    }
    ui.unclip();
}

/// A folded sliver narrower than a working width: its name's letters down the middle (the terminal UI's vertical
/// title), as many as fit.
fn draw_folded(app: &App, ui: &mut Ui, b: &PaneBox, content: Rect) {
    let Some(p) = app.panes.get(&b.id) else { return };
    // the content's width in cells, as the terminal UI's inner rect (its frame takes one on each side)
    if b.cells.width.saturating_sub(2) >= crate::priority::USEFUL[0] || content.h < 24.0 {
        return;
    }
    let title = p.title();
    let name = title.split(" — ").next().unwrap_or_default().trim().to_string();
    let st = Style::new(size::SMALL, weight::SEMIBOLD, ui.t.muted);
    let line_h = 16.0;
    let room = ((content.h - 12.0) / line_h).floor().max(0.0) as usize;
    let mut y = content.y + 6.0;
    for c in name.chars().filter(|c| !c.is_whitespace()).take(room) {
        ui.label_in(Rect::new(content.x, y, content.w, line_h), &c.to_string(), st, Align::Center);
        y += line_h;
    }
}

fn app_waker(app: &App, id: PaneId) -> crate::pane::Waker {
    crate::pane::Waker::new(id, app.tx.clone())
}

/// The header: agent mark, title, who and where, then badges, status and buttons on the right.
/// `view`: the card's focus view, when it has one (its header then says what the agent is doing).
fn draw_header(app: &mut App, ui: &mut Ui, b: &PaneBox, r: Rect, accent: Color, hover: f32, view: Option<&super::focus::View>) {
    let id = b.id;
    let t = ui.t;
    let Some(p) = app.panes.get(&id) else { return };
    let mid = r.y + r.h / 2.0;
    let mut x = r.x + 12.0;
    // right side first: buttons, status, badges (they win the room)
    let mut right = r.right() - 8.0;
    let zoomed = if app.stacked() { app.stack_zoom } else { app.tabs.get(app.cur).is_some_and(|tab| tab.zoom) };
    let show_buttons = b.focused || hover > 0.05;
    if r.w > 160.0 && !zoomed {
        let br = Rect::new(right - 24.0, mid - 12.0, 24.0, 24.0);
        if show_buttons {
            // the × turns the danger colour under the pointer (it ends the session), on a neutral fill
            let hot = ui.hover_amount(("close", id), br);
            if hot > 0.0 {
                ui.round(br, radius::CONTROL, t.text.with_alpha(if ui.pressing && hot > 0.5 { 0.04 } else { 0.07 * hot }));
            }
            ui.icon(br, "\u{F0156}", size::TITLE, t.muted.mix(t.danger, hot));
            ui.hit(br, Action::Pane(id, PaneAct::Close));
            if hot > 0.5 {
                let key = app.keymap.primary(crate::keymap::Act::Close);
                widgets::tooltip(ui, br, &if key.is_empty() { "Close".to_string() } else { format!("Close · {key}") });
            }
        } else {
            ui.hit(br, Action::Pane(id, PaneAct::Close));
        }
        right = br.x - 2.0;
    }
    // focus mode: this session's terminal, or its conversation again
    if app.focus.on && r.w > 200.0 && p.as_term_ref().is_some_and(crate::app::focus::focusable) {
        let br = Rect::new(right - 24.0, mid - 12.0, 24.0, 24.0);
        let peek = app.focus.peek.contains(&id);
        let glyph = if peek { "\u{F0369}" } else { "\u{F018D}" };
        if show_buttons {
            widgets::icon_button(ui, br, glyph, t.muted, Action::Focus(id, super::focus::TERMINAL));
            if ui.hovered(br) {
                widgets::tooltip(ui, br, if peek { "Back to the conversation" } else { "Show the terminal" });
            }
        } else {
            ui.hit(br, Action::Focus(id, super::focus::TERMINAL));
        }
        right = br.x - 2.0;
    }
    if app.tile_all && p.is_terminal() {
        let br = Rect::new(right - 24.0, mid - 12.0, 24.0, 24.0);
        let minimized = app.minimized.contains(&id);
        let glyph = if minimized { "\u{F0415}" } else { "\u{F0374}" };
        if show_buttons {
            widgets::icon_button(ui, br, glyph, t.muted, Action::Pane(id, PaneAct::Minimize));
        } else {
            ui.hit(br, Action::Pane(id, PaneAct::Minimize));
        }
        right = br.x - 4.0;
    }
    let time = app.start.elapsed().as_secs_f64();
    if let Some(term) = p.as_term_ref() {
        // status: the indicator alone (spinner, alert); the header never says "done" (the sidebar and the tabs do).
        // In focus mode it says what the agent is doing in words, since the terminal that shows it is out of sight.
        if let Some(notice) = app.session_notice(id).filter(|n| n.unread) {
            let (label, glyph, ink) = match notice.kind {
                bro_bridge::insights::NoticeKind::Input => (if term.limit_warning().is_some() { "Paused" } else { "Needs input" }, "\u{F0026}", t.warn),
                bro_bridge::insights::NoticeKind::Finished => ("Finished", "\u{F012C}", t.good),
            };
            let text = ui.line(label, Style::new(size::SMALL, weight::MEDIUM, ink), Some((r.w * 0.32).max(60.0)));
            ui.draw(&text, Point::new(right - text.w, mid - text.h / 2.0));
            ui.icon(Rect::new(right - text.w - 22.0, mid - 8.0, 16.0, 16.0), glyph, 14.0, ink);
            right -= text.w + 34.0;
        } else if app.focus_view(id) {
            let room = (r.w * 0.36).clamp(120.0, 340.0);
            if let Some(w) = super::focus::header_status(app, ui, id, view, Point::new(right - 4.0, mid), room) {
                right -= w + 14.0;
            }
        } else if let Some(w) = status_pill(ui, Point::new(right, mid), p.activity(), false, false) {
            right -= w + 6.0;
        }
        // badges: watchdog, auto-resume, remote size, scrollback
        let mut badges: Vec<(String, Color)> = vec![];
        if let Some(d) = app.svc.watchdog(&term.meta.sid) {
            let (done_n, total) = d.plan.progress();
            badges.push(match (&d.running, &d.finished, d.paused) {
                (Some(_), _, _) => ("watchdog reviewing".to_string(), t.accent),
                (None, Some(_), _) => ("watchdog done".to_string(), t.good),
                (None, None, true) => ("watchdog paused".to_string(), t.muted),
                (None, None, false) => (format!("watchdog {done_n}/{total}"), t.shine),
            });
        }
        if let Some(note) = term.resume_note() {
            badges.push((note, t.inline));
        }
        // the agent team it leads (prefix M shows the board)
        if let Some(team) = term.team().filter(|team| !team.is_empty()) {
            let n = team.teammates().count();
            badges.push(match team.working() {
                0 => (format!("team · {n} idle"), t.muted),
                w => (format!("team · {w} of {n} working"), t.accent),
            });
        }
        if term.remote_grown() {
            let (rows, cols) = term.pty_size();
            badges.push((format!("⇲ {cols}×{rows} remote"), t.muted));
        }
        // another bro window typed into it last: its size is that window's until you type here
        if term.watching() {
            badges.push(("⇄ watching · type to take over".to_string(), t.muted));
        }
        if term.scroll_offset() > 0 {
            badges.push((format!("↑{}", term.scroll_offset()), t.inline));
        }
        if let Some(sub) = subtitle(app, id, zoomed) {
            badges.push((sub, accent));
        }
        for (text, c) in badges {
            let st = Style::new(size::SMALL, weight::MEDIUM, c);
            let w = ui.measure(&text, st);
            if right - w < r.x + 180.0 {
                break;
            }
            ui.label(Point::new(right - w, mid - 8.0), &text, st);
            right -= w + 12.0;
        }
        let _ = time;
        // left: mark, title, who, where
        let mark = Rect::new(x, mid - 9.0, 18.0, 18.0);
        let m = session_mark(app, id, t);
        ui.mark_badged(m, session_badge(app, id), mark, 16.0, None);
        x = mark.right() + 8.0;
        let avail = (right - x - 8.0).max(0.0);
        // the session's own title (yours, else the agent's) leads; without one, its login · model does. Inside a
        // group frame, which names the project and agent, its number leads.
        let (who, place) = who_line(term, b.grouped);
        let (title, meta) = match term.display_title() {
            Some(t) => (t, [place, who].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")),
            None => (who, place),
        };
        let title = match (b.grouped, b.number) {
            (true, Some(n)) => format!("{n} · {title}"),
            _ => title,
        };
        // the one with the keyboard reads strongest; the others muted and lighter
        let focused = b.focused;
        let mut spans = vec![(title, Style::new(size::BODY, if focused { weight::SEMIBOLD } else { weight::MEDIUM }, if focused { t.text } else { t.text.mix(t.muted, 0.35) }))];
        if !meta.is_empty() {
            spans.push((format!("   {meta}"), Style::new(size::SMALL, weight::MEDIUM, t.muted)));
        }
        let line = ui.rich(&spans, size::BODY, Some(avail), false, Some(1));
        ui.draw(&line, Point::new(x, mid - line.h / 2.0));
    } else {
        // a view: its icon and title, and on the right what state its card is in (zoomed, grown)
        if let Some(sub) = subtitle(app, id, zoomed) {
            let st = Style::new(size::SMALL, weight::MEDIUM, accent);
            let w = ui.measure(&sub, st);
            if right - w >= r.x + 180.0 {
                ui.label(Point::new(right - w, mid - 8.0), &sub, st);
                right -= w + 12.0;
            }
        }
        let icon = view_icon(p.icon());
        ui.icon(Rect::new(x, mid - 9.0, 18.0, 18.0), icon, size::TITLE, if b.focused { t.text } else { t.muted });
        x += 26.0;
        let title = p.title();
        ui.label_in(Rect::new(x, r.y, (right - x - 8.0).max(0.0), r.h), &title, Style::new(size::BODY, if b.focused { weight::SEMIBOLD } else { weight::MEDIUM }, if b.focused { t.text } else { t.muted }), Align::Left);
    }
}

/// (login · model, where): "work · opus-5" and "bro-cli-v2 — ~\\src" (the project unless a group frame names it,
/// then the folder when it isn't the project's root).
fn who_line(term: &crate::panes::term::Term, grouped: bool) -> (String, String) {
    let m = &term.meta;
    let who = match m.harness {
        Some(h) => m.label.strip_prefix(&format!("{} · ", h.label())).unwrap_or(&m.label).to_string(),
        None => m.label.clone(),
    };
    let mut place = if m.orchestrator {
        "orchestrator".to_string()
    } else if grouped {
        String::new()
    } else {
        m.project.name.clone()
    };
    if m.cwd != m.project.root {
        let cwd = crate::util::short_path(&m.cwd, 32);
        place = if place.is_empty() { cwd } else { format!("{place} — {cwd}") };
    }
    (who, place)
}

fn subtitle(app: &App, id: PaneId, zoomed: bool) -> Option<String> {
    if app.flips.get(&id).is_some_and(|f| f.back) {
        Some("back · alt+double-click".into())
    } else if zoomed {
        Some(format!("zoomed · {}", app.keymap.primary(crate::keymap::Act::Zoom)))
    } else {
        (app.grown && app.priority == Some(id)).then(|| format!("grown · {} restores", app.keymap.primary(crate::keymap::Act::Shrink)))
    }
}

/// The status at `right_mid` (its right edge, vertical middle): the indicator, and its word when `label`.
/// Returns the width used (None when there's nothing to show: idle, untracked).
pub fn status_pill(ui: &mut Ui, right_mid: Point, activity: Option<Activity>, done: bool, label: bool) -> Option<f32> {
    let t = ui.t;
    let (word, c) = match activity? {
        Activity::Blocked => ("needs you", t.danger),
        Activity::Working => ("working", t.accent),
        Activity::Idle if done => ("done", t.good),
        Activity::Idle => return None,
    };
    let st = Style::new(size::SMALL, weight::MEDIUM, c);
    let text_w = if label { ui.measure(word, st) + 6.0 } else { 0.0 };
    let w = 16.0 + text_w;
    let at = Point::new(right_mid.x - w + 8.0, right_mid.y);
    status_dot(ui, at, activity, done);
    if label {
        ui.label(Point::new(at.x + 12.0, right_mid.y - 8.0), word, st);
    }
    let hit = Rect::new(right_mid.x - w, right_mid.y - 10.0, w, 20.0);
    if !label && ui.hovered(hit) {
        widgets::tooltip(ui, hit, word);
    }
    Some(w)
}

/// The activity indicator alone, centred on `at`: a spinner while working, an alert when it needs you, a check
/// when it's done; nothing when idle.
pub fn status_dot(ui: &mut Ui, at: Point, activity: Option<Activity>, done: bool) {
    let t = ui.t;
    let r = Rect::new(at.x - 8.0, at.y - 8.0, 16.0, 16.0);
    match activity {
        Some(Activity::Working) => ui.spinner(at, 4.6, 1.6, t.accent),
        Some(Activity::Blocked) => {
            ui.ticking = true;
            let k = 0.7 + 0.3 * ((ui.time * 4.0).sin() as f32 * 0.5 + 0.5);
            ui.icon(r, "\u{F0028}", 13.0, t.danger.fade(k));
        }
        Some(Activity::Idle) if done => ui.icon(r, "\u{F012C}", 13.0, t.good),
        _ => {}
    }
}

fn view_icon(name: &str) -> &'static str {
    match name {
        "settings" => "\u{F0493}",
        "stats" => "\u{F0128}",
        "gauge" => "\u{F029A}",
        "account" => "\u{F0004}",
        "proxy" => "\u{F04E1}",
        "bridge" => "\u{F05A9}",
        "rocket" => "\u{F0463}",
        "sound" => "\u{F057E}",
        "skills" => "\u{F0431}",
        _ => "\u{F05AF}",
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_terminal(waker: crate::pane::Waker, ui: &mut Ui, term: &mut crate::panes::term::Term, b: &PaneBox, content: Rect, panes: &mut HashMap<PaneId, PaneState>, pal: &Palette, focused_window: bool, accent: Color) {
    let id = b.id;
    let t = ui.t;
    if let Some(q) = term.queued {
        draw_queued(ui, term, q, content);
        return;
    }
    let area = Rect::new(content.x + 10.0, content.y + 6.0, (content.w - 18.0).max(0.0), (content.h - 10.0).max(0.0));
    let (rows, cols) = tdraw::fit(ui, area);
    let link_waker = waker.clone();
    term.ensure_started(rows, cols, waker);
    if let Some(e) = term.spawn_error() {
        let e = e.to_string();
        let tb = ui.para(&e, Style::new(size::BODY, weight::MEDIUM, t.danger), area.w, Some(6));
        ui.draw(&tb, Point::new(area.x, area.y + 4.0));
        ui.label(Point::new(area.x, area.y + tb.h + 14.0), "enter or esc closes this pane", Style::new(size::SMALL, weight::REGULAR, t.muted));
        return;
    }
    if term.started() {
        term.resize(rows, cols);
    }
    let Some(mirror) = term.mirror().cloned() else { return };
    let Some(m) = Mirror::of(mirror.as_ref()) else { return };
    let st = panes.entry(id).or_default();
    // links follow the screen: recompute when it changed or scrolled, or when the paths on it have been checked
    let seq = m.seq.load(std::sync::atomic::Ordering::Relaxed);
    let checked = st.links_pending && !term.file_cache().busy();
    if seq != st.links_seq || st.links_offset != term.scroll_offset() || checked {
        let screen = m.lock();
        let mut lookup = term.file_cache().lookup();
        st.links = tdraw::find_links(&screen, &mut |f| lookup.get(f));
        drop(screen);
        lookup.finish(Some(link_waker));
        st.links_pending = term.file_cache().busy();
        st.links_seq = seq;
        st.links_offset = term.scroll_offset();
    }
    // a remote viewer grew the pty past the tile: show its bottom-left
    let (pty_rows, _) = term.pty_size();
    let view_top = if pty_rows > rows && term.scroll_offset() == 0 {
        let screen = m.lock();
        let text = tdraw::screen_text(&screen);
        let last = text.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
        let cursor = screen.term.grid().cursor.point.line.0.max(0) as usize;
        drop(screen);
        let end = last.max(cursor) + 1;
        end.saturating_sub(rows as usize).min((pty_rows - rows) as usize)
    } else {
        0
    };
    term.set_view_top(view_top as u16);
    let hover_link = ui.pointer.and_then(|p| {
        let cell = st.drawn.cell_at(p)?;
        st.links.iter().find(|l| l.row == cell.0 + view_top && (l.start..l.end).contains(&cell.1)).cloned()
    });
    let view = tdraw::View {
        area,
        scroll: term.scroll_offset(),
        focused: b.focused && focused_window,
        cursor: true,
        links: &st.links,
        hover_link: hover_link.as_ref(),
        view_top,
    };
    st.drawn = tdraw::draw(ui, m, pal, &view);
    // the program's own mouse use decides the pointer; prompt options light up under it
    let body = Rect::new(content.x, content.y, content.w, content.h);
    ui.hit_with(body, Action::Pane(id, PaneAct::Body), Cursor::Text);
    if let Some(p) = ui.pointer
        && ui.hovered(area)
        && !term.wants_mouse()
        && let Some((row, _)) = st.drawn.cell_at(p)
        && term.prompt_option_at(row as u16).is_some()
    {
        let y = st.drawn.origin.y + row as f32 * st.drawn.cell_h;
        ui.round(Rect::new(area.x - 4.0, y, area.w + 8.0, st.drawn.cell_h), 4.0, accent.with_alpha(0.14));
    }
}

/// A session in focus mode isn't drawn, but it still starts and keeps the size its card gives it (what it shows
/// is read the same way, prompts and all).
fn keep_sized(waker: crate::pane::Waker, ui: &Ui, term: &mut crate::panes::term::Term, content: Rect) {
    let area = Rect::new(content.x + 10.0, content.y + 6.0, (content.w - 18.0).max(0.0), (content.h - 10.0).max(0.0));
    let (rows, cols) = tdraw::fit(ui, area);
    term.ensure_started(rows, cols, waker);
    if term.started() {
        term.resize(rows, cols);
    }
}

/// A resume waiting its turn.
fn draw_queued(ui: &mut Ui, term: &crate::panes::term::Term, q: crate::panes::term::Queued, r: Rect) {
    let t = ui.t;
    let title = term.meta.name.clone().unwrap_or_else(|| term.meta.label.clone());
    let state = if q.starting {
        "starting now".to_string()
    } else if q.ahead == 0 {
        "up next".to_string()
    } else {
        format!("in line · {} ahead", q.ahead)
    };
    let who = [term.meta.label.clone(), term.meta.project.name.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let cy = r.y + r.h * 0.38;
    let x = r.x + 32.0;
    ui.spinner(Point::new(x + 8.0, cy), 7.0, 2.2, t.accent);
    let line = ui.rich(&[("Resuming ".into(), Style::new(size::TITLE, weight::MEDIUM, t.muted)), (title, Style::new(size::TITLE, weight::SEMIBOLD, t.text))], size::TITLE, Some(r.w - 80.0), false, Some(1));
    ui.draw(&line, Point::new(x + 26.0, cy - line.h / 2.0));
    ui.label(Point::new(x + 26.0, cy + 16.0), &format!("{who} · {state}"), Style::new(size::SMALL, weight::REGULAR, t.muted));
    ui.label(Point::new(x + 26.0, cy + 40.0), "Sessions open one at a time so they don't fight over your machine.", Style::new(size::SMALL, weight::REGULAR, t.faint));
}

/// The back of a turned card: what the session is about and where it stands (the terminal UI's card, painted).
fn draw_back(app: &mut App, ui: &mut Ui, id: PaneId, content: Rect, _accent: Color, canvas: &mut TuiCanvas, out: &mut Painted) {
    // its rows open on a press, an alt+double-click turns it face up, a click focuses it (the program under it
    // gets no mouse): the mouse sorts that out from what was painted
    ui.hit_with(content, Action::Pane(id, PaneAct::Body), Cursor::Default);
    let Some(term) = app.panes.get(&id).and_then(|p| p.as_term_ref()) else { return };
    let snap = term.insights();
    let facts = snap.as_ref().and_then(|s| s.facts.as_deref());
    let view = app.pane_view(id).filter(|_| term.meta.harness.is_some());
    let title = if term.meta.renamed { None } else { facts.and_then(|f| f.custom_title.clone().or_else(|| f.ai_title.clone())) };
    let mut urls: Vec<String> = term.screen_urls();
    for u in facts.map(|f| f.urls.as_slice()).unwrap_or_default() {
        if !urls.contains(&u.url) {
            urls.push(u.url.clone());
        }
    }
    let links = urls
        .into_iter()
        .map(|url| {
            let local = bro_core::sessions::live::SeenUrl { url: url.clone(), at_ms: None }.is_local();
            let up = if local { app.probe.up(&url) } else { None };
            crate::card::Link { url, local, up }
        })
        .collect();
    let time = app.start.elapsed().as_secs_f64();
    let card = crate::card::Card {
        title: title.unwrap_or_else(|| term.display_name()),
        who: term.meta.label.clone(),
        cwd: &term.meta.cwd,
        open_for: term.meta.started.elapsed(),
        state: view.as_ref().map(|v| v.state),
        reason: view.as_ref().and_then(|v| v.reason.clone()),
        facts,
        recap: view.and_then(|v| v.recap),
        now_ms: chrono::Utc::now().timestamp_millis(),
        time,
        links,
    };
    let theme = app.theme.clone();
    let color = crate::theme::project_color(&term.meta.project.key, &theme);
    let _ = canvas;
    // the back: a tinted ground, the title in the interface font, the rest on the terminal font's grid so its
    // label column lines up
    let t = ui.t;
    ui.round(content, 0.0, t.text.with_alpha(if t.light { 0.02 } else { 0.025 }));
    let pad = 18.0;
    let inner = Rect::new(content.x + pad, content.y + pad - 4.0, (content.w - 2.0 * pad).max(0.0), (content.h - 2.0 * pad).max(0.0));
    let size = size::SMALL;
    let line_h = (size * 1.65).round();
    let cell_w = ui.measure("0000000000", Style::mono(size, t.text)) / 10.0;
    let cols = (inner.w / cell_w.max(1.0)).floor().max(10.0) as u16;
    let rows = ((inner.h - 10.0) / line_h).floor().max(1.0) as u16;
    let (lines, targets) = crate::card::lines(&card, cols, rows, color, &theme);
    let mut y = inner.y;
    for (i, line) in lines.iter().enumerate() {
        let target = targets.iter().find(|(row, _)| *row == i).map(|(_, uri)| uri.clone());
        let row_r = Rect::new(inner.x - 8.0, y - 2.0, inner.w + 16.0, if i == 0 { 30.0 } else { line_h });
        if let Some(uri) = &target {
            let h = ui.hover_amount(("card-link", id, i), row_r);
            ui.round(row_r, radius::CONTROL, t.selected.mix(t.text.with_alpha(0.12), 0.5 * h));
            out.card_links.push((row_r, id, uri.clone()));
        }
        if i == 0 {
            // the title, larger, in the interface font
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let b = ui.line(text.trim(), Style::new(size::HEADING, weight::SEMIBOLD, t.text), Some(inner.w));
            ui.draw(&b, Point::new(inner.x, y));
            y += 30.0;
            continue;
        }
        let spans: Vec<(String, Style)> = line
            .spans
            .iter()
            .map(|sp| {
                let c = sp.style.fg.map(|c| super::kit::color(c, t.text, &t.ansi)).unwrap_or(t.text);
                let mut st = Style::mono(size, c);
                if sp.style.add_modifier.contains(ratatui::style::Modifier::BOLD) {
                    st = st.weight(weight::BOLD);
                }
                if sp.style.add_modifier.contains(ratatui::style::Modifier::DIM) {
                    st = st.color(c.fade(0.6));
                }
                (sp.content.to_string(), st)
            })
            .collect();
        if !spans.is_empty() {
            let b = ui.rich(&spans, size, Some(inner.w), false, Some(1));
            ui.draw(&b, Point::new(inner.x, y + (line_h - b.h) / 2.0));
        }
        y += line_h;
    }
}

/// Grips on the gaps between cards: the split divider or tile edge under the pointer lights up, and tile edges
/// you've held keep a small grip (double-click one to let it go), as the terminal UI's ↔ / ↕ handles do.
pub fn draw_edges(app: &App, ui: &mut Ui, layout: &Layout) {
    if app.overlay.is_open() {
        return;
    }
    let t = ui.t;
    let gap = super::layout::GAP;
    // the side of `id`'s card on axis `ax` (0 = x), far or near, as a thin rect in the gap (on the line it shares
    // with its neighbour, with shared borders)
    let side = |id: PaneId, ax: usize, far: bool| -> Option<Rect> {
        let b = layout.pane(id)?;
        let c = b.card;
        let off = |joined: bool| if joined { -0.5 } else { gap / 2.0 };
        Some(match (ax, far) {
            (0, true) => Rect::new(c.right() + off(b.joined.right) - 1.5, c.y + 10.0, 3.0, c.h - 20.0),
            (0, false) => Rect::new(c.x - off(b.joined.left) - 1.5, c.y + 10.0, 3.0, c.h - 20.0),
            (_, true) => Rect::new(c.x + 10.0, c.bottom() + off(b.joined.bottom) - 1.5, c.w - 20.0, 3.0),
            (_, false) => Rect::new(c.x + 10.0, c.y - off(b.joined.top) - 1.5, c.w - 20.0, 3.0),
        })
    };
    // held edges: a short grip at the middle
    for l in &app.locks {
        if let Some(r) = side(l.pane, l.ax, l.far) {
            let grip = if l.ax == 0 { Rect::new(r.x, r.y + r.h / 2.0 - 14.0, r.w, 28.0) } else { Rect::new(r.x + r.w / 2.0 - 14.0, r.y, 28.0, r.h) };
            ui.round(grip, 1.5, t.muted.with_alpha(0.7));
        }
    }
    let Some(p) = ui.pointer else { return };
    // in a gutter, or on a line two tiles share
    let on_line = layout.panes.iter().any(|b| b.edge_strips().iter().any(|(r, _)| r.contains(p)));
    if !layout.body.outset(gap).contains(p) || (!on_line && layout.panes.iter().any(|b| b.card.contains(p))) {
        return;
    }
    if let Some((path, dir, area)) = super::mouse::divider_near(app, layout, p) {
        let t2 = app.tabs.get(app.cur);
        if let Some(ratio) = t2.and_then(|t2| t2.root.ratio_at(&path)) {
            let (a, _) = crate::layout::split_rect(area, dir, ratio);
            let (line, whole) = (layout.to_px(a), layout.to_px(area));
            let r = match dir {
                crate::layout::Dir::Right => Rect::new(line.right() - 1.5, whole.y + 10.0, 3.0, whole.h - 20.0),
                crate::layout::Dir::Down => Rect::new(whole.x + 10.0, line.bottom() - 1.5, whole.w - 20.0, 3.0),
            };
            ui.round(r, 1.5, t.accent.with_alpha(0.75));
        }
        return;
    }
    if let Some((id, ax, far)) = super::mouse::tile_edge_at(app, layout, p)
        && let Some(r) = side(id, ax, far)
    {
        ui.round(r, 1.5, t.accent.with_alpha(0.75));
    }
}
