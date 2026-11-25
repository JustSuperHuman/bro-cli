//! Motion helpers for focus mode, on top of the kit's springs ([`crate::app::gui::kit::motion`]). They live here
//! until they're promoted to the kit.

use crate::app::gui::kit::Ui;
use crate::app::gui::kit::motion::Spring;
use bro_gfx::{Quad, Rect};

/// Things appearing in the conversation: a rise of a few pixels over a fade.
pub(super) const RISE: f32 = 12.0;
/// The conversation's scroll: quick, no overshoot, keeping its velocity when the wheel moves it again.
pub(super) const SCROLL: Spring = Spring::new(0.28, 0.0);
/// The composer growing and shrinking, the strip over it coming and going.
pub(super) const GROW: Spring = Spring::new(0.3, 0.0);
/// Expanding and collapsing a card.
pub(super) const FOLD: Spring = Spring::new(0.3, 0.0);

impl Ui<'_> {
    /// Cover `r` with the card's surface so what's under it shows only `shown` (0..1): a fade for anything made of
    /// many pieces (a message, a card, a chip), drawn over the layer's text.
    pub(super) fn veil(&mut self, r: Rect, radius: f32, shown: f32) {
        let a = 1.0 - shown.clamp(0.0, 1.0);
        if a <= 0.004 || r.is_empty() {
            return;
        }
        let p = self.px(r);
        let q = Quad::new(p, self.t.surface.with_alpha(a)).radius(radius * self.scale);
        self.scene.top_glass_quad(q);
    }

    /// 0 → 1 for a thing that appears: `key`'s spring from nothing on `spring` when `fresh`, else (already there) 1.
    /// A thing no longer fresh keeps animating until it lands, so ask for it every frame it's drawn.
    pub(super) fn arrival(&mut self, key: impl std::hash::Hash, fresh: bool, spring: Spring) -> f32 {
        if fresh {
            self.spring(key, 1.0, spring, 0.0)
        } else {
            1.0
        }
    }

    /// A mark that changes state popping in: its scale (0..~1.1) `s` seconds after the change (None: no change).
    pub(super) fn pop_scale(&mut self, s: Option<f32>) -> f32 {
        match s {
            Some(s) if s < 1.0 => {
                self.anim.moving = true;
                Spring::POP.step(s).max(0.0)
            }
            _ => 1.0,
        }
    }
}
