//! Brief in-app notifications, rendered as compact lines in the top-right corner.

use std::time::{Duration, Instant};

/// What a toast is about (sets its glyph and colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Info,
    AgentDone,
    NeedsYou,
    Bridge,
    Usage,
    Error,
}

impl Kind {
    /// Glyph shown before the text.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Info => "·",
            Kind::AgentDone => "●",
            Kind::NeedsYou => "●",
            Kind::Bridge => "⌁",
            Kind::Usage => "◔",
            Kind::Error => "×",
        }
    }
}

/// One toast on screen.
#[derive(Clone, Debug)]
pub struct Toast {
    pub kind: Kind,
    pub text: String,
    pub at: Instant,
}

/// How long a toast stays up.
pub const TOAST_FOR: Duration = Duration::from_secs(2);

/// The toast queue: newest last, at most two shown.
#[derive(Default)]
pub struct Toasts {
    pub items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, kind: Kind, text: impl Into<String>) {
        let text = text.into();
        self.items.retain(|t| t.text != text);
        self.items.push(Toast { kind, text, at: Instant::now() });
        if self.items.len() > 2 {
            self.items.remove(0);
        }
    }
    /// Drop expired toasts; true if any went.
    pub fn expire(&mut self) -> bool {
        let n = self.items.len();
        self.items.retain(|t| t.at.elapsed() < TOAST_FOR);
        n != self.items.len()
    }
    /// Time until the next toast expires.
    pub fn next_expiry(&self) -> Option<Duration> {
        self.items.iter().map(|t| TOAST_FOR.saturating_sub(t.at.elapsed())).min()
    }
    #[cfg(test)]
    pub fn latest(&self) -> Option<&Toast> {
        self.items.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_queue() {
        let mut t = Toasts::default();
        for i in 0..5 {
            t.push(Kind::Info, format!("n{i}"));
        }
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.latest().unwrap().text, "n4");
        t.push(Kind::Info, "n3");
        assert_eq!(t.items.len(), 2, "duplicates replace");
        assert!(t.next_expiry().is_some());
    }

    #[test]
    fn long_notifications_expire_promptly_too() {
        let mut t = Toasts::default();
        t.push(Kind::Error, "long notification ".repeat(100));
        t.items[0].at = Instant::now() - Duration::from_millis(2100);
        t.push(Kind::Info, "still fresh");
        assert_eq!(t.next_expiry(), Some(Duration::ZERO));
        assert!(t.expire());
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.latest().unwrap().text, "still fresh");
    }
}
