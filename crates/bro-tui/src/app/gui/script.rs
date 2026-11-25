//! Scripted input for checking the app end to end: `BRO_GUI_SCRIPT="wait:800;key:alt+n;wait:400;shot:a.png;exit"`.
//!
//! Steps: `wait:<ms>`, `key:<chord>` (any chord the keymap understands: `alt+n`, `ctrl+space`, `esc`, `enter`),
//! `type:<text>`, `move:<x>,<y>` / `click:<x>,<y>` / `rclick:` / `dclick:` (logical pixels), `press:<x>,<y>` (the left
//! button down and held: `move:` steps after it drag, as to select), `wheel:<x>,<y>,<lines>`,
//! `alt:down` / `alt:up` (holding alt alone), `theme:<name>`, `material:<solid|acrylic|glass>[,<tint %>]`,
//! `tabs:<scroll|stack>` (what the session tabs do when crowded), `preview:<file>` (the file preview, as a view opens it),
//! `slop:<off|leftovers|clear|fresh|old>` (the demo's Slop Collector), `live:<off|ready|nokey|phone|two>` (live voice),
//! `icon:<project>=<picture>` (a demo project's icon: its folder isn't on disk to find one in),
//! `rune:<welcome|boot|queued>` (the demo's Rune scenes: every session closed, the focused one starting, or waiting
//! its turn to resume), `rune:<working|finish|toast>` (the sessions you aren't in set to work, then finished, for
//! the finish celebration; two toasts),
//! `shot:<file.png>`, `exit`.

use super::Gui;
use crate::pane::Event;
use bro_gfx::Point;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use winit::event::MouseButton;

#[derive(Debug, Clone)]
pub enum Step {
    Wait(Duration),
    Key(crossterm::event::KeyEvent),
    Type(String),
    /// text pasted where the keyboard is
    Paste(String),
    Move(Point),
    Click(Point, MouseButton, u32),
    /// the left button pressed and held (the moves after it drag)
    Press(Point),
    Wheel(Point, f32),
    Shot(String),
    /// a file dropped on the window
    Drop(std::path::PathBuf),
    /// the file preview on a file
    Preview(std::path::PathBuf),
    /// alt held down alone (`true`) or let go: the key hints
    Alt(bool),
    /// show a theme (not saved)
    Theme(String),
    /// the window's material and tint (not saved)
    Material(bro_core::config::Material, Option<u8>),
    /// the session tabs scrolling or stacking when crowded (not saved)
    Tabs(bro_core::config::TabOverflow),
    /// Demo-only Slop Collector reports, for settings captures.
    SlopFixture(String),
    /// Demo-only live voice state (bridge on, a call from the phone), for captures.
    LiveFixture(String),
    /// Demo-only queued questions and their native editor, for Focus captures.
    QuestionsFixture(String),
    /// a project's icon (by its name) shown from this picture, not kept
    Icon(String, std::path::PathBuf),
    /// a project opened in the sidebar (no session in it), not kept
    Project(std::path::PathBuf),
    /// the Storyboard of a folder, beside the focused session
    Storyboard(std::path::PathBuf),
    /// Demo-only: the states the Rune scenes show in (the welcome screen, a session starting, a queued resume).
    RuneFixture(String),
    Exit,
}

#[derive(Default)]
pub struct Script {
    steps: VecDeque<Step>,
    /// when the current wait ends
    until: Option<Instant>,
    pub active: bool,
}

impl Script {
    pub fn from_env() -> Script {
        let Ok(s) = std::env::var("BRO_GUI_SCRIPT") else { return Script::default() };
        let steps = s.split(';').filter_map(|raw| parse(raw.trim())).collect::<VecDeque<_>>();
        Script { active: !steps.is_empty(), steps, until: None }
    }

    /// When the script next needs a turn.
    pub fn wake(&self) -> Option<Duration> {
        if !self.active {
            return None;
        }
        Some(self.until.map(|u| u.saturating_duration_since(Instant::now())).unwrap_or(Duration::from_millis(16)))
    }
}

fn point(s: &str) -> Option<Point> {
    let mut it = s.split(',').map(|v| v.trim().parse::<f32>());
    Some(Point::new(it.next()?.ok()?, it.next()?.ok()?))
}

