//! Automatic "this session wants attention" notifications.
//!
//! Port of `tools/terminal-web/server/notifications.ts` plus the throttle and
//! bell-debounce logic from its `index.ts`, which the reference Rust host never
//! had:
//!
//! * [`BellDetector`] scans raw VT output for a bare BEL (what Claude Code /
//!   Codex ring when a turn finishes), `OSC 9;<message>` and
//!   `OSC 777;notify;<title>;<body>`. It is a real parser state machine
//!   because BEL also terminates OSC strings: a title update
//!   `ESC ] 0 ; title BEL` must not count as a bell. State persists across
//!   chunks since sequences split at arbitrary read boundaries.
//! * [`NotificationCenter`] keeps the last [`MAX_HISTORY`] notifications for
//!   `GET /api/notifications`, throttles terminal-raised notifications to one
//!   per session every [`NOTIFY_THROTTLE`], and tracks debounced bells: a bare
//!   BEL waits [`BELL_SETTLE`] so a rendered prompt can replace the generic
//!   "Task finished" with the actionable "needs input".

use crate::model::{TerminalNotification, iso_now};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Notifications kept for catch-up.
pub const MAX_HISTORY: usize = 200;
/// Longest OSC payload inspected; longer payloads are truncated.
const MAX_OSC_BUFFER: usize = 4096;
/// Minimum gap between two terminal-raised notifications of one session.
pub const NOTIFY_THROTTLE: Duration = Duration::from_millis(4000);
/// How long a bare BEL waits for a prompt to render before it is published.
pub const BELL_SETTLE: Duration = Duration::from_millis(300);

/// A signal found in a session's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BellEvent {
    /// A bare BEL outside any control string.
    Bell,
    /// `OSC 9;msg` or `OSC 777;notify;title;body`.
    Osc {
        title: Option<String>,
        body: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
    String,
    StringEsc,
}

/// Streaming BEL / OSC 9 / OSC 777 detector for one session.
#[derive(Debug, Default)]
pub struct BellDetector {
    state: ScanState,
    osc: String,
}

impl BellDetector {
    /// Scans one chunk, appending every signal found to `events`.
    pub fn feed(&mut self, data: &str, events: &mut Vec<BellEvent>) {
        let mut characters = data.chars().peekable();
        // `reprocess` replays a character in a new state (ESC aborting an OSC).
        let mut reprocess: Option<char> = None;
        while let Some(ch) = reprocess.take().or_else(|| characters.next()) {
            match self.state {
                ScanState::Ground => {
                    if ch == '\u{7}' {
                        events.push(BellEvent::Bell);
                    } else if ch == '\u{1b}' {
                        self.state = ScanState::Esc;
                    }
                }
                ScanState::Esc => {
                    self.state = match ch {
                        ']' => {
                            self.osc.clear();
                            ScanState::Osc
                        }
                        // DCS/SOS/PM/APC: opaque until ST; BEL inside is content.
                        'P' | 'X' | '^' | '_' => ScanState::String,
                        '\u{1b}' => ScanState::Esc,
                        // CSI and simple escapes never legally contain BEL;
                        // scanning their bytes as ground is safe.
                        _ => ScanState::Ground,
                    };
                }
                ScanState::Osc => {
                    if ch == '\u{7}' {
                        self.emit_osc(events);
                        self.state = ScanState::Ground;
                    } else if ch == '\u{1b}' {
                        self.state = ScanState::OscEsc;
                    } else if self.osc.len() < MAX_OSC_BUFFER {
                        self.osc.push(ch);
                    }
                }
                ScanState::OscEsc => {
                    if ch == '\\' {
                        self.emit_osc(events);
                        self.state = ScanState::Ground;
                    } else {
                        // ESC + anything else aborts the OSC; reprocess the
                        // character as the start of a fresh escape.
                        self.state = ScanState::Esc;
                        reprocess = Some(ch);
                    }
                }
                ScanState::String => {
                    if ch == '\u{1b}' {
                        self.state = ScanState::StringEsc;
                    }
                }
                ScanState::StringEsc => {
                    self.state = if ch == '\\' {
                        ScanState::Ground
                    } else {
                        ScanState::String
                    };
                }
            }
        }
    }

    fn emit_osc(&mut self, events: &mut Vec<BellEvent>) {
        let payload = std::mem::take(&mut self.osc);
        if let Some(event) = parse_notification_osc(&payload) {
            events.push(event);
        }
    }
}

