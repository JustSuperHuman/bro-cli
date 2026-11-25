//! Right-click menus for projects, sessions, conversations, and voice. A floating card where you
//! clicked; a click anywhere else or Esc closes it, ↑ ↓ and Enter work it from the keys.

use super::Gui;
use super::actions::Action;
use super::icons::{Change, Icons};
use super::kit::widgets::{self, MenuLine};
use super::kit::{Cursor, Ui};
use crate::app::App;
use crate::layout::PaneId;
use bro_core::project_icon::Icon;
use bro_gfx::{Point, Rect};
use std::path::{Path, PathBuf};

/// Whose menu it is.
#[derive(Clone, Debug)]
pub enum Of {
    Project { key: String, name: String, root: PathBuf, live: usize },
    Session(PaneId),
    Conversation { pane: PaneId, selection: Option<String>, can_fork: bool },
    Voice,
}

/// The open menu.
pub struct Menu {
    pub of: Of,
    /// its top-left (where you clicked)
    pub at: Point,
    /// the line the keys are on
    pub sel: Option<usize>,
    /// what each line drawn last does (None: a heading or a separator)
    cmds: Vec<Option<Cmd>>,
}

/// The target a click anywhere but the menu runs: it closes.
pub const OUTSIDE: usize = usize::MAX;

#[derive(Clone, Debug)]
enum Cmd {
    Icon(Change),
    NewHere,
    Folder,
    Close,
    Fork,
    Rename,
    Reload,
    ClosePane,
    Copy,
    Paste,
    Talk,
    Browser,
    HangUp,
    Settings,
}

impl Menu {
    /// A project's menu at `at`; it looks in the project for pictures to offer.
    pub fn project(key: String, name: String, root: PathBuf, live: usize, at: Point, icons: &mut Icons) -> Menu {
        icons.ask_offers(&key, &root);
        Menu { of: Of::Project { key, name, root, live }, at, sel: None, cmds: vec![] }
    }

    pub fn voice(at: Point) -> Menu {
        Menu { of: Of::Voice, at, sel: None, cmds: vec![] }
    }

    pub fn session(pane: PaneId, at: Point) -> Menu {
        Menu { of: Of::Session(pane), at, sel: None, cmds: vec![] }
    }

    pub fn conversation(pane: PaneId, selection: Option<String>, can_fork: bool, at: Point) -> Menu {
        Menu { of: Of::Conversation { pane, selection, can_fork }, at, sel: None, cmds: vec![] }
    }

    /// ↑ (-1) or ↓ (1) to the next line that does something.
    pub fn step(&mut self, d: isize) {
        let n = self.cmds.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.sel.map_or(if d > 0 { -1 } else { n }, |s| s as isize);
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if self.cmds[i as usize].is_some() {
                self.sel = Some(i as usize);
                return;
            }
        }
    }
}

/// A line: its icon, its words, what it does; or a heading (None: a separator).
enum Line {
    Do(&'static str, String, Cmd, bool),
    Head(Option<String>),
}

fn fork_line() -> Line {
    Line::Do("\u{F062C}", "Fork conversation".into(), Cmd::Fork, false)
}

/// `p` shown from inside `root` ("public/favicon.svg"), cut from the left when long.
fn short(p: &Path, root: &Path) -> String {
    let rel = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.to_string_lossy().to_string());
    let chars: Vec<char> = rel.chars().collect();
    if chars.len() > 34 { format!("…{}", chars[chars.len() - 33..].iter().collect::<String>()) } else { rel }
}

