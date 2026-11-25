//! Composed messages, paced after write receipts so a delayed poll cannot merge paste and Enter.
//!
//! Codex can still take the Enter for a line break: on Windows it reads keys rather than a paste, and a paste
//! whose keys reach it in one rush (it was busy, or its paste detector is still holding them) keeps Enter as part
//! of the paste. The message is then left in its box with a line break under it. So after a Codex message's
//! Enter, bro looks at the box: the message still there, the cursor on a new empty line under it, means the
//! Enter became a line break. bro takes the line break back out and presses Enter again, a few times at most.
//! Nothing else on screen looks like that, so a message that went is never touched.

use super::*;
use crate::focus::Part;
use crate::panes::writer::Receipt;
use std::collections::VecDeque;
use std::sync::mpsc::TryRecvError;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum DeliveryState {
    #[default]
    Sending,
    Submitted,
    Failed(String),
}

/// Submission to the terminal, separate from the agent accepting it into its transcript.
#[derive(Clone, Debug, Default)]
pub(crate) struct Delivery(Arc<Mutex<DeliveryState>>);

impl Delivery {
    pub fn state(&self) -> DeliveryState { self.0.lock().clone() }
    fn set(&self, state: DeliveryState) { *self.0.lock() = state; }
}

struct Step {
    bytes: Vec<u8>,
    wait_after: Duration,
    enter: bool,
    delivery: Delivery,
    /// once written, see that the message went (Codex): its last characters, and the tries so far
    check: Option<(String, u8)>,
}

/// After Enter, Codex's box is looked at: the message should have left it.
struct Check {
    /// the message's last characters, without spaces
    tail: String,
    /// when to look (again)
    at: Instant,
    /// how long Codex may take to read the Enter at all (it may be starting up)
    until: Instant,
    /// Enters pressed again so far
    tries: u8,
}

/// What Codex's box shows after Enter.
#[derive(Debug, PartialEq, Eq)]
enum Composer {
    /// the message, and the cursor at the start of an empty line under it: Enter became a line break
    LineBreak,
    /// the message, the cursor at its end: Enter not read yet
    Holding,
    /// anything else: the message went
    Clear,
}

/// The first look after Enter, then how often, and for how long.
const CHECK_AFTER: Duration = Duration::from_millis(1200);
const CHECK_EVERY: Duration = Duration::from_millis(400);
const CHECK_FOR: Duration = Duration::from_secs(25);
/// The most times Enter is pressed again.
const RETRIES: u8 = 3;
/// Codex shows a paste longer than this as "[Pasted Content 1234 chars]".
const CODEX_LARGE_PASTE: usize = 1000;

/// What a message's end looks like in Codex's box: its last characters without spaces (a long one is shown as a
/// placeholder ending "chars]").
fn tail_of(text: &str) -> String {
    if text.chars().count() > CODEX_LARGE_PASTE {
        return "chars]".into();
    }
    let squashed: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    squashed[squashed.len().saturating_sub(24)..].iter().collect()
}

/// Read Codex's box on `screen` (rows as shown, the cursor's row and column) for a message ending in `tail`.
fn composer(rows: &[String], (row, col): (usize, usize), tail: &str) -> Composer {
    let squash = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let Some(cur) = rows.get(row) else { return Composer::Clear };
    if tail.is_empty() {
        return Composer::Clear;
    }
    let above = rows[row.saturating_sub(4)..row].concat();
    // the box's first line starts with its prompt glyph; a line under it is indented, empty when just broken
    if cur.trim().is_empty() && col <= 4 && squash(&above).ends_with(tail) {
        return Composer::LineBreak;
    }
    if squash(&format!("{above}{cur}")).ends_with(tail) {
        return Composer::Holding;
    }
    Composer::Clear
}

struct Flight {
    step: Step,
    receipt: Receipt,
}

#[derive(Default)]
pub(super) struct Queue {
    steps: VecDeque<Step>,
    flight: Option<Flight>,
    ready_at: Option<Instant>,
    errors: VecDeque<String>,
    check: Option<Check>,
    /// the end of what the phone, the web or `bro ctl` last pasted into Codex, when, and how long the paste is so
    /// far: their Enter is checked too
    remote: Option<(String, Instant, usize)>,
}

