//! The contract every pane (terminal, usage, profiles, proxy, bridge) implements, the event loop's `Event`, and
//! the `Action`s a pane can ask the app for. The app owns layout, frames and focus; a pane draws its inside and
//! handles the input it is given. (Pattern from z4-oriel.)

use crate::alerts::Kind;
use crate::services::Services;
use crate::theme::Theme;
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect, text::Line};
use std::sync::mpsc::Sender;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

pub use crate::layout::PaneId;

/// Things the event loop wakes up for. Everything arrives on one mpsc channel.
pub enum Event {
    Input(crossterm::event::Event),
    /// The mouse's back (`true`) or forward button, or a keyboard's browser keys.
    Navigate(bool),
    /// Alt pressed (`true`) or released on its own: key hints show while it's held.
    Alt(bool),
    /// A pane's background work produced something: redraw and call `Pane::poll`.
    Wake(WakeSignal),
    /// The deadline passed (animations, agent re-scans).
    Tick,
    /// Services published new state (usage, proxy events, bridge status…): redraw.
    Services,
    /// A launch finished building its command on a background thread.
    Launched(Box<crate::services::Launched>),
    /// A remote client (Just Terminal app / web) asked for something.
    Bridge(bro_bridge::BridgeCommand),
    /// The bridge came up: register every live session with it.
    BridgeStarted,
    /// A background toast (bridge notifications, service errors).
    Toast(Kind, String),
    /// The alt+v clipboard grab finished: paste these paths into the pane, or hand it the original key.
    Clipboard(PaneId, Option<Vec<String>>, KeyEvent),
    /// A text clipboard read completed; apply only while the same input request is current.
    ClipboardText(std::time::Instant, Option<String>),
    /// A terminal link was clicked; open it outside the input/render loop.
    OpenLink(String),
    /// The file preview finished loading a page (or failed).
    FileView(Box<crate::file_view::Loaded>),
    /// Run a terminal editor (vim, hx…) in a pane beside the session.
    EditorPane(Box<crate::editor::EditorPane>),
    /// A theme file changed on disk.
    ThemeFilesChanged,
    /// Another `bro` was started in this folder: open it here as a project.
    OpenProject(std::path::PathBuf),
    /// ctrl+shift+o read the clipboard: the folder to open as a project, or why there isn't one.
    ClipboardFolder(Result<std::path::PathBuf, String>),
    /// The folder picker finished creating a directory on a background thread.
    FolderCreated(uuid::Uuid, Result<std::path::PathBuf, String>),
    /// A watchdog woke, stepped, reported or stood down (the snapshot cache is already updated).
    Watchdog(bro_bridge::WatchdogUpdate),
    /// Push-to-talk: words for the orchestrator session.
    Voice(Box<crate::services::VoiceMsg>),
    /// The session server went away, another window took it over, or a restart finished.
    Host(crate::services::host::HostEvent),
}

/// Where a new pane goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Place {
    /// split the focused pane along its longer side
    Split,
    SplitRight,
    SplitDown,
    /// a new tab
    #[default]
    Tab,
    /// a new tab behind the one you're in (a session another bro window asked for)
    Behind,
}


/// What a pane can ask the app to do.
pub enum Action {
    /// Open a pane (views, login terminals).
    Open(Box<dyn Pane>, Place),
    /// Close the asking pane.
    Close,
    Toast(Kind, String),
    /// Start a new session from an account shown in the usage view.
    LaunchUsage(bro_core::Harness, Option<String>),
    /// Connect a Google AI subscription through Bro's existing browser sign-in.
    GeminiSignIn,
    /// Run an app action (the settings view's theme picker and toggles).
    Run(crate::keymap::Act),
    /// Show a file in bro's file preview (a skill's SKILL.md).
    Preview(std::path::PathBuf),
}

/// Agent activity scraped from the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Activity {
    Idle,
    Working,
    /// waiting on you: a permission prompt, a question, (y/n)
    Blocked,
}

/// Everything a pane gets while handling input or rendering.
pub struct Cx<'a> {
    pub id: PaneId,
    pub theme: &'a Theme,
    pub svc: &'a Services,
    pub tx: &'a Sender<Event>,
    pub actions: &'a mut Vec<Action>,
    pub focused: bool,
}

impl Cx<'_> {
    pub fn act(&mut self, a: Action) {
        self.actions.push(a);
    }
    pub fn toast(&mut self, kind: Kind, s: impl Into<String>) {
        self.actions.push(Action::Toast(kind, s.into()));
    }
    /// A handle a background thread can use to wake this pane.
    pub fn waker(&self) -> Waker {
        Waker::new(self.id, self.tx.clone())
    }
}

