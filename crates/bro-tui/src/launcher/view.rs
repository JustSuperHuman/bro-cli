//! Drawing the launcher. One row holds the New / Continue switch and the project the session starts in; then
//! the agents as chips (and ★ orchestrator, gold; on its tab a second row of chips picks its harness); then the
//! "run on" list on the left and, beside it, the launch panel: what will run,
//! the model list when one is needed, the permissions / browser / placement switches and the Launch button.
//! Narrow screens stack the switches under the list instead.

use super::{AccountKind, Focus, Hit, Item, Launcher, SessionTab};
use crate::theme::{self, Theme};
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::launch::Permission;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Draw the launcher filling `area`: the body, where a view pane would go.
pub fn draw(f: &mut Frame, area: Rect, l: &mut Launcher, t: &Theme, time: f64) {
    let mut hits = vec![];
    draw_inner(f, area, l, t, time, &mut hits);
    l.hits = hits;
    l.area = area;
    if let Some(p) = &mut l.project_picker {
        crate::folder::draw_for_session(f, area, p, &l.dir, t);
    }
}

/// The panel beside the list needs this much room; below it the switches go under the list.
const SIDE_MIN_WIDTH: u16 = 84;
/// Rows the switches (heading, three rows) and the Launch button take, with a blank row between.
const SETTINGS_H: u16 = 6;

fn draw_inner(f: &mut Frame, r: Rect, l: &mut Launcher, t: &Theme, _time: f64, hits: &mut Vec<(Rect, Hit)>) {
    ui::clear(f, r, t);
    let heading = if l.tab == SessionTab::Continue { "Continue a session" } else { "New session" };
    let title = Line::from(vec![Span::raw(" "), Span::styled(heading, ui::bold_accent(t)), Span::raw(" ")]);
    let inner = ui::frame_ex(f, r, title, None, None, true, t);
    // the corner: profiles, and a close button (esc does the same)
    let close = Rect { x: r.right().saturating_sub(5), y: r.y, width: 3, height: 1 };
    ui::line(f, close, vec![Span::styled(" × ", fg(t.danger).add_modifier(Modifier::BOLD))]);
    hits.push((close, Hit::Close));
    let manage = [(" profiles ", fg(t.fg)), ("alt+o ", key_style(t))];
    let mw: u16 = manage.iter().map(|(s, _)| ui::width(s) as u16).sum();
    if r.width >= mw + 36 {
        let at = Rect { x: close.x.saturating_sub(mw + 1), y: r.y, width: mw, height: 1 };
        ui::line(f, at, manage.iter().map(|(s, st)| Span::styled(*s, *st)).collect());
        hits.push((at, Hit::Profiles));
    }
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let row = |y: u16| Rect { y, height: 1, ..inner };
    let mut y = inner.y;

    // the New / Continue switch, and the project the session starts in
    let switch_end = draw_mode_switch(f, row(y), l, t, hits);
    let chip = Rect { x: switch_end + 2, width: inner.right().saturating_sub(switch_end + 2), ..row(y) };
    draw_project_chip(f, chip, l, t, hits);
    y += 2;
    if l.tab == SessionTab::Continue {
        let body = Rect { y, height: inner.bottom().saturating_sub(y), ..inner };
        crate::continue_picker::draw_in(f, body, &mut l.continue_picker, t);
        for (rect, all) in &l.continue_picker.scope_hits {
            hits.push((*rect, Hit::ContinueScope(*all)));
        }
        for (rect, index) in &l.continue_picker.hits {
            hits.push((*rect, Hit::ContinueRow(*index)));
        }
        return;
    }

    // the agents, and on the orchestrator tab the harness it runs on
    draw_agents(f, row(y), l, t, hits);
    y += 1;
    if l.orchestrator {
        draw_brains(f, row(y), l, t, hits);
        y += 1;
    }
    if !l.installed(l.harness) {
        let text = format!("{} isn't installed yet · enter or click here to install it", l.harness.label());
        hits.push((Rect { width: ui::width(&text) as u16 + 2, ..row(y) }, Hit::Install));
        ui::line(f, row(y), vec![Span::styled("↓ ", fg(t.danger)), Span::styled(text, fg(t.danger))]);
    }
    y += 2;

    // the list, and the launch panel beside it (or the switches under it when there's no room)
    let hints_y = inner.bottom().saturating_sub(1);
    let body = Rect { y, height: hints_y.saturating_sub(y + 1), ..inner };
    if body.width >= SIDE_MIN_WIDTH && body.height >= SETTINGS_H + 5 {
        // a model list wants the room more than the (by then filtered) list does
        let panel_w = if l.needs_model() { (body.width * 11 / 20).clamp(48, 66) } else { (body.width * 2 / 5).clamp(40, 48) };
        let list = Rect { width: body.width - panel_w - 3, ..body };
        let gap_x = list.right() + 1;
        for gy in body.top()..body.bottom() {
            ui::line(f, Rect { x: gap_x, y: gy, width: 1, height: 1 }, vec![Span::styled("│", fg(t.frame))]);
        }
        draw_run_on(f, list, l, t, hits);
        draw_panel(f, Rect { x: gap_x + 2, width: panel_w, ..body }, l, t, hits);
    } else {
        draw_stacked(f, body, l, t, hits);
    }

    if hints_y > y {
        let enter = match l.focus {
            Focus::List if l.needs_model() => "pick a model",
            _ => "launch",
        };
        let enter = if l.orchestrator && l.data.orchestrator_running.is_some() { "go to it" } else { enter };
        let mut hints = vec![("⏎", enter), ("↑↓", "move"), ("←→", "agent"), ("type", if l.focus == Focus::List { "project" } else { "filter" })];
        if l.orchestrator {
            hints.insert(3, ("shift+←→", "runs on"));
        }
        if l.needs_model() {
            hints.push(("tab", "list ⇄ models"));
        }
        hints.push(("alt+p", "project"));
        hints.push(("esc", if l.focus == Focus::List { "close" } else { "back" }));
        ui::line(f, row(hints_y), ui::hints(&hints, t));
        hits.push((Rect { x: inner.x, y: hints_y, width: 3 + ui::width(enter) as u16, height: 1 }, Hit::Launch));
    }
}

