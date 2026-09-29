//! The empty state: logo, a one-line pitch, the keys that matter, and the service status at a glance.

use super::App;
use crate::keymap::Act;
use crate::services::Avail;
use crate::ui::{self, fg, muted};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};

impl App {
    pub(super) fn draw_welcome(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let inner = ui::frame(f, area, "", None, false, &t);
        let keys: Vec<(String, &str)> = vec![
            (self.keymap.primary(Act::NewSession), "launch an agent — claude, codex, pi, omp"),
            (self.keymap.primary(Act::FocusSidebar), "browse projects and resume past sessions"),
            (self.keymap.primary(Act::Palette), "command palette: every action"),
            (self.keymap.primary(Act::Usage), "usage meters for every account"),
            (self.keymap.primary(Act::Profiles), "profiles: add and log in"),
            (self.keymap.primary(Act::Bridge), "pair your phone"),
            (self.keymap.primary(Act::Help), "all keys"),
        ];
        let block_h = 6 + 2 + 1 + 2 + keys.len() as u16 + 2 + 1;
        let top = inner.y + inner.height.saturating_sub(block_h) / 2;
        let mut y = top;
        y += ui::logo(f, Rect { y, height: inner.bottom().saturating_sub(y), ..inner }, &t, time);
        y += 1;
        let tag = "the agentic terminal workspace";
        let tw = ui::width(tag) as u16;
        ui::line(f, Rect { x: inner.x + inner.width.saturating_sub(tw) / 2, y, width: tw.min(inner.width), height: 1 }, vec![Span::styled(tag, muted(&t))]);
        y += 2;
        // the call to action
        let key = self.keymap.primary(Act::NewSession);
        let cta = vec![Span::styled(format!(" {key} "), fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED)), Span::styled("  to launch your first agent", ui::bold())];
        let cw = (ui::width(&key) + 2 + 28) as u16;
        ui::line(f, Rect { x: inner.x + inner.width.saturating_sub(cw) / 2, y, width: cw.min(inner.width), height: 1 }, cta);
        y += 2;
        let kw = keys.iter().map(|k| ui::width(&k.0)).max().unwrap_or(6) as u16 + 2;
        let block_w = (kw + 44).min(inner.width);
        let x = inner.x + inner.width.saturating_sub(block_w) / 2;
        for (k, what) in &keys {
            if y >= inner.bottom() {
                break;
            }
            ui::line(f, Rect { x, y, width: block_w, height: 1 }, vec![Span::styled(ui::pad(k, kw as usize), fg(t.shine).add_modifier(Modifier::BOLD)), Span::styled(what.to_string(), muted(&t))]);
            y += 1;
        }
        y += 1;
        if y < inner.bottom() {
            let st = self.svc.state();
            let n_prof = st.profiles.ready().map(|p| p.iter().filter(|p| p.authenticated).count());
            let mut spans = vec![];
            spans.push(match n_prof {
                Some(n) => Span::styled(format!("● {n} logged-in profile{}", if n == 1 { "" } else { "s" }), fg(if n > 0 { t.good } else { t.inline })),
                None => Span::styled("◌ profiles loading", muted(&t)),
            });
            spans.push(Span::styled("   ", muted(&t)));
            spans.push(match &st.proxy.status {
                Avail::Ready(p) => Span::styled(format!("● proxy :{}", p.port), fg(t.good)),
                _ => Span::styled("○ proxy off", muted(&t)),
            });
            spans.push(Span::styled("   ", muted(&t)));
            spans.push(match &st.bridge.status {
                Avail::Ready(b) if b.running => Span::styled(format!("● bridge :{} · {} dev", b.port, b.clients), fg(t.good)),
                _ => Span::styled("○ bridge off", muted(&t)),
            });
            let w: u16 = spans.iter().map(|s| ui::width(&s.content) as u16).sum();
            ui::line(f, Rect { x: inner.x + inner.width.saturating_sub(w) / 2, y, width: w.min(inner.width), height: 1 }, spans);
        }
    }
}
