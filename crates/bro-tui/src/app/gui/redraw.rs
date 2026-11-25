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

/// A frame whose GPU part took this long blocked the loop: something is wrong with showing frames right now.
pub(super) const BLOCKED: Duration = Duration::from_millis(400);

/// When the compositor repeatedly takes longer than a frame to supply a buffer,
/// leave a short interval for input and session events before trying again.
#[derive(Default)]
pub(super) struct Pacing {
    slow_acquire: bool,
    pub until: Option<Instant>,
}

impl Pacing {
    pub fn presented(&mut self, acquire: Duration, now: Instant) {
        let slow = acquire >= Duration::from_millis(25);
        self.until = (slow && self.slow_acquire).then(|| now + acquire.min(Duration::from_millis(64)));
        self.slow_acquire = slow;
    }
}

/// Holds redraws back after a frame that blocked the loop.
///
/// Handing a frame to the window system normally takes a millisecond. When the display is asleep, the session
/// is locked or the driver is rebuilding its swapchain it can take seconds, and it takes them on the thread that
/// also handles keys, the phone's input and every session's output. Drawing again straight away blocks again,
/// so with sessions printing in the background the app spends nearly all its time stuck in frames nobody can
/// see. After a blocked frame nothing is drawn for a few times as long as it took (longer for each one in a
/// row, at most half a minute); the loop stays free for everything else. Anything the person does (a key, the
/// pointer, the window getting the focus) ends the hold at once: they are here, and so is the display.
#[derive(Default)]
pub(super) struct Stall {
    until: Option<Instant>,
    /// blocked frames in a row
    streak: u32,
}

impl Stall {
    /// A frame's GPU part took `gpu`, ending at `now`. Returns how long redraws are held when this one blocked.
    pub fn frame(&mut self, gpu: Duration, now: Instant) -> Option<Duration> {
        if gpu < BLOCKED {
            *self = Stall::default();
            return None;
        }
        self.streak += 1;
        let rest = (gpu * 4).max(Duration::from_secs(1 << self.streak.min(4))).min(Duration::from_secs(30));
        self.until = Some(now + rest);
        Some(rest)
    }

    /// When the hold ends, while there is one at `now`.
    pub fn until(&self, now: Instant) -> Option<Instant> {
        self.until.filter(|at| *at > now)
    }

    /// The person did something: draw now.
    pub fn clear(&mut self) {
        self.until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_acquire_delays_pace_frames_until_the_surface_recovers() {
        let now = Instant::now();
        let mut pace = Pacing::default();
        pace.presented(Duration::from_millis(35), now);
        assert_eq!(pace.until, None);
        pace.presented(Duration::from_millis(90), now);
        assert_eq!(pace.until, Some(now + Duration::from_millis(64)));
        pace.presented(Duration::from_millis(2), now);
        assert_eq!(pace.until, None);
    }

    #[test]
    fn a_frame_that_blocked_the_loop_holds_the_next_ones_back() {
        let now = Instant::now();
        let mut stall = Stall::default();
        assert_eq!(stall.frame(Duration::from_millis(3), now), None, "an ordinary frame holds nothing");
        assert_eq!(stall.until(now), None);
        // blocked for 2.5 s: away four times as long
        assert_eq!(stall.frame(Duration::from_millis(2500), now), Some(Duration::from_secs(10)));
        assert_eq!(stall.until(now + Duration::from_secs(9)), Some(now + Duration::from_secs(10)));
        assert_eq!(stall.until(now + Duration::from_secs(10)), None, "the hold is over");
        // a short block still earns a rest that grows with each one in a row, up to half a minute
        let mut stall = Stall::default();
        let rests: Vec<u64> = (0..6).map(|_| stall.frame(BLOCKED, now).unwrap().as_secs()).collect();
        assert_eq!(rests, [2, 4, 8, 16, 16, 16]);
        assert_eq!(stall.frame(Duration::from_secs(20), now), Some(Duration::from_secs(30)));
        // one good frame and it starts over
        assert_eq!(stall.frame(Duration::from_millis(1), now), None);
        assert_eq!(stall.frame(BLOCKED, now), Some(Duration::from_secs(2)));
    }

    #[test]
    fn anything_the_person_does_ends_the_hold() {
        let now = Instant::now();
        let mut stall = Stall::default();
        stall.frame(Duration::from_secs(5), now);
        assert!(stall.until(now).is_some());
        stall.clear();
        assert_eq!(stall.until(now), None);
        // and the next blocked frame still counts as one more in a row: the display hasn't come back
        assert_eq!(stall.frame(BLOCKED, now), Some(Duration::from_secs(4)));
    }

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
