//! Panic containment for calls into bro-core / bro-proxy / bro-bridge.
//!
//! Those crates may still contain `todo!()` bodies (or just have bugs). Every call goes through [`guard`], which
//! turns a panic into an `Err(String)` so the UI can show "unavailable" instead of crashing. The panic hook keeps
//! quiet while the TUI owns the screen (a panic message printed over the alternate screen would corrupt it):
//! guarded and background-thread panics are only appended to `~/.bro/v2-panic.log`; an unguarded panic on the
//! UI thread restores the terminal first and then prints normally.

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    static GUARDED: Cell<u32> = const { Cell::new(0) };
}

/// Set while the TUI owns the terminal (raw mode + alternate screen).
pub static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

/// Run `f`, converting a panic into `Err("<what>: <message>")`.
pub fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    GUARDED.with(|g| g.set(g.get() + 1));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    GUARDED.with(|g| g.set(g.get().saturating_sub(1)));
    r.map_err(|p| format!("{what}: {}", friendly(&panic_text(p.as_ref()))))
}

/// [`guard`] for functions returning `anyhow::Result`: panics and errors both become `Err(String)`.
pub fn guard_res<T>(what: &str, f: impl FnOnce() -> anyhow::Result<T>) -> Result<T, String> {
    match guard(what, f) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{what}: {e:#}")),
        Err(e) => Err(e),
    }
}

/// True if an error string came from a `todo!()` (the other crate isn't implemented yet).
pub fn is_unimplemented(err: &str) -> bool {
    err.contains("not implemented yet")
}

fn friendly(msg: &str) -> String {
    if msg.contains("not yet implemented") || msg.contains("not implemented") {
        "not implemented yet".into()
    } else {
        msg.to_string()
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".into()
    }
}

/// Install the quiet panic hook. Call once from `main`, on the UI thread.
pub fn install_panic_hook() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let guarded = GUARDED.with(|g| g.get()) > 0;
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        log(&format!("{} [{}] {msg} at {at}", crate::util::now_secs(), if guarded { "guarded" } else { "panic" }));
        if guarded {
            return;
        }
        let on_main = MAIN_THREAD.get().is_some_and(|id| *id == std::thread::current().id());
        if TUI_ACTIVE.load(Ordering::SeqCst) {
            if !on_main {
                return; // a background thread died: logged, the UI carries on
            }
            restore_terminal();
        }
        default(info);
    }));
}

/// Leave raw mode / the alternate screen so a crash message is readable.
pub fn restore_terminal() {
    use crossterm::{event, execute, terminal};
    TUI_ACTIVE.store(false, Ordering::SeqCst);
    let _ = terminal::disable_raw_mode();
    let _ = execute!(std::io::stdout(), event::DisableMouseCapture, event::DisableBracketedPaste, event::DisableFocusChange, terminal::LeaveAlternateScreen, crossterm::cursor::Show);
}

fn log(line: &str) {
    if cfg!(test) {
        return;
    }
    use std::io::Write;
    let path = crate::util::bro_dir().join("v2-panic.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// A flag that trips after the first panic from a hot-path call (bridge output), so a broken dependency
/// costs one caught panic instead of one per PTY chunk.
#[derive(Default)]
pub struct Fuse(AtomicBool);

impl Fuse {
    pub fn blown(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    /// Run `f` unless blown; blow on panic.
    pub fn run<T>(&self, what: &str, f: impl FnOnce() -> T) -> Option<T> {
        if self.blown() {
            return None;
        }
        match guard(what, f) {
            Ok(v) => Some(v),
            Err(_) => {
                self.0.store(true, Ordering::Relaxed);
                None
            }
        }
    }
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_catches_todo() {
        let r: Result<u32, String> = guard("usage::fetch", || todo!());
        let e = r.unwrap_err();
        assert!(e.starts_with("usage::fetch"), "{e}");
        assert!(is_unimplemented(&e), "{e}");
        assert_eq!(guard("x", || 5).unwrap(), 5);
        let r: Result<u32, String> = guard_res("cfg", || Err(anyhow::anyhow!("bad file")));
        assert_eq!(r.unwrap_err(), "cfg: bad file");
        let fuse = Fuse::default();
        assert_eq!(fuse.run("a", || 1), Some(1));
        assert_eq!(fuse.run("b", || -> i32 { panic!("boom") }), None);
        assert!(fuse.blown());
        assert_eq!(fuse.run("c", || 1), None, "stays off after a panic");
    }
}