impl Queue {
    pub(super) fn pending(&self) -> bool { !self.steps.is_empty() || self.flight.is_some() || self.check.is_some() }
    pub(super) fn error(&mut self) -> Option<String> { self.errors.pop_front() }

    pub(super) fn cancel(&mut self, error: &str) {
        self.check = None;
        let delivery = self.flight.take().map(|f| f.step.delivery).or_else(|| self.steps.front().map(|s| s.delivery.clone()));
        if let Some(delivery) = delivery { self.fail(&delivery, error.into()); }
    }

    fn fail(&mut self, delivery: &Delivery, error: String) {
        delivery.set(DeliveryState::Failed(error.clone()));
        for step in self.steps.drain(..) { step.delivery.set(DeliveryState::Failed(error.clone())); }
        self.ready_at = None;
        self.errors.push_back(format!("Message was not sent: {error}"));
    }
}

impl Term {
    /// Empty native editor, including dim placeholder text. A draft or a busy
    /// input queue defers background wakes instead of merging them with user input.
    pub(crate) fn background_editor_empty(&self) -> bool {
        if self.input.pending() || self.typed_ago().is_some_and(|age| age < Duration::from_millis(1500)) { return false; }
        let Some(mut parser) = self.parser.try_lock() else { return false; };
        let back = parser.screen().scrollback();
        parser.screen_mut().set_scrollback(0);
        let screen = parser.screen();
        let (row, cursor) = screen.cursor_position();
        let (_, columns) = screen.size();
        let mut marker = false;
        let mut empty = true;
        for col in 0..columns {
            let Some(cell) = screen.cell(row, col) else { continue; };
            let content = cell.contents();
            if content.trim().is_empty() { continue; }
            if !marker && matches!(content, "❯" | "›" | ">") && col < cursor { marker = true; continue; }
            if !marker || col < cursor || !cell.dim() { empty = false; break; }
        }
        parser.screen_mut().set_scrollback(back);
        marker && empty
    }

    /// Legacy single-line submissions use the same delivery path as focus view.
    pub fn submit(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace(['\r', '\n'], " ");
        self.send_message(&text);
    }

    pub fn send_message(&mut self, text: &str) -> Delivery {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let wait = self.submit_delay(text.len());
        let bytes = self.paste_bytes(&text);
        let tail = self.checks_enter().then(|| tail_of(&text));
        self.queue_input(vec![(bytes, wait)], tail)
    }

    pub(crate) fn insert_dictation(&mut self, text: &str, send: bool) {
        let text: String = text.replace(['\r', '\t'], " ").chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        if text.trim().is_empty() { return; }
        if send {
            self.send_message(&text);
        } else {
            self.send(&self.paste_bytes(&text));
        }
    }

    pub fn type_command(&mut self, command: &str) -> Delivery {
        self.queue_input(vec![(command.as_bytes().to_vec(), Duration::from_millis(400))], None)
    }

    /// Codex's Enter can become a line break: see that its messages went.
    fn checks_enter(&self) -> bool {
        self.meta.harness == Some(Harness::Codex)
    }

    /// Input from the phone, the web or `bro ctl` (a paste, then a bare Enter): a Codex message's Enter is checked
    /// as a composed one's is.
    pub(super) fn note_remote_input(&mut self, bytes: &[u8]) {
        if !self.checks_enter() {
            return;
        }
        let now = self.now();
        if bytes == b"\r" {
            if let Some((tail, at, _)) = self.input.remote.take()
                && now.duration_since(at) < Duration::from_secs(60)
            {
                self.input.check = Some(Check { tail, at: now + CHECK_AFTER, until: now + CHECK_FOR, tries: 0 });
            }
            return;
        }
        let raw = String::from_utf8_lossy(bytes);
        // a long paste comes in chunks: the first opens it, the last closes it
        let opens = raw.starts_with("\x1b[200~");
        let text = raw.strip_prefix("\x1b[200~").unwrap_or(&raw);
        let closes = text.ends_with("\x1b[201~");
        let text = text.strip_suffix("\x1b[201~").unwrap_or(text);
        let pasted = opens || closes || self.input.remote.is_some();
        // keys (arrows, escapes) or an answer that presses its own Enter: not a message
        if text.contains('\x1b') || text.trim().is_empty() || !pasted && text.contains('\r') {
            self.input.remote = None;
            return;
        }
        let before = match &self.input.remote {
            Some((_, at, chars)) if !opens && now.duration_since(*at) < Duration::from_secs(5) => *chars,
            _ => 0,
        };
        let chars = before + text.chars().count();
        self.input.remote = Some((if chars > CODEX_LARGE_PASTE { "chars]".into() } else { tail_of(text) }, now, chars));
    }