fn key_style(t: &Theme) -> Style {
    fg(t.shine).add_modifier(Modifier::BOLD)
}

/// A soft ground for a chip or a selected row in `c`.
fn tint(c: Color, t: &Theme) -> Color {
    theme::project_tint(c, t)
}

/// Fill a row's background (a selected row, a button).
fn fill(f: &mut Frame, r: Rect, bg: Color) {
    f.buffer_mut().set_style(r, Style::default().bg(bg));
}

/// `+ New` and `↻ Continue` as one segmented switch; returns where it ends.
fn draw_mode_switch(f: &mut Frame, r: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) -> u16 {
    let mut x = r.x;
    for (tab, label) in [(SessionTab::New, " + New "), (SessionTab::Continue, " ↻ Continue ")] {
        let on = l.tab == tab;
        let style = if on {
            Style::default().fg(Color::Black).bg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.muted).bg(tint(t.accent, t))
        };
        let w = ui::width(label) as u16;
        if x + w > r.right() {
            break;
        }
        let at = Rect { x, width: w, ..r };
        ui::line(f, at, vec![Span::styled(label, style)]);
        hits.push((at, Hit::Tab(tab)));
        x += w;
    }
    x
}

/// The project as a chip on the right of the top row: its name, its path, and the key that changes it.
fn draw_project_chip(f: &mut Frame, r: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    if r.width < 12 {
        return;
    }
    // the orchestrator always runs in its own folder in your profile
    if l.orchestrator {
        let gold = theme::gold(t);
        let ground = tint(gold, t);
        let dir = bro_core::orchestrator::workspace();
        let spans = vec![
            Span::styled(format!(" {} ", ui::icon("folder")), fg(gold).bg(ground)),
            Span::styled("runs in ", fg(t.muted).bg(ground)),
            Span::styled(format!("{} ", crate::util::short_path(&dir, 40)), fg(gold).bg(ground).add_modifier(Modifier::BOLD)),
        ];
        let w = spans.iter().map(|s| ui::width(&s.content) as u16).sum::<u16>().min(r.width);
        ui::line(f, Rect { x: r.right() - w, width: w, ..r }, spans);
        return;
    }
    let name = l.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| l.dir.to_string_lossy().into_owned());
    let ground = tint(t.shine, t);
    let icon = format!(" {} ", ui::icon("folder"));
    let action = "  change alt+p ";
    let fixed = ui::width(&icon) + ui::width(&name) + ui::width(action) + 1;
    let room = (r.width as usize).saturating_sub(fixed + 2);
    let path = if room >= 20 { crate::util::short_path(&l.dir, room.min(48)) } else { String::new() };
    let mut spans = vec![Span::styled(icon, fg(t.shine).bg(ground)), Span::styled(name, fg(t.fg).bg(ground).add_modifier(Modifier::BOLD))];
    if !path.is_empty() {
        spans.push(Span::styled(format!("  {path}"), fg(t.muted).bg(ground)));
    }
    spans.push(Span::styled("  change ", fg(t.muted).bg(ground)));
    spans.push(Span::styled("alt+p ", key_style(t).bg(ground)));
    let w = spans.iter().map(|s| ui::width(&s.content) as u16).sum::<u16>().min(r.width);
    let at = Rect { x: r.right() - w, width: w, ..r };
    ui::line(f, at, spans);
    hits.push((at, Hit::Project));
}

