//! The new-session page as a pane, so it can sit in a split beside a session, usage and tokens (the JustGains
//! showcase). The page is the launcher's own, drawn in the pane's card; it is for looking at, not for launching from.

use crate::launcher::{Launcher, view};
use crate::pane::{Cx, Pane};
use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect};

pub struct NewSessionView {
    pub launcher: Launcher,
}

impl NewSessionView {
    pub fn new(launcher: Launcher) -> NewSessionView {
        NewSessionView { launcher }
    }
}

impl Pane for NewSessionView {
    fn title(&self) -> String {
        "new session".into()
    }
    fn icon(&self) -> &'static str {
        "rocket"
    }
    fn view(&self) -> Option<&'static str> {
        Some("new-session")
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        view::draw(f, area, &mut self.launcher, cx.theme, 0.0);
    }
    fn key(&mut self, key: KeyEvent, _cx: &mut Cx) -> bool {
        // browsing the page is fine; starting a session from it is not what this pane is for
        let _ = self.launcher.key(key);
        true
    }
}
