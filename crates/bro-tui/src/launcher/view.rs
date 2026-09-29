//! Drawing the launcher: harness tabs, the "run on" list (with the model list beside it only when needed),
//! the project line, toggles and key hints.

use super::{AccountKind, Focus, Item, Launcher};
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::launch::Permission;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Clear,
};

/// Draw the launcher centered on `screen`.
pub fn draw(f: &mut Frame, screen: Rect, l: &Launcher, t: &Theme, _time: f64) {
    let r = ui::centered(screen, 100, 26);
    f.render_widget(Clear, r);
    let title = Line::from(vec![Span::raw(" "), Span::styled("new session", ui::bold_accent(t)), Span::raw(" ")]);
    let inner = ui::frame_ex(f, r, title, None, None, true, t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let row = |y: u16| Rect { y, height: 1, ..inner };
    let mut y = inner.y;

    // 1. harness tabs
    let mut tabs = vec![];
    for h in Harness::ALL {
        let on = h == l.harness;
        let brand = ui::harness_color(Some(h), t);
        let text = format!(" {} {} ", ui::harness_glyph(Some(h)), h.label());
        let style = if on {
            Style::default().fg(ratatui::style::Color::Black).bg(brand).add_modifier(Modifier::BOLD)
        } else if l.installed(h) {
            fg(brand)
        } else {
            muted(t)
        };
        tabs.push(Span::styled(text, style));
        tabs.push(Span::raw("  "));
    }
    ui::line_lr(f, row(y), tabs, vec![Span::styled("← → agent ", muted(t))]);
    y += 1;
    if !l.installed(l.harness) {
        ui::line(f, row(y), vec![Span::styled(format!("  {} isn't on your PATH — install it first", l.harness.label()), fg(t.danger))]);
    }
    y += 1;

    // 2. lists
    let list_h = inner.height.saturating_sub(9);
    let body = Rect { y, height: list_h, ..inner };
    let needs = l.needs_model();
    let left_w = if needs { body.width * 45 / 100 } else { body.width };
    draw_run_on(f, Rect { width: left_w, ..body }, l, t);
    if needs {
        draw_models(f, Rect { x: body.x + left_w + 1, width: body.width.saturating_sub(left_w + 1), ..body }, l, t);
    }
    y += list_h;

    // 3. project + summary + toggles
    ui::rule(f, row(y), "", t);
    y += 1;
    ui::line_lr(
        f,
        row(y),
        vec![Span::styled("in ", muted(t)), Span::styled(crate::util::short_path(&l.dir, 70), ui::bold())],
        vec![Span::styled("pick another project in the sidebar ", muted(t))],
    );
    y += 1;
    let summary = match l.spec() {
        Some(s) => {
            let brand = ui::harness_color(Some(s.harness), t);
            vec![Span::styled("→ ", muted(t)), Span::styled(crate::services::launch::label_for(&s), Style::default().fg(brand).add_modifier(Modifier::BOLD))]
        }
        None if l.needs_model() => vec![Span::styled("→ pick a model", fg(t.shine))],
        None => vec![Span::styled("→ pick where to run", muted(t))],
    };
    ui::line(f, row(y), summary);
    y += 1;
    let mut toggles = vec![Span::styled("permissions ", muted(t))];
    for (p, name) in [(Permission::Default, "ask"), (Permission::Auto, "auto"), (Permission::Skip, "skip all")] {
        let on = l.permission == p;
        let c = if p == Permission::Skip { t.danger } else { t.accent };
        toggles.push(Span::styled(format!(" {name} "), if on { Style::default().fg(c).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { muted(t) }));
    }
    toggles.push(Span::styled(" ^e", fg(t.shine)));
    let b = match l.browser {
        BrowserMode::Off => "off",
        BrowserMode::Auto => "on",
        BrowserMode::Edge => "edge",
        BrowserMode::Chrome => "chrome",
    };
    toggles.push(Span::styled("    browser ", muted(t)));
    toggles.push(Span::styled(b, if l.browser == BrowserMode::Off { muted(t) } else { ui::bold_accent(t) }));
    toggles.push(Span::styled(" ^b", fg(t.shine)));
    toggles.push(Span::styled("    opens as ", muted(t)));
    toggles.push(Span::styled(if l.place == crate::pane::Place::Tab { "new session" } else { "split" }, ui::bold_accent(t)));
    toggles.push(Span::styled(" ^s", fg(t.shine)));
    ui::line(f, row(y), toggles);
    y += 2;
    if y < r.bottom().saturating_sub(1) {
        let enter = match l.focus {
            Focus::List if l.needs_model() => "pick a model",
            _ => "launch",
        };
        let mut hints = vec![("⏎", enter), ("↑↓", "move"), ("type", "filter")];
        if l.needs_model() {
            hints.push(("tab", "list ⇄ models"));
        }
        hints.push(("esc", if l.focus == Focus::List { "close" } else { "back" }));
        ui::line(f, row(y), ui::hints(&hints, t));
    }
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

fn draw_run_on(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme) {
    let focused = l.focus == Focus::List;
    filter_line(f, Rect { height: 1, ..area }, &l.list_filter, "run on… (type to filter)", focused, t);
    let items = l.items();
    let rows = area.height.saturating_sub(1) as usize;
    let cursor = l.cursor_row(&items);
    let start = window(cursor.unwrap_or(0), rows);
    for (k, item) in items.iter().enumerate().skip(start).take(rows) {
        let r = Rect { y: area.y + 1 + (k - start) as u16, height: 1, ..area };
        let on = cursor == Some(k);
        let mark = match (on, focused) {
            (true, true) => Span::styled("▌", ui::accent(t)),
            (true, false) => Span::styled("›", muted(t)),
            _ => Span::raw(" "),
        };
        match item {
            Item::Header(h) => ui::line(f, r, vec![Span::styled(format!(" {h}"), muted(t).add_modifier(Modifier::BOLD))]),
            Item::Recent(i) => {
                let rc = &l.data.recents[*i];
                let who = rc.provider_id.clone().or_else(|| rc.profile_id.as_ref().map(|p| p.split(':').next_back().unwrap_or(p).to_string())).unwrap_or_else(|| "own login".into());
                let model = rc.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m)));
                let style = if on { ui::bold_accent(t) } else { Style::default() };
                let mut spans = vec![mark, Span::styled(" ↻ ", fg(t.shine)), Span::styled(who, style)];
                if let Some(m) = model {
                    spans.push(Span::styled(format!(" · {m}"), muted(t)));
                }
                let age = crate::util::short_dur((crate::util::now_secs() - rc.at).max(0) as u64);
                ui::line_lr(f, r, spans, vec![Span::styled(format!("{age} "), muted(t))]);
            }
            Item::Account(a) => {
                let glyph = match a.kind {
                    AccountKind::Pool => ("⇄ ", t.shine),
                    AccountKind::Provider(_) => ("◇ ", if a.ready { t.shine } else { t.muted }),
                    AccountKind::Native => ("● ", t.good),
                    AccountKind::Profile(_) if a.ready => ("● ", t.good),
                    AccountKind::Profile(_) => ("○ ", t.danger),
                };
                let style = if on { ui::bold_accent(t) } else if a.ready { Style::default() } else { muted(t) };
                let right = match a.left {
                    Some(left) => vec![Span::styled(format!("{left:.0}% left "), fg(ui::left_color(left, t)))],
                    None => vec![],
                };
                let left = vec![mark, Span::raw(" "), Span::styled(glyph.0, fg(glyph.1)), Span::styled(format!("{} ", a.label), style), Span::styled(a.detail.clone(), muted(t))];
                ui::line_lr(f, r, left, right);
            }
        }
    }
    if items.is_empty() {
        ui::line(f, Rect { y: area.y + 1, height: 1, ..area }, vec![Span::styled("   nothing matches", muted(t))]);
    }
}

