//! Drawing the sidebar: projects with their live and past sessions, then the footer blocks (usage meters,
//! proxy, bridge).

use super::{App, SideHit};
use crate::panes::term::status_look;
use crate::services::Avail;
use crate::sidebar::Row;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

/// A window reading: (used %, resets at).
type Win = Option<(f32, Option<i64>)>;
/// (profile name, harness, 5h, weekly, headroom capped)
type UsageRow = (String, Option<bro_core::Harness>, Win, Win, bool);

impl App {
    pub(super) fn draw_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let focused = self.side_focus && !self.overlay.is_open();
        let mut title = vec![Span::raw(" ")];
        title.extend(ui::title_spans("bro", &t, time));
        title.push(Span::raw(" "));
        let clock = Line::from(Span::styled(format!(" {} ", crate::util::clock()), muted(&t)));
        let sub = if focused { "j/k · ⏎ · f fork · / filter" } else { "alt+b focus" };
        let inner = ui::frame_ex(f, area, Line::from(title), Some(clock), Some(sub), focused, &t);
        let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner };

        // footer first (fixed height), rows get the rest
        let foot_h = self.footer_height(inner.height);
        let list = Rect { height: inner.height.saturating_sub(foot_h), ..inner };
        if foot_h > 0 {
            self.draw_footer(f, Rect { y: inner.bottom() - foot_h, height: foot_h, ..inner }, &t);
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
            let msg = if self.side.filter.is_empty() { "no sessions yet" } else { "nothing matches" };
            ui::line(f, Rect { y: y + 1, height: 1, ..list }, vec![Span::styled(format!(" {msg}"), muted(&t))]);
            let r = Rect { y: y + 3, height: 1, ..list };
            ui::line(f, r, vec![Span::styled(format!(" {} ", self.keymap.primary(crate::keymap::Act::NewSession)), fg(t.shine).add_modifier(Modifier::BOLD)), Span::styled("launch an agent", muted(&t))]);
            self.side_hits.push((r, SideHit::Launch));
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
        for (i, row) in rows.iter().enumerate().skip(self.side_scroll).take(room) {
            let r = Rect { y, height: 1, ..list };
            let selected = focused && i == self.side_sel;
            if selected {
                f.buffer_mut().set_style(r, Style::default().bg(crate::theme::mix(t.user, ratatui::style::Color::Rgb(20, 20, 24), 0.35)));
            }
            self.draw_row(f, r, row, selected, focus_pane, &t, time);
            self.side_hits.push((r, SideHit::Row(i)));
            y += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(&self, f: &mut Frame, r: Rect, row: &Row, selected: bool, focus_pane: Option<crate::layout::PaneId>, t: &Theme, time: f64) {
        let w = r.width as usize;
        match row {
            Row::Project { name, root, live, collapsed, attention, past, .. } => {
                let arrow = if *collapsed { "▸" } else { "▾" };
                let parent = root.parent().map(|p| crate::util::short_path(p, 14)).unwrap_or_default();
                let mut right = vec![];
                if *attention {
                    right.push(Span::styled("● ", fg(t.danger)));
                }
                if *live > 0 {
                    right.push(Span::styled(format!("{live} live "), fg(t.shine)));
                } else if *collapsed && *past > 0 {
                    right.push(Span::styled(format!("↺{past} "), muted(t)));
                }
                let name_style = if selected { ui::bold_accent(t) } else { Style::default().add_modifier(Modifier::BOLD) };
                ui::line_lr(f, r, vec![Span::styled(format!("{arrow} "), ui::accent(t)), Span::styled(format!("{name} "), name_style), Span::styled(parent, muted(t))], right);
            }
            Row::Live { info, n } => {
                let brand = ui::harness_color(info.harness, t);
                let (dot, dot_c) = match status_look(info.activity, info.done, t, time) {
                    Some((g, _, c)) => (g, c),
                    None => (" ", t.muted),
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
                let left = vec![
                    Span::styled(if is_focus { "▌" } else { " " }, ui::accent(t)),
                    Span::styled(num, muted(t)),
                    Span::styled(format!(" {dot} "), fg(dot_c).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{} ", ui::harness_glyph(info.harness)), fg(brand)),
                    Span::styled(format!("{what} "), name_style),
                    Span::styled(detail, muted(t)),
                ];
                let right = vec![Span::styled(format!(" {}", crate::util::short_dur(info.age_secs)), muted(t))];
                if let Some((id, text)) = &self.renaming
                    && *id == info.pane {
                        ui::line(f, r, vec![Span::styled("   ✎ ", ui::accent(t)), Span::styled(format!("{text}▏"), ui::bold_accent(t).add_modifier(Modifier::UNDERLINED)), Span::styled("  ⏎ ok · esc", muted(t))]);
                        return;
                    }
                ui::line_lr(f, r, left, right);
            }
            Row::PastHeader { count, open, .. } => {
                let arrow = if *open { "▾" } else { "▸" };
                let st = if selected { ui::bold_accent(t) } else { muted(t) };
                ui::line(f, r, vec![Span::styled(format!("   {arrow} ↺ {count} past"), st)]);
            }
            Row::Past { info } => {
                let brand = ui::harness_color(Some(info.harness), t);
                let who = info.profile.as_deref().map(|p| p.split(':').next_back().unwrap_or(p).to_string()).unwrap_or_else(|| info.harness.label().to_string());
                let right = vec![Span::styled(format!(" {} {}", ui::fit(&who, 8), crate::util::short_dur(info.age_secs)), muted(t))];
                let title_w = w.saturating_sub(8 + ui::width(&who).min(8) + 4);
                let st = if selected { ui::bold_accent(t) } else { Style::default() };
                ui::line_lr(f, r, vec![Span::raw("     "), Span::styled(format!("{} ", ui::harness_glyph(Some(info.harness))), fg(brand)), Span::styled(ui::fit(&info.title, title_w), st)], right);
            }
        }
    }

    fn usage_rows(&self) -> Vec<UsageRow> {
        let st = self.svc.state();
        let Some(profiles) = st.profiles.ready() else { return vec![] };
        profiles
            .iter()
            .filter(|p| p.authenticated)
            .filter_map(|p| {
                let e = st.usage.get(&p.id)?;
                let u = e.usage.as_ref()?;
                let h = if p.is_codex() { bro_core::Harness::Codex } else { bro_core::Harness::Claude };
                let five = u.five_hour.as_ref().map(|w| (w.used_pct, w.resets_at));
                let week = u.weekly.as_ref().map(|w| (w.used_pct, w.resets_at));
                Some((p.name.clone(), Some(h), five, week, e.headroom.as_ref().is_some_and(|h| h.capped)))
            })
            .collect()
    }

    fn footer_height(&self, avail: u16) -> u16 {
        let n = self.usage_rows().len() as u16;
        let want_two = 1 + n * 2 + 2;
        let want_one = 1 + n + 2;
        if avail >= want_two + 14 {
            want_two
        } else if avail >= want_one + 8 {
            want_one
        } else if avail >= 12 {
            2
        } else {
            0
        }
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let usage = self.usage_rows();
        let two = area.height >= 1 + usage.len() as u16 * 2 + 2;
        let mut y = area.y;
        let row = |y: u16| Rect { y, height: 1, ..area };
        let now = crate::util::now_secs();
        if area.height > 2 {
            let head = row(y);
            ui::rule(f, head, "usage", t);
            self.side_hits.push((head, SideHit::Usage));
            y += 1;
            let name_w = usage.iter().map(|u| ui::width(&u.0)).max().unwrap_or(4).clamp(4, 9);
            for (name, h, five, week, capped) in &usage {
                if y + 2 >= area.bottom() {
                    break;
                }
                let brand = ui::harness_color(*h, t);
                let meter_n = if area.width >= 36 { 6 } else { 5 };
                let win = |label: &str, w: &Option<(f32, Option<i64>)>, approx: bool, reset: bool| -> Vec<Span<'static>> {
                    let mut v = vec![Span::styled(format!("{label} "), muted(t))];
                    match w {
                        Some((p, at)) => {
                            v.extend(ui::meter(*p, meter_n, t));
                            v.push(Span::styled(format!(" {:>3.0}%{}", p, if approx { "≈" } else { " " }), fg(ui::pct_color(*p, t))));
                            if reset
                                && let Some(at) = at {
                                    v.push(Span::styled(format!(" {}", crate::util::countdown(at - now)), muted(t)));
                                }
                        }
                        None => v.push(Span::styled("—", muted(t))),
                    }
                    v
                };
                let r = row(y);
                let mut spans = vec![Span::styled(format!("{} ", ui::harness_glyph(*h)), fg(brand)), Span::styled(ui::pad(name, name_w), ui::bold())];
                spans.push(Span::raw(" "));
                spans.extend(win("5h", five, *capped, two));
                ui::line(f, r, spans);
                self.side_hits.push((r, SideHit::Usage));
                y += 1;
                if two {
                    let r = row(y);
                    let mut spans = vec![Span::raw(" ".repeat(name_w + 3))];
                    spans.extend(win("wk", week, false, true));
                    ui::line(f, r, spans);
                    self.side_hits.push((r, SideHit::Usage));
                    y += 1;
                }
            }
        }
        // proxy + bridge, pinned to the bottom
        let st = self.svc.state();
        let pr = row(area.bottom() - 2);
        let proxy = match &st.proxy.status {
            Avail::Ready(i) => {
                let last = st.proxy.events.back().map(|e| format!(" · {}", crate::util::short_dur(((crate::util::now_ms() - e.at_ms).max(0) / 1000) as u64))).unwrap_or_default();
                vec![Span::styled(format!("{} proxy ", ui::icon("proxy")), muted(t)), Span::styled("● ", fg(t.good)), Span::styled(format!(":{}", i.port), ui::bold()), Span::styled(format!(" · {} rt{last}", st.proxy.routes.len()), muted(t))]
            }
            Avail::Loading => vec![Span::styled(format!("{} proxy ", ui::icon("proxy")), muted(t)), Span::styled("◌ starting", muted(t))],
            Avail::Unavailable(_) => vec![Span::styled(format!("{} proxy ", ui::icon("proxy")), muted(t)), Span::styled("○ off", fg(t.danger))],
        };
        ui::line(f, pr, proxy);
        let br = row(area.bottom() - 1);
        let bridge = match &st.bridge.status {
            Avail::Ready(s) if s.running => vec![
                Span::styled(format!("{} bridge ", ui::icon("bridge")), muted(t)),
                Span::styled("● ", fg(t.good)),
                Span::styled(format!(":{}", s.port), ui::bold()),
                Span::styled(format!(" · {} dev", s.clients), if s.clients > 0 { fg(t.shine) } else { muted(t) }),
            ],
            Avail::Loading if st.bridge.enabled => vec![Span::styled(format!("{} bridge ", ui::icon("bridge")), muted(t)), Span::styled("◌ starting", muted(t))],
            _ => vec![Span::styled(format!("{} bridge ", ui::icon("bridge")), muted(t)), Span::styled("○ off", fg(t.danger))],
        };
        ui::line(f, br, bridge);
        drop(st);
        self.side_hits.push((pr, SideHit::Proxy));
        self.side_hits.push((br, SideHit::Bridge));
    }
}
