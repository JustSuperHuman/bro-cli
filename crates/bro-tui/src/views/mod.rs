//! The service views, opened as panes (alt+u usage, alt+o profiles, alt+y proxy, alt+g bridge, prefix S stats, prefix A settings, quick install from settings or the palette).

pub mod settings;
pub mod quick_install;
pub mod bridge;
pub mod profiles;
pub mod proxy;
pub mod stats;
pub mod usage;

use crate::pane::Pane;

/// Make a view pane by name.
pub fn open(name: &str) -> Option<Box<dyn Pane>> {
    Some(match name {
        "usage" => Box::new(usage::UsageView::new()),
        "profiles" => Box::new(profiles::ProfilesView::new()),
        "proxy" => Box::new(proxy::ProxyView::new()),
        "bridge" => Box::new(bridge::BridgeView::new()),
        "stats" => Box::new(stats::StatsView::new()),
        "settings" => Box::new(settings::SettingsView::new()),
        "quick-install" => Box::new(quick_install::QuickInstallView::new()),
        _ => return None,
    })
}