/// The agents as chips: the chosen one filled in its brand color, the rest tinted, uninstalled ones dim.
fn draw_agents(f: &mut Frame, r: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let mut x = r.x;
    for h in Harness::ALL {
        let on = h == l.harness && !l.orchestrator;
        let brand = ui::harness_color(Some(h), t);
        let text = format!("  {} {}  ", ui::harness_glyph(Some(h)), h.label());
        let style = if on {
            Style::default().fg(Color::Black).bg(brand).add_modifier(Modifier::BOLD)
        } else if l.installed(h) {
            Style::default().fg(brand).bg(tint(brand, t))
        } else {
            Style::default().fg(t.muted).bg(tint(t.muted, t))
        };
        let w = ui::width(&text) as u16;
        if x + w > r.right() {
            break;
        }
        let at = Rect { x, width: w, ..r };
        ui::line(f, at, vec![Span::styled(text, style)]);
        hits.push((at, Hit::Harness(h)));
        x += w + 1;
    }
    // ★ orchestrator, set a little apart in gold
    let gold = theme::gold(t);
    let text = if l.data.orchestrator_running.is_some() { "  ★ orchestrator ● " } else { "  ★ orchestrator  " };
    let style = if l.orchestrator {
        Style::default().fg(Color::Black).bg(gold).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(gold).bg(tint(gold, t)).add_modifier(Modifier::BOLD)
    };
    let w = ui::width(text) as u16;
    let x = x + 1;
    if x + w <= r.right() {
        let at = Rect { x, width: w, ..r };
        ui::line(f, at, vec![Span::styled(text, style)]);
        hits.push((at, Hit::Orchestrator));
    }
}

/// The orchestrator tab's second row: which harness it runs on, as small chips.
fn draw_brains(f: &mut Frame, r: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let gold = theme::gold(t);
    let lead = "  runs on ";
    ui::line(f, Rect { width: (ui::width(lead) as u16).min(r.width), ..r }, vec![Span::styled(lead, fg(gold))]);
    let mut x = r.x + ui::width(lead) as u16;
    for h in Harness::ALL {
        let on = h == l.harness;
        let brand = ui::harness_color(Some(h), t);
        let text = format!(" {} {} ", ui::harness_glyph(Some(h)), h.label());
        let style = if on {
            Style::default().fg(Color::Black).bg(brand).add_modifier(Modifier::BOLD)
        } else if l.installed(h) {
            Style::default().fg(brand)
        } else {
            muted(t)
        };
        let w = ui::width(&text) as u16;
        if x + w > r.right() {
            break;
        }
        let at = Rect { x, width: w, ..r };
        ui::line(f, at, vec![Span::styled(text, style)]);
        hits.push((at, Hit::Brain(h)));
        x += w + 1;
    }
    let note = match l.harness {
        Harness::Claude | Harness::Codex => "  tools built in",
        Harness::Pi | Harness::Omp => "  tools through bro ctl",
    };
    if x + ui::width(note) as u16 <= r.right() {
        ui::line(f, Rect { x, width: ui::width(note) as u16, ..r }, vec![Span::styled(note, muted(t))]);
    }
}

