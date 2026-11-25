//! The launcher's Create Slop tab: who is cast and which media tools are on ([`bro_core::studio`]).
//!
//! The cast is a grid of characters (Claudia, then SLOPCANNON's roster), filtered by typing; space or a click
//! casts one. The toolkit beside it lists the media tools with what the machine has for each; a tool that can be
//! used is on until you switch it off. Where the session runs is what the New tab has selected (shift+←/→ changes
//! the agent here). Enter launches with the kit: [`Launcher::spec`] carries it while this tab is open.
//!
//! A tool the machine can't use yet says what gets it going, and bro does it: ffmpeg, Node.js and JustImagine
//! install the way Quick Install does ([`Outcome::InstallTool`]), and a tool that takes a key gets a field to
//! paste it into ([`KeyEntry`]; bro keeps the key, [`Outcome::SaveKey`]). Enter on a tool does that; ctrl+k opens
//! the key field of a tool that works without one (it signs in, and a key saves the sign-in).
//!
//! Nothing here is asked of the network: the roster and the machine's state arrive in [`super::Data::studio`]
//! (`services::studio`). This file is the model and keys; `view.rs` and `studio_gui.rs` draw it.

use super::{Focus, Launcher, Outcome, SessionTab};
use crate::fuzzy;
use bro_core::Harness;
use bro_core::launch::Permission;
use bro_core::studio::{self, Character, Fix, Kit, Ready, ToolStatus};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::path::PathBuf;

/// Which half of the tab has the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StudioFocus {
    #[default]
    Cast,
    Tools,
}

/// A portrait the window has ready to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picture {
    /// the renderer's image
    pub image: u64,
    pub w: u32,
    pub h: u32,
}

/// A key being typed for a tool, in the toolkit's key field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEntry {
    /// the tool's id
    pub tool: &'static str,
    pub text: String,
}

/// The Create Slop tab's state.
#[derive(Default)]
pub struct Studio {
    pub filter: String,
    /// the cursor among the filtered cast
    pub sel: usize,
    /// who is cast, in the order chosen
    pub picked: Vec<Character>,
    /// tools switched off by hand (every other tool that can be used is on)
    pub off: Vec<String>,
    pub focus: StudioFocus,
    /// the cursor in the toolkit
    pub tool_sel: usize,
    /// the key field, while a tool's key is being typed (it has the keyboard)
    pub key: Option<KeyEntry>,
    /// the tool whose key was just handed to bro: its row says so until the toolkit is looked at again
    pub saved: Option<&'static str>,
    /// the cast grid's columns as last drawn (↑/↓ move a row)
    pub cols: usize,
    /// you changed the cast: the project's last kit no longer replaces it
    pub touched: bool,
    /// the project whose last kit is showing
    applied_for: Option<PathBuf>,
    /// the tab was opened: the roster has been asked for
    asked: bool,
    /// the roster should be looked at (`Some(force)`): the app takes this after each key and click
    pub ask: Option<bool>,
    /// The window's: portraits ready to draw, by [`Character::key`].
    pub pictures: HashMap<String, Picture>,
    /// The window's: the portraits the last draw wanted, as (key, address).
    pub wanted: Vec<(String, String)>,
    /// The window's: how far the cast grid is scrolled, and whether the next draw brings the cursor into view.
    pub scroll: f32,
    pub reveal: bool,
}

impl Launcher {
    /// Open on the Create Slop tab.
    pub fn with_studio(mut self) -> Launcher {
        self.enter_studio();
        self
    }

    /// The page takes the window's whole width, over the sidebar: the Create Slop tab, a studio with a cast to
    /// browse and a toolkit to set up.
    pub fn fills_window(&self) -> bool {
        self.tab == SessionTab::Slop
    }

    /// Switch tabs (the Create Slop tab asks for its roster the first time).
    pub fn set_tab(&mut self, tab: SessionTab) {
        match tab {
            SessionTab::Slop => self.enter_studio(),
            other => self.tab = other,
        }
        self.hits.clear();
    }

    fn enter_studio(&mut self) {
        self.tab = SessionTab::Slop;
        // the orchestrator is its own kind of session: back to a plain agent
        if self.orchestrator {
            self.orchestrator = false;
            self.no_project = false;
            self.list_filter.clear();
            self.list_sel = 0;
            self.focus = Focus::List;
            self.reset_models();
        }
        if !self.studio.asked {
            self.studio.asked = true;
            self.studio.ask = Some(false);
        }
        self.studio_sync();
    }

