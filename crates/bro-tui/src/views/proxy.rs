//! Proxy view: where the proxy listens, the routes launches registered, and the live request log (status,
//! tokens, latency colour-coded).

use crate::pane::{Cx, Pane};
use crate::services::Avail;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_proxy::{ProxyEvent, Upstream};
use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};

/// The proxy pane.
#[derive(Default)]
pub struct ProxyView {
    /// rows scrolled back in the log (0 = newest)
    scroll: usize,
}

impl ProxyView {
    pub fn new() -> ProxyView {
        ProxyView::default()
    }
}

/// "12.3k"
pub fn tokens(n: Option<u64>) -> String {
    match n {
        None => "—".into(),
        Some(n) if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        Some(n) if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        Some(n) => n.to_string(),
    }
}

/// Local "HH:MM:SS" for a unix-ms timestamp.
pub fn hms(at_ms: i64) -> String {
    let s = (at_ms / 1000 + crate::util::local_offset_secs()).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Where an upstream goes, briefly.
pub fn upstream_label(u: &Upstream) -> String {
    let host = |url: &str| url.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or(url).to_string();
    match u {
        Upstream::OpenAiChat { base_url, .. } => format!("chat · {}", host(base_url)),
        Upstream::OpenAiResponses { base_url, .. } => format!("responses · {}", host(base_url)),
        Upstream::ChatGptCodex { .. } => "chatgpt codex backend".into(),
        Upstream::Anthropic { base_url, .. } => format!("messages · {}", host(base_url)),
        Upstream::ClaudeOAuth { config_dir } => format!("claude oauth · {}", config_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        Upstream::ClaudePool { config_dirs } => format!("claude pool · {} accounts", config_dirs.len()),
    }
}

fn status_style(e: &ProxyEvent, t: &Theme) -> Style {
    match e.status {
        200..=299 => fg(t.good),
        429 => fg(t.inline).add_modifier(Modifier::BOLD),
        400..=499 => fg(t.inline),
        _ => fg(t.danger).add_modifier(Modifier::BOLD),
    }
}

fn latency_style(ms: u64, t: &Theme) -> Style {
    if ms < 2_000 {
        fg(t.good)
    } else if ms < 8_000 {
        fg(t.inline)
    } else {
        fg(t.danger)
    }
}

impl Pane for ProxyView {
    fn title(&self) -> String {
        "proxy".into()
    }
    fn icon(&self) -> &'static str {
        "proxy"
    }
    fn view(&self) -> Option<&'static str> {
        Some("proxy")
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        let st = cx.svc.state();
        let p = &st.proxy;
        let area = ui::hint_line(f, area, &[("j/k", "scroll log"), ("g", "newest")], t);
        let row = |y: u16| Rect { y, height: 1, ..area };
        let mut y = area.y;
        let head = match &p.status {
            Avail::Ready(i) => vec![
                Span::styled(" PROXY ", ui::bold_accent(t)),
                Span::styled(" ● ", fg(t.good)),
                Span::styled(format!(":{}", i.port), ui::bold()),
                Span::styled(format!("   {}", i.base), muted(t)),
                Span::styled(format!("   {} routes · {} requests seen", p.routes.len(), p.events.len()), muted(t)),
            ],
            other => vec![Span::styled(" PROXY ", ui::bold_accent(t)), Span::styled(" ○ ", fg(t.danger)), Span::styled(other.why(), muted(t))],
        };
        ui::line(f, row(y), head);
        y += 2;
        ui::rule(f, row(y), "routes", t);
        y += 1;
        if p.routes.is_empty() {
            ui::line(f, row(y), vec![Span::styled("  none yet — launching a harness on a foreign provider (e.g. claude on gpt, codex on claude) adds one", muted(t))]);
            y += 1;
        }
        for r in p.routes.iter().take(6) {
            let model = r.default_model.clone().unwrap_or_else(|| "client's model".into());
            ui::line(f, row(y), vec![
                Span::styled(format!("  {:<8}", r.id), fg(t.shine)),
                Span::styled(ui::pad(&r.label, 34), ui::bold()),
                Span::styled(ui::pad(&upstream_label(&r.upstream), 34), muted(t)),
                Span::styled(model, fg(t.inline)),
            ]);
            y += 1;
        }
        if !p.pool.is_empty() {
            y += 1;
            ui::rule(f, row(y), "claude pool", t);
            y += 1;
            let now = crate::util::now_ms();
            for a in p.pool.iter().take(6) {
                let cooling = a.rate_limited_until_ms.filter(|t| *t > now);
                let (dot, dc, state) = match (a.available, cooling) {
                    (_, Some(until)) => ("◌", t.inline, format!("cooling {}", crate::util::countdown((until - now) / 1000))),
                    (true, None) => ("●", t.good, "ready".to_string()),
                    (false, None) => ("○", t.danger, "unavailable".to_string()),
                };
                let u = &a.usage;
                ui::line(f, row(y), vec![
                    Span::styled(format!("  {dot} "), fg(dc)),
                    Span::styled(ui::pad(&a.name, 16), ui::bold()),
                    Span::styled(ui::pad(&state, 16), fg(dc)),
                    Span::styled(format!("5h {:>4} req  {:>7} in  {:>7} out  ${:.2}", u.window_requests, tokens(Some(u.window_input_tokens)), tokens(Some(u.window_output_tokens)), u.window_cost_usd), muted(t)),
                    Span::styled(u.last_error.as_deref().map(|e| format!("   {e}")).unwrap_or_default(), fg(t.danger)),
                ]);
                y += 1;
            }
        }
        y += 1;
        if y >= area.bottom() {
            return;
        }
        ui::rule(f, row(y), "live requests", t);
        y += 1;
        let cols = format!("  {:<9} {:<4} {:<22} {:<26} {:>7} {:>7} {:>7} {:>7}  {}", "time", "code", "in → upstream", "model", "in", "out", "cache", "latency", "note");
        ui::line(f, row(y), vec![Span::styled(cols, muted(t).add_modifier(Modifier::BOLD))]);
        y += 1;
        let room = area.bottom().saturating_sub(y) as usize;
        self.scroll = self.scroll.min(p.events.len().saturating_sub(room.max(1)));
        for e in p.events.iter().rev().skip(self.scroll).take(room) {
            let route = format!("{} → {}", e.inbound, e.upstream);
            let note = e.error.clone().or_else(|| e.account.as_ref().map(|a| format!("acct {a}"))).unwrap_or_else(|| if e.stream { "stream".into() } else { String::new() });
            ui::line(f, row(y), vec![
                Span::styled(format!("  {:<9} ", hms(e.at_ms)), muted(t)),
                Span::styled(format!("{:<4} ", e.status), status_style(e, t)),
                Span::styled(format!("{} ", ui::pad(&route, 22)), Style::default()),
                Span::styled(format!("{} ", ui::pad(&e.model, 26)), fg(t.shine)),
                Span::styled(format!("{:>7} ", tokens(e.input_tokens)), Style::default()),
                Span::styled(format!("{:>7} ", tokens(e.output_tokens)), Style::default()),
                Span::styled(format!("{:>7} ", tokens(e.cache_read_tokens)), muted(t)),
                Span::styled(format!("{:>6.1}s  ", e.latency_ms as f64 / 1000.0), latency_style(e.latency_ms, t)),
                Span::styled(note, if e.error.is_some() { fg(t.danger) } else { muted(t) }),
            ]);
            y += 1;
        }
        if p.events.is_empty() {
            ui::line(f, row(y), vec![Span::styled("  no requests yet", muted(t))]);
        }
    }
    fn key(&mut self, key: KeyEvent, _cx: &mut Cx) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, _cx: &mut Cx) {
        match ev.kind {
            MouseEventKind::ScrollDown => self.scroll += 3,
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(tokens(Some(950)), "950");
        assert_eq!(tokens(Some(12_300)), "12.3k");
        assert_eq!(tokens(Some(2_500_000)), "2.5M");
        assert_eq!(tokens(None), "—");
        assert_eq!(upstream_label(&Upstream::OpenAiChat { base_url: "https://api.x.ai/v1".into(), api_key: None }), "chat · api.x.ai");
        assert_eq!(hms(0).len(), 8);
    }
}