/// A section heading: a quiet label and a hairline to the edge.
fn heading(f: &mut Frame, r: Rect, label: &str, t: &Theme) {
    let rule = (r.width as usize).saturating_sub(ui::width(label) + 1);
    ui::line(f, r, vec![Span::styled(label.to_string(), muted(t).add_modifier(Modifier::BOLD)), Span::styled(format!(" {}", "─".repeat(rule)), fg(t.frame))]);
}

/// A filter line: "› text▏" or a muted prompt.
fn filter_line(f: &mut Frame, r: Rect, q: &str, prompt: &str, focused: bool, t: &Theme) {
    let mut spans = vec![Span::styled("› ", if focused { ui::bold_accent(t) } else { muted(t) })];
    if q.is_empty() {
        spans.push(Span::styled(prompt.to_string(), muted(t)));
    } else {
        spans.push(Span::styled(q.to_string(), ui::bold()));
    }
    if focused {
        spans.push(Span::styled("▏", ui::accent(t)));
    }
    ui::line(f, r, spans);
}

/// Scroll so `cursor` stays visible in `rows` lines.
fn window(cursor: usize, rows: usize) -> usize {
    cursor.saturating_sub(rows.saturating_sub(1))
}

/// A list row's selection look: a filled bar and accent bar when the list has focus, a quiet marker otherwise.
fn select_row(f: &mut Frame, r: Rect, focused: bool, t: &Theme) -> Span<'static> {
    if focused {
        fill(f, r, tint(t.accent, t));
        Span::styled("▌", ui::accent(t))
    } else {
        Span::styled("›", muted(t))
    }
}

