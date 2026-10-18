//! Back/forward through what you looked at, like a browser: the mouse's back/forward buttons (and a keyboard's
//! browser keys) step through recently focused sessions and the new-session screen. A full-screen (zoomed) view
//! stays full screen while you step; in a tiled view only the tiled sessions are stepped through.

use super::App;
use super::overlays::Overlay;
use crate::layout::{Node, PaneId};
use crate::pane::Place;

/// Entries kept; older ones fall off the back.
const MAX: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    Pane(PaneId),
    /// the new-session screen
    NewSession,
}

#[derive(Default)]
pub(crate) struct History {
    entries: Vec<Seen>,
    /// where you are in `entries`
    at: usize,
}

impl History {
    /// Note what's on screen now. A new view drops the forward entries, like following a link.
    fn record(&mut self, now: Seen) {
        if self.entries.get(self.at) == Some(&now) {
            return;
        }
        // a new-session screen closed without starting anything isn't somewhere you went
        if self.at > 0 && self.entries[self.at] == Seen::NewSession && self.entries[self.at - 1] == now {
            self.entries.truncate(self.at);
            self.at -= 1;
            return;
        }
        self.entries.truncate(self.at + 1);
        self.entries.push(now);
        if self.entries.len() > MAX {
            self.entries.drain(..self.entries.len() - MAX);
        }
        self.at = self.entries.len() - 1;
    }

    /// The nearest entry behind (`back`) or ahead of where you are that `ok` accepts.
    fn step(&self, back: bool, ok: impl Fn(Seen) -> bool) -> Option<usize> {
        if back {
            (0..self.at.min(self.entries.len())).rev().find(|&i| ok(self.entries[i]))
        } else {
            (self.at + 1..self.entries.len()).find(|&i| ok(self.entries[i]))
        }
    }
}

impl App {
    /// What's on screen for history: the new-session screen while it's open, else the focused session.
    fn seen_now(&self) -> Option<Seen> {
        if matches!(self.overlay, Overlay::Launcher(_)) {
            return Some(Seen::NewSession);
        }
        self.focused().map(Seen::Pane)
    }

    /// After each batch of events: remember a view you moved to.
    pub(crate) fn record_view(&mut self) {
        if let Some(now) = self.seen_now() {
            self.history.record(now);
        }
    }

    fn full_screen(&self) -> bool {
        if self.stacked() {
            self.stack_zoom
        } else {
            self.tabs.get(self.cur).is_some_and(|t| t.zoom)
        }
    }

    /// The mouse's back (`true`) or forward button.
    pub(crate) fn navigate(&mut self, back: bool) {
        // other overlays (a file preview, the palette…) close first, like a dialog in a browser
        if self.overlay.is_open() && !matches!(self.overlay, Overlay::Launcher(_)) {
            self.overlay = Overlay::None;
            return;
        }
        self.record_view();
        let tiled = self.stacked() && !self.stack_zoom;
        let ok = |seen: Seen| match seen {
            Seen::NewSession => true,
            Seen::Pane(id) => self.panes.contains_key(&id) && (!tiled || self.stack.contains(&id)),
        };
        let Some(i) = self.history.step(back, ok) else { return };
        self.history.at = i;
        match self.history.entries[i] {
            Seen::NewSession => self.open_launcher(None, Place::Tab),
            Seen::Pane(id) => {
                let full = self.full_screen();
                if matches!(self.overlay, Overlay::Launcher(_)) {
                    self.overlay = Overlay::None;
                }
                self.leave_sidebar();
                self.focus_pane(id);
                // a zoomed view zooms the session you step to (a lone pane is full screen already)
                if full
                    && !self.stacked()
                    && let Some(t) = self.tabs.get_mut(self.cur)
                    && !matches!(t.root, Node::Leaf(_))
                {
                    t.zoom = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_steps_back_and_forward_and_new_views_drop_the_forward_ones() {
        let mut h = History::default();
        for id in [1, 2, 3] {
            h.record(Seen::Pane(id));
        }
        h.record(Seen::Pane(3));
        assert_eq!(h.entries.len(), 3, "the same view isn't recorded twice");
        let any = |_| true;
        h.at = h.step(true, any).unwrap();
        assert_eq!(h.entries[h.at], Seen::Pane(2));
        h.at = h.step(true, |s| s != Seen::Pane(1)).unwrap_or(h.at);
        assert_eq!(h.entries[h.at], Seen::Pane(2), "nothing acceptable behind: stay");
        h.at = h.step(false, any).unwrap();
        assert_eq!(h.entries[h.at], Seen::Pane(3));
        assert!(h.step(false, any).is_none());
        h.at = h.step(true, any).unwrap();
        h.record(Seen::Pane(4));
        assert_eq!(h.entries, [Seen::Pane(1), Seen::Pane(2), Seen::Pane(4)]);
    }

    #[test]
    fn a_cancelled_new_session_screen_is_forgotten_but_a_used_one_stays() {
        let mut h = History::default();
        h.record(Seen::Pane(1));
        h.record(Seen::NewSession);
        h.record(Seen::Pane(1));
        assert_eq!(h.entries, [Seen::Pane(1)]);
        h.record(Seen::NewSession);
        h.record(Seen::Pane(2));
        assert_eq!(h.entries, [Seen::Pane(1), Seen::NewSession, Seen::Pane(2)]);
    }
}