/// Wakes one pane from any thread.
#[derive(Clone)]
pub struct Waker {
    pub id: PaneId,
    pub tx: Sender<Event>,
    pending: Arc<AtomicBool>,
}

impl Waker {
    pub fn new(id: PaneId, tx: Sender<Event>) -> Waker {
        Waker { id, tx, pending: Arc::new(AtomicBool::new(false)) }
    }

    pub fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel)
            && self.tx.send(Event::Wake(WakeSignal { id: self.id, pending: self.pending.clone() })).is_err()
        {
            self.pending.store(false, Ordering::Release);
        }
    }
}

/// One pending redraw per worker; acknowledge before polling so a concurrent update can wake again.
pub struct WakeSignal {
    pub id: PaneId,
    pending: Arc<AtomicBool>,
}

impl WakeSignal {
    pub fn acknowledge(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

impl From<PaneId> for WakeSignal {
    fn from(id: PaneId) -> Self {
        WakeSignal { id, pending: Arc::new(AtomicBool::new(false)) }
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;
    use crossterm::event::{Event as Input, KeyCode, KeyModifiers};

    #[test]
    fn output_floods_do_not_queue_thousands_of_redraws_ahead_of_input() {
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = Waker::new(7, tx.clone());
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let worker = waker.clone();
                scope.spawn(move || {
                    for _ in 0..5_000 {
                        worker.wake();
                    }
                });
            }
        });
        tx.send(Event::Input(Input::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))).unwrap();
        let Event::Wake(wake) = rx.try_recv().unwrap() else { panic!("one output notification") };
        assert_eq!(wake.id, 7);
        assert!(matches!(rx.try_recv(), Ok(Event::Input(Input::Key(k))) if k.code == KeyCode::Down), "input immediately follows the coalesced notification");
        assert!(rx.try_recv().is_err());
        wake.acknowledge();
        waker.wake();
        assert!(matches!(rx.try_recv(), Ok(Event::Wake(w)) if w.id == 7), "an update after acknowledgement is not lost");
    }
}

/// Down from a pane to its own type (the desktop app draws each view with that view's state).
pub trait AsAny {
    #[allow(dead_code)]
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

impl<T: 'static> AsAny for T {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A pane inside the split tree.
pub trait Pane: AsAny {
    /// Plain title (tabs, palette, toasts).
    fn title(&self) -> String;
    /// Icon name for the frame (see `ui::icon`).
    fn icon(&self) -> &'static str {
        "term"
    }
    /// The frame header: styled title (left) and status (right). Default: icon + title.
    fn header(&self, _t: &Theme, _time: f64) -> (Line<'static>, Option<Line<'static>>) {
        (Line::from(format!(" {}{} ", crate::ui::lead(self.icon()), self.title())), None)
    }
    /// Draw inside `area` (already inside the frame).
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx);
    /// Return true if the key was used. Unused keys fall through to the app.
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool;
    /// Mouse event in screen coordinates; `area` is the pane's inner rect.
    fn mouse(&mut self, _ev: MouseEvent, _area: Rect, _cx: &mut Cx) {}
    fn paste(&mut self, _text: &str, _cx: &mut Cx) {}
    /// Link at the last rendered pane-local cell (row, column).
    fn link_at(&self, _row: u16, _col: u16) -> Option<String> { None }
    /// Called after a Wake for this pane, and on ticks when `tick_every` asks.
    fn poll(&mut self, _cx: &mut Cx) {}
    /// If Some, the app calls `poll` at least this often.
    fn tick_every(&self) -> Option<Duration> {
        None
    }
    /// A terminal pane is dead once its process exits; the app then closes it.
    fn alive(&self) -> bool {
        true
    }
    /// True if the pane wants raw keys (terminal): only global chords are intercepted.
    fn is_terminal(&self) -> bool {
        false
    }
    /// The agent is in the middle of a turn. Terminals ask the agent itself when they can.
    fn busy(&self) -> bool {
        self.activity() == Some(Activity::Working)
    }
    /// For panes running a coding agent: what it's doing.
    fn activity(&self) -> Option<Activity> {
        None
    }
    /// True when the program inside wants mouse events itself.
    fn wants_mouse(&self) -> bool {
        false
    }
    /// Singleton views ("usage", "proxy"…) return their name so alt+u etc. focus instead of reopening.
    fn view(&self) -> Option<&'static str> {
        None
    }
    fn as_term(&mut self) -> Option<&mut crate::panes::term::Term> {
        None
    }
    fn as_term_ref(&self) -> Option<&crate::panes::term::Term> {
        None
    }
}
