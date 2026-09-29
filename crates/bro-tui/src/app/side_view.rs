//! Drawing the sidebar: "+ new session", projects with their live and earlier sessions, then the footer
//! blocks (usage — Claude / Codex totals or every profile — proxy, bridge).

use super::{App, SideHit};
use crate::panes::term::status_look;
use crate::services::Avail;
use crate::sidebar::Row;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::usage::{AppSummary, LoginUsage};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

/// One line of the usage block: a glyph + label, then the 5h / week figures (% left).
struct UsageLine {
    harness: Option<bro_core::Harness>,
    label: String,
    h5: Option<f64>,
    wk: Option<f64>,
    /// the 5h figure was capped by the week
    approx: bool,
    /// still loading / couldn't be read
    note: Option<String>,
    /// countdown to the tighter window's reset (expanded view)
    resets_at: Option<i64>,
    /// the Claude total with a Fable line folded under it: Some(open)
    fable: Option<bool>,
}

impl App {
    pub(super) fn draw_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let focused = self.side_focus && !self.overlay.is_open();
        let title = Line::from(vec![Span::raw(" "), Span::styled("bro", ui::bold_accent(&t)), Span::raw(" ")]);
        let clock = Line::from(Span::styled(format!(" {} ", crate::util::clock()), muted(&t)));
        let sub = if focused { "? keys" } else { "alt+b" };
        let inner = ui::frame_ex(f, area, title, Some(clock), Some(sub), focused, &t);
        let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner };

        // footer first (fixed height), rows get the rest
        let usage = self.usage_lines();
        let foot_h = footer_height(usage.len() as u16, self.usage_expanded, inner.height);
        let list = Rect { height: inner.height.saturating_sub(foot_h), ..inner };
        if foot_h > 0 {
            self.draw_footer(f, Rect { y: inner.bottom() - foot_h, height: foot_h, ..inner }, &usage, &t);
        }

        let mut y = list.y;
        if self.side_filtering || !self.side.filter.is_empty() {
            let cursor = if self.side_filtering { "▏" } else { "" };
            ui::line(f, Rect { y, height: 1, ..list }, vec![Span::styled("/ ", ui::bold_accent(&t)), Span::styled(format!("{}{cursor}", self.side.filter), ui::bold())]);
            y += 1;
        }
        let rows = self.rows();
        self.side_sel = self.side_sel.min(rows.len().saturating_sub(1));
        let room = list.bottom().saturating_sub(y) as usize;
        if rows.is_empty() {
            ui::line(f, Rect { y: y + 1, height: 1, ..list }, vec![Span::styled(" nothing matches", muted(&t))]);
            return;
        }
        // keep the selection in view
        if self.side_focus {
            if self.side_sel < self.side_scroll {
                self.side_scroll = self.side_sel;
            } else if room > 0 && self.side_sel >= self.side_scroll + room {
                self.side_scroll = self.side_sel + 1 - room;
            }
        }
        self.side_scroll = self.side_scroll.min(rows.len().saturating_sub(room.max(1)));
        let focus_pane = self.focused();
        let cur_key = self.current_project().map(|r| self.svc.project_for(&r).key);
        for (i, row) in rows.iter().enumerate().skip(self.side_scroll).take(room) {
            let r = Rect { y, height: 1, ..list };
            let selected = focused && i == self.side_sel;
            if selected {
                f.buffer_mut().set_style(r, Style::default().bg(crate::theme::mix(t.user, ratatui::style::Color::Rgb(20, 20, 24), 0.35)));
            }
            self.draw_row(f, r, row, selected, focus_pane, cur_key.as_deref(), &t, time);
            if let Row::Live { info, .. } = row
                && self.renaming.as_ref().is_none_or(|(id, _)| *id != info.pane)
            {
                // the × cell wins over the row (hits are searched in order)
                self.side_hits.push((Rect { x: r.right().saturating_sub(1), width: 1, ..r }, SideHit::Close(info.pane)));
            }
            self.side_hits.push((r, SideHit::Row(i)));
            y += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(&self, f: &mut Frame, r: Rect, row: &Row, selected: bool, focus_pane: Option<crate::layout::PaneId>, cur_key: Option<&str>, t: &Theme, time: f64) {
        let w = r.width as usize;
        match row {
            Row::New => {
                let key = self.keymap.primary(crate::keymap::Act::NewSession);
                let st = if selected { ui::bold_accent(t) } else { fg(t.shine).add_modifier(Modifier::BOLD) };
                ui::line_lr(f, r, vec![Span::styled("+ new session", st)], vec![Span::styled(format!("{key} "), muted(t))]);
            }
            Row::OpenFolder => {
                let key = self.keymap.primary(crate::keymap::Act::OpenProject);
                let st = if selected { ui::bold_accent(t) } else { muted(t) };
                ui::line_lr(f, r, vec![Span::styled("+ open project", st)], vec![Span::styled(format!("{key} "), muted(t))]);
            }
            Row::Project { key, name, root, live, collapsed, attention, last_age, .. } => {
                // the current project (where new sessions start) gets a bar
                let current = cur_key == Some(key.as_str());
                let arrow = if *collapsed { "▸" } else { "▾" };
                let active = *live > 0;
                let parent_w = w.saturating_sub(ui::width(name) + 10).min(16);
                let parent = if parent_w >= 6 { root.parent().map(|p| crate::util::short_path(p, parent_w)).unwrap_or_default() } else { String::new() };
                let mut right = vec![];
                if *attention {
                    right.push(Span::styled("● ", fg(t.danger)));
                }
                if active {
                    right.push(Span::styled(format!("{live} live "), fg(t.shine)));
                } else if let Some(age) = last_age {
                    right.push(Span::styled(format!("{} ", crate::util::short_dur(*age)), muted(t)));
                }
                let name_style = if selected || current { ui::bold_accent(t) } else { Style::default().add_modifier(Modifier::BOLD) };
                let lead = if current { Span::styled("▍", ui::accent(t)) } else { Span::raw(" ") };
                ui::line_lr(f, r, vec![lead, Span::styled(format!("{arrow} "), ui::accent(t)), Span::styled(format!("{name} "), name_style), Span::styled(parent, muted(t))], right);
            }
            Row::Live { info, n } => {
                let brand = ui::harness_color(info.harness, t);
                let (dot, dot_c) = match status_look(info.activity, info.done, t, time) {
                    Some((g, _, c)) => (g, c),
                    None => ("•", t.muted),
                };
                let is_focus = focus_pane == Some(info.pane);
                let num = n.map(|n| n.to_string()).unwrap_or_else(|| " ".into());
                let what = match (&info.name, info.harness) {
                    (Some(n), _) => n.clone(),
                    (None, Some(h)) => h.label().to_string(),
                    (None, None) => "shell".into(),
                };
                let who = info.profile.as_deref().map(|p| p.split(':').next_back().unwrap_or(p).to_string()).unwrap_or_default();
                let model = info.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m))).unwrap_or_default();
                let detail = [who, model].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                let name_style = if is_focus || selected { Style::default().fg(brand).add_modifier(Modifier::BOLD) } else { Style::default().fg(brand) };
                if let Some((id, text)) = &self.renaming
                    && *id == info.pane
                {
                    ui::line(f, r, vec![Span::styled("   ✎ ", ui::accent(t)), Span::styled(format!("{text}▏"), ui::bold_accent(t).add_modifier(Modifier::UNDERLINED)), Span::styled("  ⏎ ok · esc", muted(t))]);
                    return;
                }
                let left = vec![
                    Span::styled(if is_focus { "▌" } else { " " }, ui::accent(t)),
                    Span::styled(num, muted(t)),
                    Span::styled(format!(" {dot} "), fg(dot_c).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{} ", ui::harness_glyph(info.harness)), fg(brand)),
                    Span::styled(format!("{what} "), name_style),
                    Span::styled(detail, muted(t)),
                ];
                // age, then a clickable × that closes the session
                let right = vec![Span::styled(format!(" {}", crate::util::short_dur(info.age_secs)), muted(t)), Span::styled(" ×", if selected { fg(t.danger) } else { fg(t.frame) })];
                ui::line_lr(f, r, left, right);
            }
            Row::Past { info } => {
                // an earlier session: dim, indented under the live ones, ⏎ resumes it
                let brand = ui::harness_color(Some(info.harness), t);
                let age = crate::util::short_dur(info.age_secs);
                let title_w = w.saturating_sub(7 + ui::width(&age) + 1);
                let mut st = if selected { ui::bold_accent(t) } else { muted(t) };
                let mut right = vec![Span::styled(format!(" {age}"), muted(t))];
                if info.archived {
                    st = st.add_modifier(Modifier::CROSSED_OUT);
                    right.insert(0, Span::styled(" archived", fg(t.frame)));
                }
                let title_w = title_w.saturating_sub(if info.archived { 9 } else { 0 });
                ui::line_lr(
                    f,
                    r,
                    vec![Span::raw("    "), Span::styled(format!("{} ", ui::harness_glyph(Some(info.harness))), fg(crate::theme::mix(brand, t.muted, 0.5))), Span::styled(ui::fit(&info.title, title_w), st)],
                    right,
                );
            }
            Row::More { hidden, .. } => {
                let st = if selected { ui::bold_accent(t) } else { muted(t) };
                ui::line(f, r, vec![Span::styled(format!("      … {hidden} more"), st)]);
            }
        }
    }

    /// The usage block's lines: the Claude and Codex totals (v1's Usage box), or one line per profile.
    fn usage_lines(&self) -> Vec<UsageLine> {
        let st = self.svc.state();
        let Some(profiles) = st.profiles.ready() else { return vec![] };
        let signed_in: Vec<_> = profiles.iter().filter(|p| p.authenticated).collect();
        let state_of = |id: &str| match st.usage.get(id) {
            Some(e) => match (&e.usage, &e.error) {
                (Some(u), _) => LoginUsage::Ready(u),
                (None, Some(_)) => LoginUsage::Unavailable,
                (None, None) => LoginUsage::Loading,
            },
            None => LoginUsage::Loading,
        };
        if !self.usage_expanded {
            let total = |claude: bool| -> Option<UsageLine> {
                let family: Vec<_> = signed_in.iter().filter(|p| p.is_claude() == claude).collect();
                if family.is_empty() {
                    return None;
                }
                let s: AppSummary = bro_core::usage::app_summary(family.iter().map(|p| (**p, state_of(&p.id))));
                let note = if s.h5.is_none() && s.wk.is_none() {
                    Some(if s.loading > 0 { "…".to_string() } else { "unavailable".to_string() })
                } else if s.unavailable > 0 {
                    Some(format!("{}✗", s.unavailable))
                } else {
                    None
                };
                let h = if claude { bro_core::Harness::Claude } else { bro_core::Harness::Codex };
                Some(UsageLine { harness: Some(h), label: h.label().into(), h5: s.h5, wk: s.wk, approx: s.h5_estimated, note, resets_at: None, fable: None })
            };
            let mut v: Vec<UsageLine> = [total(true), total(false)].into_iter().flatten().collect();
            // Fable has its own allowance: show it under Claude when any account has it
            let claude: Vec<_> = signed_in.iter().filter(|p| p.is_claude()).map(|p| (*p, state_of(&p.id))).collect();
            let s = bro_core::usage::app_summary(claude);
            if s.fable_wk.is_some() {
                // folded under the Claude line; clicking it (or alt+U twice) shows it
                if let Some(c) = v.first_mut().filter(|l| l.harness == Some(bro_core::Harness::Claude)) {
                    c.fable = Some(self.show_fable);
                }
                if self.show_fable {
                    v.insert(1, UsageLine { harness: None, label: "fable".into(), h5: s.fable_5h, wk: s.fable_wk, approx: false, note: None, resets_at: None, fable: None });
                }
            }
            return v;
        }
        signed_in
            .iter()
            .map(|p| {
                let h = if p.is_codex() { bro_core::Harness::Codex } else { bro_core::Harness::Claude };
                let e = st.usage.get(&p.id);
                let (h5, wk, approx, resets_at, note) = match e.and_then(|e| e.usage.as_ref()) {
                    Some(u) => {
                        let d = bro_core::usage::headroom_detail(p, u);
                        let reset = [&u.five_hour, &u.weekly].into_iter().flatten().filter_map(|w| w.resets_at).min();
                        (d.h5, d.wk, d.h5_estimated, reset, None)
                    }
                    None if e.is_some_and(|e| e.error.is_some()) => (None, None, false, None, Some("unavailable".to_string())),
                    None => (None, None, false, None, Some("…".to_string())),
                };
                UsageLine { harness: Some(h), label: p.name.clone(), h5, wk, approx, note, resets_at, fable: None }
            })
            .collect()
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect, usage: &[UsageLine], t: &Theme) {
        let mut y = area.y;
        let row = |y: u16| Rect { y, height: 1, ..area };
        let now = crate::util::now_secs();
        if area.height > 2 && !usage.is_empty() {
            // header: "▸ usage left   5h  wk" — click to switch totals ⇄ every profile
            let head = row(y);
            let arrow = if self.usage_expanded { "▾" } else { "▸" };
            let label = format!("{arrow} usage left ");
            // column heads sit over the figures: " 46%" + " " + " 39%" + " "
            let heads = " 5h   wk  ";
            let fill = (area.width as usize).saturating_sub(ui::width(&label) + ui::width(heads) + 1);
            ui::line_lr(f, head, vec![Span::styled(label, ui::accent(t)), Span::styled("─".repeat(fill), fg(t.frame))], vec![Span::styled(heads, muted(t))]);
            self.side_hits.push((head, SideHit::UsageToggle));
            y += 1;
            let name_w = usage.iter().map(|u| ui::width(&u.label)).max().unwrap_or(6).clamp(5, 10);
            let meter_n = (area.width as usize).saturating_sub(name_w + 3 + 12).clamp(0, 8);
            let status_rows = if self.usage_expanded { 2 } else { 0 };
            for u in usage {
                if y + status_rows >= area.bottom() {
                    break;
                }
                let r = row(y);
                let brand = u.harness.map(|h| ui::harness_color(Some(h), t)).unwrap_or(t.muted);
                let glyph = u.harness.map(|h| ui::harness_glyph(Some(h))).unwrap_or(" ");
                let name_style = if u.harness.is_some() { ui::bold() } else { muted(t) };
                let label = match u.fable {
                    Some(open) => format!("{} {}", u.label, if open { "▾" } else { "▸" }),
                    None if u.harness.is_none() => format!(" {}", u.label),
                    None => u.label.clone(),
                };
                let mut left = vec![Span::styled(format!("{glyph} "), fg(brand)), Span::styled(ui::pad(&label, name_w + 2), name_style), Span::raw(" ")];
                let mut right = vec![];
                match &u.note {
                    Some(n) if u.h5.is_none() && u.wk.is_none() => right.push(Span::styled(format!("{n} "), muted(t))),
                    _ => {
                        // the meter shows the tighter window
                        let tight = [u.h5, u.wk].into_iter().flatten().fold(f64::INFINITY, f64::min);
                        if meter_n >= 3 && tight.is_finite() {
                            left.extend(ui::left_meter(tight, meter_n, t));
                        }
                        if self.usage_expanded
                            && let Some(at) = u.resets_at
                            && meter_n < 3
                        {
                            left.push(Span::styled(crate::util::countdown(at - now), muted(t)));
                        }
                        right.extend(figure(u.h5, u.approx, t));
                        right.push(Span::raw(" "));
                        right.extend(figure(u.wk, false, t));
                        right.push(Span::raw(" "));
                    }
                }
                ui::line_lr(f, r, left, right);
                self.side_hits.push((r, if u.fable.is_some() { SideHit::Fable } else { SideHit::Usage }));
                y += 1;
            }
        }
        // proxy + phone bridge, pinned to the bottom, in plain words (click either for details) —
        // only with the usage block expanded
        if !self.usage_expanded {
            return;
        }
        let st = self.svc.state();
        let pr = row(area.bottom() - 2);
        let label = |icon: &str, name: &str| Span::styled(format!("{} {:<8}", ui::icon(icon), name), muted(t));
        let proxy = match &st.proxy.status {
            Avail::Ready(i) => {
                let n = st.proxy.routes.len();
                let use_ = match n {
                    0 => "idle".to_string(),
                    1 => "1 session".to_string(),
                    n => format!("{n} sessions"),
                };
                vec![label("proxy", "proxy"), Span::styled("● ", fg(t.good)), Span::styled(use_, if n > 0 { fg(t.shine) } else { muted(t) }), Span::styled(format!(" · port {}", i.port), muted(t))]
            }
            Avail::Loading => vec![label("proxy", "proxy"), Span::styled("◌ starting", muted(t))],
            Avail::Unavailable(_) => vec![label("proxy", "proxy"), Span::styled("○ off", fg(t.danger))],
        };
        ui::line(f, pr, proxy);
        let br = row(area.bottom() - 1);
        let bridge = match &st.bridge.status {
            Avail::Ready(s) if s.running => {
                let who = match s.clients {
                    0 => "no phones".to_string(),
                    1 => "1 connected".to_string(),
                    n => format!("{n} connected"),
                };
                vec![
                    label("bridge", "phone"),
                    Span::styled("● ", fg(t.good)),
                    Span::styled(who, if s.clients > 0 { fg(t.shine) } else { muted(t) }),
                    Span::styled(format!(" · port {}", s.port), muted(t)),
                ]
            }
            Avail::Loading if st.bridge.enabled => vec![label("bridge", "phone"), Span::styled("◌ starting", muted(t))],
            _ => vec![label("bridge", "phone"), Span::styled("○ off", fg(t.danger))],
        };
        ui::line(f, br, bridge);
        drop(st);
        self.side_hits.push((pr, SideHit::Proxy));
        self.side_hits.push((br, SideHit::Bridge));
    }
}

/// A right-aligned "% left" figure, 4 columns + an optional ≈.
fn figure(v: Option<f64>, approx: bool, t: &Theme) -> Vec<Span<'static>> {
    match v {
        Some(v) => vec![Span::styled(format!("{}{:>3.0}%", if approx { "≈" } else { " " }, v), fg(ui::left_color(v, t)))],
        None => vec![Span::styled("    —", muted(t))],
    }
}

/// Footer rows: header + usage lines (+ proxy + bridge when expanded), or less when space is short.
fn footer_height(usage: u16, expanded: bool, avail: u16) -> u16 {
    let status = if expanded { 2 } else { 0 };
    let full = if usage > 0 { 1 + usage + status } else { status };
    if avail >= full + 8 {
        full
    } else if avail >= 12 {
        status
    } else {
        0
    }
}