    pub fn send_parts(&mut self, parts: &[Part]) -> Delivery {
        let mut steps = vec![];
        let mut bytes = 0;
        for part in parts {
            let (text, wait) = match part {
                Part::Text(t) => (t.replace("\r\n", "\n").replace('\r', "\n"), Duration::from_millis(80)),
                Part::Picture(p) => (crate::clip::paste_form(&[p.to_string_lossy().into_owned()]), Duration::from_millis(500)),
            };
            if text.is_empty() { continue; }
            bytes += text.len();
            steps.push((self.paste_bytes(&text), wait));
        }
        if let Some((_, wait)) = steps.last_mut() { *wait += self.submit_delay(bytes); }
        // a message that ends in words is checked by them (a picture shows as Codex's own placeholder)
        let tail = match parts.last() {
            Some(Part::Text(t)) if self.checks_enter() && !t.trim().is_empty() => Some(tail_of(t)),
            _ => None,
        };
        self.queue_input(steps, tail)
    }

    fn submit_delay(&self, bytes: usize) -> Duration {
        let agent = self.meta.harness.map(|h| h.label());
        bro_bridge::input_submit_delay(agent, bytes)
    }

    fn paste_bytes(&self, text: &str) -> Vec<u8> {
        if self.program_state.lock().bracketed_paste {
            format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", "")).into_bytes()
        } else {
            text.replace('\n', " ").into_bytes()
        }
    }

    fn queue_input(&mut self, steps: Vec<(Vec<u8>, Duration)>, tail: Option<String>) -> Delivery {
        self.note_typed();
        self.scroll = 0;
        // An automatic retry must never share its pending Enter with user input.
        self.enter_at = None;
        // a new message: the last one's check is over
        self.input.check = None;
        let delivery = Delivery::default();
        for (bytes, wait_after) in steps {
            self.input.steps.push_back(Step { bytes, wait_after, enter: false, delivery: delivery.clone(), check: None });
        }
        self.input.steps.push_back(Step { bytes: b"\r".to_vec(), wait_after: Duration::from_millis(150), enter: true, delivery: delivery.clone(), check: tail.map(|t| (t, 0)) });
        self.send_due();
        delivery
    }

    /// Look at Codex's box after Enter: a line break where the Enter should have sent goes, and Enter is pressed
    /// again; a message still waiting to be read is looked at again later.
    fn check_due(&mut self, now: Instant) {
        let Some(c) = &self.input.check else { return };
        if now < c.at || self.input.flight.is_some() || !self.input.steps.is_empty() {
            return;
        }
        let seen = {
            let Some(mut p) = self.parser.try_lock() else { return };
            let back = p.screen().scrollback();
            p.screen_mut().set_scrollback(0);
            let s = p.screen();
            let (_, cols) = s.size();
            let rows: Vec<String> = s.rows(0, cols).collect();
            let (row, col) = s.cursor_position();
            let seen = composer(&rows, (row as usize, col as usize), &c.tail);
            p.screen_mut().set_scrollback(back);
            seen
        };
        let Some(c) = self.input.check.as_mut() else { return };
        match seen {
            Composer::LineBreak if c.tries < RETRIES => {
                let (tail, tries) = (c.tail.clone(), c.tries + 1);
                self.input.check = None;
                // the line break back out (Backspace), a moment for Codex to take it, then Enter again
                let delivery = Delivery::default();
                self.input.steps.push_back(Step { bytes: b"\x7f".to_vec(), wait_after: Duration::from_millis(400), enter: false, delivery: delivery.clone(), check: None });
                self.input.steps.push_back(Step { bytes: b"\r".to_vec(), wait_after: Duration::from_millis(150), enter: false, delivery, check: Some((tail, tries)) });
            }
            Composer::Holding if now < c.until => c.at = now + CHECK_EVERY,
            _ => self.input.check = None,
        }
    }

    pub(super) fn send_due(&mut self) { self.send_due_at(self.now()); }

