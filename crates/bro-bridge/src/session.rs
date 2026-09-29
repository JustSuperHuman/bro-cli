//! One mirrored session: a vt100 screen model, the bounded VT-safe replay
//! buffer, the output filter, the notification detector and the agent
//! observation state. Everything here runs under the registry lock and never
//! blocks.

use crate::agents;
use crate::model::{TerminalSessionSummary, iso_now};
use crate::notifications::{BellDetector, BellEvent};
use crate::output_filter::OutputFilter;
use crate::prompt;
use crate::vt_stream::SafeReplayBuffer;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Scrollback rows replayed to a client that subscribes to a session.
const SNAPSHOT_HISTORY_ROWS: usize = 1000;
/// Minimum gap between two screen observations of one session; a repainting
/// TUI would otherwise be fingerprinted on every chunk.
pub(crate) const OBSERVE_INTERVAL: Duration = Duration::from_millis(180);
/// Scrollback kept by the screen model.
const SCROLLBACK_ROWS: usize = 5000;

/// Client-visible column/row bounds, as in the reference host.
pub(crate) fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(20, 400), rows.clamp(8, 200))
}

/// Incremental UTF-8 decoding of raw PTY bytes: a multi-byte character split
/// across reads is held back until complete; invalid bytes become U+FFFD.
#[derive(Debug, Default)]
pub(crate) struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub fn decode(&mut self, data: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(data);
        let mut out = String::with_capacity(bytes.len());
        let mut rest = bytes.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                    match error.error_len() {
                        Some(length) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid + length..];
                        }
                        None => {
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }
}

/// An agent-state change worth telling the user about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Transition {
    /// working -> idle: the agent finished its turn.
    Done,
    /// -> awaiting: the agent is blocked on a rendered question.
    Awaiting { agent_label: &'static str },
}

/// Result of [`Session::observe`].
#[derive(Debug, Default)]
pub(crate) struct Observed {
    /// A client-visible summary field moved.
    pub changed: bool,
    pub transition: Option<Transition>,
}

pub(crate) struct Session {
    pub summary: TerminalSessionSummary,
    pub parser: vt100::Parser,
    pub output_filter: OutputFilter,
    pub bells: BellDetector,
    pub utf8: Utf8Stream,
    pub replay: SafeReplayBuffer,
    pub seq: u64,
    /// Directory the session is grouped under (project root, else cwd).
    pub project_dir: Option<String>,
    /// The agent bro says it launched here.
    pub agent_hint: Option<agents::Agent>,
    /// When the rendered screen was last fingerprinted for agent state.
    pub last_observed: Option<Instant>,
    /// Output arrived since the last observation.
    pub observation_pending: bool,
}

/// One session as the orchestrator and the composed-input context see it,
/// read under a single lock.
pub(crate) struct SessionView {
    pub summary: TerminalSessionSummary,
    /// Plain text of the visible screen.
    pub screen: String,
    /// The question an agent is blocked on, when one is rendered.
    pub prompt: Option<Value>,
    pub bracketed_paste: bool,
}

impl Session {
    pub fn new(summary: TerminalSessionSummary) -> Self {
        Self {
            parser: vt100::Parser::new(summary.rows, summary.cols, SCROLLBACK_ROWS),
            summary,
            output_filter: OutputFilter::default(),
            bells: BellDetector::default(),
            utf8: Utf8Stream::default(),
            replay: SafeReplayBuffer::default(),
            seq: 0,
            project_dir: None,
            agent_hint: None,
            last_observed: None,
            observation_pending: false,
        }
    }

    /// The directory used for automatic project grouping.
    pub fn grouping_dir(&self) -> &str {
        self.project_dir.as_deref().unwrap_or(&self.summary.cwd)
    }

    /// Feeds decoded output: detects notification signals, strips the private
    /// metadata envelope, updates the screen model and replay. Returns the
    /// sequence number (0 when nothing visible was appended) and the visible
    /// text.
    pub fn feed(&mut self, data: &str, bells: &mut Vec<BellEvent>) -> (u64, String) {
        self.bells.feed(data, bells);
        let visible = self.output_filter.feed(data, &mut self.summary);
        if visible.is_empty() {
            return (0, visible);
        }
        let seq = self.append(visible.clone());
        self.observation_pending = true;
        (seq, visible)
    }

