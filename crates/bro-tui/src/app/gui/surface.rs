//! What a natively drawn overlay or view gets besides the [`Ui`](super::kit::Ui).
//!
//! The contract every surface module follows (see docs/gui/ARCHITECTURE.md):
//! - `draw(state, ui, area, …) -> bool` draws it and registers clickable targets with
//!   `ui.hit(rect, Action::Overlay(n))` (overlays) or `ui.hit(rect, Action::View(pane, n))` (views), numbering its
//!   targets however it likes; returning false falls back to the terminal UI's own drawing for that surface.
//! - `click(state, n, …)` runs target `n`; `wheel(state, n, down, …)` scrolls.
//! - Keys never come here: they reach the same state machine through the workspace, exactly as in the terminal.

use crate::keymap::Keymap;
use crate::services::Services;

#[allow(dead_code)]
pub struct Cx<'a> {
    pub svc: &'a Services,
    pub keymap: &'a Keymap,
    /// seconds since the app started (spinners)
    pub time: f64,
    /// the window has the keyboard
    pub focused: bool,
    /// alt has been held long enough to show key tips (a surface draws them on its own alt keys)
    pub alt: bool,
}