/// Interprets one OSC payload (without `ESC ]` and terminator).
pub fn parse_notification_osc(payload: &str) -> Option<BellEvent> {
    if let Some(message) = payload.strip_prefix("9;") {
        // ConEmu / Windows Terminal subcommands (`9;4;` progress, `9;9;` cwd,
        // ...) are numeric and are not notifications. The Node host only
        // excluded 9;9; progress bars would otherwise ring on every update.
        let head: String = message.chars().take_while(char::is_ascii_digit).collect();
        if !head.is_empty() && matches!(message[head.len()..].chars().next(), None | Some(';')) {
            return None;
        }
        let body = message.trim();
        return (!body.is_empty()).then(|| BellEvent::Osc {
            title: None,
            body: Some(body.to_owned()),
        });
    }
    if let Some(rest) = payload.strip_prefix("777;") {
        let mut parts = rest.split(';');
        if parts.next() != Some("notify") {
            return None;
        }
        let title = parts.next().map(str::trim).unwrap_or_default().to_owned();
        let body = parts.collect::<Vec<_>>().join(";").trim().to_owned();
        if title.is_empty() && body.is_empty() {
            return None;
        }
        return Some(BellEvent::Osc {
            title: (!title.is_empty()).then_some(title),
            body: (!body.is_empty()).then_some(body),
        });
    }
    None
}

/// History, throttle and bell-debounce bookkeeping shared by all sessions.
#[derive(Default)]
pub struct NotificationCenter {
    history: VecDeque<TerminalNotification>,
    last_notify_at: HashMap<String, Instant>,
    pending_bells: HashMap<String, u64>,
    next_bell: u64,
}

impl NotificationCenter {
    /// Stamps id/time when missing and appends to the history.
    pub fn record(&mut self, mut notification: TerminalNotification) -> TerminalNotification {
        if notification.id.is_empty() {
            notification.id = uuid::Uuid::new_v4().simple().to_string()[..16].to_owned();
        }
        if notification.at.is_empty() {
            notification.at = iso_now();
        }
        self.history.push_back(notification.clone());
        while self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        notification
    }

    /// History, optionally only entries strictly newer than `since`.
    pub fn list(&self, since: Option<chrono::DateTime<chrono::Utc>>) -> Vec<TerminalNotification> {
        self.history
            .iter()
            .filter(|notification| {
                since.is_none_or(|since| {
                    chrono::DateTime::parse_from_rfc3339(&notification.at)
                        .map(|at| at.with_timezone(&chrono::Utc) > since)
                        .unwrap_or(true)
                })
            })
            .cloned()
            .collect()
    }

    /// True (and marks the session) when a terminal-raised notification may
    /// be published now.
    pub fn allow(&mut self, session_id: &str, now: Instant) -> bool {
        if self
            .last_notify_at
            .get(session_id)
            .is_some_and(|last| now.duration_since(*last) < NOTIFY_THROTTLE)
        {
            return false;
        }
        self.last_notify_at.insert(session_id.to_owned(), now);
        true
    }

    /// Marks the session as just notified without publishing anything.
    pub fn mark(&mut self, session_id: &str, now: Instant) {
        self.last_notify_at.insert(session_id.to_owned(), now);
    }

    /// Starts (or restarts) the settle window for a bare BEL; returns the
    /// generation the timer must present to [`Self::take_bell`].
    pub fn schedule_bell(&mut self, session_id: &str) -> u64 {
        self.next_bell += 1;
        self.pending_bells
            .insert(session_id.to_owned(), self.next_bell);
        self.next_bell
    }

    /// Claims a pending bell; false when it was cancelled or superseded.
    pub fn take_bell(&mut self, session_id: &str, generation: u64) -> bool {
        if self.pending_bells.get(session_id) == Some(&generation) {
            self.pending_bells.remove(session_id);
            true
        } else {
            false
        }
    }

    /// Drops a pending bell (a more specific notification replaced it).
    pub fn cancel_bell(&mut self, session_id: &str) {
        self.pending_bells.remove(session_id);
    }

