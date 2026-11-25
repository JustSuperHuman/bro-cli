//! What a click can do. Hits carry one of these; [`super::mouse`] runs it.
//!
//! Surfaces with many targets (overlays, views, the media panel) keep their own list of targets from the last
//! frame and register an index into it, so this enum stays small.

use crate::app::SideHit;
use crate::layout::PaneId;

#[derive(Clone, Debug)]
pub enum Action {
    /// Swallows the click (panels, card backgrounds).
    None,
    /// A sidebar target (the same targets as the terminal UI's sidebar).
    Side(SideHit),
    /// Focus the sidebar's filter.
    SideFilter,
    /// The sidebar's right edge: drag to resize, double-click for the automatic width.
    SideResize,
    /// The sidebar's list (wheel scrolls it).
    SideList,
    /// The sidebar list's scrollbar: drag the thumb, or press the track to jump there.
    SideScroll,
    /// Something on a pane.
    Pane(PaneId, PaneAct),
    /// The open overlay's target `n` (its own list from the last frame).
    Overlay(usize),
    /// An entry in the shared session context menu.
    Menu(usize),
    /// A view pane's target `n`.
    View(PaneId, usize),
    ThemeEditor(usize),
    /// Target `n` of a session's focus view (its conversation, reply box, choice).
    Focus(PaneId, usize),
    /// The media panel's target `n`.
    Media(usize),
    /// The media panel's resize edge.
    MediaResize,
    /// Show or hide the media panel.
    MediaToggle,
    /// Voice controls: 0 toggle push-to-talk, 2 settings, 3 browser call, 4 end calls.
    Voice(u8),
    /// Line `n` of the open context menu. `side_menu::OUTSIDE` closes it.
    SideMenu(usize),
    /// The + on the sidebar's projects: open a project folder.
    OpenProject,
    /// Dismiss toast `n`.
    Toast(usize),
    /// The welcome screen's target `n`.
    Welcome(usize),
    /// An exact agent-requested handoff; the id protects against a stale button.
    BrowserHandoff { tab_id: u32, id: String, finish: bool },
    BrowserHandoffNext,
    /// Close the open overlay (clicks on a scrim).
    CloseOverlay,
}

/// What a click on a pane does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneAct {
    /// the card's header: focus, double-click zooms
    Header,
    /// the content: a terminal (selection, links, mouse reporting) or a view
    Body,
    Close,
    Minimize,
}
