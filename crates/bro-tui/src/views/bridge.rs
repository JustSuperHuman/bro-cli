//! Bridge view: pairing for the Just Terminal phone app and web client — URLs, the token (masked until you
//! reveal it), a pairing QR, connected clients, and an on/off toggle.

use crate::alerts::Kind;
use crate::pane::{Cx, Pane};
use crate::services::Avail;
use crate::ui::{self, fg, muted};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};

/// The bridge pane.
#[derive(Default)]
pub struct BridgeView {
    reveal: bool,
    sel: usize,
}

impl BridgeView {
    pub fn new() -> BridgeView {
        BridgeView::default()
    }
}

/// `abcd…` → `••••••••wxyz` (last four kept).
pub fn mask(token: &str) -> String {
    let n = token.chars().count();
    if n <= 4 {
        return "•".repeat(n);
    }
    format!("{}{}", "•".repeat(8), token.chars().skip(n - 4).collect::<String>())
}

/// A URL with its token masked (unless revealed).
pub fn show_url(url: &str, token: &str, reveal: bool) -> String {
    if reveal || token.is_empty() { url.to_string() } else { url.replace(token, &mask(token)) }
}

impl Pane for BridgeView {
    fn title(&self) -> String {
        "bridge".into()
    }
    fn icon(&self) -> &'static str {
        "bridge"
    }
    fn view(&self) -> Option<&'static str> {
        Some("bridge")
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        let st = cx.svc.state();
        let b = &st.bridge;
        let area = ui::hint_line(f, area, &[("t", if b.enabled { "turn off" } else { "turn on" }), ("r", if self.reveal { "hide token" } else { "reveal token" }), ("j/k", "pick url"), ("c", "copy url")], t);
        let row = |y: u16| Rect { y, height: 1, ..area };
        let mut y = area.y;
        let status = match &b.status {
            Avail::Ready(s) if s.running => vec![
                Span::styled(" BRIDGE ", ui::bold_accent(t)),
                Span::styled(" ● ", fg(t.good)),
                Span::styled(format!("running on :{}", s.port), ui::bold()),
                Span::styled(format!("   {} client{} connected", s.clients, if s.clients == 1 { "" } else { "s" }), if s.clients > 0 { fg(t.shine) } else { muted(t) }),
            ],
            Avail::Ready(s) => vec![Span::styled(" BRIDGE ", ui::bold_accent(t)), Span::styled(" ○ stopped ", fg(t.danger)), Span::styled(s.error.clone().unwrap_or_default(), muted(t))],
            other => vec![Span::styled(" BRIDGE ", ui::bold_accent(t)), Span::styled(" ○ ", fg(t.danger)), Span::styled(if b.enabled { other.why() } else { "off — t turns it on".into() }, muted(t))],
        };
        ui::line(f, row(y), status);
        y += 1;
        ui::line(f, row(y), vec![Span::styled("  Pair the Just Terminal app or open a URL in a browser: your agent sessions show up there live.", muted(t))]);
        y += 2;
        let Avail::Ready(s) = &b.status else { return };
        // URLs + token on the left, QR on the right
        let qr_w = b.qr.as_ref().and_then(|q| q.first()).map(|l| l.chars().count() as u16).unwrap_or(0);
        let left_w = area.width.saturating_sub(qr_w + 4);
        let left = |y: u16| Rect { y, height: 1, width: left_w, ..area };
        ui::rule(f, left(y), "urls", t);
        y += 1;
        self.sel = self.sel.min(s.urls.len().saturating_sub(1));
        for (i, u) in s.urls.iter().enumerate() {
            let on = i == self.sel;
            let kind = if i == 0 { "this machine" } else if u.contains("://100.") { "tailscale" } else { "lan" };
            ui::line(f, left(y), vec![
                Span::styled(if on { " ▌" } else { "  " }, ui::accent(t)),
                Span::styled(ui::pad(kind, 13), muted(t)),
                Span::styled(show_url(u, &s.token, self.reveal), if on { ui::bold_accent(t) } else { ui::bold() }),
            ]);
            y += 1;
        }
        y += 1;
        ui::rule(f, left(y), "token", t);
        y += 1;
        ui::line(f, left(y), vec![
            Span::styled("  ", muted(t)),
            Span::styled(if self.reveal { s.token.clone() } else { mask(&s.token) }, fg(t.inline).add_modifier(Modifier::BOLD)),
            Span::styled(if self.reveal { "   r hides it" } else { "   r reveals it" }, muted(t)),
        ]);
        y += 2;
        if let Some(e) = &s.error {
            ui::line(f, left(y), vec![Span::styled(format!("  {e}"), fg(t.danger))]);
        }
        if let Some(qr) = &b.qr {
            let x = area.right().saturating_sub(qr_w + 2);
            let top = area.y + 3;
            ui::line(f, Rect { x, y: top, width: qr_w, height: 1 }, vec![Span::styled(ui::pad("  scan to pair", qr_w as usize), muted(t))]);
            for (k, l) in qr.iter().enumerate() {
                let yy = top + 1 + k as u16;
                if yy >= area.bottom() {
                    break;
                }
                ui::line(f, Rect { x, y: yy, width: qr_w, height: 1 }, vec![Span::styled(l.clone(), fg(ratatui::style::Color::White).bg(ratatui::style::Color::Black))]);
            }
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        match key.code {
            KeyCode::Char('r') => self.reveal = !self.reveal,
            KeyCode::Char('t') => {
                let on = !cx.svc.state().bridge.enabled;
                cx.svc.set_bridge_enabled(on);
                cx.toast(Kind::Bridge, if on { "starting the bridge…" } else { "bridge off" });
            }
            KeyCode::Char('j') | KeyCode::Down => self.sel += 1,
            KeyCode::Char('k') | KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('c') | KeyCode::Enter => {
                let url = match &cx.svc.state().bridge.status {
                    Avail::Ready(s) => s.urls.get(self.sel).cloned(),
                    _ => None,
                };
                if let Some(u) = url {
                    crate::clip::copy(&u);
                    cx.toast(Kind::Info, "copied the pairing URL (it includes the token)");
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking() {
        assert_eq!(mask("abcdef123456"), "••••••••3456");
        assert_eq!(mask("abc"), "•••");
        assert_eq!(show_url("http://x/?token=secret99", "secret99", false), "http://x/?token=••••••••et99");
        assert_eq!(show_url("http://x/?token=secret99", "secret99", true), "http://x/?token=secret99");
    }
}
