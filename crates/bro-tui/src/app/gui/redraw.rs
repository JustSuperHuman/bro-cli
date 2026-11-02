//! One outstanding redraw at a time. Wayland may hold it until the window is visible again.

use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Redraw {
    pending: bool,
}

impl Redraw {
    /// True when the caller needs to ask winit for a redraw.
    pub fn request(&mut self) -> bool {
        !std::mem::replace(&mut self.pending, true)
    }

    pub fn received(&mut self) {
        self.pending = false;
    }

    /// Background work keeps its deadline while the compositor holds a redraw. Frame deadlines are
    /// consumed by drawing, so an overdue one must not wake the loop again until that redraw arrives.
    pub fn next_wake(&self, now: Instant, background: Duration, frames: impl IntoIterator<Item = Option<Instant>>) -> Duration {
        if self.pending {
            return background;
        }
        frames.into_iter().flatten().fold(background, |wait, at| wait.min(at.saturating_duration_since(now)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withheld_wayland_redraw_keeps_background_work_sleeping() {
        let drawn = Instant::now();
        let ambient = drawn + Duration::from_millis(33);
        let output = drawn + Duration::from_millis(250);
        let background = Duration::from_millis(400);
        let mut redraw = Redraw::default();
        assert!(redraw.request());

        // Simulate an unseen workspace for fifteen seconds: neither overdue deadline can be
        // consumed by a frame, but terminal polls and other background work must keep running.
        for tick in 1..=38 {
            let now = drawn + background * tick;
            assert_eq!(redraw.next_wake(now, background, [Some(ambient), Some(output)]), background);
            assert!(!redraw.request(), "background events share the outstanding redraw");
        }

        // The compositor releases the redraw when the workspace returns. New output can draw
        // immediately, and the next animation frame once again has its normal deadline.
        redraw.received();
        assert!(redraw.request());
        redraw.received();
        let now = drawn + Duration::from_secs(16);
        assert_eq!(redraw.next_wake(now, background, [Some(now + Duration::from_millis(33))]), Duration::from_millis(33));
    }

    #[test]
    fn pending_redraw_defers_every_frame_timer() {
        let now = Instant::now();
        let mut redraw = Redraw::default();
        let overdue = now - Duration::from_millis(1);
        let background = Duration::from_secs(1);
        assert_eq!(redraw.next_wake(now, background, [Some(overdue)]), Duration::ZERO);
        assert!(redraw.request());
        // Animation, deferred output, captures, media and embedded pages all need a frame.
        for at in [overdue, now, now + Duration::from_millis(16)] {
            assert_eq!(redraw.next_wake(now, background, [None, Some(at)]), background);
        }
    }

    #[test]
    fn earliest_deadline_wins_when_no_redraw_is_pending() {
        let now = Instant::now();
        let redraw = Redraw::default();
        let frames = [None, Some(now + Duration::from_millis(33)), Some(now + Duration::from_millis(250))];
        assert_eq!(redraw.next_wake(now, Duration::from_secs(30), frames), Duration::from_millis(33));
        assert_eq!(redraw.next_wake(now, Duration::from_millis(4), frames), Duration::from_millis(4));
        assert_eq!(redraw.next_wake(now, Duration::from_secs(30), [None]), Duration::from_secs(30));
    }
}
