//! Fonts for bro's own window. bro bundles a handful of open-source monospace fonts (licences in
//! `assets/fonts/licenses`); picking one unpacks it into `~/.bro/fonts` and asks the terminal to switch with
//! xterm's "set font" sequence, `OSC 50 ; <face> ST`.
//!
//! Only the Just Terminal fork understands that sequence (it also loads `~/.bro/fonts` without the fonts being
//! installed, and notices new files there), so the choice is offered only when bro-app opened bro in the fork
//! (`--app-window`), or with `BRO_TERMINAL_FONTS=1`. The Nerd Font symbols go along as a fallback face so bro's
//! icons survive any font.

use crate::alerts::Kind;
use crate::pane::Event;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

pub struct Font {
    pub id: &'static str,
    pub label: &'static str,
    /// the family name inside the file, which the terminal matches on
    pub family: &'static str,
    pub file: &'static str,
    pub data: &'static [u8],
    /// a few words on its character
    pub blurb: &'static str,
}

macro_rules! font {
    ($id:literal, $label:literal, $family:literal, $file:literal, $blurb:literal) => {
        Font { id: $id, label: $label, family: $family, file: $file, data: include_bytes!(concat!("../assets/fonts/", $file)), blurb: $blurb }
    };
}

/// The bundled fonts, roughly calm to wild.
pub const FONTS: &[Font] = &[
    font!("jetbrains-mono", "JetBrains Mono", "JetBrains Mono", "JetBrainsMono-Variable.ttf", "clean, tall, made for code"),
    font!("fira-code", "Fira Code", "Fira Code", "FiraCode-Variable.ttf", "ligatures: -> and != join up"),
    font!("ibm-plex-mono", "IBM Plex Mono", "IBM Plex Mono", "IBMPlexMono-Regular.ttf", "corporate typewriter"),
    font!("maple-mono", "Maple Mono", "Maple Mono", "MapleMono-Regular.ttf", "soft, rounded, friendly"),
    font!("victor-mono", "Victor Mono", "Victor Mono", "VictorMono-Variable.ttf", "slender, with cursive italics"),
    font!("monaspace-krypton", "Monaspace Krypton", "Monaspace Krypton", "MonaspaceKrypton-Regular.otf", "mechanical, machined edges"),
    font!("monaspace-radon", "Monaspace Radon", "Monaspace Radon", "MonaspaceRadon-Regular.otf", "handwritten code"),
    font!("space-mono", "Space Mono", "Space Mono", "SpaceMono-Regular.ttf", "retro-future, wide and geometric"),
    font!("share-tech-mono", "Share Tech Mono", "Share Tech Mono", "ShareTechMono-Regular.ttf", "sci-fi console"),
    font!("comic-mono", "Comic Mono", "Comic Mono", "ComicMono.ttf", "Comic Sans, but monospace"),
    font!("departure-mono", "Departure Mono", "Departure Mono", "DepartureMono-Regular.otf", "pixel font from a spaceship"),
    font!("vt323", "VT323", "VT323", "VT323-Regular.ttf", "a real DEC VT320 terminal"),
    // video game eras
    font!("kode-mono", "Kode Mono", "Kode Mono", "KodeMono-Variable.ttf", "sci-fi game HUD"),
    font!("doto", "Doto", "Doto", "Doto-Variable.ttf", "dot-matrix arcade scoreboard"),
    font!("monocraft", "Monocraft", "Monocraft", "Monocraft.ttf", "blocky, like Minecraft"),
    font!("ibm-vga", "IBM VGA 8x16", "PxPlus IBM VGA 8x16", "PxPlus_IBM_VGA_8x16.ttf", "DOS games: Doom, Keen, Monkey Island"),
    font!("workbench", "Workbench", "Workbench", "Workbench-Variable.ttf", "the Amiga"),
    font!("sixtyfour", "Sixtyfour", "Sixtyfour", "Sixtyfour-Variable.ttf", "Commodore 64, LOAD\"*\",8,1"),
    font!("press-start-2p", "Press Start 2P", "Press Start 2P", "PressStart2P-Regular.ttf", "8-bit arcade, huge"),
];

/// Kept behind every font so Nerd Font icons still draw.
const SYMBOLS: Font = font!("symbols", "Symbols Nerd Font Mono", "Symbols Nerd Font Mono", "SymbolsNerdFontMono-Regular.ttf", "");

