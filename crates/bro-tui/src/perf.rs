//! Slow passes of the UI loop, written down. Each pass (events in, housekeeping, draw, write to the terminal)
//! is timed; one that keeps input waiting or runs longer than [`SLOW`] goes to `~/.bro/v2-perf.log` with
//! where the time went, so "typing felt slow just now" points at a cause instead of a guess.

use crate::pane::{Event, PaneId};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

/// A pass longer than this (or input kept waiting this long) is logged.
pub const SLOW: Duration = Duration::from_millis(50);
/// The log is cut back to its newest half past this size.
const MAX_LOG: u64 = 512 * 1024;

/// When the oldest input not yet taken by the UI was read, in µs since [`epoch`] (0 = none waiting).
static INPUT_READ: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn micros(at: Instant) -> u64 {
    at.saturating_duration_since(epoch()).as_micros() as u64 + 1
}

/// The input thread read a key, click or paste: the clock starts on how long it waits for the UI.
pub fn input_read() {
    let _ = INPUT_READ.compare_exchange(0, micros(Instant::now()), Ordering::AcqRel, Ordering::Relaxed);
}

/// The UI took the waiting input: how long the oldest waited.
fn input_taken() -> Option<Duration> {
    let at = INPUT_READ.swap(0, Ordering::AcqRel);
    (at != 0).then(|| Duration::from_micros(micros(Instant::now()).saturating_sub(at)))
}

/// What an event was, for the log.
fn kind(ev: &Event) -> &'static str {
    use crossterm::event::Event as C;
    match ev {
        Event::Input(C::Key(_)) => "key",
        Event::Input(C::Mouse(_)) => "mouse",
        Event::Input(C::Paste(_)) => "paste",
        Event::Input(C::Resize(..)) => "resize",
        Event::Input(_) => "input",
        Event::Navigate(_) => "navigate",
        Event::Alt(_) => "alt",
        Event::Wake(_) => "pane output",
        Event::Tick => "tick",
        Event::Services => "services",
        Event::Launched(_) => "launch",
        Event::Bridge(_) => "bridge command",
        Event::BridgeStarted => "bridge started",
        Event::Toast(..) => "toast",
        Event::Clipboard(..) | Event::ClipboardText(..) => "clipboard",
        Event::OpenLink(_) => "open link",
        Event::FileView(_) => "file preview",
        Event::EditorPane(_) => "editor",
        Event::ThemeFilesChanged => "theme files",
        Event::OpenProject(_) => "open project",
        Event::FolderCreated(..) => "folder",
        Event::Watchdog(_) => "watchdog",
        Event::Voice(_) => "voice",
    }
}

/// One pass of the loop.
#[derive(Default)]
pub struct Pass {
    start: Option<Instant>,
    events: u32,
    /// the slowest event: what it was, which pane, how long
    worst: Option<(&'static str, Option<PaneId>, Duration)>,
    input_wait: Option<Duration>,
    handle: Duration,
    after: Duration,
    render: Duration,
    write: Duration,
    /// the slowest pane render this pass
    pane: Option<(PaneId, Duration)>,
}

impl Pass {
    /// An event arrived: the pass starts with the first.
    pub fn begin(&mut self) {
        self.start.get_or_insert_with(Instant::now);
    }

    /// Handle `ev` with `f`, timed.
    pub fn event<R>(&mut self, ev: Event, f: impl FnOnce(Event) -> R) -> R {
        let (what, pane) = (kind(&ev), if let Event::Wake(w) = &ev { Some(w.id) } else { None });
        if matches!(ev, Event::Input(_) | Event::Navigate(_))
            && let Some(w) = input_taken()
        {
            self.input_wait = Some(self.input_wait.map_or(w, |k| k.max(w)));
        }
        let t = Instant::now();
        let r = f(ev);
        let took = t.elapsed();
        self.events += 1;
        self.handle += took;
        if self.worst.is_none_or(|(_, _, d)| took > d) {
            self.worst = Some((what, pane, took));
        }
        r
    }

    pub fn housekeeping(&mut self, took: Duration) {
        self.after += took;
    }

    /// One pane drew itself.
    pub fn pane_rendered(&mut self, id: PaneId, took: Duration) {
        if self.pane.is_none_or(|(_, d)| took > d) {
            self.pane = Some((id, took));
        }
    }

