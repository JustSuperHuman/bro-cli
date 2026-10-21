//! Terminal screens: alacritty_terminal mirrors of each session ([`mirror`]) and drawing them ([`draw`]).

pub mod draw;
pub mod mirror;

use crate::app::gui::kit::Tokens;
use parking_lot::Mutex;
use std::sync::{Arc, OnceLock};

static ENV: OnceLock<Arc<Mutex<mirror::Env>>> = OnceLock::new();

/// What new screens answer colour and size queries with (shared with the mirror factory).
pub fn shared_env() -> Arc<Mutex<mirror::Env>> {
    ENV.get_or_init(|| Arc::new(Mutex::new(mirror::Env::default()))).clone()
}

/// The theme or font changed: every screen answers with the new colours and cell size from now on.
pub fn update_env(t: &Tokens, cell: (u16, u16)) {
    let rgb = |c: bro_gfx::Color| {
        let [r, g, b, _] = c.to_rgba8();
        [r, g, b]
    };
    let env = mirror::Env { fg: rgb(t.term_fg), bg: rgb(t.term_bg), cursor: rgb(t.cursor), palette: t.ansi.map(rgb), cell_w: cell.0, cell_h: cell.1 };
    *shared_env().lock() = env;
}