fn draw_run_on(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let focused = l.focus == Focus::List;
    filter_line(f, Rect { height: 1, ..area }, &l.list_filter, "run on… ↑↓ to pick · type a project", focused, t);
    let items = l.items();
    let top = area.y + 2;
    let rows = area.bottom().saturating_sub(top) as usize;
    let cursor = l.cursor_row(&items);
    let start = window(cursor.unwrap_or(0), rows);
    // usage meters when the list is wide enough for them
    let meter = if area.width >= 56 { 10 } else if area.width >= 44 { 6 } else { 0 };
    for (k, item) in items.iter().enumerate().skip(start).take(rows) {
        let r = Rect { y: top + (k - start) as u16, height: 1, ..area };
        let on = cursor == Some(k);
        if !matches!(item, Item::Header(_)) {
            let sel_idx = items[..k].iter().filter(|i| !matches!(i, Item::Header(_))).count();
            hits.push((r, Hit::Row(sel_idx)));
        }
        let mark = if on { select_row(f, r, focused, t) } else { Span::raw(" ") };
        let label_style = if on { ui::bold_accent(t) } else { Style::default().fg(t.fg) };
        match item {
            Item::Header(h) => heading(f, Rect { x: r.x + 1, width: r.width.saturating_sub(1), ..r }, h, t),
            Item::Recent(i) => {
                let rc = &l.data.recents[*i];
                let who = rc.provider_id.clone().or_else(|| rc.profile_id.as_ref().map(|p| p.split(':').next_back().unwrap_or(p).to_string())).unwrap_or_else(|| "own login".into());
                let model = rc.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m)));
                let kind = match (&rc.provider_id, &rc.profile_id) {
                    (Some(p), _) if p == "pool" => AccountKind::Pool,
                    (Some(p), _) => AccountKind::Provider(p.clone()),
                    (None, Some(p)) => AccountKind::Profile(p.clone()),
                    (None, None) => AccountKind::Native,
                };
                let (g, c) = account_glyph(&kind, l.harness, t);
                let mut spans = vec![mark, Span::raw(" "), Span::styled(format!("{g} "), fg(c)), Span::styled(who, label_style)];
                if let Some(m) = model {
                    spans.push(Span::styled(format!("  {m}"), muted(t)));
                }
                let age = crate::util::short_dur((crate::util::now_secs() - rc.at).max(0) as u64);
                ui::line_lr(f, r, spans, vec![Span::styled(format!("{age} ago "), muted(t))]);
            }
            Item::Account(a) => {
                let (g, c) = account_glyph(&a.kind, l.harness, t);
                let glyph = (format!("{g} "), if a.ready { c } else { t.muted });
                let label_style = if on || a.ready { label_style } else { muted(t) };
                let mut right = vec![];
                if let Some(left) = a.left {
                    if meter > 0 {
                        right.extend(ui::left_meter(left, meter, t));
                    }
                    right.push(Span::styled(format!(" {left:>3.0}% left "), fg(ui::left_color(left, t))));
                } else if !a.ready {
                    right.push(Span::styled("not signed in ", muted(t)));
                }
                let left = vec![mark, Span::raw(" "), Span::styled(glyph.0, fg(glyph.1)), Span::styled(format!("{}  ", a.label), label_style), Span::styled(a.detail.clone(), muted(t))];
                ui::line_lr(f, r, left, right);
            }
        }
    }
    if items.is_empty() {
        ui::line(f, Rect { y: top, height: 1, ..area }, vec![Span::styled("   nothing matches · esc clears the filter", muted(t))]);
    }
}

/// The brand mark for a run-on row: Claude / Codex logins show their logo, providers theirs (see
/// [`ui::provider_glyph`]), the pool the Claude mark (it's Claude accounts), a harness's own login its own mark.
fn account_glyph(kind: &AccountKind, harness: Harness, t: &Theme) -> (&'static str, Color) {
    let brand = |h: Harness| (ui::harness_glyph(Some(h)), ui::harness_color(Some(h), t));
    match kind {
        AccountKind::Profile(p) if p.starts_with("codex:") => brand(Harness::Codex),
        AccountKind::Profile(_) | AccountKind::Pool => brand(Harness::Claude),
        AccountKind::Provider(id) => ui::provider_glyph(id, t.shine, t),
        AccountKind::Native => brand(harness),
    }
}

fn draw_models(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let focused = l.focus == Focus::Models;
    let models = l.models();
    let idx = l.models_view();
    let prompt = format!("model… {} to pick from", models.len());
    filter_line(f, Rect { height: 1, ..area }, &l.model_filter, &prompt, focused, t);
    let top = area.y + 1;
    let rows = area.bottom().saturating_sub(top) as usize;
    let start = window(l.model_sel, rows);
    for (k, &i) in idx.iter().enumerate().skip(start).take(rows) {
        let m = &models[i];
        let r = Rect { y: top + (k - start) as u16, height: 1, ..area };
        let on = k == l.model_sel;
        hits.push((r, Hit::Model(k)));
        let mark = if on { select_row(f, r, focused, t) } else { Span::raw(" ") };
        let mut right = vec![];
        if let Some(c) = m.context {
            right.push(Span::styled(format!("{} ", short_ctx(c)), muted(t)));
        }
        if let Some((a, b)) = m.pricing {
            right.push(Span::styled(format!("${}/{} ", short_price(a), short_price(b)), muted(t)));
        }
        let style = if on { ui::bold_accent(t) } else { Style::default().fg(t.fg) };
        ui::line_lr(f, r, vec![mark, Span::raw(" "), Span::styled(m.id.clone(), style)], right);
    }
    if idx.is_empty() {
        let msg = if models.is_empty() { "   no models known for this" } else { "   nothing matches" };
        ui::line(f, Rect { y: top, height: 1, ..area }, vec![Span::styled(msg, muted(t))]);
    }
}