fn draw_models(f: &mut Frame, area: Rect, l: &Launcher, t: &Theme) {
    let focused = l.focus == Focus::Models;
    let models = l.models();
    let idx = l.models_view();
    let prompt = format!("model… {} to pick from", models.len());
    filter_line(f, Rect { height: 1, ..area }, &l.model_filter, &prompt, focused, t);
    let rows = area.height.saturating_sub(1) as usize;
    let start = window(l.model_sel, rows);
    for (k, &i) in idx.iter().enumerate().skip(start).take(rows) {
        let m = &models[i];
        let r = Rect { y: area.y + 1 + (k - start) as u16, height: 1, ..area };
        let on = k == l.model_sel;
        let mark = match (on, focused) {
            (true, true) => Span::styled("▌", ui::accent(t)),
            (true, false) => Span::styled("›", muted(t)),
            _ => Span::raw(" "),
        };
        let mut right = vec![];
        if let Some(c) = m.context {
            right.push(Span::styled(format!("{} ", short_ctx(c)), muted(t)));
        }
        if let Some((a, b)) = m.pricing {
            right.push(Span::styled(format!("${}/{} ", short_price(a), short_price(b)), muted(t)));
        }
        let style = if on { ui::bold_accent(t) } else { Style::default() };
        ui::line_lr(f, r, vec![mark, Span::raw(" "), Span::styled(m.id.clone(), style)], right);
    }
    if idx.is_empty() {
        let msg = if models.is_empty() { "   no models known for this" } else { "   nothing matches" };
        ui::line(f, Rect { y: area.y + 1, height: 1, ..area }, vec![Span::styled(msg, muted(t))]);
    }
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
