//! Holding alt shows what it does, where it does it (as the terminal UI does): each session's card names its
//! jump key and the flip, the focused one adds its pane keys, the project tabs show theirs, and the sidebar
//! marks its session numbers and the keys of the things you can click. Keys show without the `alt+`.

use super::actions::Action;
use super::kit::{Style, Ui, radius, size, weight, widgets};
use super::layout::Layout;
use crate::app::App;
use crate::app::SideHit;
use crate::app::project_tabs::{StripHit, StripTab};
use crate::keymap::Act;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};

/// `act`'s key with alt held ("i" for alt+i), when it has an alt chord.
fn alt_key(app: &App, act: Act) -> Option<String> {
    app.keymap.chords_for(act).into_iter().find_map(|c| c.strip_prefix("alt+").map(str::to_string))
}

fn key_width(ui: &mut Ui, key: &str) -> f32 {
    let label = widgets::chord_keys(key).join(" + ");
    (ui.measure(&label, Style::new(size::SMALL, weight::SEMIBOLD, ui.t.text)) + 12.0).max(22.0)
}

/// A keycap inside a tips bar at `at` (top-left); returns its width.
fn cap(ui: &mut Ui, at: Point, key: &str) -> f32 {
    let t = ui.t;
    let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let b = ui.line(key, st, None);
    let r = Rect::new(at.x, at.y, (b.w + 12.0).max(22.0), 22.0);
    ui.panel(r, radius::CONTROL, t.control, t.stroke);
    ui.hline(r.x + 2.0, r.bottom() - ui.hair(), r.w - 4.0, if t.light { bro_gfx::Color::rgba(0.0, 0.0, 0.0, 0.18) } else { bro_gfx::Color::rgba(0.0, 0.0, 0.0, 0.45) });
    ui.draw(&b, Point::new(r.x + (r.w - b.w) / 2.0, r.y + (22.0 - b.h) / 2.0 - 0.5));
    r.w
}

/// The width of each chip of a tips bar ("i" + "flip").
fn tip_widths(ui: &mut Ui, tips: &[(String, &str)]) -> Vec<f32> {
    let t = ui.t;
    let label = Style::new(size::SMALL, weight::MEDIUM, t.text.mix(t.muted, 0.25));
    let mut widths = vec![];
    for (k, what) in tips {
        let kw = (ui.measure(k, Style::new(size::SMALL, weight::SEMIBOLD, t.text)) + 12.0).max(22.0);
        widths.push(kw + 8.0 + ui.measure(what, label) + 16.0);
    }
    widths
}

/// Chips with what they do: "i flip  z zoom".
fn tips(ui: &mut Ui, center: Point, tips: &[(String, &str)], max_w: f32) {
    let t = ui.t;
    let label = Style::new(size::SMALL, weight::MEDIUM, t.text.mix(t.muted, 0.25));
    let widths = tip_widths(ui, tips);
    let mut n = tips.len();
    while n > 0 && widths[..n].iter().sum::<f32>() > max_w {
        n -= 1;
    }
    if n == 0 {
        return;
    }
    let total: f32 = widths[..n].iter().sum::<f32>() + 14.0;
    let bar = Rect::new(center.x - total / 2.0, center.y - 18.0, total, 36.0);
    ui.shadow(bar, radius::OVERLAY, 24.0, 6.0, t.shadow.fade(0.8));
    ui.panel(bar, radius::OVERLAY, t.raised, t.border);
    let mut x = bar.x + 10.0;
    for ((k, what), w) in tips[..n].iter().zip(widths) {
        let kw = cap(ui, Point::new(x, bar.y + 7.0), k);
        ui.label_in(Rect::new(x + kw + 8.0, bar.y, w - kw - 8.0, bar.h), what, label, Align::Left);
        x += w;
    }
}

/// The key tips while alt is held, then the notifications over them (see [`super::toasts`]).
pub fn draw(app: &App, ui: &mut Ui, layout: &Layout) {
    if app.alt_hints() {
        draw_tips(app, ui, layout);
    }
    super::toasts::draw_late(app, ui, layout.window);
}