    /// The frame is on screen: `render` built it, `total` includes writing it out.
    pub fn drawn(&mut self, render: Duration, total: Duration) {
        self.render += render;
        self.write += total.saturating_sub(render);
    }

    /// End the pass; a slow one is logged.
    pub fn finish(&mut self) {
        let pass = std::mem::take(self);
        let Some(start) = pass.start else { return };
        let total = start.elapsed();
        if total < SLOW && pass.input_wait.is_none_or(|w| w < SLOW) {
            return;
        }
        log(pass.line(total));
    }

    fn line(&self, total: Duration) -> String {
        let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
        let mut s = format!("slow pass {}", ms(total));
        if let Some(w) = self.input_wait {
            s += &format!(" · input waited {}", ms(w));
        }
        s += &format!(" · events {} ({})", ms(self.handle), self.events);
        if let Some((what, pane, d)) = self.worst {
            s += &format!(" worst {what}{} {}", pane.map(|p| format!(" pane {p}")).unwrap_or_default(), ms(d));
        }
        s += &format!(" · housekeeping {} · render {}", ms(self.after), ms(self.render));
        if let Some((id, d)) = self.pane {
            s += &format!(" (pane {id} {})", ms(d));
        }
        s += &format!(" · terminal write {}", ms(self.write));
        s
    }
}

/// Append a line to the log on a background thread (never the UI's).
fn log(line: String) {
    if cfg!(test) {
        return;
    }
    static TX: OnceLock<Option<parking_lot::Mutex<Sender<String>>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = channel::<String>();
        std::thread::Builder::new()
            .name("bro-perf-log".into())
            .spawn(move || {
                use std::io::Write;
                let path = crate::util::bro_dir().join("v2-perf.log");
                for line in rx {
                    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG)
                        && let Ok(text) = std::fs::read_to_string(&path)
                    {
                        // from the first whole line past the middle (bytes: the middle may split a character)
                        let mid = text.len() / 2;
                        let keep = text.as_bytes()[mid..].iter().position(|b| *b == b'\n').map_or("", |i| &text[mid + i + 1..]);
                        let _ = std::fs::write(&path, keep);
                    }
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"));
                    }
                }
            })
            .ok()
            .map(|_| parking_lot::Mutex::new(tx))
    });
    if let Some(tx) = tx {
        let _ = tx.lock().send(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_pass_names_where_the_time_went() {
        let mut p = Pass { start: Some(Instant::now()), ..Pass::default() };
        p.event(Event::Tick, |_| std::thread::sleep(Duration::from_millis(2)));
        p.event(Event::Wake(7.into()), |_| std::thread::sleep(Duration::from_millis(6)));
        p.pane_rendered(3, Duration::from_millis(1));
        p.pane_rendered(4, Duration::from_millis(9));
        p.drawn(Duration::from_millis(10), Duration::from_millis(14));
        let line = p.line(Duration::from_millis(80));
        assert!(line.starts_with("slow pass 80.0 ms"), "{line}");
        assert!(line.contains("events") && line.contains("(2) worst pane output pane 7"), "{line}");
        assert!(line.contains("render 10.0 ms (pane 4 9.0 ms)") && line.contains("terminal write 4.0 ms"), "{line}");
    }

    #[test]
    fn input_counts_from_when_it_was_read() {
        input_read();
        let first = INPUT_READ.load(Ordering::Acquire);
        input_read();
        assert_eq!(INPUT_READ.load(Ordering::Acquire), first, "the oldest waiting input sets the clock");
        std::thread::sleep(Duration::from_millis(3));
        let key = || Event::Input(crossterm::event::Event::FocusGained);
        let mut p = Pass::default();
        p.begin();
        p.event(Event::Tick, |_| ());
        assert!(p.input_wait.is_none(), "output and ticks don't take the input's clock");
        p.event(key(), |_| ());
        assert!(p.input_wait.is_some_and(|w| w >= Duration::from_millis(3)));
        assert_eq!(INPUT_READ.load(Ordering::Acquire), 0);
        // input that didn't wait for anything is not kept
        let mut q = Pass::default();
        q.event(key(), |_| ());
        assert!(q.input_wait.is_none());
    }
}