    /// The cast as filtered: indices into the roster. What is typed is looked for as it is spelled (spaces
    /// aside, since space casts): at the start of a name first, then inside one, then at the start of who made it,
    /// then at the start of a word in its tags. Only when nothing is spelled that way do the letters match in
    /// order, as the palette matches.
    pub fn studio_cast(&self) -> Vec<usize> {
        let cast = &self.data.studio.cast;
        let squash = |s: &str| s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let q = squash(&self.studio.filter);
        if q.is_empty() {
            return (0..cast.len()).collect();
        }
        let mut ranked: Vec<(u8, usize)> = cast
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let name = squash(&c.name);
                let rank = if name.starts_with(&q) {
                    0
                } else if c.name.to_lowercase().split_whitespace().any(|w| w.starts_with(&q)) {
                    1
                } else if name.contains(&q) {
                    2
                } else if squash(&c.creator).trim_start_matches('@').starts_with(&q) {
                    3
                } else if c.tags.iter().any(|t| squash(t).starts_with(&q) || t.to_lowercase().split_whitespace().any(|w| w.starts_with(&q))) {
                    4
                } else {
                    return None;
                };
                Some((rank, i))
            })
            .collect();
        if ranked.is_empty() {
            return fuzzy::filter(&self.studio.filter, cast, |c| c.name.clone());
        }
        ranked.sort();
        ranked.into_iter().map(|r| r.1).collect()
    }

    /// The character under the cursor.
    pub fn studio_current(&self) -> Option<&Character> {
        self.studio_cast().get(self.studio.sel).and_then(|&i| self.data.studio.cast.get(i))
    }

    pub fn studio_is_picked(&self, c: &Character) -> bool {
        let key = c.key();
        self.studio.picked.iter().any(|p| p.key() == key)
    }

    /// Cast `c`, or take them out of the cast.
    pub fn studio_toggle(&mut self, c: Character) {
        let key = c.key();
        match self.studio.picked.iter().position(|p| p.key() == key) {
            Some(i) => {
                self.studio.picked.remove(i);
            }
            None => self.studio.picked.push(c),
        }
        self.studio.touched = true;
    }

    /// A tool as the machine has it.
    pub fn studio_tool(&self, id: &str) -> Option<&ToolStatus> {
        self.data.studio.tools.iter().find(|s| s.id == id)
    }

    /// The tool can be used and isn't switched off.
    pub fn studio_tool_on(&self, id: &str) -> bool {
        self.studio_tool(id).is_some_and(|s| s.ready != Ready::No) && !self.studio.off.iter().any(|o| o == id)
    }

    /// Switch a tool that can be used off or back on.
    pub fn studio_toggle_tool(&mut self, id: &str) {
        if !self.studio_tool(id).is_some_and(|s| s.ready != Ready::No) {
            return;
        }
        match self.studio.off.iter().position(|o| o == id) {
            Some(i) => {
                self.studio.off.remove(i);
            }
            None => self.studio.off.push(id.to_string()),
        }
    }

    /// What gets a tool going, when it can't be used: bro installs it, starts it, or takes its key.
    pub fn studio_fix(&self, id: &str) -> Option<Fix> {
        self.studio_tool(id).and_then(|s| s.fix)
    }

    /// The tool takes a key and has none (it may work without: by signing in, or through JustImagine).
    pub fn studio_keyless(&self, id: &str) -> bool {
        self.studio_tool(id).is_some_and(|s| s.keyless) && studio::key_spec(id).is_some()
    }

    /// Open the key field for tool `i` (one that takes a key).
    pub fn studio_open_key(&mut self, i: usize) {
        let Some(tool) = studio::TOOLS.get(i).filter(|t| studio::key_spec(t.id).is_some()) else { return };
        self.studio.focus = StudioFocus::Tools;
        self.studio.tool_sel = i;
        self.studio.key = Some(KeyEntry { tool: tool.id, text: String::new() });
    }

    /// Do what gets tool `i` going: install or start it, or open its key field.
    pub fn studio_act(&mut self, i: usize) -> Outcome {
        let Some(tool) = studio::TOOLS.get(i) else { return Outcome::None };
        self.studio.focus = StudioFocus::Tools;
        self.studio.tool_sel = i;
        match self.studio_fix(tool.id) {
            Some(Fix::Install(what)) => Outcome::InstallTool(what, false),
            Some(Fix::Start(what)) => Outcome::InstallTool(what, true),
            Some(Fix::Key) => {
                self.studio_open_key(i);
                Outcome::None
            }
            None => Outcome::None,
        }
    }

    /// Hand the typed key to bro (an empty field just closes).
    pub fn studio_save_key(&mut self) -> Outcome {
        let Some(entry) = self.studio.key.take() else { return Outcome::None };
        let Some(spec) = studio::key_spec(entry.tool) else { return Outcome::None };
        let key = studio::tidy_key(spec, &entry.text);
        if key.is_empty() {
            return Outcome::None;
        }
        self.studio.saved = Some(entry.tool);
        // a tool that was waiting on its key is wanted: it comes on with it
        self.studio.off.retain(|o| o != entry.tool);
        Outcome::SaveKey(entry.tool, key)
    }

    /// What the session launches with.
    pub fn studio_kit(&self) -> Kit {
        Kit { cast: self.studio.picked.clone(), tools: studio::TOOLS.iter().filter(|t| self.studio_tool_on(t.id)).map(|t| t.id.to_string()).collect() }
    }

    /// The machine has been looked at: there is a toolkit to launch with.
    pub fn studio_ready(&self) -> bool {
        self.data.studio.checked
    }

    /// After the roster or the project changed: keep the cursors in range, show a chosen character's newest
    /// version, and start from the project's last cast until you change it.
    pub(super) fn studio_sync(&mut self) {
        let n = self.studio_cast().len();
        self.studio.sel = self.studio.sel.min(n.saturating_sub(1));
        self.studio.tool_sel = self.studio.tool_sel.min(studio::TOOLS.len() - 1);
        // the toolkit was looked at again with the key in it: the row says what it found
        if self.studio.saved.is_some_and(|id| !self.studio_tool(id).is_some_and(|s| s.keyless)) {
            self.studio.saved = None;
        }
        for p in &mut self.studio.picked {
            let key = p.key();
            if let Some(c) = self.data.studio.cast.iter().find(|c| c.key() == key) {
                *p = c.clone();
            }
        }
        if self.tab != SessionTab::Slop || self.studio.touched || !self.data.studio.checked || self.studio.applied_for.as_ref() == Some(&self.dir) {
            return;
        }
        self.studio.applied_for = Some(self.dir.clone());
        let last = self.data.studio.remembered.iter().find(|(dir, _)| bro_core::util::same_path(dir, &self.dir));
        self.studio.picked = last.map(|(_, kit)| kit.cast.clone()).unwrap_or_default();
    }

    /// Move the cast cursor `dx` cards along and `dy` rows down.
    pub fn studio_move(&mut self, dx: i32, dy: i32) {
        let n = self.studio_cast().len() as i32;
        if n == 0 {
            return;
        }
        let cols = self.studio.cols.max(1) as i32;
        let at = self.studio.sel as i32;
        let mut next = at + dx;
        // a row down from the last full row lands on the last card
        let down = at + dy * cols;
        if dy != 0 {
            next = if down >= n && at / cols < (n - 1) / cols { n - 1 } else { down };
        }
        if (0..n).contains(&next) {
            self.studio.sel = next as usize;
            self.studio.reveal = true;
        }
    }

    /// The agent the session runs on, a step along the agents (the orchestrator isn't one of them here).
    fn studio_agent(&mut self, d: i32) {
        let all = Harness::ALL;
        let i = all.iter().position(|h| *h == self.harness).unwrap_or(0) as i32;
        self.harness = all[(i + d).rem_euclid(all.len() as i32) as usize];
        self.list_filter.clear();
        self.list_sel = 0;
        self.focus = Focus::List;
        self.reset_models();
    }

    /// Start cooking: install the agent first, pick where it runs when that's undecided, else launch with the kit.
    pub(super) fn studio_launch(&mut self) -> Outcome {
        if !self.installed(self.harness) {
            return Outcome::Install(self.harness);
        }
        if self.sign_in_needed() {
            return self.launch();
        }
        if !self.studio_ready() {
            return Outcome::None;
        }
        if self.spec().is_none() {
            // nothing to run on yet (a model to pick, no login): the New tab is where that's chosen
            self.tab = SessionTab::New;
            if self.needs_model() {
                self.focus = Focus::Models;
            }
            self.hits.clear();
            return Outcome::None;
        }
        self.launch()
    }

    /// A key on the Create Slop tab.
    pub(super) fn studio_key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        // the key field has the keyboard while it is open
        if let Some(entry) = &mut self.studio.key {
            match k.code {
                KeyCode::Esc => self.studio.key = None,
                KeyCode::Enter => return self.studio_save_key(),
                KeyCode::Backspace => {
                    entry.text.pop();
                }
                KeyCode::Char('u') if ctrl => entry.text.clear(),
                KeyCode::Char(ch) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => entry.text.push(ch),
                _ => {}
            }
            return Outcome::None;
        }
        let tools = self.studio.focus == StudioFocus::Tools;
        let tool = studio::TOOLS[self.studio.tool_sel.min(studio::TOOLS.len() - 1)].id;
        match k.code {
            KeyCode::Esc => {
                if !self.studio.filter.is_empty() {
                    self.studio.filter.clear();
                    self.studio.sel = 0;
                    self.studio.reveal = true;
                } else if tools {
                    self.studio.focus = StudioFocus::Cast;
                } else {
                    return Outcome::Close;
                }
            }
            // on a tool that isn't ready, enter gets it going; anywhere else it starts the session
            KeyCode::Enter if tools && self.studio_fix(tool).is_some() => return self.studio_act(self.studio.tool_sel),
            KeyCode::Enter => return self.studio_launch(),
            KeyCode::Char('k') if ctrl && tools && studio::key_spec(tool).is_some() => self.studio_open_key(self.studio.tool_sel),
            KeyCode::Tab | KeyCode::BackTab => self.studio.focus = if tools { StudioFocus::Cast } else { StudioFocus::Tools },
            KeyCode::Right if shift => self.studio_agent(1),
            KeyCode::Left if shift => self.studio_agent(-1),
            KeyCode::Char('n' | 'j') if ctrl => self.studio_step(1),
            KeyCode::Char('p' | 'k') if ctrl => self.studio_step(-1),
            KeyCode::Down => self.studio_step(1),
            KeyCode::Up => self.studio_step(-1),
            KeyCode::Right if !tools => self.studio_move(1, 0),
            KeyCode::Left if !tools => self.studio_move(-1, 0),
            KeyCode::PageDown if !tools => self.studio_move(0, 3),
            KeyCode::PageUp if !tools => self.studio_move(0, -3),
            KeyCode::Home if !tools => {
                self.studio.sel = 0;
                self.studio.reveal = true;
            }
            KeyCode::End if !tools => {
                self.studio.sel = self.studio_cast().len().saturating_sub(1);
                self.studio.reveal = true;
            }
            KeyCode::Char(' ') if tools => {
                let id = studio::TOOLS[self.studio.tool_sel.min(studio::TOOLS.len() - 1)].id;
                self.studio_toggle_tool(id);
            }
            KeyCode::Char(' ') => {
                if let Some(c) = self.studio_current().cloned() {
                    self.studio_toggle(c);
                }
            }
            KeyCode::Char('e') if ctrl => {
                self.permission = match self.permission {
                    Permission::Default => Permission::Auto,
                    Permission::Auto => Permission::Skip,
                    Permission::Skip => Permission::Default,
                }
            }
            KeyCode::Char('r') if ctrl => self.studio.ask = Some(true),
            KeyCode::Char('u') if ctrl => self.studio_filter(|f| f.clear()),
            KeyCode::Backspace => self.studio_filter(|f| {
                f.pop();
            }),
            // typing looks for a character, wherever the keyboard was
            KeyCode::Char(ch) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => self.studio_filter(|f| f.push(ch)),
            _ => {}
        }
        Outcome::None
    }

    /// ↑/↓: a row in the cast, a tool in the toolkit.
    fn studio_step(&mut self, d: i32) {
        match self.studio.focus {
            StudioFocus::Cast => self.studio_move(0, d),
            StudioFocus::Tools => self.studio.tool_sel = (self.studio.tool_sel as i32 + d).clamp(0, studio::TOOLS.len() as i32 - 1) as usize,
        }
    }

    /// Change the filter: the cast has the keyboard again and the cursor goes to the best match.
    fn studio_filter(&mut self, f: impl FnOnce(&mut String)) {
        f(&mut self.studio.filter);
        self.studio.focus = StudioFocus::Cast;
        self.studio.sel = 0;
        self.studio.scroll = 0.0;
        self.studio.reveal = true;
    }

    /// Text pasted on the tab looks for a character; into the key field, it is the key.
    pub(super) fn studio_paste(&mut self, s: &str) {
        if let Some(entry) = &mut self.studio.key {
            if let Some(spec) = studio::key_spec(entry.tool) {
                entry.text.push_str(&studio::tidy_key(spec, s));
            }
            return;
        }
        let text: String = s.trim().chars().filter(|c| !c.is_control()).collect();
        self.studio_filter(|f| f.push_str(&text));
    }

    /// "Claudia", "Claudia and Becky", "Claudia, Becky and Astra", "Claudia, Becky and 2 more": who is cast, for
    /// the button.
    pub fn studio_cast_names(&self) -> Option<String> {
        let names: Vec<&str> = self.studio.picked.iter().map(|c| c.name.as_str()).collect();
        match names.as_slice() {
            [] => None,
            [a] => Some((*a).to_string()),
            [a, b] => Some(format!("{a} and {b}")),
            [a, b, c] => Some(format!("{a}, {b} and {c}")),
            [a, b, rest @ ..] => Some(format!("{a}, {b} and {} more", rest.len())),
        }
    }
}
