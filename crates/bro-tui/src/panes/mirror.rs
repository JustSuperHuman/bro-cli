//! A second screen fed the same output as a session's vt100 parser: the desktop app keeps an
//! alacritty_terminal grid per session through this, for drawing, while vt100 stays the model the agent
//! tracking reads (activity scan, prompts, early failures).
//!
//! The desktop app registers a factory at startup; every terminal pane created after that gets a mirror.

use crossterm::event::KeyEvent;
use std::any::Any;
use std::sync::{Arc, OnceLock};

/// A screen that mirrors a session's output.
pub trait ScreenMirror: Send + Sync {
    /// The program's output, in order. Returns the bytes that answer queries in it (device attributes,
    /// colours, keyboard protocol). The session answers the cursor position report itself, so a mirror must
    /// leave that one out.
    fn feed(&self, bytes: &[u8]) -> Vec<u8>;
    /// The pty changed size (rows, cols).
    fn resize(&self, rows: u16, cols: u16);
    /// The bytes for `key` when the program asked for an enhanced keyboard protocol (kitty); None = the
    /// usual xterm encoding.
    fn encode_key(&self, _key: &KeyEvent) -> Option<Vec<u8>> {
        None
    }
    fn as_any(&self) -> &dyn Any;
}

type Factory = Box<dyn Fn() -> Arc<dyn ScreenMirror> + Send + Sync>;

static FACTORY: OnceLock<Factory> = OnceLock::new();

/// Give every terminal pane created from now on a mirror (the desktop app calls this once).
pub fn install(factory: impl Fn() -> Arc<dyn ScreenMirror> + Send + Sync + 'static) {
    let _ = FACTORY.set(Box::new(factory));
}

/// A new mirror, when an app installed a factory.
pub fn create() -> Option<Arc<dyn ScreenMirror>> {
    FACTORY.get().map(|f| f())
}

/// True when panes are mirrored (the desktop app is running).
pub fn active() -> bool {
    FACTORY.get().is_some()
}

/// Something only the desktop app can do, asked for by an action (it reads these after each batch of events).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// show or hide the media panel
    MediaPanel,
}

static REQUESTS: std::sync::Mutex<Vec<Request>> = std::sync::Mutex::new(Vec::new());

pub fn request(r: Request) {
    if let Ok(mut q) = REQUESTS.lock() {
        q.push(r);
    }
}

/// The requests since the last call, oldest first.
pub fn take_requests() -> Vec<Request> {
    REQUESTS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}