    fn send_due_at(&mut self, now: Instant) {
        loop {
            if let Some(flight) = self.input.flight.take() {
                match flight.receipt.try_recv() {
                    Err(TryRecvError::Empty) => { self.input.flight = Some(flight); return; }
                    Err(TryRecvError::Disconnected) => {
                        self.input.fail(&flight.step.delivery, "The session input closed before confirming the write.".into());
                        return;
                    }
                    Ok(Err(error)) => { self.input.fail(&flight.step.delivery, error.to_string()); return; }
                    Ok(Ok(())) => {
                        if flight.step.enter { flight.step.delivery.set(DeliveryState::Submitted); }
                        if let Some((tail, tries)) = flight.step.check {
                            self.input.check = Some(Check { tail, at: now + CHECK_AFTER, until: now + CHECK_FOR, tries });
                        }
                        // Start from the receipt we just observed, never from when the batch was queued.
                        self.input.ready_at = Some(now + flight.step.wait_after);
                    }
                }
            }
            if self.input.ready_at.is_some_and(|at| now < at) { return; }
            if self.input.steps.is_empty() {
                self.check_due(now);
            }
            let Some(step) = self.input.steps.pop_front() else { return };
            match self.write_input(&step.bytes) {
                Ok(receipt) => self.input.flight = Some(Flight { step, receipt }),
                Err(error) => { self.input.fail(&step.delivery, error); return; }
            }
        }
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<Receipt, String> {
        #[cfg(test)]
        self.sent.extend_from_slice(bytes);
        let mut writer = self.writer.lock();
        if let Some(writer) = writer.as_mut() {
            return writer.write_receipt(bytes).map_err(|e| format!("The session could not receive input: {e}"));
        }
        #[cfg(test)]
        if self.spawn.is_none() && self.live.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(Ok(()));
            return Ok(rx);
        }
        Err("The session input is not available.".into())
    }

