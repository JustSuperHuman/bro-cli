//! Drawing the launcher modal.

use super::{AccountKind, Col, Launcher};
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::browser::BrowserMode;
use bro_core::launch::Permission;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::Clear,
};

/// Draw the launcher centered on `screen`.
pub fn draw(f: &mut Frame, screen: Rect, l: &Launcher, t: &Theme, time: f64) {
    let recents = l.recents_view();
    let rec_rows = recents.len().min(4) as u16;
    let h = 19 + if rec_rows > 0 { rec_rows + 2 } else { 0 };
    let r = ui::centered(screen, 104, h);
    f.render_widget(Clear, r);
    let title = ratatui::text::Line::from({
        let mut s = vec![Span::raw(" "), Span::styled(ui::lead("rocket"), ui::bold_accent(t))];
        s.extend(ui::title_spans("launch an agent", t, time));
        s.push(Span::raw(" "));
        s
    });
    let status = ratatui::text::Line::from(Span::styled(format!(" {} ", l.place.label()), muted(t)));
    let inner = ui::frame_ex(f, r, title, Some(status), None, true, t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let mut y = inner.y;
    let row = |y: u16| Rect { y, height: 1, ..inner };

    // filter line
    let q = &l.filters[l.col as usize];
    let mut spans = vec![Span::styled("› ", ui::bold_accent(t))];
    if q.is_empty() {
        spans.push(Span::styled(format!("type to filter {}", l.col.label()), muted(t)));
    } else {
        spans.push(Span::styled(q.clone(), ui::bold()));
    }
    spans.push(Span::styled("▏", ui::accent(t)));
    ui::line_lr(f, row(y), spans, vec![Span::styled(format!("{} ", l.col.label()), fg(t.shine).add_modifier(Modifier::BOLD))]);
    y += 2;

    // recents
    if rec_rows > 0 {
        header(f, row(y), &[("RECENT", inner.width)], l.col == Col::Recent, t);
        y += 1;
        let start = l.sel[0].saturating_sub(rec_rows as usize - 1);
        for (k, &i) in recents.iter().enumerate().skip(start).take(rec_rows as usize) {
            let rc = &l.data.recents[i];
            let on = l.col == Col::Recent && k == l.sel[0];
            let who = rc.provider_id.clone().or_else(|| rc.profile_id.as_ref().map(|p| p.split(':').next_back().unwrap_or(p).to_string())).unwrap_or_default();
            let model = rc.model.as_deref().map(crate::services::launch::short_model).unwrap_or_else(|| "default".into());
            let brand = ui::harness_color(Some(rc.harness), t);
            let left = vec![
                Span::styled(if on { "▌" } else { " " }, ui::accent(t)),
                Span::styled(format!("{} ", ui::harness_glyph(Some(rc.harness))), fg(brand)),
                Span::styled(format!("{} · {who} · {model}", rc.harness.label()), if on { ui::bold_accent(t) } else { ui::bold() }),
                Span::styled(format!("   {}", crate::util::short_path(&rc.cwd, 40)), muted(t)),
            ];
            let right = vec![Span::styled(format!("{} ", crate::util::short_dur((crate::util::now_secs() - rc.at).max(0) as u64)), muted(t))];
            ui::line_lr(f, row(y), left, right);
            y += 1;
        }
        y += 1;
    }

    // the four columns
    let w = inner.width;
    let wh = 12u16;
    let wa = (w.saturating_sub(wh) * 34 / 100).max(18);
    let wm = (w.saturating_sub(wh) * 28 / 100).max(14);
    let wd = w.saturating_sub(wh + wa + wm);
    let xs = [inner.x, inner.x + wh, inner.x + wh + wa, inner.x + wh + wa + wm];
    let widths = [wh, wa, wm, wd];
    let cols = [Col::Harness, Col::Account, Col::Model, Col::Dir];
    for (i, c) in cols.iter().enumerate() {
        header(f, Rect { x: xs[i], width: widths[i], y, height: 1 }, &[(&c.label().to_uppercase(), widths[i])], l.col == *c, t);
    }
    y += 1;
    let list_h = 8u16;
    let cell = |i: usize, k: usize| Rect { x: xs[i], y: y + k as u16, width: widths[i].saturating_sub(1), height: 1 };

    // harness
    let hs = l.harnesses();
    let hidx = crate::fuzzy::filter(&l.filters[1], &hs, |h| h.label().to_string());
    for (k, &i) in hidx.iter().enumerate().take(list_h as usize) {
        let h = hs[i];
        let on = k == l.sel[1];
        let brand = ui::harness_color(Some(h), t);
        let style = if on { Style::default().fg(brand).add_modifier(Modifier::BOLD) } else if l.installed(h) { Style::default() } else { muted(t) };
        let spans = vec![marker(on, l.col == Col::Harness, t), Span::styled(format!("{} ", ui::harness_glyph(Some(h))), fg(brand)), Span::styled(h.label().to_string(), style)];
        ui::line(f, cell(0, k), spans);
    }

    // accounts
    let (accts, aidx) = l.accounts_view();
    let a_start = l.sel[2].saturating_sub(list_h as usize - 1);
    for (k, &i) in aidx.iter().enumerate().skip(a_start).take(list_h as usize) {
        let a = &accts[i];
        let on = k == l.sel[2];
        let glyph = match a.kind {
            AccountKind::Pool => ("⇄ ", t.shine),
            AccountKind::Provider(_) => ("◇ ", t.muted),
            AccountKind::Profile(_) if a.ready => ("● ", t.good),
            AccountKind::Profile(_) => ("○ ", t.danger),
        };
        let style = if on { ui::bold_accent(t) } else if a.ready { Style::default() } else { muted(t) };
        let mut right = vec![];
        if let Some(p) = a.five_hour {
            // shown as what's left, like the sidebar
            let left = (100.0 - p as f64).clamp(0.0, 100.0);
            right.push(Span::styled(format!("{left:.0}% left "), fg(ui::left_color(left, t))));
        }
        let left = vec![marker(on, l.col == Col::Account, t), Span::styled(glyph.0, fg(glyph.1)), Span::styled(format!("{} ", a.label), style), Span::styled(a.detail.clone(), muted(t))];
        ui::line_lr(f, cell(1, k - a_start), left, right);
    }
    if aidx.is_empty() {
        ui::line(f, cell(1, 0), vec![Span::styled("  nothing matches", muted(t))]);
    }

    // models
    let (models, midx) = l.models_view();
    let m_start = l.sel[3].saturating_sub(list_h as usize - 1);
    for (k, &i) in midx.iter().enumerate().skip(m_start).take(list_h as usize) {
        let m = &models[i];
        let on = k == l.sel[3];
        let style = if on { ui::bold_accent(t) } else if m.id.is_none() { muted(t) } else { Style::default() };
        ui::line(f, cell(2, k - m_start), vec![marker(on, l.col == Col::Model, t), Span::styled(m.label.clone(), style)]);
    }

    // dirs
    let dirs = l.dirs_view();
    let d_start = l.sel[4].saturating_sub(list_h as usize - 1);
    for (k, d) in dirs.iter().enumerate().skip(d_start).take(list_h as usize) {
        let on = k == l.sel[4];
        let name = d.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| d.path.to_string_lossy().to_string());
        let style = if on { ui::bold_accent(t) } else { Style::default() };
        let mut spans = vec![marker(on, l.col == Col::Dir, t)];
        if d.typed {
            spans.push(Span::styled("↳ ", fg(t.shine)));
        }
        spans.push(Span::styled(format!("{name} "), style));
        spans.push(Span::styled(crate::util::short_path(d.path.parent().unwrap_or(&d.path), 30), muted(t)));
        ui::line(f, cell(3, k - d_start), spans);
    }
    y += list_h + 1;

    // summary + toggles
    ui::rule(f, row(y), "", t);
    y += 1;
    let summary = match l.spec() {
        Some(s) => {
            let brand = ui::harness_color(Some(s.harness), t);
            vec![
                Span::styled(format!("{} ", ui::harness_glyph(Some(s.harness))), fg(brand)),
                Span::styled(crate::services::launch::label_for(&s), Style::default().fg(brand).add_modifier(Modifier::BOLD)),
                Span::styled("  →  ", muted(t)),
                Span::styled(crate::util::short_path(&s.cwd, 50), ui::bold()),
            ]
        }
        None => vec![Span::styled("pick an account and a project", muted(t))],
    };
    ui::line(f, row(y), summary);
    y += 1;
    let mut toggles = vec![Span::styled("perm ", muted(t))];
    for (p, name) in [(Permission::Default, "default"), (Permission::Auto, "auto"), (Permission::Skip, "skip")] {
        let on = l.permission == p;
        let c = if p == Permission::Skip { t.danger } else { t.accent };
        toggles.push(Span::styled(format!(" {name} "), if on { Style::default().fg(c).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { muted(t) }));
    }
    let b = match l.browser {
        BrowserMode::Off => "off",
        BrowserMode::Auto => "auto",
        BrowserMode::Edge => "edge",
        BrowserMode::Chrome => "chrome",
    };
    toggles.push(Span::styled("    browser ", muted(t)));
    toggles.push(Span::styled(b, if l.browser == BrowserMode::Off { muted(t) } else { ui::bold_accent(t) }));
    toggles.push(Span::styled("    open in ", muted(t)));
    toggles.push(Span::styled(l.place.label(), ui::bold_accent(t)));
    ui::line(f, row(y), toggles);
    y += 2;
    if y < r.bottom().saturating_sub(1) {
        let hints = ui::hints(&[("⏎", "launch"), ("tab ←→", "column"), ("↑↓", "pick"), ("type", "filter"), ("^e", "permission"), ("^b", "browser"), ("^s", "tab/split"), ("esc", "close")], t);
        ui::line(f, row(y), hints);
    }
}

fn marker(on: bool, col_focused: bool, t: &Theme) -> Span<'static> {
    match (on, col_focused) {
        (true, true) => Span::styled("▌", ui::accent(t)),
        (true, false) => Span::styled("›", muted(t)),
        _ => Span::raw(" "),
    }
}

fn header(f: &mut Frame, r: Rect, items: &[(&str, u16)], focused: bool, t: &Theme) {
    let style = if focused { fg(t.accent).add_modifier(Modifier::BOLD) } else { muted(t).add_modifier(Modifier::BOLD) };
    let spans: Vec<Span> = items.iter().map(|(s, w)| Span::styled(ui::pad(&format!(" {s}"), *w as usize), style)).collect();
    ui::line(f, r, spans);
}