/// The launch panel: what will run and its options, or the model list above the options when one is needed;
/// the Launch button sits on the bottom row, level with the end of the list.
fn draw_panel(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let button_y = area.bottom().saturating_sub(1);
    let options_y = if l.needs_model() {
        let y = button_y.saturating_sub(SETTINGS_H - 1);
        draw_models(f, Rect { height: y.saturating_sub(area.y + 1), ..area }, l, t, hits);
        y
    } else {
        draw_summary(f, Rect { height: 5, ..area }, l, t);
        (area.y + 6).min(button_y.saturating_sub(SETTINGS_H - 1))
    };
    draw_options(f, Rect { y: options_y, height: 4, ..area }, l, t, hits);
    draw_button(f, Rect { y: button_y, height: 1, ..area }, l, t, hits);
}

/// What will run: the agent and login in the agent's color, the login's plan, and how much of it is left.
fn draw_summary(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme) {
    if area.height == 0 {
        return;
    }
    let row = |i: u16| Rect { y: area.y + i, height: 1, ..area };
    heading(f, row(0), "launches", t);
    if l.orchestrator {
        return draw_orchestrator_summary(f, area, l, t);
    }
    let brand = ui::harness_color(Some(l.harness), t);
    let Some(spec) = l.spec() else {
        let msg = if l.installed(l.harness) { "pick where it runs on the left" } else { "install it first" };
        ui::line(f, row(2), vec![Span::styled(msg, muted(t))]);
        return;
    };
    ui::line(f, row(2), vec![
        Span::styled(format!("{} ", ui::harness_glyph(Some(spec.harness))), fg(brand)),
        Span::styled(ui::fit(&crate::services::launch::label_for(&spec), area.width.saturating_sub(2) as usize), fg(brand).add_modifier(Modifier::BOLD)),
    ]);
    let items = l.items();
    let chosen = l.cursor_row(&items).and_then(|k| items.get(k));
    if let Some(Item::Account(a)) = chosen {
        if !a.detail.is_empty() && area.height > 3 {
            ui::line(f, row(3), vec![Span::raw("  "), Span::styled(ui::fit(&a.detail, area.width.saturating_sub(2) as usize), muted(t))]);
        }
        if let Some(left) = a.left.filter(|_| area.height > 4) {
            let n = (area.width as usize).saturating_sub(14).min(24);
            let mut spans = vec![Span::raw("  ")];
            spans.extend(ui::left_meter(left, n, t));
            spans.push(Span::styled(format!(" {left:.0}% left"), fg(ui::left_color(left, t))));
            ui::line(f, row(4), spans);
        }
    } else if let Some(Item::Recent(_)) = chosen {
        ui::line(f, row(3), vec![Span::raw("  "), Span::styled("the way you last ran it here", muted(t))]);
    }
}

/// The orchestrator tab's summary: what it is, where it runs, and whether one is already up.
fn draw_orchestrator_summary(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme) {
    let row = |i: u16| Rect { y: area.y + i, height: 1, ..area };
    let gold = theme::gold(t);
    let w = area.width.saturating_sub(2) as usize;
    if let Some(running) = &l.data.orchestrator_running {
        ui::line(f, row(1), vec![Span::styled("★ ", fg(gold)), Span::styled(ui::fit(running, w), fg(gold).add_modifier(Modifier::BOLD))]);
        ui::line(f, row(2), vec![Span::raw("  "), Span::styled("Already running. There's only ever one.", fg(t.fg))]);
        if area.height > 3 {
            ui::line(f, row(3), vec![Span::raw("  "), Span::styled("⏎ takes you to it.", muted(t))]);
        }
        return;
    }
    let what = match l.spec() {
        Some(spec) => crate::services::launch::label_for(&spec),
        None => format!("orchestrator · {}", l.harness.label()),
    };
    ui::line(f, row(1), vec![Span::styled("★ ", fg(gold)), Span::styled(ui::fit(&what, w), fg(gold).add_modifier(Modifier::BOLD))]);
    ui::line(f, row(2), vec![Span::raw("  "), Span::styled(ui::fit("Opens sessions and hands them work,", w), fg(t.fg))]);
    if area.height > 3 {
        ui::line(f, row(3), vec![Span::raw("  "), Span::styled(ui::fit("then tells you how it went.", w), fg(t.fg))]);
    }
    let items = l.items();
    let left = match l.cursor_row(&items).and_then(|k| items.get(k)) {
        Some(Item::Account(a)) => a.left,
        _ => None,
    };
    if let Some(left) = left.filter(|_| area.height > 4) {
        let n = (area.width as usize).saturating_sub(14).min(24);
        let mut spans = vec![Span::raw("  ")];
        spans.extend(ui::left_meter(left, n, t));
        spans.push(Span::styled(format!(" {left:.0}% left"), fg(ui::left_color(left, t))));
        ui::line(f, row(4), spans);
    }
}

