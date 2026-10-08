//! A minimum terminal size while a remote client (phone, web) watches a session.
//!
//! The phone mirrors the session's grid, so a small desktop tile made a squat, unreadable
//! mobile view. While someone watches remotely the pty is grown to at least
//! `[bridge] remote_min_cols` × `remote_min_rows`; the desktop tile then shows the bottom-left
//! of the bigger screen (where the prompt is). The minimum is held for a grace period after the
//! last viewer leaves, so a phone briefly backgrounding the app doesn't resize the session back
//! and forth (every resize makes the agent redraw).

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinSize {
    /// 0 = no minimum on this axis
    pub rows: u16,
    pub cols: u16,
    /// how long the minimum outlives the last remote viewer
    pub grace: Duration,
}

/// Remembers when the session was last watched remotely.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteHold {
    last_watched: Option<Instant>,
}

impl RemoteHold {
    /// Whether the minimum applies at `now`: someone watches, or did within `grace`.
    pub fn active(&mut self, viewers: usize, now: Instant, grace: Duration) -> bool {
        if viewers > 0 {
            self.last_watched = Some(now);
            return true;
        }
        self.last_watched.is_some_and(|t| now.saturating_duration_since(t) < grace)
    }
}

/// (rows, cols) for the pty: `base` (the tile, or what a remote asked for while the pane is
/// hidden) grown to the minimum on each enabled axis, when the minimum is active.
pub fn effective(base: (u16, u16), min: MinSize, active: bool) -> (u16, u16) {
    if !active {
        return base;
    }
    let grow = |have: u16, want: u16| if want == 0 { have } else { have.max(want) };
    (grow(base.0, min.rows), grow(base.1, min.cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: MinSize = MinSize { rows: 50, cols: 80, grace: Duration::from_secs(15) };

    #[test]
    fn grows_only_small_axes_and_only_while_active() {
        assert_eq!(effective((20, 60), MIN, true), (50, 80));
        assert_eq!(effective((20, 120), MIN, true), (50, 120), "a wide tile keeps its width");
        assert_eq!(effective((70, 200), MIN, true), (70, 200), "a big tile is untouched");
        assert_eq!(effective((20, 60), MIN, false), (20, 60));
        let rows_off = MinSize { rows: 0, ..MIN };
        assert_eq!(effective((20, 60), rows_off, true), (20, 80), "0 turns an axis off");
        let cols_off = MinSize { cols: 0, ..MIN };
        assert_eq!(effective((20, 60), cols_off, true), (50, 60));
    }

    #[test]
    fn the_minimum_outlives_the_last_viewer_by_the_grace_period() {
        let t0 = Instant::now();
        let mut hold = RemoteHold::default();
        assert!(!hold.active(0, t0, MIN.grace), "never watched");
        assert!(hold.active(1, t0, MIN.grace));
        assert!(hold.active(0, t0 + Duration::from_secs(14), MIN.grace), "phone backgrounded briefly");
        assert!(!hold.active(0, t0 + Duration::from_secs(15), MIN.grace), "grace over");
        assert!(hold.active(2, t0 + Duration::from_secs(20), MIN.grace), "watched again");
        assert!(!hold.active(0, t0 + Duration::from_secs(20), Duration::ZERO), "no grace: off at once");
    }
}