    /// Re-reads the rendered screen for agent identity and activity.
    pub fn observe(&mut self) -> Observed {
        let text = self.plain_text();
        let osc_agent = self
            .output_filter
            .osc_agent
            .as_deref()
            .and_then(agents::Agent::parse);
        let observation =
            agents::observe_with_hint(&self.summary, osc_agent, &text, self.agent_hint);
        let waiting = observation.agent.is_some() && prompt::detect_prompt(&text).is_some();
        let activity = agents::activity(&observation, waiting).map(str::to_owned);
        let agent = observation.agent.map(|agent| agent.id().to_owned());
        let source = observation.source.map(str::to_owned);
        let transition = match (self.summary.agent_activity.as_deref(), activity.as_deref()) {
            (Some("working"), Some("idle")) => Some(Transition::Done),
            (previous, Some("awaiting")) if previous != Some("awaiting") => {
                Some(Transition::Awaiting {
                    agent_label: observation
                        .agent
                        .map(agents::Agent::label)
                        .unwrap_or("Agent"),
                })
            }
            _ => None,
        };
        let changed = self.summary.agent != agent
            || self.summary.agent_source != source
            || self.summary.agent_activity != activity;
        self.summary.agent = agent;
        self.summary.agent_source = source;
        self.summary.agent_activity = activity;
        self.last_observed = Some(Instant::now());
        self.observation_pending = false;
        Observed {
            changed,
            transition,
        }
    }

    /// Whether a throttled observation is due.
    pub fn observation_due(&self) -> bool {
        self.last_observed
            .is_none_or(|at| at.elapsed() >= OBSERVE_INTERVAL)
    }

    pub fn view(&self) -> SessionView {
        let screen = self.plain_text();
        let prompt = if self.summary.agent.is_some() {
            prompt::detect_prompt(&screen)
        } else {
            None
        };
        SessionView {
            summary: self.summary.clone(),
            screen,
            prompt,
            bracketed_paste: self.parser.screen().bracketed_paste(),
        }
    }

    /// The composed-input context (`GET /api/sessions/{id}/input-context`).
    pub fn input_context(&self) -> Value {
        prompt::input_context(
            &self.summary,
            &self.plain_text(),
            self.parser.screen().bracketed_paste(),
            self.parser.screen().application_cursor(),
        )
    }