/// bro-app opened bro in the fork (`--app-window`).
pub static APP_WINDOW: AtomicBool = AtomicBool::new(false);
/// An OSC 50 waiting for the next frame (written between frames so it never splits one).
static PENDING: Mutex<Option<String>> = Mutex::new(None);
/// A font was switched this run, so restore the terminal's own on exit.
static CHANGED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    /// Tests turn the fork's window on for their own thread only.
    pub static TEST_APP_WINDOW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The terminal can switch fonts for us.
pub fn available() -> bool {
    #[cfg(test)]
    return TEST_APP_WINDOW.with(|w| w.get());
    #[cfg(not(test))]
    return APP_WINDOW.load(Ordering::Relaxed) || std::env::var_os("BRO_TERMINAL_FONTS").is_some_and(|v| v == "1");
}

pub fn get(id: &str) -> Option<&'static Font> {
    FONTS.iter().find(|f| f.id == id)
}

fn dir() -> PathBuf {
    crate::util::bro_dir().join("fonts")
}

/// Write `font` into `dir` unless an identical copy is there (temp + rename, so the terminal never reads half).
fn unpack(dir: &Path, font: &Font) -> std::io::Result<()> {
    let path = dir.join(font.file);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == font.data.len() as u64) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    crate::util::atomic_write(&path, font.data)
}

/// The OSC 50 for a font id (None = the terminal's own font).
fn osc(font: Option<&Font>) -> String {
    let face = font.map(|f| format!("{}, {}", f.family, SYMBOLS.family)).unwrap_or_default();
    format!("\x1b]50;{face}\x1b\\")
}

/// Switch to `id` (None = the terminal's own font): unpack it off the UI thread, then wake the loop to send the
/// sequence. Does nothing where the terminal can't switch.
pub fn apply(id: Option<&str>, tx: &Sender<Event>) {
    if !available() {
        return;
    }
    let font = id.and_then(get);
    let tx = tx.clone();
    let _ = std::thread::Builder::new().name("bro-fonts".into()).spawn(move || {
        if let Some(f) = font {
            let d = dir();
            if let Err(e) = unpack(&d, f).and_then(|_| unpack(&d, &SYMBOLS)) {
                let _ = tx.send(Event::Toast(Kind::Error, format!("couldn't unpack {}: {e}", f.label)));
                return;
            }
        }
        *PENDING.lock() = Some(osc(font));
        let _ = tx.send(Event::Services);
    });
}

/// Send a waiting font switch (call between frames).
pub fn flush(out: &mut impl std::io::Write) -> std::io::Result<()> {
    let Some(seq) = PENDING.lock().take() else { return Ok(()) };
    CHANGED.store(true, Ordering::Relaxed);
    out.write_all(seq.as_bytes())?;
    out.flush()
}

/// On exit: hand the terminal its own font back if bro changed it.
pub fn restore(out: &mut impl std::io::Write) -> std::io::Result<()> {
    if !CHANGED.swap(false, Ordering::Relaxed) {
        return Ok(());
    }
    out.write_all(osc(None).as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_unpack_once_and_switch_with_osc_50() {
        let dir = tempfile::tempdir().unwrap();
        let f = get("departure-mono").unwrap();
        unpack(dir.path(), f).unwrap();
        let path = dir.path().join(f.file);
        assert_eq!(std::fs::read(&path).unwrap(), f.data);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        unpack(dir.path(), f).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp, "an identical copy isn't rewritten");
        assert_eq!(osc(Some(f)), "\x1b]50;Departure Mono, Symbols Nerd Font Mono\x1b\\");
        assert_eq!(osc(None), "\x1b]50;\x1b\\", "empty = the terminal's own font");
        let mut ids: Vec<_> = FONTS.iter().map(|f| f.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), FONTS.len(), "unique ids");
        assert!(FONTS.iter().all(|f| f.data.len() > 10_000), "every file is bundled");
    }

    #[test]
    fn flush_sends_a_pending_switch_once_and_restore_undoes_it() {
        *PENDING.lock() = Some(osc(get("vt323")));
        let mut out = vec![];
        flush(&mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b]50;VT323, Symbols Nerd Font Mono\x1b\\");
        let mut again = vec![];
        flush(&mut again).unwrap();
        assert!(again.is_empty());
        let mut back = vec![];
        restore(&mut back).unwrap();
        assert_eq!(String::from_utf8(back).unwrap(), "\x1b]50;\x1b\\");
    }
}
