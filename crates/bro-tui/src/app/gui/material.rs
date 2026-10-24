//! The window's material (Settings → Appearance → Window): solid, or see-through with the desktop behind it,
//! frosted (Acrylic), washed in the wallpaper's colours (Mica) or clear (Glass). The system draws what's behind
//! (`window::style`); bro lays the theme over it evenly: the window's surfaces (its content area, the session
//! cards, the panels on them) are glass fills, all exactly as see-through as the window, and on Windows the title
//! bar is bro's own (`caption`) on the same glass. Text, controls and popups stay solid.
//!
//! A see-through window is made that way (`window::create`) and drawn through a swap chain that carries alpha.
//! The first time a material is picked in a solid window, that window is swapped for a see-through one in the
//! same place (the sessions don't notice); going back to solid keeps it and draws it opaque.

use super::{Gfx, Gui, window};
use bro_core::config::Material;
use bro_gfx::{Options, Renderer};
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;

/// How solid the theme's colour is over a see-through window, from the tint setting (percent). The whole window
/// is that see-through, its surfaces included (they're glass fills: see `Ui::glass`).
pub fn alpha(material: Material, tint: u8) -> f32 {
    if !material.see_through() {
        return 1.0;
    }
    tint.clamp(10, 100) as f32 / 100.0
}

impl Gui {
    /// The interface's colours for the theme, thinned for the material that's showing.
    pub(super) fn make_tokens(&self) -> super::kit::Tokens {
        let t = super::kit::Tokens::from_theme(&self.app.theme);
        match self.material {
            Some((m, tint)) if self.see_through => t.with_material(alpha(m, tint)),
            _ => t,
        }
    }

    /// The window can show a material: made see-through, drawn through a swap chain with alpha.
    fn can_see_through(&self) -> bool {
        self.window_see_through && self.gfx.as_ref().is_some_and(|g| g.renderer.see_through())
    }

    /// Put the material the settings ask for on the window: the system's backdrop, the thinned fills, and greyscale
    /// text over anything see-through (subpixel colour fringes would show on what's behind). A solid window asked
    /// for a material is swapped first (`about_to_wait` does that: it has the event loop).
    pub(super) fn sync_material(&mut self) {
        let s = self.app.svc.settings();
        let want = (s.material, s.material_tint);
        if self.material == Some(want) || self.gfx.is_none() || self.swap_window {
            return;
        }
        let asked = want.0.see_through();
        if asked && !self.can_see_through() && !self.see_through_tried {
            self.see_through_tried = true;
            self.swap_window = true;
            return;
        }
        self.material = Some(want);
        let can = self.can_see_through();
        let styled = match &self.window {
            Some(w) => window::style(w, self.tokens.bg, self.tokens.light, if can { want.0 } else { Material::Solid }),
            None => false,
        };
        let shown = asked && can && styled;
        // the system's title bar would sit in the bare material: bro draws its own on the window's glass (Windows,
        // where the frame is the app's to lay out)
        if let Some(w) = &self.window {
            super::caption::set(w, shown && cfg!(windows));
        }
        if asked && !shown && self.material_warned != Some(want.0) {
            self.material_warned = Some(want.0);
            let why = if can { "Acrylic and Mica need Windows 11 · Glass works here" } else { "this graphics driver can't draw a see-through window" };
            self.app.toast(crate::alerts::Kind::Info, format!("window stays solid: {why}"));
        }
        self.see_through = shown;
        self.tokens = self.make_tokens();
        if let Some(g) = &mut self.gfx {
            g.text.set_antialias(window::text_antialias(g.renderer.subpixel_text() && !shown));
        }
        self.dirty = true;
    }

    /// Swap the solid window for a see-through one in the same place and state, with a renderer whose swap chain
    /// carries alpha. Every glyph and picture uploads again; the old window goes once the new one is on screen.
    pub(super) fn swap_window(&mut self, el: &ActiveEventLoop) {
        self.swap_window = false;
        let (Some(old), Some(gfx)) = (self.window.clone(), self.gfx.take()) else { return };
        self.prefs.note_size(Some(&old));
        let maximized = old.is_maximized();
        let fullscreen = old.fullscreen();
        let Gfx { renderer, mut text, scene } = gfx;
        // the old window keeps its renderer until we know the new one starts
        let new = window::create(el, &self.prefs, true).map(Arc::new).and_then(|w| {
            if let Ok(p) = old.outer_position() {
                w.set_outer_position(p);
            }
            let size = w.inner_size();
            Renderer::with_options(w.clone(), size.width, size.height, Options { vsync: true, transparent: true }).map(|r| (w, r))
        });
        let (w, renderer) = match new {
            Ok((w, r)) if r.see_through() => {
                drop(renderer);
                (w, r)
            }
            _ => {
                // no see-through window here: carry on as we were (the material says why, once)
                self.gfx = Some(Gfx { renderer, text, scene });
                self.material = None;
                return;
            }
        };
        if maximized {
            w.set_maximized(true);
        }
        w.set_fullscreen(fullscreen);
        w.set_ime_allowed(true);
        window::remember_handle(&w);
        self.scale = w.scale_factor() as f32;
        self.font_px = 0.0;
        text.reset_atlases();
        text.set_antialias(window::text_antialias(renderer.subpixel_text()));
        self.media.gpu_lost();
        self.gfx = Some(Gfx { renderer, text, scene });
        self.window = Some(w);
        self.old_window = Some(old);
        self.window_see_through = true;
        // shown after its first frame (which puts the material on), and the old one goes then
        self.first_frame_done = false;
        self.title.clear();
        self.material = None;
        self.theme_key.clear();
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tint_is_the_whole_windows_alpha() {
        assert_eq!(alpha(Material::Solid, 30), 1.0, "solid ignores the tint");
        for m in [Material::Acrylic, Material::Mica, Material::Glass] {
            for tint in bro_core::config::MATERIAL_TINTS {
                assert!((alpha(m, tint) - tint as f32 / 100.0).abs() < 1e-6);
            }
        }
    }
}