    /// Existing fixed-pane tests inspect queued input without waiting for wall-clock delays.
    #[cfg(test)]
    pub fn flush_outbox(&mut self) {
        let until = Instant::now() + Duration::from_secs(3);
        let mut now = self.now();
        while self.input.pending() {
            now += Duration::from_secs(5);
            self.send_due_at(now);
            assert!(Instant::now() < until, "input writer did not finish");
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc;

    fn term(harness: Harness) -> Term {
        crate::testkit::isolate();
        let (tx, _rx) = mpsc::channel();
        let svc = Services::offline(Default::default(), tx);
        let cwd = std::env::temp_dir();
        let meta = Meta { sid: "input-test".into(), harness: Some(harness), profile: None, store: None, model: None,
            label: harness.label().into(), name: None, renamed: false, project: svc.project_for(&cwd), cwd,
            started: Instant::now(), route_id: None, cleanup: vec![], orchestrator: false };
        Term::fixed(meta, "\x1b[?2004h❯ ", svc)
    }

    #[test]
    fn background_wakes_require_an_empty_editor_and_no_pending_submission() {
        let mut term = term(Harness::Codex);
        assert!(term.background_editor_empty());
        term.feed("\x1b[2J\x1b[H› keep this draft".as_bytes());
        assert!(!term.background_editor_empty());
        term.feed(b"\r\x1b[2C"); // The cursor can be at Home inside a non-empty draft.
        assert!(!term.background_editor_empty());
        term.feed("\x1b[2J\x1b[H› \x1b[2mPlaceholder\x1b[0m\r\x1b[2C".as_bytes());
        assert!(term.background_editor_empty());
        term.send_message("already sending");
        assert!(!term.background_editor_empty());
    }

    #[test]
    fn late_polls_keep_picture_pastes_and_enter_separate() {
        let mut term = term(Harness::Codex);
        let now = term.now();
        let delivery = term.send_parts(&[
            Part::Text("Explain this with a Quip\n<bro-quip>Use everyday language.</bro-quip>".into()),
            Part::Picture(std::env::temp_dir().join("input.png")),
            Part::Text("\nThen check the result.".into()),
        ]);
        assert!(!term.sent.contains(&b'\r'));
        term.send_due_at(now + Duration::from_secs(10));
        assert_eq!(term.sent.iter().filter(|b| **b == 27).count(), 4, "only the first text and picture have been pasted");
        assert!(!term.sent.contains(&b'\r'), "a late poll must not flush all the old deadlines at once");
        term.send_due_at(now + Duration::from_secs(20));
        assert!(!term.sent.contains(&b'\r'));
        term.send_due_at(now + Duration::from_secs(20) + Duration::from_millis(1499));
        assert_eq!(delivery.state(), DeliveryState::Sending);
        assert!(!term.sent.contains(&b'\r'), "Codex gets its full paste-burst window after the last piece");
        term.send_due_at(now + Duration::from_secs(22));
        assert_eq!(term.sent.iter().filter(|b| **b == b'\r').count(), 1);
        assert_eq!(delivery.state(), DeliveryState::Submitted);
    }

    #[test]
    fn consecutive_messages_each_get_one_enter_without_combining_their_pastes() {
        let mut term = term(Harness::Claude);
        let now = term.now();
        let first = term.send_message("first\nmessage");
        let second = term.send_message("second\nmessage");
        term.send_due_at(now + Duration::from_secs(10));
        assert_eq!(first.state(), DeliveryState::Submitted);
        assert_eq!(second.state(), DeliveryState::Sending);
        assert!(!String::from_utf8_lossy(&term.sent).contains("second"));
        term.send_due_at(now + Duration::from_secs(11));
        assert!(String::from_utf8_lossy(&term.sent).contains("second"));
        assert_eq!(term.sent.iter().filter(|b| **b == b'\r').count(), 1);
        term.send_due_at(now + Duration::from_secs(12));
        assert_eq!(second.state(), DeliveryState::Submitted);
        assert_eq!(term.sent.iter().filter(|b| **b == b'\r').count(), 2);
    }

    struct SlowPipe {
        entered: Option<mpsc::Sender<()>>,
        release: mpsc::Receiver<()>,
        written: mpsc::Sender<Vec<u8>>,
    }

    impl Write for SlowPipe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            self.written.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    #[test]
    fn a_slow_write_cannot_use_up_the_settle_window_before_the_paste_arrives() {
        let mut term = term(Harness::Codex);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (written_tx, written_rx) = mpsc::channel();
        let pipe = SlowPipe { entered: Some(entered_tx), release: release_rx, written: written_tx };
        *term.writer.lock() = Some(crate::panes::writer::Writer::new(Box::new(pipe)).unwrap());
        let now = term.now();
        let delivery = term.send_message(&format!("Check this\n<bro-quip>{}</bro-quip>", "Keep the existing behavior.\n".repeat(1000)));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        term.send_due_at(now + Duration::from_secs(30));
        assert_eq!(delivery.state(), DeliveryState::Sending);
        assert_eq!(term.input.steps.len(), 1, "Enter is still waiting for the paste receipt");
        release_tx.send(()).unwrap();
        let paste = written_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(paste.starts_with(b"\x1b[200~Check this\n<bro-quip>"));
        let until = Instant::now() + Duration::from_secs(2);
        while term.input.flight.is_some() {
            term.send_due_at(now + Duration::from_secs(40));
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        term.send_due_at(now + Duration::from_secs(41));
        assert!(written_rx.try_recv().is_err(), "even a thirty-second write delay cannot make Enter immediate");
        term.send_due_at(now + Duration::from_secs(42));
        assert_eq!(written_rx.recv_timeout(Duration::from_secs(2)).unwrap(), b"\r");
        let until = Instant::now() + Duration::from_secs(2);
        while delivery.state() == DeliveryState::Sending {
            term.send_due_at(now + Duration::from_secs(43));
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(delivery.state(), DeliveryState::Submitted);
        assert!(written_rx.try_recv().is_err(), "no duplicate Enter");
    }

    #[test]
    fn failed_writes_do_not_send_enter_or_report_success() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::ErrorKind::BrokenPipe.into()) }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let mut term = term(Harness::Claude);
        term.set_test_writer(Box::new(Closed));
        let delivery = term.send_message("keep this message");
        term.flush_outbox();
        assert!(matches!(delivery.state(), DeliveryState::Failed(_)));
        assert!(!term.sent.contains(&b'\r'));
        assert!(term.input.error().is_some());
    }

    #[test]
    fn a_provider_retry_wait_cannot_cancel_a_user_messages_enter() {
        let mut term = term(Harness::Claude);
        term.feed("\x1b[2J\x1b[H✻ Weekly limit reached · Retrying in 5h (Nov 1, 7pm) · attempt 1/30\r\nYou've used 99% of your weekly limit\r\n❯\r\n⏵⏵ bypass permissions on (shift+tab to cycle) · esc to interrupt".as_bytes());
        term.scan();
        assert!(term.limit_warning().unwrap().is_retrying());
        let delivery = term.send_message("Please change direction.");
        let svc = term.svc.clone();
        let (tx, _rx) = mpsc::channel();
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        term.auto_resume(&mut Cx { id: 1, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true });
        term.flush_outbox();
        assert_eq!(delivery.state(), DeliveryState::Submitted);
        assert_eq!(term.sent.iter().filter(|b| **b == b'\r').count(), 1);
    }

    #[test]
    fn a_failed_enter_never_becomes_submitted() {
        struct RejectEnter;
        impl Write for RejectEnter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes == b"\r" { Err(io::ErrorKind::BrokenPipe.into()) } else { Ok(bytes.len()) }
            }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let mut term = term(Harness::Claude);
        term.set_test_writer(Box::new(RejectEnter));
        let first = term.send_message("first");
        let second = term.send_message("second");
        term.flush_outbox();
        assert!(matches!(first.state(), DeliveryState::Failed(_)));
        assert!(matches!(second.state(), DeliveryState::Failed(_)));
        assert!(!String::from_utf8_lossy(&term.sent).contains("second"), "a failed submission cannot spill into the next message");
    }

