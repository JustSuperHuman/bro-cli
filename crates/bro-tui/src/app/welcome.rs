//! The empty state: a turning world (when there's room), the logo, a one-line pitch, the keys that matter, a
//! "Did you know?" tip, and the service status at a glance.

use super::App;
use super::tips::{self, Tip};
use crate::keymap::Act;
use crate::services::Avail;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};

impl App {
    /// Once, when the profiles first load: open the setup guide on a bare start (no tabs) while nothing is signed in,
    /// unless it was dismissed. Never for demos or tests.
    pub(super) fn offer_setup(&mut self) {
        if !self.persist || self.svc.is_demo() {
            self.setup_offered = true;
            return;
        }
        let open = {
            let st = self.svc.state();
            match st.profiles.ready() {
                Some(ps) => !st.settings.setup_done && !ps.iter().any(|p| p.authenticated),
                None if matches!(st.profiles, Avail::Loading) => return,
                None => !st.settings.setup_done,
            }
        };
        self.setup_offered = true;
        // The desktop's welcome screen now provides agent, browser and computer onboarding.
        // Keep the automatic text guide in terminals, which don't have those setup cards.
        if open && self.tabs.is_empty() && !crate::panes::mirror::active() {
            self.open_view("setup");
        }
    }

    pub(super) fn draw_welcome(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let inner = ui::frame(f, area, "", None, false, &t);
        let keys: Vec<(String, &str)> = vec![
            ("w / c".into(), "browser and computer: set up JustBrowse and CUA"),
            (self.keymap.primary(Act::NewSession), "launch an agent — claude, codex, pi, omp"),
            (self.keymap.primary(Act::FocusSidebar), "browse projects and resume past sessions"),
            (self.keymap.primary(Act::Palette), "command palette: every action"),
            (self.keymap.primary(Act::Usage), "usage meters for every account"),
            (self.keymap.primary(Act::Profiles), "profiles: add and log in"),
            (self.keymap.primary(Act::Setup), "setup guide: install an agent, sign in"),
            (self.keymap.primary(Act::Bridge), "pair your phone"),
            (self.keymap.primary(Act::Help), "all keys"),
        ];
        let block_h = 6 + 2 + 1 + 2 + keys.len() as u16 + 2 + 1;
        // a tip under the keys when the screen has the height (before the world gets any)
        self.tip_hits.clear();
        let tips = self.tip_list();
        let tip_on = !tips.is_empty() && inner.height >= block_h + TIP_ROWS + 1 && inner.width >= 44;
        let block_h = block_h + if tip_on { TIP_ROWS } else { 0 };
        // a turning world above it all, with the agents for moons, when the screen has the room
        let world_h = inner.height.saturating_sub(block_h + 2).min(18);
        let world_h = if world_h >= 9 && inner.width >= 50 { world_h } else { 0 };
        let top = inner.y + inner.height.saturating_sub(block_h + world_h + 1) / 2;
        let mut y = top;
        if world_h > 0 {
            // tests draw one fixed moment
            let at = if cfg!(test) { 40.0 } else { 40.0 + time };
            ui::rune(f, Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: world_h }, crate::rune::Scene::World, at, &t);
            y += world_h + 1;
        }
        y += ui::logo(f, Rect { y, height: inner.bottom().saturating_sub(y), ..inner }, &t, time);
        y += 1;
        let tag = "the agentic terminal workspace";
        let tw = ui::width(tag) as u16;
        ui::line(f, Rect { x: inner.x + inner.width.saturating_sub(tw) / 2, y, width: tw.min(inner.width), height: 1 }, vec![Span::styled(tag, muted(&t))]);
        y += 2;
        // the call to action
        // nothing signed in yet: setup comes first
        let signed_in = self.svc.state().profiles.ready().is_none_or(|ps| ps.iter().any(|p| p.authenticated));
        let (key, what) = if signed_in { (self.keymap.primary(Act::NewSession), "  to launch your first agent") } else { (self.keymap.primary(Act::Setup), "  to set up: install an agent and sign in") };
        let cta = vec![Span::styled(format!(" {key} "), fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED)), Span::styled(what, ui::bold())];
        let cw = (ui::width(&key) + 2 + ui::width(what)) as u16;
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
        if tip_on && y + TIP_ROWS <= inner.bottom() {
            let tw = block_w.max(66).min(inner.width.saturating_sub(2));
            let tx = inner.x + inner.width.saturating_sub(tw) / 2;
            self.draw_tip(f, Rect { x: tx, y, width: tw, height: TIP_ROWS - 1 }, &tips, &t);
            y += TIP_ROWS;
        }
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

    /// "Did you know?": one tip in `r` (a heading with where it is among them and the arrows to move through them,
    /// the keys and what they get you, two lines more). A click on an arrow moves, on the tip goes on; so does the
    /// wheel over it.
    fn draw_tip(&mut self, f: &mut Frame, r: Rect, tips: &[Tip], t: &Theme) {
        let i = self.tips.current(tips.len(), std::time::Instant::now(), false);
        let Some(tip) = tips.get(i) else { return };
        let count = format!("{}/{}", i + 1, tips.len());
        let nav_w = ui::width(&count) as u16 + 4;
        let nx = r.right().saturating_sub(nav_w);
        ui::line(f, Rect { height: 1, ..r }, vec![Span::styled(format!("{}Did you know?", ui::lead("tip")), fg(t.accent).add_modifier(Modifier::BOLD))]);
        ui::line(f, Rect { x: nx, y: r.y, width: nav_w, height: 1 }, vec![Span::styled("‹ ", fg(t.shine)), Span::styled(count, muted(t)), Span::styled(" ›", fg(t.shine))]);
        // the arrows first: the first target under the pointer wins
        self.tip_hits.push((Rect { x: nx, y: r.y, width: 2, height: 1 }, -1));
        self.tip_hits.push((Rect { x: r.right().saturating_sub(2), y: r.y, width: 2, height: 1 }, 1));
        self.tip_hits.push((r, 0));
        let how = tips::spoken(&tip.how);
        let gw = ui::width(&how) + 3;
        ui::line(f, Rect { y: r.y + 1, height: 1, ..r }, vec![Span::styled(ui::pad(&how, gw), fg(t.shine).add_modifier(Modifier::BOLD)), Span::styled(tip.title, ui::bold())]);
        for (n, l) in crate::card::wrap(&tip.detail, r.width as usize, r.height.saturating_sub(2) as usize).into_iter().enumerate() {
            ui::line(f, Rect { y: r.y + 2 + n as u16, height: 1, ..r }, vec![Span::styled(l, muted(t))]);
        }
    }
}

/// The terminal UI's tip: its heading, the keys and title, two lines of detail and a blank row above it.
const TIP_ROWS: u16 = 5;
