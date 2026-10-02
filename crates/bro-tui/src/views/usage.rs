//! Usage view: big 5h / weekly meters for every profile, reset countdowns, headroom (≈ when capped by the
//! measured 5h↔week ratio), plan, and the large/small task picks.

use crate::pane::{Action, Cx, Pane};
use crate::services::UsageEntry;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::profiles::Profile;
use bro_core::usage::Window;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};

/// The usage pane.
#[derive(Default)]
pub struct UsageView {
    scroll: usize,
    hits: Vec<(Rect, bro_core::Harness, String)>,
}

impl UsageView {
    pub fn new() -> UsageView {
        UsageView::default()
    }
}

fn window_row(f: &mut Frame, r: Rect, name: &str, w: Option<&Window>, t: &Theme) {
    let Some(w) = w else {
        ui::line(f, r, vec![Span::styled(format!("   {name:<6}"), muted(t)), Span::styled("—", muted(t))]);
        return;
    };
    let bar_w = (r.width as usize).saturating_sub(34).clamp(10, 48);
    let mut spans = vec![Span::styled(format!("   {name:<6}"), muted(t))];
    spans.extend(ui::bar(w.used_pct, bar_w, t));
    spans.push(Span::styled(format!(" {:>4.0}%", w.used_pct), fg(ui::pct_color(w.used_pct, t)).add_modifier(Modifier::BOLD)));
    if let Some(at) = w.resets_at {
        spans.push(Span::styled(format!("   resets in {}", crate::util::countdown(at - crate::util::now_secs())), muted(t)));
    }
    ui::line(f, r, spans);
}

/// Draw one profile's block (5 rows). Returns rows used.
fn block(f: &mut Frame, area: Rect, y: u16, p: &Profile, e: Option<&UsageEntry>, picks: &(Option<String>, Option<String>), t: &Theme) -> u16 {
    let row = |k: u16| Rect { y: y + k, height: 1, ..area };
    if y >= area.bottom() {
        return 0;
    }
    let h = if p.is_claude() { Some(bro_core::Harness::Claude) } else { Some(bro_core::Harness::Codex) };
    let brand = ui::harness_color(h, t);
    let plan = e.and_then(|e| e.usage.as_ref()).and_then(|u| u.plan.clone()).or_else(|| p.plan.clone()).unwrap_or_else(|| "—".into());
    let mut left = vec![
        Span::styled(format!(" {} ", ui::harness_glyph(h)), fg(brand)),
        Span::styled(p.id.clone(), fg(brand).add_modifier(Modifier::BOLD)),
        Span::styled(format!("   {plan}"), fg(t.shine)),
    ];
    if let Some(m) = &p.email {
        left.push(Span::styled(format!(" · {m}"), muted(t)));
    }
    let mut right = vec![];
    if p.authenticated {
        right.push(Span::styled(" + session ", ui::accent(t)));
    }
    if picks.0.as_deref() == Some(p.id.as_str()) {
        right.push(Span::styled(" large task ", fg(t.good).add_modifier(Modifier::REVERSED | Modifier::BOLD)));
    }
    if picks.1.as_deref() == Some(p.id.as_str()) {
        right.push(Span::styled(" small task ", fg(t.shine).add_modifier(Modifier::REVERSED | Modifier::BOLD)));
    }
    if e.is_some_and(|e| e.fetching) {
        right.push(Span::styled("  ↻ ", ui::accent(t)));
    }
    ui::line_lr(f, row(0), left, right);
    match e.and_then(|e| e.usage.as_ref()) {
        Some(u) => {
            window_row(f, row(1), "5h", u.five_hour.as_ref(), t);
            window_row(f, row(2), "week", u.weekly.as_ref(), t);
            let mut spans = vec![Span::styled("   headroom ", muted(t))];
            if let Some(hr) = e.and_then(|e| e.headroom.as_ref()) {
                let approx = if hr.capped { "≈" } else { "" };
                spans.push(Span::styled(format!("now {approx}{:.0}%", hr.now), fg(ui::pct_color(100.0 - hr.now, t)).add_modifier(Modifier::BOLD)));
                spans.push(Span::styled(format!("   week {:.0}%", hr.week), fg(ui::pct_color(100.0 - hr.week, t))));
                if hr.capped {
                    spans.push(Span::styled("   capped by the weekly ratio", muted(t)));
                }
            } else {
                spans.push(Span::styled("—", muted(t)));
            }
            for (name, w) in u.scoped.iter().take(2) {
                spans.push(Span::styled(format!("   {name} {:.0}%", w.used_pct), fg(ui::pct_color(w.used_pct, t))));
            }
            ui::line(f, row(3), spans);
            if let Some(err) = e.and_then(|e| e.error.as_ref()) {
                ui::line(f, row(4), vec![Span::styled(format!("   last refresh failed: {err}"), fg(t.danger))]);
            }
        }
        None => {
            let why = match e {
                Some(UsageEntry { error: Some(err), .. }) => err.clone(),
                Some(UsageEntry { fetching: true, .. }) => "fetching…".into(),
                _ if !p.authenticated => "not logged in — alt+o to log in".into(),
                _ => "no reading yet".into(),
            };
            ui::line(f, row(1), vec![Span::styled(format!("   {why}"), muted(t))]);
        }
    }
    5
}