    /// Codex's screen: its box holding `first` (with the prompt glyph), then `rest` lines, the cursor at (row, col).
    fn codex_box(term: &Term, lines: &[&str], cursor: (u16, u16)) {
        let mut screen = "\x1b[2J\x1b[H".to_string();
        screen.push_str(&lines.join("\r\n"));
        screen.push_str(&format!("\x1b[{};{}H", cursor.0 + 1, cursor.1 + 1));
        term.feed(screen.as_bytes());
    }

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn codex_box_reads_as_a_line_break_a_message_waiting_or_gone() {
        let tail = tail_of("Reply with only the word pong");
        assert_eq!(tail, "Replywithonlythewordpong");
        // the Enter became a line break: the cursor at the start of an empty line under the message
        let broke = rows(&["", "\u{203a} Reply with only the word pong", "  ", "", "  GPT-6.1 max"]);
        assert_eq!(composer(&broke, (2, 2), &tail), Composer::LineBreak);
        // a message that wrapped across two rows
        let wrapped = rows(&["\u{203a} Reply with only the", "  word pong", "  "]);
        assert_eq!(composer(&wrapped, (2, 2), &tail), Composer::LineBreak);
        // not read yet: the cursor still at its end
        let holding = rows(&["", "\u{203a} Reply with only the word pong", "", "  GPT-6.1 max"]);
        assert_eq!(composer(&holding, (1, 31), &tail), Composer::Holding);
        // it went: the box is empty again, the message up in the conversation
        let gone = rows(&["\u{203a} Reply with only the word pong", "", "\u{2022} pong", "", "\u{203a} Ask Codex to do anything"]);
        assert_eq!(composer(&gone, (4, 2), &tail), Composer::Clear);
        // an empty line in the box under someone else's words is left alone
        let other = rows(&["\u{203a} Something else entirely", "  "]);
        assert_eq!(composer(&other, (1, 2), &tail), Composer::Clear);
        // a long message shows as Codex's placeholder
        assert_eq!(tail_of(&"x".repeat(1200)), "chars]");
        let long = rows(&["\u{203a} [Pasted Content 1200 chars]", "  "]);
        assert_eq!(composer(&long, (1, 2), "chars]"), Composer::LineBreak);
    }

