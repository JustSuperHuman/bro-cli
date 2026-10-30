//! Session scrolling keeps wheel notches separate from the touchpad's logical pixels.
use super::{App, PaneState};
use crate::layout::PaneId;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::HashMap;

const ROWS_PER_NOTCH: f32 = 2.0;

#[derive(Clone, Copy, Debug)]
pub(super) enum Wheel {
    Lines(f32),
    Pixels(f32),
}

impl Wheel {
    pub fn from_event(delta: winit::event::MouseScrollDelta, scale: f32) -> Self {
        match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => Self::Lines(y),
            winit::event::MouseScrollDelta::PixelDelta(p) => Self::Pixels(p.y as f32 / scale),
        }
    }

    /// Other surfaces retain their existing wheel units.
    pub fn units(self) -> f32 {
        match self {
            Self::Lines(n) => n,
            Self::Pixels(px) => px / 18.0,
        }
    }

    pub fn pixels(self, pixels_per_notch: f32) -> f32 {
        match self {
            Self::Lines(n) => n * height(pixels_per_notch),
            Self::Pixels(px) => px,
        }
    }

    pub fn rows(self, row_height: f32) -> f32 {
        self.pixels(ROWS_PER_NOTCH * height(row_height)) / height(row_height)
    }

    pub fn notches(self, row_height: f32) -> f32 {
        self.rows(row_height) / ROWS_PER_NOTCH
    }
}

fn height(row_height: f32) -> f32 {
    if row_height.is_finite() && row_height > 0.0 {
        row_height
    } else {
        18.0
    }
}

/// Keep sub-row motion without turning every tiny touchpad event into a full scroll.
#[derive(Default)]
pub(super) struct Accumulator {
    remainder: f32,
}

impl Accumulator {
    pub fn take(&mut self, amount: f32) -> i32 {
        if !amount.is_finite() || amount == 0.0 {
            return 0;
        }
        if self.remainder.signum() != amount.signum() {
            self.remainder = 0.0;
        }
        self.remainder += amount;
        let whole = self.remainder.trunc() as i32;
        self.remainder -= whole as f32;
        whole
    }
}

pub(super) fn history(app: &mut App, id: PaneId, available: usize, rows: i32) {
    if let Some(term) = app.panes.get_mut(&id).and_then(|pane| pane.as_term()) {
        term.set_scroll_offset(
            term.scroll_offset()
                .saturating_add_signed(rows as isize)
                .min(available),
        );
    }
}