fn draw_tips(app: &App, ui: &mut Ui, layout: &Layout) {
    let prev = ui.layer();
    let focus = app.focused().filter(|_| !app.side_focus);
    let order = crate::sidebar::live_order(&app.live_infos(), &app.past_infos(), &app.open_infos());
    let jump = |id| order.iter().position(|p| *p == id).filter(|i| *i < 9).and_then(|i| alt_key(app, Act::Jump(i as u8 + 1)));
    // cards: what alt does to each, centred near the bottom
    for b in &layout.panes {
        if b.card.w < 160.0 || b.card.h < 90.0 || !app.panes.get(&b.id).is_some_and(|p| p.as_term_ref().is_some()) {
            continue;
        }
        let mut list: Vec<(String, &str)> = vec![("dbl-click".into(), "flip")];
        if focus != Some(b.id)
            && let Some(k) = jump(b.id)
        {
            list.push((k, "go"));
        }
        if focus == Some(b.id) {
            for (act, what) in [(Act::Flip, "flip"), (Act::FlipAll, "flip all"), (Act::Zoom, "zoom"), (Act::Grow, "grow")] {
                if let Some(k) = alt_key(app, act) {
                    list.push((k, what));
                }
            }
        }
        tips(ui, Point::new(b.card.x + b.card.w / 2.0, b.card.bottom() - 30.0), &list, b.card.w - 24.0);
    }
    // sidebar and tabs: keys on the things you can click
    let projects: Vec<String> = app.project_tabs().into_iter().map(|t| t.key).collect();
    let rows = app.rows();
    let hits: Vec<(Rect, Action)> = ui.hits.iter().map(|h| (h.rect, h.action.clone())).collect();
    let mut shown: Vec<Act> = vec![];
    for (r, action) in hits {
        let Action::Side(hit) = action else { continue };
        let act = match &hit {
            SideHit::Row(i) => {
                if let Some(crate::sidebar::Row::Live { info, n: Some(_) }) = rows.get(*i)
                    && let Some(k) = jump(info.pane)
                {
                    // over the session's number, clear of its mark
                    widgets::keytip(ui, Point::new(r.x + 21.0, r.y + r.h.min(32.0) / 2.0), &k);
                }
                continue;
            }
            SideHit::Usage => Act::Usage,
            SideHit::Stats => Act::Stats,
            SideHit::Proxy => Act::Proxy,
            SideHit::Bridge => Act::Bridge,
            SideHit::Settings => Act::Settings,
            SideHit::Launcher => Act::NewSession,
            SideHit::Continue => Act::Continue,
            SideHit::TileAll => Act::TileAll,
            SideHit::UsageToggle => Act::UsageDetails,
            SideHit::Strip(StripHit::Tab(StripTab::All)) => Act::ShowTab(0),
            SideHit::Strip(StripHit::Tab(StripTab::Orchestrator)) => Act::Orchestrator,
            SideHit::Strip(StripHit::Tab(StripTab::Project(k))) => match projects.iter().position(|p| p == k).filter(|i| *i < 9) {
                Some(i) => Act::ShowTab(i as u8 + 1),
                None => continue,
            },
            SideHit::Strip(StripHit::New) => Act::NewSession,
            _ => continue,
        };
        if shown.contains(&act) && !matches!(hit, SideHit::Strip(_)) {
            continue;
        }
        let Some(k) = alt_key(app, act) else { continue };
        shown.push(act);
        // where the tip goes: on a button or tab, across its bottom edge (WinUI's key tips); at the end of a row
        let centre = match &hit {
            SideHit::Launcher | SideHit::Continue | SideHit::Settings | SideHit::Strip(_) => Point::new(r.x + r.w / 2.0, r.bottom() - 2.0),
            _ => {
                let w = key_width(ui, &k);
                Point::new(r.right() - w / 2.0 - 4.0, r.y + r.h / 2.0)
            }
        };
        widgets::keytip(ui, centre, &k);
    }
    // the project tabs: after the last tab and its +, the keys that step through them, when there's room for all
    if layout.strip.h > 0.0 {
        let strip: Vec<(Rect, StripHit)> = ui
            .hits
            .iter()
            .filter_map(|h| match &h.action {
                Action::Side(SideHit::Strip(s)) => Some((h.rect, s.clone())),
                _ => None,
            })
            .collect();
        let last = strip.iter().filter(|(_, s)| matches!(s, StripHit::Tab(_) | StripHit::More | StripHit::New)).map(|(r, _)| r.right()).fold(layout.strip.x, f32::max);
        let end = strip.iter().filter(|(_, s)| matches!(s, StripHit::Minimized)).map(|(r, _)| r.x).fold(layout.strip.right() - 8.0, f32::min);
        let list: Vec<(String, &str)> = [(Act::PrevProject, "‹ tab"), (Act::NextProject, "tab ›"), (Act::LastTab, "last")]
            .into_iter()
            .filter_map(|(a, what)| alt_key(app, a).map(|k| (k, what)))
            .collect();
        if !list.is_empty() {
            let total = tip_widths(ui, &list).iter().sum::<f32>() + 14.0;
            let x = last + 12.0;
            if x + total <= end {
                tips(ui, Point::new(x + total / 2.0, layout.strip.y + layout.strip.h / 2.0 + 3.0), &list, total);
            }
        }
    }
    ui.set_layer(prev);
}