    #[test]
    fn a_codex_enter_that_became_a_line_break_is_taken_back_and_pressed_again() {
        let mut term = term(Harness::Codex);
        let t0 = term.now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let delivery = term.send_message("Reply with only the word pong");
        term.send_due_at(at(1000));
        term.send_due_at(at(3000));
        term.send_due_at(at(3100));
        assert_eq!(delivery.state(), DeliveryState::Submitted);
        assert_eq!(term.sent.iter().filter(|b| **b == b'\r').count(), 1);
        // Codex kept the message and put a line break under it
        codex_box(&term, &["", "\u{203a} Reply with only the word pong", "  ", "", "  GPT-6.1 max"], (2, 2));
        let before = term.sent.len();
        term.send_due_at(at(3500));
        assert_eq!(term.sent.len(), before, "it looks only once Codex has had a moment");
        term.send_due_at(at(5000));
        assert_eq!(&term.sent[before..], b"\x7f", "the line break goes first");
        term.send_due_at(at(5100));
        term.send_due_at(at(6000));
        assert_eq!(&term.sent[before..], b"\x7f\r", "then Enter again");
        // this time it went
        codex_box(&term, &["\u{203a} Reply with only the word pong", "", "\u{203a} Ask Codex to do anything"], (2, 2));
        term.send_due_at(at(6100));
        term.send_due_at(at(8000));
        assert!(!term.input.pending(), "nothing left to do");
        assert_eq!(&term.sent[before..], b"\x7f\r", "a message that went is never touched");
    }

    #[test]
    fn a_codex_that_has_not_read_the_enter_yet_is_waited_for_and_never_retried_forever() {
        let mut term = term(Harness::Codex);
        let t0 = term.now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        term.send_message("Reply with only the word pong");
        term.send_due_at(at(1));
        term.send_due_at(at(3));
        term.send_due_at(at(4));
        // still starting: the message typed, the Enter not read
        codex_box(&term, &["\u{203a} Reply with only the word pong"], (0, 31));
        let before = term.sent.len();
        for s in 5..40 {
            term.send_due_at(at(s));
        }
        assert_eq!(term.sent.len(), before, "a message waiting to be read isn't pressed again");
        assert!(!term.input.pending(), "the look gives up in time");
        // a line break that keeps coming back: three tries, then it's left to you
        let mut term = super::tests::term(Harness::Codex);
        let t0 = term.now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        term.send_message("Reply with only the word pong");
        for ms in (0..60_000).step_by(100) {
            codex_box(&term, &["\u{203a} Reply with only the word pong", "  "], (1, 2));
            term.send_due_at(at(ms));
        }
        assert_eq!(term.sent.iter().filter(|b| **b == 0x7f).count(), usize::from(RETRIES));
        assert!(!term.input.pending());
        // Claude's Enter is never second-guessed
        let mut claude = super::tests::term(Harness::Claude);
        claude.send_message("Reply with only the word pong");
        claude.flush_outbox();
        assert!(!claude.input.pending());
    }

    #[test]
    fn a_codex_message_from_the_phone_is_checked_too() {
        let mut term = term(Harness::Codex);
        term.set_test_writer(Box::new(io::sink()));
        // a bracketed paste in chunks, then the bare Enter, as the bridge sends them
        term.send_confirmed(b"\x1b[200~Reply with only ").unwrap();
        term.send_confirmed(b"the word pong\x1b[201~").unwrap();
        term.send_confirmed(b"\r").unwrap();
        assert!(term.input.check.as_ref().is_some_and(|c| "Replywithonlythewordpong".ends_with(&c.tail)));
        codex_box(&term, &["\u{203a} Reply with only the word pong", "  "], (1, 2));
        let before = term.sent.len();
        let t0 = term.now();
        term.send_due_at(t0 + Duration::from_secs(2));
        assert_eq!(&term.sent[before..], b"\x7f");
        // keys and answers aren't messages: no check after them
        let mut term = super::tests::term(Harness::Codex);
        term.set_test_writer(Box::new(io::sink()));
        term.send_confirmed(b"\x1b[A").unwrap();
        term.send_confirmed(b"\r").unwrap();
        assert!(term.input.check.is_none());
        term.send_confirmed(b"1\r").unwrap();
        term.send_confirmed(b"\r").unwrap();
        assert!(term.input.check.is_none());
    }

    #[test]
    fn antigravity_dictation_pastes_into_the_prompt_and_sends_only_on_request() {
        let mut term = term(Harness::Antigravity);
        term.feed(b"\x1b[?2004h");
        term.insert_dictation("First line\nSecond line", false);
        assert_eq!(term.sent, b"\x1b[200~First line\nSecond line\x1b[201~");
        assert!(!term.input.pending());
        term.sent.clear();
        term.insert_dictation("Review the patch.", true);
        term.flush_outbox();
        assert_eq!(term.sent, b"\x1b[200~Review the patch.\x1b[201~\r");
    }