    /// The last `tail` rows of the screen plus scrollback, oldest first.
    pub fn text_with_scrollback(&mut self, tail: usize) -> String {
        let screen = self.parser.screen_mut();
        let (rows, cols) = screen.size();
        let rows_per_window = usize::from(rows).max(1);
        screen.set_scrollback(usize::MAX);
        let max_offset = screen.scrollback();
        let mut collected: VecDeque<String> = VecDeque::new();
        let mut offset = 0usize;
        loop {
            let effective = offset.min(max_offset);
            screen.set_scrollback(effective);
            let mut window: Vec<String> = screen.rows(0, cols).collect();
            if effective < offset {
                // Clamped at the top of the scrollback: only the rows above
                // the previous window are new.
                let overlap = offset - effective;
                window.truncate(window.len().saturating_sub(overlap));
            }
            for row in window.into_iter().rev() {
                collected.push_front(row.trim_end().to_string());
            }
            if effective >= max_offset || collected.len() >= tail + rows_per_window {
                break;
            }
            offset += rows_per_window;
        }
        screen.set_scrollback(0);
        while collected.back().is_some_and(|row| row.trim().is_empty()) {
            collected.pop_back();
        }
        let start = collected.len().saturating_sub(tail);
        collected
            .iter()
            .skip(start)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn append(&mut self, data: String) -> u64 {
        self.seq += 1;
        self.parser.process(data.as_bytes());
        self.replay.push(self.seq, data, iso_now());
        self.summary.updated_at = iso_now();
        self.summary.buffered_bytes = self.replay.bytes();
        self.seq
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = clamp_size(cols, rows);
        self.summary.cols = cols;
        self.summary.rows = rows;
        self.parser.screen_mut().set_size(rows, cols);
        self.summary.updated_at = iso_now();
    }

    pub fn screen_ansi(&self) -> String {
        String::from_utf8_lossy(&self.parser.screen().contents_formatted()).into_owned()
    }

    /// A byte stream that rebuilds this terminal in a fresh emulator: the
    /// newest scrollback rows, then the visible screen, then the input modes.
    pub fn replay_ansi(&mut self) -> String {
        let screen = self.parser.screen_mut();
        let mut out: Vec<u8> = Vec::new();
        if screen.alternate_screen() {
            // vt100 exposes only the active grid; the primary screen's history
            // is unreachable while an alt-screen app owns the terminal.
            out.extend_from_slice(b"\x1b[?1049h");
        } else {
            let (rows, cols) = screen.size();
            screen.set_scrollback(usize::MAX);
            let history = screen.scrollback().min(SNAPSHOT_HISTORY_ROWS);
            // With offset k the visible rows are history rows [H-k, H-k+rows),
            // so the first min(k, rows) of them are still above the screen.
            let mut offset = history;
            while offset > 0 {
                screen.set_scrollback(offset);
                let take = offset.min(usize::from(rows));
                for row in screen.rows_formatted(0, cols).take(take) {
                    out.extend_from_slice(b"\x1b[m");
                    out.extend_from_slice(&row);
                    out.extend_from_slice(b"\r\n");
                }
                offset -= take;
            }
            screen.set_scrollback(0);
            if history > 0 {
                // contents_formatted() starts by clearing the visible screen;
                // scroll the last history rows off it first so they survive.
                out.extend(std::iter::repeat_n(
                    b'\n',
                    usize::from(rows.saturating_sub(1)),
                ));
            }
        }
        out.extend_from_slice(&screen.contents_formatted());
        out.extend_from_slice(&screen.input_mode_formatted());
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn plain_text(&self) -> String {
        self.parser.screen().contents()
    }

    /// The `snapshot` event. `screen` rebuilds the terminal on its own and
    /// every client renders it in preference to `chunks`, which stays in the
    /// message (empty) so older clients still parse it.
    pub fn snapshot(&mut self) -> Value {
        json!({
            "type": "snapshot",
            "sessionId": self.summary.id,
            "screen": self.replay_ansi(),
            "chunks": [],
            "session": self.summary
        })
    }

    /// `GET /api/sessions/{id}/export?format=json`.
    pub fn export(&self) -> Value {
        let chunks: Vec<Value> = self
            .replay
            .chunks()
            .map(|chunk| json!({ "seq": chunk.seq, "data": chunk.data, "at": chunk.at }))
            .collect();
        json!({
            "session": self.summary,
            "screen": self.screen_ansi(),
            "transcript": self.replay.transcript(),
            "chunks": chunks
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(cols: u16, rows: u16) -> Session {
        Session::new(TerminalSessionSummary::native(
            "id".into(),
            "tab".into(),
            "pwsh.exe".into(),
            "C:\\work".into(),
            1,
            cols,
            rows,
        ))
    }

    #[test]
    fn utf8_stream_holds_split_characters() {
        let mut stream = Utf8Stream::default();
        let bytes = "a✓b".as_bytes();
        assert_eq!(stream.decode(&bytes[..2]), "a");
        assert_eq!(stream.decode(&bytes[2..]), "✓b");
        assert_eq!(stream.decode(&[0xff, b'x']), "\u{FFFD}x");
    }

    #[test]
    fn snapshot_replays_scrollback_and_input_modes() {
        let mut session = session(40, 10);
        let lines: String = (0..60)
            .map(|i| format!("\x1b[3{}mline {i}\x1b[m\r\n", i % 7))
            .collect();
        session.append(lines + "\x1b[?1000h\x1b[?1006h\x1b[?2004h$ ");
        let before = session.text_with_scrollback(10_000);

        let replay = session.snapshot()["screen"].as_str().unwrap().to_owned();
        let mut mirror = Session::new(session.summary.clone());
        mirror.append(replay);
        assert_eq!(mirror.text_with_scrollback(10_000), before);
        assert!(before.starts_with("line 0\nline 1\n"));
        let screen = mirror.parser.screen();
        assert_eq!(
            screen.cursor_position(),
            session.parser.screen().cursor_position()
        );
        assert_eq!(
            screen.mouse_protocol_mode(),
            vt100::MouseProtocolMode::PressRelease
        );
        assert!(screen.bracketed_paste());

        session.append("\x1b[?1049h\x1b[Hmenu".into());
        let replay = session.snapshot()["screen"].as_str().unwrap().to_owned();
        let mut mirror = Session::new(session.summary.clone());
        mirror.append(replay);
        assert!(mirror.parser.screen().alternate_screen());
        assert!(mirror.plain_text().starts_with("menu"));
    }

    #[test]
    fn agent_transitions_are_reported_once() {
        let mut session = session(100, 30);
        let mut bells = Vec::new();
        session.feed(
            "❯ fix it\r\n· Thinking… (esc to interrupt)\r\n? for shortcuts   shift+tab to cycle",
            &mut bells,
        );
        let first = session.observe();
        assert_eq!(session.summary.agent_activity.as_deref(), Some("working"));
        assert_eq!(first.transition, None);

        session.feed(
            "\x1b[2J\x1b[H❯ \r\n? for shortcuts   shift+tab to cycle",
            &mut bells,
        );
        assert_eq!(session.observe().transition, Some(Transition::Done));
        assert_eq!(session.observe().transition, None);

        session.feed(
            "\x1b[2J\x1b[HDo you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\nEnter to confirm · Esc to cancel",
            &mut bells,
        );
        assert_eq!(
            session.observe().transition,
            Some(Transition::Awaiting {
                agent_label: "Claude Code"
            })
        );
        assert_eq!(session.observe().transition, None);
    }
}
