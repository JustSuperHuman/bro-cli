//! The open overlay. Each one is drawn natively by its own module when that module does (see
//! [`super::surface`]); otherwise the terminal UI's drawing of it is painted cell for cell and clicks are
//! translated to its cells, so every overlay works either way.

mod confirm;
mod help;
mod palette;
mod resume;

use super::Gui;
use super::kit::Ui;
use super::surface::Cx;
use super::tui::{Grid, TuiCanvas};
use crate::app::{App, Overlay};
use bro_gfx::{Point, Rect};
use crossterm::event::{KeyModifiers, MouseButton as CButton};

/// How the open overlay was drawn this frame.
#[derive(Clone, Copy, Debug)]
pub enum Drawn {
    Native,
    Fallback(Grid),
}

pub fn draw(app: &mut App, ui: &mut Ui, canvas: &mut TuiCanvas, window: Rect, body: Rect, focused: bool) -> Option<Drawn> {
    if !app.overlay.is_open() {
        return None;
    }
    let time = app.start.elapsed().as_secs_f64();
    app.refresh_team_view();
    let prev = ui.layer();
    let native = match &app.overlay {
        Overlay::Palette(_) => palette::draw(app, ui, window, time),
        Overlay::Help(_) => help::draw(app, ui, window, time),
        Overlay::Resume(_) => resume::draw(app, ui, window, time),
        Overlay::Confirm(_) => confirm::draw(app, ui, window, time),
        _ => {
            let alt = app.alt_down.is_some_and(|t| t.elapsed() >= crate::app::alt_hints::HOLD);
            let cx = Cx { svc: &app.svc, keymap: &app.keymap, time, focused, alt };
            match &mut app.overlay {
                Overlay::Launcher(l) => crate::launcher::gui::draw(l, ui, body, &cx),
                Overlay::File(v) => crate::file_view::gui::draw(v, ui, window, &cx),
                Overlay::AgentFiles(v) => crate::agent_files::gui::draw(v, ui, window, &cx),
                Overlay::Team(v) => crate::team_board::gui::draw(v, ui, window, &cx),
                Overlay::Watchdog(v) => {
                    let dog = app.svc.watchdog(&v.sid);
                    crate::watchdog::view::gui::draw(v, ui, window, dog.as_ref(), &cx)
                }
                Overlay::WatchdogPrompt(e) => crate::watchdog::prompt_editor::gui::draw(e, ui, window, &cx),
                _ => false,
            }
        }
    };
    ui.set_layer(prev);
    if native {
        return Some(Drawn::Native);
    }
    super::tui::draw_overlay(app, ui, canvas, window, body).map(Drawn::Fallback)
}

/// Target `n` of the natively drawn overlay was clicked.
pub fn click(g: &mut Gui, n: usize, p: Point, button: CButton, m: KeyModifiers, count: u32) {
    if button != CButton::Left {
        return;
    }
    let app = &mut g.app;
    match &app.overlay {
        Overlay::Palette(_) => return palette::click(app, n, p, m, count),
        Overlay::Help(_) => return help::click(app, n, p, m, count),
        Overlay::Resume(_) => return resume::click(app, n, p, m, count),
        Overlay::Confirm(_) => return confirm::click(app, n, p, m, count),
        _ => {}
    }
    match &mut app.overlay {
        Overlay::Launcher(l) => {
            let o = crate::launcher::gui::click(l, n, p, m, count);
            apply_launcher(app, o);
        }
        Overlay::File(v) => {
            let o = crate::file_view::gui::click(v, n, p, m, count);
            app.file_view_outcome(o);
        }
        Overlay::AgentFiles(v) => {
            let o = crate::agent_files::gui::click(v, n, p, m, count);
            app.agent_files_outcome(o);
        }
        Overlay::Team(v) => {
            let o = crate::team_board::gui::click(v, n, p, m, count);
            app.team_outcome(o);
        }
        Overlay::Watchdog(v) => {
            let dog = app.svc.watchdog(&v.sid);
            let o = crate::watchdog::view::gui::click(v, n, p, m, count, dog.as_ref());
            app.watchdog_outcome(o);
        }
        Overlay::WatchdogPrompt(e) => {
            let o = crate::watchdog::prompt_editor::gui::click(e, n, p, m, count);
            app.watchdog_prompt_outcome(o);
        }
        _ => {}
    }
}

/// The wheel over the natively drawn overlay (`n`: the target under the pointer, if any).
pub fn wheel(g: &mut Gui, n: Option<usize>, down: bool) {
    let shift = g.mods.shift_key();
    let app = &mut g.app;
    match &mut app.overlay {
        // shift+wheel moves a file sideways 8 columns a notch, as the terminal UI's horizontal wheel does
        Overlay::File(v) if shift => v.left = if down { v.left + 8 } else { v.left.saturating_sub(8) },
        Overlay::Palette(_) => palette::wheel(app, n, down),
        Overlay::Help(_) => help::wheel(app, n, down),
        Overlay::Resume(_) => resume::wheel(app, n, down),
        Overlay::Launcher(l) => crate::launcher::gui::wheel(l, n, down),
        Overlay::File(v) => crate::file_view::gui::wheel(v, n, down),
        Overlay::AgentFiles(v) => crate::agent_files::gui::wheel(v, n, down),
        Overlay::Team(v) => crate::team_board::gui::wheel(v, n, down),
        Overlay::Watchdog(v) => crate::watchdog::view::gui::wheel(v, n, down),
        Overlay::WatchdogPrompt(e) => crate::watchdog::prompt_editor::gui::wheel(e, n, down),
        _ => {}
    }
}

/// What the launcher asked for (the same handling as a key or a terminal-UI click).
pub fn apply_launcher(app: &mut App, o: crate::launcher::Outcome) {
    use crate::launcher::Outcome;
    match o {
        Outcome::None => app.remember_launcher_permission(),
        Outcome::Close => app.overlay = Overlay::None,
        Outcome::Profiles => {
            app.overlay = Overlay::None;
            app.open_view("profiles");
        }
        Outcome::Project(dir) => app.select_launcher_project(dir),
        Outcome::CreateProject(dir, request) => app.create_project_folder(dir, request),
        Outcome::Launch(spec, place) => {
            app.overlay = Overlay::None;
            app.launch_spec(spec, place);
        }
        Outcome::Resume(idx) => {
            app.overlay = Overlay::None;
            app.open_resume(idx);
        }
        Outcome::ResumeMany(idx) => {
            app.overlay = Overlay::None;
            app.resume_many(&idx);
        }
        Outcome::Archive(id, on) => app.continue_archive(id, on),
        Outcome::UndoArchive => app.undo_archive(),
        Outcome::Install(h) => {
            app.overlay = Overlay::None;
            app.install_agent(h);
        }
        Outcome::SignIn => {
            app.overlay = Overlay::None;
            app.sign_in_gemini();
        }
    }
}