    #[test]
    fn queued_questions_keep_the_last_scan_during_output_and_clear_with_the_live_screen() {
        let mut term = term(Harness::Codex);
        let queued = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../bro-core/tests/fixtures/codex-queued-question.txt")).replace('\n', "\r\n");
        term.feed(format!("\x1b[2J\x1b[H{queued}").as_bytes());
        term.scan();
        let parser = term.parser.clone();
        {
            let _reading = parser.lock();
            assert_eq!(term.focus_queued_questions().unwrap().count, 1);
            assert_eq!(term.activity(), Some(Activity::Blocked));
            assert!(!term.open_queued_questions(), "a busy screen must not send a stale shortcut");
        }
        term.feed(b"\x1b[2J\x1b[H> Ask Codex to do anything\r\n? for shortcuts");
        assert!(term.focus_queued_questions().is_none(), "a completed read clears the notice immediately");
        term.scan();
        let _reading = parser.lock();
        assert!(term.focus_queued_questions().is_none(), "the cleared queue must stay cleared during later output");
    }

    #[test]
    fn codex_activity_follows_a_running_turn_when_the_terminal_banner_is_missing() {
        let mut term = term(Harness::Codex);
        term.born = Instant::now() - Duration::from_secs(10);
        term.feed(b"\x1b[2J\x1b[HCommand exited (status 1)\r\nAsk Codex to do anything");
        term.scan();
        assert_eq!(term.activity(), Some(Activity::Idle));
        let started = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        term.set_insights(super::super::super::insights::Insights::fixed_transcript(Harness::Codex, started));
        assert_eq!(term.activity(), Some(Activity::Working), "a tool's exit status does not end the agent's turn");
        assert!(term.busy());
        for event in ["task_complete", "turn_aborted", "error"] {
            let transcript = format!("{started}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"{event}\"}}}}");
            term.set_insights(super::super::super::insights::Insights::fixed_transcript(Harness::Codex, &transcript));
            assert_eq!(term.activity(), Some(Activity::Idle), "{event} ends the working indicator");
            assert!(!term.busy());
        }
    }

    #[test]
    fn codex_activity_recognizes_a_clipped_working_banner_without_a_transcript() {
        let mut term = term(Harness::Codex);
        term.born = Instant::now() - Duration::from_secs(10);
        for cols in [20, 35, 60] {
            term.parser.lock().screen_mut().set_size(12, cols);
            term.feed("\x1b[2J\x1b[H• Exited (status 1)\r\n• Working (40m 23s • esc to interrupt)".as_bytes());
            term.scan();
            assert_eq!(term.activity(), Some(Activity::Working), "working at {cols} columns after output goes quiet");
        }
    }

    #[test]
    fn codex_running_turn_does_not_hide_a_question_or_provider_limit() {
        let mut term = term(Harness::Codex);
        let started = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        term.set_insights(super::super::super::insights::Insights::fixed_transcript(Harness::Codex, started));
        for screen in [
            "Question 1/2\r\nWhere?\r\n› Type your answer\r\nenter to submit answer | esc to interrupt",
            "Weekly limit reached · Retrying in 5h (Nov 1, 7pm) · attempt 1/30\r\nesc to interrupt",
        ] {
            term.feed(format!("\x1b[2J\x1b[H{screen}").as_bytes());
            term.scan();
            assert_eq!(term.activity(), Some(Activity::Blocked));
            assert!(!term.busy());
        }
    }

    #[test]
    fn a_new_codex_animating_its_welcome_is_idle_not_working() {
        let mut term = term(Harness::Codex);
        term.feed(b"\x1b[2J\x1b[H>_ OpenAI Codex (v0.166.0)\r\n\r\n   .-=-.\r\n\r\n\xe2\x80\xba Ask Codex to do anything\r\n\r\n  ? for shortcuts");
        term.scan();
        assert_eq!(term.activity(), Some(Activity::Idle), "the logo moves, Codex waits");
        // working says so on screen
        term.feed(b"\x1b[2J\x1b[H\xe2\x80\xa2 Working (2s \xe2\x80\xa2 esc to interrupt)\r\n\xe2\x80\xba Ask Codex to do anything");
        term.scan();
        assert_eq!(term.activity(), Some(Activity::Working));
    }
}