fn lines(menu: &Menu, app: &App, icons: &Icons) -> Vec<Line> {
    match &menu.of {
        Of::Project { key, root, live, .. } => {
            let icon = icons.icon(key);
            let mut v = vec![Line::Do("\u{F087C}", "Choose an icon from a file…".into(), Cmd::Icon(Change::Choose), false)];
            // drawn by an agent (Settings::draw_project_icons): under way, or one to ask for
            match icons.drawing(key) {
                Some("") => v.push(Line::Head(Some("Waiting its turn to be drawn…".into()))),
                Some(by) => v.push(Line::Head(Some(format!("Drawing its icon with {by}…")))),
                None if icons.draws() && matches!(icon, Some(Icon::Made(_))) => v.push(Line::Do("\u{F0674}", "Draw a new icon".into(), Cmd::Icon(Change::Draw), false)),
                None if icons.draws() && matches!(icon, Some(Icon::Letter { asked: false })) => v.push(Line::Do("\u{F0674}", "Draw an icon for it".into(), Cmd::Icon(Change::Draw), false)),
                None => {}
            }
            match icons.offers(key) {
                None => v.push(Line::Head(Some("Looking through the project for icons…".into()))),
                Some(offers) => {
                    let shown = icon.and_then(Icon::picture);
                    for p in offers.iter().filter(|p| Some(p.as_path()) != shown).take(4) {
                        v.push(Line::Do("\u{F0976}", format!("Use {}", short(p, root)), Cmd::Icon(Change::Use(p.clone())), false));
                    }
                }
            }
            if !matches!(icon, Some(Icon::Letter { asked: true })) {
                v.push(Line::Do("\u{F0BEB}", "Use its letter instead".into(), Cmd::Icon(Change::Letter), false));
            }
            if matches!(icon, Some(Icon::Set(_) | Icon::Letter { asked: true })) {
                v.push(Line::Do("\u{F0068}", "Let bro find its icon".into(), Cmd::Icon(Change::Reset), false));
            }
            v.push(Line::Head(None));
            v.push(Line::Do("\u{F0415}", "New session here".into(), Cmd::NewHere, false));
            v.push(Line::Do("\u{F0770}", "Open the folder".into(), Cmd::Folder, false));
            v.push(Line::Head(None));
            let close = if *live > 0 { format!("Close project · stops {live} session{}", if *live == 1 { "" } else { "s" }) } else { "Close project".into() };
            v.push(Line::Do("\u{F0156}", close, Cmd::Close, true));
            v
        }
        Of::Session(pane) => {
            let Some(p) = app.panes.get(pane) else { return vec![] };
            let Some(term) = p.as_term_ref() else {
                return vec![Line::Do("\u{F0156}", "Close view".into(), Cmd::ClosePane, true)];
            };
            let mut v = vec![];
            if app.supports_conversation_fork(*pane) {
                v.push(fork_line());
            }
            v.push(Line::Do("\u{F03EB}", "Rename session…".into(), Cmd::Rename, false));
            if term.meta.harness.is_some() && term.queued.is_none() && !term.reloading {
                v.push(Line::Do("\u{F0450}", "Reload session…".into(), Cmd::Reload, false));
            }
            v.push(Line::Head(None));
            v.push(Line::Do("\u{F0156}", "Close session".into(), Cmd::ClosePane, true));
            v
        }
        Of::Conversation { pane, selection, can_fork } => {
            if !app.panes.contains_key(pane) { return vec![]; }
            let mut v = vec![];
            if selection.is_some() {
                v.push(Line::Do("\u{F018F}", "Copy".into(), Cmd::Copy, false));
            }
            v.push(Line::Do("\u{F0192}", "Paste".into(), Cmd::Paste, false));
            if *can_fork {
                v.push(Line::Head(None));
                v.push(fork_line());
            }
            v
        }
        Of::Voice => {
            let st = app.svc.state();
            let mut v = vec![];
            if !st.bridge.voice_calls.is_empty() {
                let n = st.bridge.voice_calls.len();
                v.push(Line::Do("\u{F03F5}", if n > 1 { format!("End the {n} calls · agents keep working") } else { "End the call · agents keep working".into() }, Cmd::HangUp, true));
            }
            v.push(Line::Do("\u{F036C}", if st.settings.voice.enabled { "Turn off push-to-talk" } else { "Turn on push-to-talk" }.into(), Cmd::Talk, false));
            if st.bridge.enabled {
                v.push(Line::Do("\u{F02CE}", "Talk in your browser".into(), Cmd::Browser, false));
            }
            v.push(Line::Head(None));
            v.push(Line::Do("\u{F0493}", "Voice settings".into(), Cmd::Settings, false));
            v
        }
    }
}