fn parse(raw: &str) -> Option<Step> {
    let (cmd, arg) = raw.split_once(':').unwrap_or((raw, ""));
    Some(match cmd {
        "wait" => Step::Wait(Duration::from_millis(arg.parse().ok()?)),
        "key" => {
            let c = crate::keymap::Chord::parse(arg).ok()?;
            Step::Key(crossterm::event::KeyEvent::new(c.code, c.mods))
        }
        "type" => Step::Type(arg.to_string()),
        "paste" => Step::Paste(arg.to_string()),
        "move" => Step::Move(point(arg)?),
        "click" => Step::Click(point(arg)?, MouseButton::Left, 1),
        "dclick" => Step::Click(point(arg)?, MouseButton::Left, 2),
        "rclick" => Step::Click(point(arg)?, MouseButton::Right, 1),
        "press" => Step::Press(point(arg)?),
        "wheel" => {
            let parts: Vec<&str> = arg.split(',').collect();
            Step::Wheel(Point::new(parts.first()?.trim().parse().ok()?, parts.get(1)?.trim().parse().ok()?), parts.get(2)?.trim().parse().ok()?)
        }
        "shot" => Step::Shot(arg.to_string()),
        "drop" => Step::Drop(std::path::PathBuf::from(arg)),
        "preview" => Step::Preview(std::path::PathBuf::from(arg)),
        "alt" => Step::Alt(arg != "up"),
        "theme" => Step::Theme(arg.trim().to_string()),
        "material" => {
            let (m, tint) = arg.split_once(',').map_or((arg, None), |(m, t)| (m, t.trim().parse().ok()));
            Step::Material(bro_core::config::Material::ALL.into_iter().find(|x| x.id() == m.trim())?, tint)
        }
        "tabs" => Step::Tabs(bro_core::config::TabOverflow::ALL.into_iter().find(|x| x.id() == arg.trim())?),
        "slop" if matches!(arg, "off" | "leftovers" | "clear" | "fresh" | "old") => Step::SlopFixture(arg.into()),
        "live" if matches!(arg, "off" | "ready" | "nokey" | "phone" | "two") => Step::LiveFixture(arg.into()),
        "questions" if matches!(arg, "queued" | "form" | "trust" | "done") => Step::QuestionsFixture(arg.into()),
        "icon" => {
            let (name, picture) = arg.split_once('=')?;
            Step::Icon(name.trim().to_string(), std::path::PathBuf::from(picture.trim()))
        }
        "project" => Step::Project(std::path::PathBuf::from(arg.trim())),
        "storyboard" => Step::Storyboard(std::path::PathBuf::from(arg.trim())),
        "rune" if matches!(arg, "welcome" | "boot" | "queued" | "working" | "finish" | "toast") => Step::RuneFixture(arg.into()),
        "exit" => Step::Exit,
        _ => return None,
    })
}

