//! View panes (settings, stats, usage, profiles, proxy, bridge, quick install, setup, sounds, skills): each is drawn natively by
//! its own `gui` module next to its terminal-UI drawing, with that view's own state, when the module does; else
//! the terminal UI's drawing is painted into the card (see [`super::surface`]).

use super::kit::Ui;
use crate::app::App;
use crate::pane::{Cx, PaneId};
use crate::views::coordination::CoordinationView;
use crate::views::{bridge::BridgeView, profiles::ProfilesView, proxy::ProxyView, quick_install::QuickInstallView, settings::SettingsView, setup::SetupView, skills::SkillsView, sounds::SoundsView, stats::StatsView, usage::UsageView};
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;

/// Run `f` with the pane's own view type and a pane context, then apply what it asked for.
fn with_view<R>(app: &mut App, id: PaneId, focused: bool, f: impl FnOnce(&mut dyn std::any::Any, &mut Cx) -> R) -> Option<R> {
    let theme = app.theme.clone();
    let svc = app.svc.clone();
    let tx = app.tx.clone();
    let mut actions = vec![];
    let r = {
        let p = app.panes.get_mut(&id)?;
        let mut cx = Cx { id, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused };
        f((**p).as_any_mut(), &mut cx)
    };
    if !actions.is_empty() {
        app.apply(id, actions);
        let _ = app.tx.send(crate::pane::Event::Tick);
    }
    Some(r)
}

/// Draw view pane `id` natively in `area`. False when its module doesn't (yet): the caller paints the terminal
/// UI's drawing instead.
pub fn draw(app: &mut App, ui: &mut Ui, id: PaneId, area: Rect, focused: bool) -> bool {
    with_view(app, id, focused, |any, cx| {
        if let Some(v) = any.downcast_mut::<CoordinationView>() {
            crate::views::coordination::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<SettingsView>() {
            crate::views::settings::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<StatsView>() {
            crate::views::stats::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<UsageView>() {
            crate::views::usage::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<ProfilesView>() {
            crate::views::profiles::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<ProxyView>() {
            crate::views::proxy::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<BridgeView>() {
            crate::views::bridge::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<QuickInstallView>() {
            crate::views::quick_install::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<SetupView>() {
            crate::views::setup::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<SoundsView>() {
            crate::views::sounds::gui::draw(v, ui, area, cx)
        } else if let Some(v) = any.downcast_mut::<SkillsView>() {
            crate::views::skills::gui::draw(v, ui, area, cx)
        } else {
            false
        }
    })
    .unwrap_or(false)
}

/// Target `n` of view pane `id` was clicked.
pub fn click(app: &mut App, id: PaneId, n: usize, p: Point, m: KeyModifiers, count: u32) {
    with_view(app, id, true, |any, cx| {
        if let Some(v) = any.downcast_mut::<CoordinationView>() {
            crate::views::coordination::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<SettingsView>() {
            crate::views::settings::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<StatsView>() {
            crate::views::stats::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<UsageView>() {
            crate::views::usage::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<ProfilesView>() {
            crate::views::profiles::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<ProxyView>() {
            crate::views::proxy::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<BridgeView>() {
            crate::views::bridge::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<QuickInstallView>() {
            crate::views::quick_install::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<SetupView>() {
            crate::views::setup::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<SoundsView>() {
            crate::views::sounds::gui::click(v, n, p, m, count, cx)
        } else if let Some(v) = any.downcast_mut::<SkillsView>() {
            crate::views::skills::gui::click(v, n, p, m, count, cx)
        }
    });
}

/// Target `n` of view pane `id` was right-clicked at `p`: its menu, in the views that have one (Skills).
pub fn context(app: &mut App, id: PaneId, n: usize, p: Point) {
    with_view(app, id, true, |any, cx| {
        if let Some(v) = any.downcast_mut::<SkillsView>() {
            crate::views::skills::gui::context(v, n, p, cx)
        }
    });
}

/// The wheel over view pane `id` (`n`: the target under the pointer, if any).
pub fn wheel(app: &mut App, id: PaneId, n: Option<usize>, down: bool) {
    with_view(app, id, true, |any, cx| {
        if let Some(v) = any.downcast_mut::<CoordinationView>() {
            crate::views::coordination::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<SettingsView>() {
            crate::views::settings::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<StatsView>() {
            crate::views::stats::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<UsageView>() {
            crate::views::usage::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<ProfilesView>() {
            crate::views::profiles::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<ProxyView>() {
            crate::views::proxy::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<BridgeView>() {
            crate::views::bridge::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<QuickInstallView>() {
            crate::views::quick_install::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<SetupView>() {
            crate::views::setup::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<SoundsView>() {
            crate::views::sounds::gui::wheel(v, n, down, cx)
        } else if let Some(v) = any.downcast_mut::<SkillsView>() {
            crate::views::skills::gui::wheel(v, n, down, cx)
        }
    });
}