    /// Forgets per-session state when a session goes away.
    pub fn forget(&mut self, session_id: &str) {
        self.pending_bells.remove(session_id);
        self.last_notify_at.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&str]) -> Vec<BellEvent> {
        let mut detector = BellDetector::default();
        let mut events = Vec::new();
        for chunk in chunks {
            detector.feed(chunk, &mut events);
        }
        events
    }

    #[test]
    fn bare_bel_is_a_bell() {
        assert_eq!(scan(&["done\u{7}"]), vec![BellEvent::Bell]);
        assert_eq!(scan(&["\u{7}\u{7}"]).len(), 2);
    }

    #[test]
    fn bel_terminating_an_osc_is_not_a_bell() {
        assert!(scan(&["\u{1b}]0;my title\u{7}text"]).is_empty());
        // Split across chunks at every point of the sequence.
        let sequence = "\u{1b}]2;window title\u{7}";
        for split in 1..sequence.len() {
            assert!(
                scan(&[&sequence[..split], &sequence[split..]]).is_empty(),
                "split {split}"
            );
        }
        // BEL inside DCS / APC strings is content.
        assert!(scan(&["\u{1b}Pq\u{7}data\u{1b}\\"]).is_empty());
        assert!(scan(&["\u{1b}_x\u{7}\u{1b}\\"]).is_empty());
        // CSI followed by a real bell.
        assert_eq!(scan(&["\u{1b}[31m\u{7}"]), vec![BellEvent::Bell]);
    }

    #[test]
    fn osc_9_is_a_message_but_conemu_subcommands_are_not() {
        assert_eq!(
            scan(&["\u{1b}]9;Build finished\u{7}"]),
            vec![BellEvent::Osc {
                title: None,
                body: Some("Build finished".into())
            }]
        );
        assert_eq!(
            scan(&["\u{1b}]9;hello\u{1b}\\"]),
            vec![BellEvent::Osc {
                title: None,
                body: Some("hello".into())
            }]
        );
        assert!(scan(&["\u{1b}]9;9;C:\\work\u{7}"]).is_empty());
        assert!(scan(&["\u{1b}]9;4;1;50\u{7}"]).is_empty());
        assert!(scan(&["\u{1b}]9;   \u{7}"]).is_empty());
        assert_eq!(
            parse_notification_osc("9;5 files done"),
            Some(BellEvent::Osc {
                title: None,
                body: Some("5 files done".into())
            })
        );
    }

    #[test]
    fn osc_777_notify_carries_title_and_body() {
        assert_eq!(
            scan(&["\u{1b}]777;notify;Claude;Done; all tests pass\u{7}"]),
            vec![BellEvent::Osc {
                title: Some("Claude".into()),
                body: Some("Done; all tests pass".into())
            }]
        );
        assert_eq!(
            scan(&["\u{1b}]777;notify;Only title\u{1b}\\"]),
            vec![BellEvent::Osc {
                title: Some("Only title".into()),
                body: None
            }]
        );
        assert!(scan(&["\u{1b}]777;preexec\u{7}"]).is_empty());
        assert!(scan(&["\u{1b}]777;notify;;\u{7}"]).is_empty());
    }

    #[test]
    fn escape_aborting_an_osc_is_reprocessed() {
        // ESC [ aborts the OSC and starts a CSI; the BEL after it is real.
        assert_eq!(scan(&["\u{1b}]0;x\u{1b}[m\u{7}"]), vec![BellEvent::Bell]);
    }

    #[test]
    fn throttle_and_bell_generations() {
        let mut center = NotificationCenter::default();
        let now = Instant::now();
        assert!(center.allow("a", now));
        assert!(!center.allow("a", now + Duration::from_secs(1)));
        assert!(center.allow("b", now));
        assert!(center.allow("a", now + NOTIFY_THROTTLE));

        let first = center.schedule_bell("a");
        let second = center.schedule_bell("a");
        assert!(!center.take_bell("a", first));
        assert!(center.take_bell("a", second));
        let third = center.schedule_bell("a");
        center.cancel_bell("a");
        assert!(!center.take_bell("a", third));
    }

    #[test]
    fn history_is_bounded_and_filterable() {
        let mut center = NotificationCenter::default();
        for index in 0..(MAX_HISTORY + 5) {
            center.record(TerminalNotification {
                id: String::new(),
                at: format!("2026-01-01T00:00:{:02}.000Z", index % 60),
                origin: "api".into(),
                session_id: None,
                session_title: None,
                title: Some(format!("n{index}")),
                body: None,
                sound: None,
            });
        }
        assert_eq!(center.list(None).len(), MAX_HISTORY);
        let since = crate::model::parse_since(Some("1767225650000")); // 00:00:50
        assert!(
            center
                .list(since)
                .iter()
                .all(|n| n.at.as_str() > "2026-01-01T00:00:50")
        );
    }
}