/// Draw the open menu over everything (call last).
pub fn draw(app: &App, ui: &mut Ui, menu: &mut Menu, icons: &Icons) {
    let prev = ui.layer();
    let screen = ui.screen;
    // anywhere else closes it
    ui.hit_with(screen, Action::SideMenu(OUTSIDE), Cursor::Default);
    let lines = lines(menu, app, icons);
    menu.cmds = lines.iter().map(|l| if let Line::Do(_, _, c, _) = l { Some(c.clone()) } else { None }).collect();
    if lines.is_empty() {
        ui.set_layer(prev);
        return;
    }
    let drawn: Vec<MenuLine> = lines
        .iter()
        .map(|l| match l {
            Line::Do(icon, label, _, danger) => MenuLine { icon: Some(icon), label, danger: *danger, ..Default::default() },
            Line::Head(text) => MenuLine { heading: true, label: text.as_deref().unwrap_or(""), ..Default::default() },
        })
        .collect();
    let title = match &menu.of {
        Of::Project { name, .. } => Some(name.as_str()),
        Of::Session(_) | Of::Conversation { .. } => None,
        Of::Voice => Some("Voice"),
    };
    let bounds = Rect::new(screen.x + 6.0, screen.y + 6.0, (screen.w - 12.0).max(1.0), (screen.h - 12.0).max(1.0));
    widgets::menu(ui, menu.at, bounds, title, &drawn, menu.sel, Action::SideMenu);
    ui.set_layer(prev);
}

/// Run line `n` of the open menu (or close it, for [`OUTSIDE`]).
pub fn run(g: &mut Gui, n: usize) {
    let Some(menu) = g.context_menu.take() else { return };
    let Some(cmd) = menu.cmds.get(n).cloned().flatten() else {
        // a heading keeps it open
        if n != OUTSIDE {
            g.context_menu = Some(menu);
        }
        return;
    };
    let app = &mut g.app;
    match (menu.of, cmd) {
        (Of::Project { key, name, root, .. }, Cmd::Icon(change)) => g.icons.change(&key, &root, &name, change),
        (Of::Project { root, .. }, Cmd::NewHere) => {
            app.pick_project(root.clone());
            app.open_launcher(Some(root), crate::pane::Place::Tab);
        }
        (Of::Project { root, .. }, Cmd::Folder) => {
            if let Err(why) = crate::panes::links::open(&root.to_string_lossy()) {
                app.toast(crate::alerts::Kind::Error, why);
            }
        }
        (Of::Project { root, live, .. }, Cmd::Close) => app.close_project(root, live),
        (Of::Session(pane) | Of::Conversation { pane, .. }, Cmd::Fork) => app.fork_conversation(pane),
        (Of::Session(pane), Cmd::Rename) => app.start_rename(pane),
        (Of::Session(pane), Cmd::Reload) => app.open_reload(pane),
        (Of::Session(pane), Cmd::ClosePane) => app.ask_close(pane),
        (Of::Conversation { selection: Some(text), .. }, Cmd::Copy) => super::window::set_clipboard(&text),
        (Of::Conversation { pane, .. }, Cmd::Paste) if app.panes.contains_key(&pane) => {
            app.focus_pane(pane);
            app.leave_sidebar();
            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            g.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        }
        (_, Cmd::Talk) => {
            let mut settings = app.svc.settings();
            settings.voice.enabled = !settings.voice.enabled;
            app.svc.save_settings(settings);
            app.svc.restart_voice();
        }
        (_, Cmd::Browser) => app.svc.open_voice_page(),
        (_, Cmd::HangUp) => app.svc.hangup_live_voice(),
        (_, Cmd::Settings) => app.show_voice_settings(),
        _ => {}
    }
}

/// Keys while a menu is open: ↑ ↓ pick, Enter runs, Esc closes. True when the key was the menu's.
pub fn key(g: &mut Gui, k: crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    let Some(menu) = g.context_menu.as_mut() else { return false };
    if !k.modifiers.is_empty() && !(k.code == KeyCode::BackTab && k.modifiers == KeyModifiers::SHIFT) {
        g.context_menu = None;
        return false;
    }
    match k.code {
        KeyCode::Esc => g.context_menu = None,
        KeyCode::Up | KeyCode::BackTab => menu.step(-1),
        KeyCode::Down | KeyCode::Tab => menu.step(1),
        KeyCode::Enter | KeyCode::Char(' ') => match menu.sel {
            Some(n) => run(g, n),
            None => menu.step(1),
        },
        // anything else closes it and goes on where it was going
        _ => {
            g.context_menu = None;
            return false;
        }
    }
    true
}