/// Permissions, browser and placement as labelled rows with their keys, under an "options" heading.
fn draw_options(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    heading(f, Rect { height: 1, ..area }, "options", t);
    let row = |i: u16| Rect { y: area.y + 1 + i, height: 1, ..area };
    const LABEL: u16 = 13;
    let label = |s: &str| Span::styled(format!("{s:<w$}", w = LABEL as usize), muted(t));
    let key = |s: &str| vec![Span::styled(format!("{s} "), key_style(t))];

    // permissions: three pills
    let r = row(0);
    let mut spans = vec![label("permissions")];
    let mut x = r.x + LABEL;
    for (p, name) in [(Permission::Default, "ask"), (Permission::Auto, "auto"), (Permission::Skip, "skip all")] {
        let on = l.permission == p;
        let c = if p == Permission::Skip { t.danger } else { t.accent };
        let text = format!(" {name} ");
        let w = ui::width(&text) as u16;
        hits.push((Rect { x, width: w, ..r }, Hit::Perm(p)));
        x += w + 1;
        spans.push(Span::styled(text, if on { Style::default().fg(t.bg).bg(c).add_modifier(Modifier::BOLD) } else { muted(t) }));
        spans.push(Span::raw(" "));
    }
    ui::line_lr(f, r, spans, key("^e"));

    // browser
    let r = row(1);
    let (b, on) = match l.browser {
        BrowserMode::Off => ("off", false),
        BrowserMode::Auto => ("on", true),
        BrowserMode::Edge => ("edge", true),
        BrowserMode::Chrome => ("chrome", true),
    };
    ui::line_lr(f, r, vec![label("browser"), Span::styled(format!(" {b} "), if on { ui::bold_accent(t) } else { muted(t) })], key("^b"));
    hits.push((r, Hit::Browser));

    // placement
    let r = row(2);
    if l.orchestrator {
        ui::line(f, r, vec![label("opens as"), Span::styled(" its own, pinned on top ", fg(theme::gold(t)).add_modifier(Modifier::BOLD))]);
    } else {
        let place = if l.place == crate::pane::Place::Tab { "new session" } else { "split beside this one" };
        ui::line_lr(f, r, vec![label("opens as"), Span::styled(format!(" {place} "), ui::bold_accent(t))], key("^s"));
        hits.push((r, Hit::Place));
    }
}

