//! Scripted input for checking the app end to end: `BRO_GUI_SCRIPT="wait:800;key:alt+n;wait:400;shot:a.png;exit"`.
//!
//! Steps: `wait:<ms>`, `key:<chord>` (any chord the keymap understands: `alt+n`, `ctrl+space`, `esc`, `enter`),
//! `type:<text>`, `move:<x>,<y>` / `click:<x>,<y>` / `rclick:` / `dclick:` (logical pixels), `wheel:<x>,<y>,<lines>`,
//! `alt:down` / `alt:up` (holding alt alone), `shot:<file.png>`, `exit`.

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
    Move(Point),
    Click(Point, MouseButton, u32),
    Wheel(Point, f32),
    Shot(String),
    /// a file dropped on the window
    Drop(std::path::PathBuf),
    /// alt held down alone (`true`) or let go: the key hints
    Alt(bool),
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
        "move" => Step::Move(point(arg)?),
        "click" => Step::Click(point(arg)?, MouseButton::Left, 1),
        "dclick" => Step::Click(point(arg)?, MouseButton::Left, 2),
        "rclick" => Step::Click(point(arg)?, MouseButton::Right, 1),
        "wheel" => {
            let parts: Vec<&str> = arg.split(',').collect();
            Step::Wheel(Point::new(parts.first()?.trim().parse().ok()?, parts.get(1)?.trim().parse().ok()?), parts.get(2)?.trim().parse().ok()?)
        }
        "shot" => Step::Shot(arg.to_string()),
        "drop" => Step::Drop(std::path::PathBuf::from(arg)),
        "alt" => Step::Alt(arg != "up"),
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
            Step::Drop(path) => super::mouse::dropped(g, path),
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
            Step::Wheel(p, lines) => {
                g.pointer = Some(p);
                super::mouse::wheel(g, p, 0.0, lines);
            }
            Step::Shot(path) => {
                g.frame();
                g.screenshot(&path);
            }
            Step::Alt(down) => g.app_event(Event::Alt(down)),
            Step::Exit => {
                g.app.quit = true;
                g.script.active = false;
                return;
            }
        }
        g.dirty = true;
    }
}
