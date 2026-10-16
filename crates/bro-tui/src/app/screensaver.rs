//! The summary screensaver (`summary_screensaver_secs`): leave bro alone that long and every session on screen
//! turns over to its summary; the next key, click, scroll, mouse move or return to the window turns them back.
//! The key or click that wakes it does only that, so nothing lands in a session you couldn't see.

use super::App;
use std::time::{Duration, Instant};

impl App {
    /// You did something: the clock restarts, and the cards the screensaver turned over turn back. True when
    /// it was showing (the touch that wakes it does nothing else).
    pub(crate) fn touched(&mut self) -> bool {
        self.last_input = Instant::now();
        if self.saver.is_empty() {
            return false;
        }
        let mut delay = Duration::ZERO;
        for id in std::mem::take(&mut self.saver) {
            // one you turned face up yourself, or closed, stays as it is
            if self.flips.get(&id).is_some_and(|f| f.back) {
                self.turn_card(id, delay);
                delay += crate::card::CASCADE;
            }
        }
        true
    }

    fn saver_after(&self) -> Option<Duration> {
        let secs = self.svc.settings().summary_screensaver_secs;
        (secs > 0 && self.saver.is_empty()).then(|| Duration::from_secs(secs))
    }

    /// After each batch of events: turn everything over once you've been away long enough.
    pub(crate) fn screensaver_step(&mut self) {
        let Some(after) = self.saver_after() else { return };
        if self.overlay.is_open() || self.last_input.elapsed() < after {
            return;
        }
        self.saver = self.turn_all(true);
        if self.saver.is_empty() {
            // nothing to turn (all already over, or no sessions): look again after another full wait
            self.last_input = Instant::now();
        }
    }

    /// How long until the screensaver is due (None when it's off or showing).
    pub(crate) fn screensaver_wake(&self) -> Option<Duration> {
        let after = self.saver_after()?;
        Some(after.saturating_sub(self.last_input.elapsed()) + Duration::from_millis(10))
    }
}