/// The Launch button, filled in the agent's color once there's something to launch.
fn draw_button(f: &mut Frame, r: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let brand = if l.orchestrator { theme::gold(t) } else { ui::harness_color(Some(l.harness), t) };
    let (text, style) = match l.spec() {
        Some(_) if l.orchestrator && l.data.orchestrator_running.is_some() => ("⏎  Go to the orchestrator".to_string(), Style::default().fg(Color::Black).bg(brand).add_modifier(Modifier::BOLD)),
        Some(s) if l.orchestrator => (format!("⏎  Start it on {}", crate::services::launch::label_for(&s).trim_start_matches("orchestrator · ")), Style::default().fg(Color::Black).bg(brand).add_modifier(Modifier::BOLD)),
        Some(s) => (format!("⏎  Launch {}", crate::services::launch::label_for(&s)), Style::default().fg(Color::Black).bg(brand).add_modifier(Modifier::BOLD)),
        None if !l.installed(l.harness) => (format!("⏎  Install {}", l.harness.label()), Style::default().fg(t.bg).bg(t.danger).add_modifier(Modifier::BOLD)),
        None if l.needs_model() => ("pick a model to launch".to_string(), Style::default().fg(t.muted).bg(tint(t.muted, t))),
        None => ("pick where it runs".to_string(), Style::default().fg(t.muted).bg(tint(t.muted, t))),
    };
    fill(f, r, style.bg.unwrap_or(t.bg));
    let text = ui::fit(&text, r.width.saturating_sub(2) as usize);
    let tw = ui::width(&text) as u16;
    ui::line(f, Rect { x: r.x + (r.width.saturating_sub(tw)) / 2, width: tw.min(r.width), ..r }, vec![Span::styled(text, style)]);
    hits.push((r, if l.installed(l.harness) { Hit::Launch } else { Hit::Install }));
}

/// Narrow screens: the list (and models beside it), then what will run and the switches in two rows.
fn draw_stacked(f: &mut Frame, body: Rect, l: &Launcher, t: &Theme, hits: &mut Vec<(Rect, Hit)>) {
    let list_h = body.height.saturating_sub(3);
    let list = Rect { height: list_h, ..body };
    let needs = l.needs_model();
    let left_w = if needs { list.width * 45 / 100 } else { list.width };
    if list_h > 0 {
        draw_run_on(f, Rect { width: left_w, ..list }, l, t, hits);
        if needs {
            draw_models(f, Rect { x: list.x + left_w + 1, width: list.width.saturating_sub(left_w + 1), ..list }, l, t, hits);
        }
    }
    let mut y = body.y + list_h;
    let row = |y: u16| Rect { y, height: 1, ..body };
    if y >= body.bottom() {
        return;
    }
    ui::rule(f, row(y), "", t);
    y += 1;
    let summary = match l.spec() {
        Some(s) => {
            let brand = ui::harness_color(Some(s.harness), t);
            vec![Span::styled("→ ", muted(t)), Span::styled(crate::services::launch::label_for(&s), Style::default().fg(brand).add_modifier(Modifier::BOLD))]
        }
        None if needs => vec![Span::styled("→ pick a model", fg(t.shine))],
        None => vec![Span::styled("→ pick where to run", muted(t))],
    };
    if y < body.bottom() {
        ui::line(f, row(y), summary);
        y += 1;
    }
    if y >= body.bottom() {
        return;
    }
    let r = row(y);
    let mut toggles = vec![];
    let mut x = r.x;
    for (p, name) in [(Permission::Default, "ask"), (Permission::Auto, "auto"), (Permission::Skip, "skip all")] {
        let on = l.permission == p;
        let c = if p == Permission::Skip { t.danger } else { t.accent };
        let text = format!(" {name} ");
        let w = ui::width(&text) as u16;
        hits.push((Rect { x, width: w, ..r }, Hit::Perm(p)));
        x += w;
        toggles.push(Span::styled(text, if on { Style::default().fg(t.bg).bg(c).add_modifier(Modifier::BOLD) } else { muted(t) }));
    }
    let b = match l.browser {
        BrowserMode::Off => "browser off",
        BrowserMode::Auto => "browser on",
        BrowserMode::Edge => "edge",
        BrowserMode::Chrome => "chrome",
    };
    let place = if l.place == crate::pane::Place::Tab { "new session" } else { "split" };
    for (text, hit) in [(format!("  {b}"), Hit::Browser), (format!("  {place}"), Hit::Place)] {
        let w = ui::width(&text) as u16;
        hits.push((Rect { x, width: w, ..r }, hit));
        x += w;
        toggles.push(Span::styled(text, ui::bold_accent(t)));
    }
    ui::line(f, r, toggles);
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
