//! The desktop app's own preferences (`~/.bro/v2-gui.json`): window placement, terminal font size, panel
//! widths. Everything the terminal UI also has stays in `v2.toml`.

use serde::{Deserialize, Serialize};

pub const DEFAULT_FONT_SIZE: f32 = 14.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// terminal font size, logical pixels
    pub font_size: f32,
    /// terminal line height as a multiple of the font's
    pub line_height: f32,
    pub sidebar_width: f32,
    pub media_width: f32,
    pub media_open: bool,
    /// logical size and position; maximized
    pub window: Option<(f64, f64)>,
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { font_size: DEFAULT_FONT_SIZE, line_height: 1.18, sidebar_width: 272.0, media_width: 380.0, media_open: false, window: None, position: None, maximized: false }
    }
}

fn path() -> std::path::PathBuf {
    crate::util::bro_dir().join("v2-gui.json")
}

impl Prefs {
    /// Read them (`persist` off: demo and tests use the defaults).
    pub fn load(persist: bool) -> Prefs {
        if !persist {
            return Prefs::default();
        }
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, persist: bool) {
        if !persist {
            return;
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = crate::util::atomic_write(&path(), s.as_bytes());
        }
    }

    /// Remember where the window is (not while maximised or minimised: keep the restore size).
    pub fn note_size(&mut self, w: Option<&winit::window::Window>) {
        let Some(w) = w else { return };
        if w.is_minimized() == Some(true) {
            return;
        }
        self.maximized = w.is_maximized();
        if self.maximized {
            return;
        }
        let scale = w.scale_factor();
        let s = w.inner_size();
        // bro's own title bar is client area: the size kept is the one under a system title bar
        let band = if super::caption::on() && w.fullscreen().is_none() { super::caption::HEIGHT as f64 } else { 0.0 };
        self.window = Some((s.width as f64 / scale, (s.height as f64 / scale - band).max(1.0)));
        if let Ok(p) = w.outer_position() {
            self.position = Some((p.x, p.y));
        }
    }
}