impl Pane for UsageView {
    fn title(&self) -> String {
        "usage".into()
    }
    fn icon(&self) -> &'static str {
        "gauge"
    }
    fn view(&self) -> Option<&'static str> {
        Some("usage")
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        self.hits.clear();
        let t = cx.theme;
        let st = cx.svc.state();
        let area = ui::hint_line(f, area, &[("click account", "new session"), ("r", "refresh"), ("j/k", "scroll"), ("alt+o", "profiles")], t);
        let ago = match st.usage_at.map(|a| a.elapsed().as_secs()) {
            Some(s) if s < 5 => "just refreshed".to_string(),
            Some(s) => format!("refreshed {} ago", crate::util::short_dur(s)),
            None => "not refreshed yet".into(),
        };
        ui::line_lr(f, Rect { height: 1, ..area }, vec![Span::styled(" USAGE ", ui::bold_accent(t)), Span::styled(format!(" {ago} · every 60s"), muted(t))], vec![]);
        let Some(profiles) = st.profiles.ready() else {
            ui::line(f, Rect { y: area.y + 2, height: 1, ..area }, vec![Span::styled(format!("  profiles {}", st.profiles.why()), muted(t))]);
            return;
        };
        let body = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
        let mut y = body.y;
        let shown: Vec<&Profile> = profiles.iter().skip(self.scroll).collect();
        for p in shown {
            if y + 3 > body.bottom() {
                break;
            }
            let height = block(f, body, y, p, st.usage.get(&p.id), &st.picks, t);
            if p.authenticated {
                let h = if p.is_claude() { bro_core::Harness::Claude } else { bro_core::Harness::Codex };
                self.hits.push((Rect { y, height: height.min(body.bottom() - y), ..body }, h, p.id.clone()));
            }
            y += height;
        }
        if profiles.is_empty() {
            ui::line(f, Rect { y, height: 1, ..body }, vec![Span::styled("  no profiles yet — alt+o to add a Claude account or Codex profile", muted(t))]);
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        match key.code {
            KeyCode::Char('r') => {
                cx.svc.refresh_usage();
                cx.toast(crate::alerts::Kind::Usage, "refreshing usage…");
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => return false,
        }
        let n = cx.svc.state().profiles.ready().map(|p| p.len()).unwrap_or(0);
        self.scroll = self.scroll.min(n.saturating_sub(1));
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = ratatui::layout::Position { x: ev.column, y: ev.row };
                if let Some((_, h, profile)) = self.hits.iter().find(|(r, _, _)| r.contains(pos)) {
                    cx.act(Action::LaunchUsage(*h, Some(profile.clone())));
                }
            }
            MouseEventKind::ScrollDown => self.scroll += 1,
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
    }
}
