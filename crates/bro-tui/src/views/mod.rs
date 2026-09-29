//! The service views, opened as panes (alt+u usage, alt+o profiles, alt+y proxy, alt+g bridge).

pub mod bridge;
pub mod profiles;
pub mod proxy;
pub mod usage;

use crate::pane::Pane;

/// Make a view pane by name.
pub fn open(name: &str) -> Option<Box<dyn Pane>> {
    Some(match name {
        "usage" => Box::new(usage::UsageView::new()),
        "profiles" => Box::new(profiles::ProfilesView::new()),
        "proxy" => Box::new(proxy::ProxyView::new()),
        "bridge" => Box::new(bridge::BridgeView::new()),
        _ => return None,
    })
}
