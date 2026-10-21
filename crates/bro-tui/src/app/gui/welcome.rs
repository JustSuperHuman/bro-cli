//! The empty workspace: the wordmark, what to do first, the keys that matter, and whether the logins, the
//! proxy and the phone bridge are up. Same content as the terminal UI's welcome screen.

use super::actions::Action;
use super::kit::{Style, Ui, radius, size, weight, widgets};
use crate::app::App;
use crate::keymap::Act;
use crate::services::Avail;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};

pub fn draw(app: &mut App, ui: &mut Ui, area: Rect) {
    let t = ui.t;
    let w = 520.0f32.min(area.w - 48.0);
    let h = 470.0f32.min(area.h - 32.0);
    let r = Rect::new(area.x + (area.w - w) / 2.0, area.y + (area.h - h) / 2.0, w, h);
    // the wordmark in the terminal UI's logo colours: a rainbow moving across it on animated themes, else the
    // accent fading to the theme's shine
    let time = ui.time;
    let letters = ["b", "r", "o"];
    let colors: Vec<Color> = (0..letters.len())
        .map(|i| {
            // where the letter sits along the terminal UI's 25-column logo
            let col = 4.0 + 8.0 * i as f64;
            if app.theme.animated {
                let c = crate::theme::rainbow_at(col * 0.018 - time * 0.06, 0.5);
                super::kit::color(c, t.accent, &t.ansi)
            } else {
                t.accent.mix(t.shine, (col / 25.0) as f32 * 0.8)
            }
        })
        .collect();
    let spans: Vec<(String, Style)> = letters.iter().zip(&colors).map(|(l, c)| (l.to_string(), Style::new(64.0, weight::BOLD, *c))).collect();
    let mark = ui.rich(&spans, 64.0, None, false, Some(1));
    let mx = r.x + (r.w - mark.w - 18.0) / 2.0;
    ui.draw(&mark, Point::new(mx, r.y));
    ui.circle(Point::new(mx + mark.w + 10.0, r.y + mark.h - 22.0), 7.0, colors[colors.len() - 1]);
    if app.theme.animated {
        ui.ticking = true;
    }
    let tag = ui.line("The agentic terminal workspace", Style::new(size::TITLE, weight::MEDIUM, t.muted), None);
    ui.draw(&tag, Point::new(r.x + (r.w - tag.w) / 2.0, r.y + mark.h + 4.0));
    let mut y = r.y + mark.h + 44.0;
    // call to action: nothing signed in yet (once the profiles have loaded), setting up comes first
    let nobody = app.svc.state().profiles.ready().is_some_and(|ps| !ps.iter().any(|p| p.authenticated));
    let (label, act) = if nobody { ("Install an agent and sign in", Act::Setup) } else { ("Launch your first agent", Act::NewSession) };
    let bw = widgets::button_width(ui, label) + 24.0;
    let br = Rect::new(r.x + (r.w - bw) / 2.0, y, bw, 36.0);
    widgets::button(ui, br, label, widgets::Kind::Primary, Action::Welcome(if nobody { 1 } else { 0 }));
    let k = app.keymap.primary(act);
    if !k.is_empty() {
        let st = Style::new(size::SMALL, weight::MEDIUM, t.faint);
        let kw = ui.measure(&k, st);
        ui.label(Point::new(r.x + (r.w - kw) / 2.0, br.bottom() + 8.0), &k, st);
    }
    y = br.bottom() + 40.0;
    // the keys that matter
    let keys: [(Act, &str); 8] = [
        (Act::NewSession, "Launch an agent"),
        (Act::FocusSidebar, "Browse projects"),
        (Act::Palette, "Command palette"),
        (Act::Usage, "Usage meters"),
        (Act::Profiles, "Profiles and logins"),
        (Act::Setup, "Setup guide"),
        (Act::Bridge, "Pair your phone"),
        (Act::Help, "All keys"),
    ];
    let col_w = (r.w - 24.0) / 2.0;
    for (i, (act, what)) in keys.iter().enumerate() {
        let cx = r.x + 12.0 + (i % 2) as f32 * col_w;
        let cy = y + (i / 2) as f32 * 34.0;
        let row = Rect::new(cx, cy, col_w - 8.0, 30.0);
        let h = ui.hover_amount(("welcome-key", i), row);
        ui.round(row, radius::CONTROL, t.hover.fade(h));
        ui.label_in(Rect::new(row.x + 10.0, row.y, row.w - 120.0, row.h), what, Style::new(size::BODY, weight::MEDIUM, t.text.mix(t.muted, 0.25)), Align::Left);
        let chord = app.keymap.primary(*act);
        if !chord.is_empty() {
            // measure the caps off-screen first, then right-align them
            let kw = widgets::kbd_width(ui, &chord);
            widgets::kbd(ui, Point::new(row.right() - kw - 8.0, row.y + 6.0), &chord);
        }
        ui.hit(row, Action::Welcome(10 + i));
    }
    y += 4.0 * 34.0 + 22.0;
    // status line
    let st = app.svc.state();
    let profiles = match &st.profiles {
        Avail::Ready(ps) => {
            let n = ps.iter().filter(|p| p.authenticated).count();
            (if n > 0 { t.good } else { t.inline }, format!("{n} logged-in profile{}", if n == 1 { "" } else { "s" }))
        }
        Avail::Loading => (t.muted, "profiles loading".into()),
        Avail::Unavailable(_) => (t.danger, "profiles unavailable".into()),
    };
    let proxy = match &st.proxy.status {
        Avail::Ready(i) => (t.good, format!("proxy :{}", i.port)),
        Avail::Loading => (t.muted, "proxy starting".into()),
        Avail::Unavailable(_) => (t.danger, "proxy off".into()),
    };
    let bridge = match &st.bridge.status {
        Avail::Ready(s) if s.running => (t.good, format!("bridge :{} · {} dev", s.port, s.clients)),
        Avail::Loading if st.bridge.enabled => (t.muted, "bridge starting".into()),
        _ => (t.muted, "bridge off".into()),
    };
    drop(st);
    let items = [profiles, proxy, bridge];
    let sst = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let widths: Vec<f32> = items.iter().map(|(_, s)| ui.measure(s, sst) + 16.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 16.0 * (items.len() - 1) as f32;
    let mut x = r.x + (r.w - total) / 2.0;
    for ((c, s), w) in items.iter().zip(widths) {
        let ir = Rect::new(x - 2.0, y, 14.0, 16.0);
        if *c == t.good {
            ui.icon(ir, "\u{F012C}", size::SMALL, t.good);
        } else if *c == t.danger {
            ui.icon(ir, "\u{F0156}", size::SMALL, t.danger);
        } else if *c == t.inline {
            ui.icon(ir, "\u{F0028}", size::SMALL, t.inline);
        } else {
            ui.icon(ir, "\u{F0374}", size::SMALL, t.faint);
        }
        ui.label(Point::new(x + 14.0, y), s, sst);
        x += w + 16.0;
    }
}

/// The welcome screen's targets: 0 launch, 1 set up, 10.. the key rows.
pub fn click(app: &mut App, n: usize) {
    let acts = [Act::NewSession, Act::FocusSidebar, Act::Palette, Act::Usage, Act::Profiles, Act::Setup, Act::Bridge, Act::Help];
    match n {
        0 => app.run_act(Act::NewSession),
        1 => app.run_act(Act::Setup),
        n if n >= 10 => {
            if let Some(a) = acts.get(n - 10) {
                app.run_act(*a);
            }
        }
        _ => {}
    }
}