/// Ctrl+Shift+Up/Down scrolls locally; Shift+Up/Down remains available to the session's program.
pub(super) fn key(app: &mut App, panes: &mut HashMap<PaneId, PaneState>, key: KeyEvent) -> bool {
    if key.kind == KeyEventKind::Release
        || key.modifiers != (KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        || app.side_focus
        || app.overlay.is_open()
        || app.prefix_armed
        || app.renaming.is_some()
    {
        return false;
    }
    let rows = match key.code {
        KeyCode::Up => 1,
        KeyCode::Down => -1,
        _ => return false,
    };
    let Some(id) = app.focused() else {
        return false;
    };
    if app.flips.contains_key(&id)
        || app
            .panes
            .get(&id)
            .and_then(|pane| pane.as_term_ref())
            .is_none()
    {
        return false;
    }
    if app.touched() {
        return true;
    }
    let state = panes.entry(id).or_default();
    state.scroll_acc = Accumulator::default();
    state.program_scroll_acc = Accumulator::default();
    if app.focus_view(id) {
        state.focus.scroll_rows(rows as f32);
    } else if app.panes.get(&id).is_some_and(|pane| pane.wants_mouse())
        && app.panes[&id].as_term_ref().is_some_and(|term| term.scroll_offset() == 0)
    {
        // Full-screen agents own their history. Use the same unmodified wheel event as the mouse,
        // in the output area rather than the prompt; sending arrow keys can change agent settings.
        use crossterm::event::{MouseEvent, MouseEventKind};
        let drawn = state.drawn;
        let area = ratatui::layout::Rect::new(0, 0, drawn.cols.max(1) as u16, drawn.rows.max(1) as u16);
        let event = MouseEvent {
            kind: if rows > 0 { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown },
            column: area.width / 2,
            row: area.height / 2,
            modifiers: KeyModifiers::NONE,
        };
        app.with_pane(id, |pane, cx| pane.mouse(event, area, cx));
    } else {
        history(app, id, state.drawn.history, rows);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_agent_scrolls_its_own_history_with_wheel_events_instead_of_arrow_keys() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc.clone(), tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        let mut meta = app.panes[&id].as_term_ref().unwrap().meta.clone();
        meta.harness = Some(bro_core::Harness::Codex);
        let term = crate::panes::term::Term::fixed(meta, "\x1b[?1049h\x1b[?1000h\x1b[?1006hCodex", svc);
        app.panes.insert(id, Box::new(term));
        let mut panes = HashMap::new();
        let drawn = &mut panes.entry(id).or_insert_with(PaneState::default).drawn;
        drawn.cols = 80;
        drawn.rows = 24;
        // Alternate screens have no local scrollback: the old handler silently clamped both keys to zero.
        assert_eq!(drawn.history, 0);
        assert!(key(&mut app, &mut panes, KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT)));
        assert!(key(&mut app, &mut panes, KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT)));
        assert_eq!(app.panes[&id].as_term_ref().unwrap().sent, b"\x1b[<64;41;13M\x1b[<65;41;13M");
        assert!(!key(&mut app, &mut panes, KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)));
        app.panes.get_mut(&id).unwrap().as_term().unwrap().set_scroll_offset(2);
        panes.get_mut(&id).unwrap().drawn.history = 10;
        assert!(key(&mut app, &mut panes, KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT)));
        assert_eq!(app.panes[&id].as_term_ref().unwrap().scroll_offset(), 3, "keep reading local history once scrolled back");
    }

    #[test]
    fn wheel_notches_move_two_rows_and_touchpad_pixels_keep_their_distance() {
        assert_eq!(Wheel::Lines(1.0).rows(24.0), 2.0);
        assert_eq!(Wheel::Lines(-1.0).pixels(30.0), -30.0);
        assert_eq!(Wheel::Pixels(12.0).rows(24.0), 0.5);
        assert_eq!(Wheel::Pixels(12.0).pixels(30.0), 12.0);
        assert_eq!(Wheel::Pixels(48.0).notches(24.0), 1.0);
        let physical = winit::event::MouseScrollDelta::PixelDelta(
            winit::dpi::PhysicalPosition::new(0.0, 24.0),
        );
        assert_eq!(Wheel::from_event(physical, 2.0).pixels(30.0), 12.0);
    }

    #[test]
    fn small_touchpad_events_accumulate_and_reverse_without_a_stale_remainder() {
        let mut acc = Accumulator::default();
        for _ in 0..3 {
            assert_eq!(acc.take(0.25), 0);
        }
        assert_eq!(acc.take(0.25), 1);
        assert_eq!(acc.take(0.75), 0);
        assert_eq!(acc.take(-1.0), -1);
        assert_eq!(acc.take(f32::NAN), 0);
        assert_eq!(acc.take(1.0), 1);
        let mut program = Accumulator::default();
        for _ in 0..7 {
            assert_eq!(program.take(Wheel::Pixels(6.0).notches(24.0)), 0);
        }
        assert_eq!(program.take(Wheel::Pixels(6.0).notches(24.0)), 1);
    }

    #[test]
    fn ctrl_shift_arrows_scroll_only_the_active_session_and_preserve_agent_shortcuts() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc =
            crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(
            svc,
            tx,
            crate::app::Opts {
                demo: true,
                fixed_demo: true,
                load_recents: false,
            },
        );
        let id = app.focused().unwrap();
        let mut panes = HashMap::new();
        panes
            .entry(id)
            .or_insert_with(PaneState::default)
            .drawn
            .history = 2;
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        let offset = |app: &App| app.panes[&id].as_term_ref().unwrap().scroll_offset();
        assert!(key(&mut app, &mut panes, up));
        assert_eq!(offset(&app), 1);
        assert!(key(&mut app, &mut panes, up));
        assert!(key(&mut app, &mut panes, up));
        assert_eq!(offset(&app), 2);
        assert!(key(&mut app, &mut panes, down));
        assert_eq!(offset(&app), 1);
        assert!(key(&mut app, &mut panes, down));
        assert!(key(&mut app, &mut panes, down));
        assert_eq!(offset(&app), 0);
        assert!(!key(
            &mut app,
            &mut panes,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)
        ));
        assert!(!key(
            &mut app,
            &mut panes,
            KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)
        ));
        assert!(!key(&mut app, &mut panes, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)));
        assert_eq!(offset(&app), 0, "Codex's reasoning shortcuts must not scroll history");
        app.side_focus = true;
        assert!(!key(&mut app, &mut panes, up));
        app.side_focus = false;
        app.prefix_armed = true;
        assert!(!key(&mut app, &mut panes, up));
        app.prefix_armed = false;
        app.open_launcher(None, crate::pane::Place::Tab);
        assert!(!key(&mut app, &mut panes, up));
    }
}