/// Run the steps that are due.
pub fn step(g: &mut Gui) {
    if !g.script.active {
        return;
    }
    loop {
        if let Some(u) = g.script.until {
            if Instant::now() < u {
                return;
            }
            g.script.until = None;
        }
        let Some(step) = g.script.steps.pop_front() else {
            g.script.active = false;
            return;
        };
        match step {
            Step::Wait(d) => {
                g.script.until = Some(Instant::now() + d);
            }
            Step::Key(k) => g.key(k),
            Step::Type(text) => {
                for c in text.chars() {
                    g.key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
                }
            }
            Step::Paste(text) => super::mouse::type_text(g, &text),
            Step::Drop(path) => super::mouse::dropped(g, path),
            Step::Preview(path) => g.app.preview_file(path),
            Step::Move(p) => {
                g.pointer = Some(p);
                super::mouse::moved(g, p);
            }
            Step::Click(p, b, n) => {
                g.pointer = Some(p);
                for _ in 0..n {
                    super::mouse::pressed(g, p, b);
                    super::mouse::released(g, p, b);
                }
            }
            Step::Press(p) => {
                g.pointer = Some(p);
                g.pressing = true;
                super::mouse::pressed(g, p, MouseButton::Left);
            }
            Step::Wheel(p, lines) => {
                g.pointer = Some(p);
                super::mouse::wheel(g, p, super::scroll::Wheel::Lines(lines));
            }
            Step::Shot(path) => {
                g.frame();
                g.screenshot(&path);
            }
            Step::Alt(down) => g.app_event(Event::Alt(down)),
            Step::Theme(name) => g.app.set_theme(&name, false),
            Step::Material(m, tint) => {
                // the settings the window reads, in memory only (a demo never saves)
                let mut s = g.app.svc.settings();
                s.material = m;
                if let Some(t) = tint {
                    s.material_tint = t;
                }
                g.app.svc.set_settings_unsaved(s);
            }
            Step::Tabs(overflow) => {
                let mut s = g.app.svc.settings();
                s.tab_overflow = overflow;
                g.app.svc.set_settings_unsaved(s);
            }
            Step::SlopFixture(state) => g.app.svc.slop_demo_fixture(&state),
            Step::LiveFixture(state) => g.app.svc.live_voice_demo_fixture(&state),
            Step::QuestionsFixture(state) if g.app.svc.is_demo() => {
                if let Some(id) = g.app.focused()
                    && let Some(mut meta) = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.clone())
                {
                    meta.harness = Some(bro_core::Harness::Codex);
                    meta.profile = Some("codex:local".into());
                    meta.label = "Codex".into();
                    let screen = match state.as_str() {
                        "queued" => include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../bro-core/tests/fixtures/codex-queued-question.txt")),
                        "form" => "Question 1/2\nWhere should files go?\n› Type your answer\nenter to submit answer | esc to interrupt",
                        "trust" => include_str!("../../../tests/fixtures/codex-folder-trust.txt"),
                        _ => "Working again\nesc to interrupt",
                    }.replace('\n', "\r\n");
                    let mut term = crate::panes::term::Term::scene(meta, move |_, _| screen.clone(), g.app.svc.clone());
                    term.demo_activity = Some(if state == "done" { crate::pane::Activity::Working } else { crate::pane::Activity::Blocked });
                    if let Some(transcript) = crate::services::demo::conversation(1) { term.set_demo_transcript(&transcript); }
                    g.app.panes.insert(id, Box::new(term));
                }
            }
            Step::QuestionsFixture(_) => {}
            Step::RuneFixture(state) if g.app.svc.is_demo() => match state.as_str() {
                "welcome" => {
                    let ids: Vec<_> = g.app.panes.keys().copied().collect();
                    for id in ids {
                        g.app.close(id);
                    }
                }
                "working" | "finish" => {
                    let focused = g.app.focused();
                    let activity = (state == "working").then_some(crate::pane::Activity::Working);
                    for (id, p) in g.app.panes.iter_mut() {
                        if Some(*id) != focused
                            && let Some(t) = p.as_term()
                            && t.meta.harness.is_some()
                        {
                            t.demo_activity = activity;
                        }
                    }
                    g.app.update_insights();
                }
                "toast" => {
                    g.app.toasts.push(crate::alerts::Kind::AgentDone, "Claude finished in bro-cli-v2: the launcher keeps its route on reload");
                    g.app.toasts.push(crate::alerts::Kind::Info, "Theme saved");
                }
                _ => {
                    if let Some(id) = g.app.focused()
                        && let Some(mut meta) = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.clone())
                    {
                        meta.started = Instant::now();
                        let term = if state == "boot" {
                            crate::panes::term::Term::scene(meta, |_, _| String::new(), g.app.svc.clone())
                        } else {
                            crate::panes::term::Term::queued(meta, 2, g.app.svc.clone())
                        };
                        g.app.panes.insert(id, Box::new(term));
                    }
                }
            },
            Step::RuneFixture(_) => {}
            Step::Icon(name, picture) => {
                let projects = crate::sidebar::projects(&g.app.live_infos(), &g.app.open_infos());
                if let Some((key, ..)) = projects.into_iter().find(|(_, n, _)| *n == name) {
                    g.icons.inject(&key, &picture);
                }
            }
            Step::Project(root) => {
                g.app.open_projects.add(root);
            }
            Step::Storyboard(project) => {
                let from = g.app.focused().filter(|id| g.app.panes.get(id).is_some_and(|p| p.is_terminal()));
                let id = g.app.open_storyboard(from, project, true);
                g.app.focus_pane(id);
            }
            Step::Exit => {
                g.app.quit = true;
                g.script.active = false;
                return;
            }
        }
        g.dirty = true;
    }
}
