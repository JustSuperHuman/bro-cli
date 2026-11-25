//! Focus mode's keys and pastes (see [`crate::focus`]): what you type goes to the session's reply box, Enter
//! sends it, Esc stops a working agent, and while the session shows a choice the keys that answer it go to the
//! session itself, so its own menu moves and picks as it does in the terminal. A visible choice takes priority
//! over the reply box and its lists; the draft stays until the choice is answered.

use super::App;
use crate::alerts::Kind;
use crate::focus::{Change, Choice, Command, Draft, Menu, MenuKind, Scroll, Sent, command_query, matching, visible};
use crate::keymap::Act;
use crate::layout::PaneId;
use crate::pane::{Activity, Event};
use crate::panes::term::Term;
use bro_core::Harness;
use bro_core::catalogue::{Level, ModelChoice};
use bro_core::sessions::conversation::Item;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

mod quips;
mod terminal_quips;
mod remote;

/// Agents whose transcript bro follows (Pi and omp, shells, a resume still in line and a failed start keep
/// their terminal).
pub(crate) fn focusable(t: &Term) -> bool {
    matches!(t.meta.harness, Some(Harness::Claude | Harness::Codex)) && t.queued.is_none() && t.spawn_error().is_none()
}

/// A row of the model or thinking list in the reply box's footer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MenuRow {
    /// what picking it switches to (a model's id, a level); empty: nothing (what it runs on now)
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    /// what it runs on now
    pub current: bool,
    /// the model's own starting level
    pub default: bool,
}

/// A thinking level as people say it: "xhigh" → "extra high".
pub(crate) fn level_name(level: &str) -> String {
    match level.to_ascii_lowercase().as_str() {
        "xhigh" => "extra high".into(),
        "none" => "off".into(),
        l => l.to_string(),
    }
}

/// `current` (as the transcript names it: an id, a name, an alias like `opus` for the newest of its kind) is `m`.
pub(crate) fn same_model(current: &str, m: &ModelChoice) -> bool {
    let c = current.trim().to_ascii_lowercase();
    let c = c.strip_suffix("[1m]").unwrap_or(&c);
    let id = m.id.to_ascii_lowercase();
    let alias = !c.is_empty() && !c.chars().any(|ch| ch.is_ascii_digit());
    c == id || c == m.name.to_ascii_lowercase() || (alias && (id.starts_with(&format!("{c}-")) || id.starts_with(&format!("claude-{c}-"))))
}

/// What a session's models are, by what it was launched on.
enum ModelSource {
    Claude,
    /// a Codex login's (its home)
    Codex(PathBuf),
    /// an API provider's or a Gemini profile's (its id in bro's catalogue)
    Provider(String),
}

/// What `t` runs on, for the models it can switch to: its own login (through bro's proxy or not), proxy mode on
/// its own family and the Claude pool give its agent's own models; the other family's logins give theirs (Claude
/// Code on your ChatGPT logins switches between GPT models); an API provider gives its own.
fn model_source(t: &Term, listed: impl Fn(&str) -> bool) -> Option<ModelSource> {
    use bro_core::launch::{LOGINS_CLAUDE, LOGINS_CODEX};
    use bro_core::profiles::{ProfileKind, dir_for, parse_id};
    let harness = t.meta.harness?;
    let provider = match &t.launch_spec {
        Some(s) => s.provider_id.as_deref(),
        // a session the session server kept from before has no launch: its profile says the same when it names a
        // login, the shared logins, the pool or a provider bro lists models for
        None => t.meta.profile.as_deref().filter(|p| p.contains(':') || *p == "pool" || listed(p)),
    }
    .filter(|p| !p.is_empty());
    Some(match (harness, provider) {
        (Harness::Claude, None | Some(LOGINS_CLAUDE | "pool")) => ModelSource::Claude,
        (Harness::Codex, None | Some(LOGINS_CODEX)) => ModelSource::Codex(t.config_home().unwrap_or_else(bro_core::paths::codex_local_dir)),
        (_, Some(LOGINS_CLAUDE | "pool")) => ModelSource::Claude,
        (_, Some(LOGINS_CODEX)) => ModelSource::Codex(bro_core::paths::codex_local_dir()),
        (_, Some(id)) => match parse_id(id) {
            Some((ProfileKind::ClaudeLocal | ProfileKind::ClaudeAccount, _)) => ModelSource::Claude,
            Some((kind @ (ProfileKind::CodexLocal | ProfileKind::CodexProfile), name)) => ModelSource::Codex(dir_for(kind, name)),
            _ => ModelSource::Provider(id.to_string()),
        },
        // Pi and omp keep their terminal
        (Harness::Pi | Harness::Omp | Harness::Antigravity, None) => return None,
    })
}

/// A provider's model to switch to: its name, and its id and context window to tell it from the rest.
fn provider_choice(r: bro_core::catalogue::ModelRow) -> ModelChoice {
    let context = r.context.map(|c| if c >= 1_000_000 { format!("{}M context", c / 1_000_000) } else { format!("{}K context", c / 1000) });
    let id = (r.name != r.id).then(|| r.id.clone());
    let detail = [id, context].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    ModelChoice { name: r.name, detail: (!detail.is_empty()).then_some(detail), levels: vec![], default_level: None, id: r.id }
}

/// Claude prints its selected model in the welcome banner before it writes a transcript.
fn claude_banner_model(rows: &[String]) -> Option<String> {
    static MODEL: OnceLock<regex::Regex> = OnceLock::new();
    let header = rows.iter().position(|row| row.contains("Claude Code v"))?;
    let pattern = MODEL.get_or_init(|| regex::Regex::new(r"\b(Opus|Sonnet|Fable|Haiku)\s+\d+(?:\.\d+)+\b").unwrap());
    rows.iter().skip(header + 1).take(3).find_map(|row| pattern.find(row).map(|m| m.as_str().to_string()))
}

/// Codex takes `/model <model> --reasoning=<level>` from 0.165 on (before that, only its own menu).
fn codex_takes_arguments(version: Option<&str>) -> bool {
    let Some(v) = version else { return false };
    let mut n = v.trim_start_matches('v').split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().unwrap_or(0));
    let (major, minor) = (n.next().unwrap_or(0), n.next().unwrap_or(0));
    (major, minor) >= (0, 165)
}

/// Before its first turn, a transcript can learn its model and level after a choice was queued.
fn change_matches(change: &Change, said: &(Option<String>, Option<String>)) -> bool {
    if change.sent.is_some() { return change.was == *said; }
    (change.was.0.is_none() || change.was.0 == said.0) && (change.was.1.is_none() || change.was.1 == said.1)
}

/// The commands bro's catalogue knows for `agent` (its own; what's in the folders comes later).
fn builtin_commands(agent: &str) -> Vec<Command> {
    bro_bridge::slash_commands::builtin_slash_commands(agent).into_iter().map(|c| Command { name: c.name, hint: c.argument_hint, detail: c.description }).collect()
}

/// A key for the box's text. A picture's token takes no room, so a key that only stepped over or deleted tokens
/// goes on to the next character: every key does something you can see. A deleted token goes back at the end.
fn edit_box(d: &mut Draft, k: KeyEvent) -> bool {
    let cols = d.cols.max(8);
    let seen = |d: &Draft| {
        let t = d.area.text();
        (visible(t), visible(&t[..d.area.cursor()]).chars().count())
    };
    let before = seen(d);
    let mut used = false;
    for _ in 0..=d.attachments.len() {
        let raw = (d.area.text().to_string(), d.area.cursor());
        if !d.area.key(k, cols) {
            break;
        }
        used = true;
        if seen(d) != before || (d.area.text(), d.area.cursor()) == (raw.0.as_str(), raw.1) || d.attachments.is_empty() {
            break;
        }
    }
    if used {
        d.keep_tokens();
    }
    used
}

impl App {
    /// `id` shows its conversation instead of its terminal.
    pub(crate) fn focus_view(&self, id: PaneId) -> bool {
        self.focus.on && !self.focus.peek.contains(&id) && self.panes.get(&id).is_some_and(|p| {
            p.as_term_ref().is_some_and(|t| focusable(t)
                && (p.activity() != Some(Activity::Blocked) || t.limit_warning().is_some() || t.browser_waiting().is_some() || t.focus_choice().is_some() || t.focus_queued_questions().is_some()))
        })
    }

    /// Focus mode on / off (the desktop app's; the terminal UI says where it is). A session showing its terminal
    /// while focus mode is on goes back to its conversation first.
    pub(crate) fn toggle_focus_mode(&mut self) {
        if !crate::panes::mirror::active() {
            self.toast(Kind::Info, "focus mode is in the desktop app: bro gui");
            return;
        }
        if self.focus.on
            && let Some(id) = self.focused()
            && self.focus.peek.remove(&id)
        {
            return;
        }
        self.focus.on = !self.focus.on;
        self.focus.peek.clear();
        let key = self.keymap.primary(Act::Focus);
        let back = if key.is_empty() { String::new() } else { format!(" · {key} for the terminals") };
        self.toast(Kind::Info, if self.focus.on { format!("focus mode{back}") } else { "focus mode off".to_string() });
    }

    /// The choice `id` waits on, as its screen shows it (None while it isn't waiting on you, or while its output
    /// is being read: the next frame looks again).
    pub(crate) fn focus_choice(&self, id: PaneId) -> Option<Choice> {
        let p = self.panes.get(&id)?;
        if p.activity() != Some(Activity::Blocked) {
            return None;
        }
        p.as_term_ref()?.focus_choice()
    }

    pub(crate) fn focus_answer(&mut self, id: PaneId, shown: &Choice, key: &str) -> bool {
        self.panes.get_mut(&id).and_then(|p| p.as_term()).is_some_and(|t| t.answer_prompt_option(key, Some(shown)))
    }

    pub(crate) fn focus_open_queued_questions(&mut self, id: PaneId) -> bool {
        self.panes.get_mut(&id).and_then(|p| p.as_term()).is_some_and(Term::open_queued_questions)
    }

    /// What you've said in `id`, oldest first: the conversation's messages, then those on their way.
    fn your_messages(&self, id: PaneId) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        if let Some(conv) = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation) {
            for it in &conv.items {
                if let Item::You { text, .. } = it.as_ref() {
                    out.push(crate::quips::collapse(text));
                }
            }
        }
        if let Some(d) = self.focus.drafts.get(&id) {
            out.extend(d.sent.iter().map(|s| crate::quips::collapse(&s.text)));
        }
        out.dedup();
        out
    }

    /// A key for `id`'s conversation.
    pub(crate) fn focus_key(&mut self, id: PaneId, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let dictating = self.dictation.phase(id).is_some();
        if dictating && k.code == KeyCode::Esc && !self.overlay.is_open() {
            self.dictation.cancel(id);
            return;
        }
        if self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(Term::focus_queued_questions).is_some_and(|q| {
            use bro_core::terminal_prompt::QuestionShortcut;
            matches!((q.shortcut, k.code, k.modifiers),
                (QuestionShortcut::ShiftLeft, KeyCode::Left, KeyModifiers::SHIFT)
                | (QuestionShortcut::AltUp, KeyCode::Up, KeyModifiers::ALT))
        }) {
            self.focus_open_queued_questions(id);
            return;
        }
        // A visible choice owns its answer keys, even with a saved draft or a list over the reply box.
        // Numbers use the same checked, single-commit path as clicks: not every agent accepts digit shortcuts.
        if !self.overlay.is_open() && let Some(choice) = self.focus_choice(id) {
            if k.modifiers.is_empty() {
                match k.code {
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        self.focus_answer(id, &choice, &c.to_string());
                        return;
                    }
                    KeyCode::Enter => {
                        if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                            t.confirm_prompt(&choice);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            let answers = matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' '))
                || matches!(k.code, KeyCode::Char(c) if c.is_ascii_digit() && !ctrl && !alt);
            if answers {
                self.with_pane(id, |p, cx| p.key(k, cx));
                return;
            }
        }
        if dictating {
            if k.code == KeyCode::Enter && !shift && !alt && self.focus_choice(id).is_none() {
                self.dictation.finish(id, true);
            }
            return;
        }
        // With no answer key to handle, the box's lists move and pick before its text does.
        if self.focus_quips_key(id, k) || self.focus_menu_key(id, k) || self.focus_commands_key(id, k) {
            return;
        }
        let blank = self.focus.drafts.get(&id).is_none_or(|d| d.blank());
        // a selection in the box: esc lets go of it (the words stay)
        if k.code == KeyCode::Esc
            && let Some(d) = self.focus.drafts.get_mut(&id)
            && d.area.selection().is_some()
        {
            d.area.clear_selection();
            return;
        }
        // a subagent showing: esc goes back to the conversation (it never stops the agent from there)
        if k.code == KeyCode::Esc && blank && self.focus.subagent.remove(&id).is_some() {
            return;
        }
        match k.code {
            // everything you've written, selected (as any text field does)
            KeyCode::Char('a' | 'A') if ctrl && !alt => {
                self.focus.draft(id).area.select_all();
                return;
            }
            KeyCode::Enter if !shift && !alt => {
                // never into a choice: the paste would be dropped and the Enter after it would pick for you
                if !blank && self.focus_choice(id).is_some() {
                    self.toast(Kind::Info, "pick an answer first (esc cancels it), then send");
                    return;
                }
                return self.focus_submit(id);
            }
            // the box first, then the agent: a working agent stops, a question is cancelled (an idle one keeps
            // its own uses of esc, menus you can't see here). The pictures stay: their popover takes them off.
            KeyCode::Esc => {
                if blank {
                    if matches!(self.panes.get(&id).and_then(|p| p.activity()), Some(Activity::Working | Activity::Blocked)) {
                        self.with_pane(id, |p, cx| p.key(k, cx));
                    }
                } else {
                    self.focus_clear(id);
                }
                return;
            }
            // Claude's modes (accept edits, plan) and Codex's: as in the terminal
            KeyCode::BackTab => {
                self.with_pane(id, |p, cx| p.key(k, cx));
                return;
            }
            KeyCode::Char('c' | 'C') if ctrl => {
                self.focus_clear(id);
                return;
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                self.focus.draft(id).scroll = Some(if k.code == KeyCode::PageUp { Scroll::PageUp } else { Scroll::PageDown });
                return;
            }
            KeyCode::Home | KeyCode::End if ctrl && blank => {
                self.focus.draft(id).scroll = Some(if k.code == KeyCode::Home { Scroll::Top } else { Scroll::Bottom });
                return;
            }
            // alt chords bro doesn't bind type nothing here (in a terminal they'd reach the program)
            KeyCode::Char(_) if alt => return,
            _ => {}
        }
        let history = if matches!(k.code, KeyCode::Up | KeyCode::Down) { self.your_messages(id) } else { vec![] };
        let opened = matches!(k.code, KeyCode::Char('/')) && blank;
        let d = self.focus.draft(id);
        if edit_box(d, k) {
            if !matches!(k.code, KeyCode::Up | KeyCode::Down) {
                d.recall = None;
            }
            d.command = 0;
            // the commands list is about to open: read the folders' own commands while you type
            if opened {
                let _ = self.focus_commands(id);
            }
            return;
        }
        // up from the top of an empty box (or one showing an earlier message): what you said before, as a shell
        // walks its history; down walks forward again
        let back = match (k.code, d.recall) {
            (KeyCode::Up, None) if d.blank() => Some(1),
            (KeyCode::Up, Some(n)) => Some((n + 1).min(history.len())),
            (KeyCode::Down, Some(n)) => n.checked_sub(1).filter(|n| *n > 0),
            _ => return,
        };
        match back.filter(|n| *n > 0 && !history.is_empty()) {
            Some(n) => {
                d.area.set_text(&history[history.len() - n]);
                d.recall = Some(n);
            }
            None => {
                d.area.set_text("");
                d.recall = None;
            }
        }
        d.keep_tokens();
    }

    /// Empty `id`'s box of what's written (the pictures stay).
    fn focus_clear(&mut self, id: PaneId) {
        let d = self.focus.draft(id);
        d.area.set_text("");
        d.keep_tokens();
        d.recall = None;
        d.dismissed = None;
    }

    /// A paste into `id`'s reply box: pictures' paths alone (a copied screenshot's file) go with the message as
    /// thumbnails, anything else into the text.
    pub(crate) fn focus_paste(&mut self, id: PaneId, text: &str) {
        if self.dictation.phase(id).is_some() {
            if let Some(paths) = crate::focus::only_pictures(text) {
                self.focus_attach(id, paths);
            }
            return;
        }
        let d = self.focus.draft(id);
        if let Some(e) = &mut d.quips.editor {
            e.fields[e.field].insert(&if e.field < 2 { text.replace(['\n', '\r'], " ") } else { text.to_string() });
            return;
        }
        if d.quips.open {
            d.quips.search.push_str(&text.replace(['\n', '\r'], " "));
            d.quips.selected = 0;
            return;
        }
        match crate::focus::only_pictures(text) {
            Some(paths) => paths.into_iter().for_each(|p| d.attach(p)),
            None => {
                d.area.insert(text);
                d.keep_tokens();
            }
        }
        d.recall = None;
    }

    /// Files for `id`'s reply box (a pasted image, a drop): pictures go with the message as thumbnails, held where
    /// the caret is; other files go into the text as paths.
    pub(crate) fn focus_attach(&mut self, id: PaneId, paths: Vec<PathBuf>) {
        let (pictures, others): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| crate::focus::is_picture(p));
        let d = self.focus.draft(id);
        if self.dictation.phase(id).is_some() && let Some(selection) = d.area.selection() {
            d.area.select(selection.end, selection.end);
        }
        for p in pictures {
            d.attach(p);
        }
        if !others.is_empty() {
            let text = crate::clip::paste_form(&others.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>());
            d.area.insert(&format!("{text} "));
            d.keep_tokens();
            d.recall = None;
        }
    }

    /// Send what's in `id`'s reply box: a slash command (or a `!` shell command) key by key so the agent's own
    /// menu takes it, a message with pictures piece by piece (each picture where it was), anything else as one
    /// paste; then Enter. It shows at once, as sent, until the transcript has it.
    pub(crate) fn focus_submit(&mut self, id: PaneId) {
        self.focus_follow_session(id);
        let after = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).map_or(0, |c| c.items.len());
        let d = self.focus.draft(id);
        let text = visible(d.area.text()).trim().to_string();
        if text.is_empty() && d.attachments.is_empty() {
            return;
        }
        let raw = d.area.text().to_string();
        // Expand before clearing anything: a conflict leaves the draft and all its pictures intact.
        let expanded = match self.focus.quips.expand(&raw) {
            Ok(text) => text,
            Err(e) => { self.toast(Kind::Info, e); return; }
        };
        let text = visible(&expanded).trim().to_string();
        // what you send goes to the session: back to its conversation, where it shows
        self.focus.subagent.remove(&id);
        let d = self.focus.draft(id);
        let parts = d.parts_with_text(&expanded);
        let pictures: Vec<PathBuf> = d.attachments.drain(..).map(|a| a.path).collect();
        d.area.set_text("");
        d.recall = None;
        d.dismissed = None;
        d.menu = None;
        d.quips = crate::quips::Picker::default();
        d.scroll = Some(Scroll::Bottom);
        let queued = self.panes.get(&id).is_some_and(|p| p.busy());
        let command = pictures.is_empty() && !text.contains('\n') && (text.starts_with('/') || text.starts_with('!'));
        let screen = command && crate::focus::opens_screen(&text);
        let reset = command && text == "/clear" && self.panes.get(&id).and_then(|p| p.as_term_ref()).is_some_and(|t| t.meta.harness == Some(Harness::Claude));
        if reset {
            let d = self.focus.draft(id);
            d.sent.clear();
            d.reset_at = Some(Instant::now());
        }
        let delivery = if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
            if command {
                t.type_command(&text)
            } else if pictures.is_empty() {
                t.send_message(&text)
            } else {
                t.send_parts(&parts)
            }
        } else { return };
        // A command that opens its own screen isn't a conversation message.
        if !screen && !reset {
            self.focus.draft(id).sent.push(Sent { text: text.clone(), delivery, at: Instant::now(), after, queued, pictures: pictures.clone() });
        }
        if screen {
            self.focus.peek.insert(id);
            let key = self.keymap.primary(Act::Focus);
            let back = if key.is_empty() { String::new() } else { format!(" · {key} for the conversation") };
            self.toast(Kind::Info, format!("{} opens a screen of its own: here's the terminal{back}", text.split_whitespace().next().unwrap_or(&text)));
        }
    }

    /// Send `text` to `id`'s agent as a message from you (an MCP app's page asked to), leaving the box as it is.
    pub(crate) fn focus_send_text(&mut self, id: PaneId, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.focus_follow_session(id);
        let after = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).map_or(0, |c| c.items.len());
        let queued = self.panes.get(&id).is_some_and(|p| p.busy());
        let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) else { return };
        let delivery = t.send_message(text);
        let d = self.focus.draft(id);
        d.sent.push(Sent { text: text.to_string(), delivery, at: Instant::now(), after, queued, pictures: vec![] });
        d.scroll = Some(Scroll::Bottom);
    }

    // ------------------------------------------------------------------------------------- the commands list

    /// The commands `id`'s agent takes: its own, then (read off this thread, a moment after first asked) the
    /// project's and yours, and for Claude the skills it can run.
    fn focus_commands(&mut self, id: PaneId) -> Arc<Vec<Command>> {
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return Arc::default() };
        let agent = match t.meta.harness {
            Some(Harness::Claude) => "claude",
            Some(Harness::Codex) => "codex",
            _ => return Arc::default(),
        };
        let cwd = t.meta.cwd.clone();
        let root = t.meta.project.root.clone();
        let key = format!("{agent} {}", cwd.display());
        let cached = self.focus.lists.lock().commands.get(&key).cloned();
        let fresh = cached.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(20));
        if !fresh && !cfg!(test) && self.focus.lists.lock().asking.insert(key.clone()) {
            let (lists, tx) = (self.focus.lists.clone(), self.tx.clone());
            let folder = cwd.to_string_lossy().into_owned();
            let agent = agent.to_string();
            std::thread::spawn(move || {
                let found: Vec<Command> = bro_bridge::slash_commands::list_slash_commands(&agent, &folder).into_iter().map(|c| Command { name: c.name, hint: c.argument_hint, detail: c.description }).collect();
                let mut l = lists.lock();
                l.commands.insert(key.clone(), (Instant::now(), Arc::new(found)));
                l.asking.remove(&key);
                drop(l);
                let _ = tx.send(Event::Services);
            });
        }
        let mut all: Vec<Command> = match cached {
            Some((_, list)) => list.as_ref().clone(),
            None => builtin_commands(agent),
        };
        // Claude runs a skill as a command of its name
        if agent == "claude" {
            let inventory = self.svc.state().skills.inventory.clone();
            match inventory {
                Some(inv) => {
                    for s in inv.everywhere().chain(inv.of_project(&root)) {
                        if s.agents().contains(&bro_core::skill_inventory::Agent::Claude) && !all.iter().any(|c| c.name == s.name) {
                            all.push(Command { name: s.name.clone(), hint: None, detail: s.summary() });
                        }
                    }
                }
                None if !cfg!(test) && !self.svc.state().skills.scanning => self.svc.scan_skills(),
                None => {}
            }
        }
        Arc::new(all)
    }

    /// The commands list for `id`'s box as it stands, best match first (empty while it's closed): open while
    /// the box holds the start of a command.
    pub(crate) fn focus_command_rows(&mut self, id: PaneId) -> Vec<Command> {
        let Some(d) = self.focus.drafts.get(&id) else { return vec![] };
        if d.menu.is_some() || d.dismissed.as_deref() == Some(d.area.text()) {
            return vec![];
        }
        let text = visible(d.area.text());
        let Some(query) = command_query(&text).map(str::to_string) else { return vec![] };
        let all = self.focus_commands(id);
        matching(&all, &query)
    }

    /// The commands list's keys: up and down move, tab completes, enter runs (or completes one that needs more
    /// after it), esc closes it. True when the key was its.
    fn focus_commands_key(&mut self, id: PaneId, k: KeyEvent) -> bool {
        if !matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::Enter | KeyCode::Esc) || k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT) {
            return false;
        }
        let rows = self.focus_command_rows(id);
        if rows.is_empty() {
            return false;
        }
        let n = rows.len();
        let d = self.focus.draft(id);
        let sel = d.command.min(n - 1);
        match k.code {
            KeyCode::Up => d.command = (sel + n - 1) % n,
            KeyCode::Down => d.command = (sel + 1) % n,
            KeyCode::Esc => d.dismissed = Some(d.area.text().to_string()),
            KeyCode::Tab => self.focus_complete(id, &rows[sel], false),
            _ => self.focus_complete(id, &rows[sel], true),
        }
        true
    }

    /// Put command `c` in `id`'s box (with a space after when it takes more); `run`: send it, unless it needs
    /// something after it first.
    pub(crate) fn focus_complete(&mut self, id: PaneId, c: &Command, run: bool) {
        let send = run && !c.needs_more();
        let d = self.focus.draft(id);
        d.area.set_text(&if c.hint.is_some() && !send { format!("/{} ", c.name) } else { format!("/{}", c.name) });
        d.keep_tokens();
        d.command = 0;
        d.dismissed = None;
        d.recall = None;
        if send {
            self.focus_submit(id);
        }
    }

    // ------------------------------------------------------------------------------- the model and thinking

    /// What `id`'s transcript says it runs on: the model (the one it was started with until then) and how hard it
    /// thinks.
    fn focus_said(&self, id: PaneId) -> (Option<String>, Option<String>) {
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return (None, None) };
        let conv = t.insights().and_then(|s| s.conversation);
        let known = self.focus.drafts.get(&id).filter(|d| d.owner.as_deref() == Some(t.meta.sid.as_str())).map(|d| d.known.clone()).unwrap_or_default();
        let model = conv.as_ref().and_then(|c| c.model.clone())
            .or_else(|| {
                if t.meta.harness != Some(Harness::Claude) { return None; }
                claude_banner_model(&t.try_live_screen_rows()?)
            })
            .or(known.0).or_else(|| t.meta.model.clone());
        (model, conv.and_then(|c| c.effort.clone()).or(known.1))
    }

    /// Keep pending messages tied to their conversation, even while the terminal is showing.
    fn focus_follow_session(&mut self, id: PaneId) {
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return };
        let owner = t.meta.sid.clone();
        let session = t.insights().and_then(|s| s.session_id);
        let known = self.focus_said(id);
        let d = self.focus.draft(id);
        if d.owner.as_deref().is_some_and(|old| old != owner) {
            d.sent.clear();
            d.session = None;
            d.reset_at = None;
            d.change = None;
        }
        d.owner = Some(owner);
        d.follow_session(session.as_deref());
        d.known = known;
    }

    fn focus_items(&self, id: PaneId) -> usize {
        self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).map_or(0, |c| c.items.len())
    }

    /// The model and thinking level `id` runs on, as focus mode shows them: what its transcript says, or a change
    /// you picked until the transcript says something new (or the conversation moved on without it taking).
    pub(crate) fn focus_running_on(&self, id: PaneId) -> (Option<String>, Option<String>) {
        let said = self.focus_said(id);
        let items = self.focus_items(id);
        match self.focus.drafts.get(&id).and_then(|d| d.change.as_ref()) {
            Some(c) if change_matches(c, &said) && c.sent.is_none_or(|n| items <= n + 1) => (c.model.clone().or(said.0), c.effort.clone().or(said.1)),
            _ => said,
        }
    }

    /// The change picked for `id` hasn't gone in yet (Codex takes it when its turn ends).
    pub(crate) fn focus_change_waits(&self, id: PaneId) -> bool {
        self.focus.drafts.get(&id).and_then(|d| d.change.as_ref()).is_some_and(|c| c.sent.is_none())
    }

    /// The models `id` can switch to, by what it runs on (see [`model_source`]): Claude Code's; a Codex login's own
    /// (read off this thread the first time they're wanted); or an API provider's, from bro's catalogue. Each
    /// switches with the agent's own `/model`: bro's proxy sends a model of the family it talks to as asked. None
    /// until they're read, and for a provider bro has no list of.
    pub(crate) fn focus_models(&mut self, id: PaneId) -> Option<Arc<Vec<ModelChoice>>> {
        let t = self.panes.get(&id)?.as_term_ref()?;
        let svc = &self.svc;
        match model_source(t, |p| svc.state().models.contains_key(p))? {
            ModelSource::Claude => Some(self.claude_models()),
            ModelSource::Codex(home) => {
                // the demo and tests touch no login: Codex's usual models
                let demo = self.svc.is_demo() || cfg!(test);
                let home = if demo { PathBuf::new() } else { home };
                if let Some(m) = self.focus.lists.lock().models.get(&home) {
                    return Some(m.clone());
                }
                if demo {
                    let m = Arc::new(bro_core::catalogue::codex_default_choices());
                    self.focus.lists.lock().models.insert(home, m.clone());
                    return Some(m);
                }
                let key = format!("models {}", home.display());
                if self.focus.lists.lock().asking.insert(key.clone()) {
                    let (lists, tx) = (self.focus.lists.clone(), self.tx.clone());
                    std::thread::spawn(move || {
                        let m = Arc::new(bro_core::catalogue::codex_choices(&home));
                        let mut l = lists.lock();
                        l.models.insert(home, m);
                        l.asking.remove(&key);
                        drop(l);
                        let _ = tx.send(Event::Services);
                    });
                }
                Some(Arc::new(bro_core::catalogue::codex_default_choices()))
            }
            ModelSource::Provider(provider) => {
                // the catalogue's list, made once per version of it (a provider can list hundreds)
                let listed = self.svc.state().models.get(&provider).map_or(0, Vec::len);
                if listed == 0 {
                    return None;
                }
                if let Some((n, m)) = self.focus.lists.lock().providers.get(&provider)
                    && *n == listed
                {
                    return Some(m.clone());
                }
                let rows = self.svc.state().models.get(&provider).cloned().unwrap_or_default();
                let m = Arc::new(rows.into_iter().map(provider_choice).collect::<Vec<_>>());
                self.focus.lists.lock().providers.insert(provider, (listed, m.clone()));
                Some(m)
            }
        }
    }

    /// The models Claude Code switches between (`/model <id>`): the newest of each family in what Anthropic last
    /// listed (the services keep it as "claude" and refresh it every few hours), else bro's own list. Made again
    /// only when that list changes.
    fn claude_models(&self) -> Arc<Vec<ModelChoice>> {
        let st = self.svc.state();
        let built_in;
        let rows = match st.models.get("claude").filter(|r| !r.is_empty()) {
            Some(r) => r.as_slice(),
            None => {
                built_in = bro_core::catalogue::claude_built_in();
                built_in.as_slice()
            }
        };
        let mut lists = self.focus.lists.lock();
        if let Some((seen, m)) = &lists.claude
            && seen.as_slice() == rows
        {
            return m.clone();
        }
        let m = Arc::new(bro_core::catalogue::claude_choices_from(rows));
        lists.claude = Some((rows.to_vec(), m.clone()));
        m
    }

    /// The thinking levels `id` can pick from: its model's, else its agent's usual ones (Claude Code sends its
    /// effort whatever the model, and bro's proxy carries it on; Codex on another family's model gets the levels
    /// every model takes). With the model's own starting level.
    pub(crate) fn focus_levels(&mut self, id: PaneId) -> Option<(Vec<Level>, Option<String>)> {
        let harness = self.panes.get(&id)?.as_term_ref()?.meta.harness?;
        let models = self.focus_models(id);
        let (model, _) = self.focus_running_on(id);
        if let Some(m) = models.as_ref().and_then(|ms| model.as_deref().and_then(|cur| ms.iter().find(|m| same_model(cur, m))))
            && !m.levels.is_empty()
        {
            return Some((m.levels.clone(), m.default_level.clone()));
        }
        match harness {
            Harness::Claude => self.claude_models().first().map(|m| (m.levels.clone(), None)),
            Harness::Codex if model.is_none() => bro_core::catalogue::codex_default_choices().first().map(|m| (m.levels.clone(), m.default_level.clone())),
            _ => model.is_some().then(|| (["low", "medium", "high"].map(|l| Level { id: l.into(), detail: None }).to_vec(), None)),
        }
    }

    /// The rows of `id`'s model or thinking list, narrowed to what's typed in it while it's open. The model list
    /// names what it runs on now first when that isn't one of its models.
    pub(crate) fn focus_menu_rows(&mut self, id: PaneId, kind: MenuKind) -> Vec<MenuRow> {
        let (model, effort) = self.focus_running_on(id);
        let query = self.focus.drafts.get(&id).and_then(|d| d.menu.as_ref()).filter(|m| m.kind == kind).map(|m| m.query.clone()).unwrap_or_default();
        let mut rows: Vec<MenuRow> = match kind {
            MenuKind::Model => {
                let mut rows: Vec<MenuRow> = self
                    .focus_models(id)
                    .iter()
                    .flat_map(|ms| ms.iter())
                    .map(|m| MenuRow { id: m.id.clone(), label: m.name.clone(), detail: m.detail.clone(), current: model.as_deref().is_some_and(|cur| same_model(cur, m)), default: false })
                    .collect();
                if !rows.iter().any(|r| r.current)
                    && let Some(cur) = model.as_deref()
                {
                    rows.insert(0, MenuRow { id: String::new(), label: crate::focus::model_name(cur), detail: Some("What it runs on now".into()), current: true, default: false });
                }
                rows
            }
            MenuKind::Thinking => self
                .focus_levels(id)
                .map(|(levels, default)| {
                    levels
                        .iter()
                        .map(|l| {
                            let name = level_name(&l.id);
                            let mut chars = name.chars();
                            MenuRow {
                                id: l.id.clone(),
                                label: chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default(),
                                detail: l.detail.clone(),
                                current: effort.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(&l.id)),
                                default: default.as_deref() == Some(l.id.as_str()),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        if !query.trim().is_empty() {
            let keep = crate::fuzzy::filter(&query, &rows, |r| format!("{} {}", r.label, r.id));
            rows = keep.into_iter().map(|i| rows[i].clone()).collect();
        }
        rows
    }

    /// Open (or close) `id`'s model or thinking list, the keys on what it runs on now.
    pub(crate) fn focus_toggle_menu(&mut self, id: PaneId, kind: MenuKind) {
        if self.focus.drafts.get(&id).and_then(|d| d.menu.as_ref()).is_some_and(|m| m.kind == kind) {
            self.focus.draft(id).menu = None;
            return;
        }
        // a fresh list, unfiltered
        self.focus.draft(id).menu = None;
        let rows = self.focus_menu_rows(id, kind);
        if rows.is_empty() {
            return;
        }
        let sel = rows.iter().position(|r| r.current).unwrap_or(0);
        self.focus.draft(id).menu = Some(Menu { kind, sel, query: String::new() });
    }

    /// The model and thinking lists' keys: up and down move, enter picks, typing searches (backspace and esc
    /// take it back), esc closes. Any other key closes the list and does what it does. True when the key was the
    /// list's.
    fn focus_menu_key(&mut self, id: PaneId, k: KeyEvent) -> bool {
        let Some(mut m) = self.focus.drafts.get(&id).and_then(|d| d.menu.clone()) else { return false };
        let n = self.focus_menu_rows(id, m.kind).len().max(1);
        let plain = !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let open = match k.code {
            KeyCode::Up => {
                m.sel = (m.sel.min(n - 1) + n - 1) % n;
                true
            }
            KeyCode::Down | KeyCode::Tab => {
                m.sel = (m.sel.min(n - 1) + 1) % n;
                true
            }
            KeyCode::Enter => {
                self.focus_pick(id, m.kind, m.sel.min(n - 1));
                return true;
            }
            KeyCode::Esc if !m.query.is_empty() => {
                m.query.clear();
                m.sel = 0;
                true
            }
            KeyCode::Esc => false,
            KeyCode::Backspace if !m.query.is_empty() => {
                m.query.pop();
                m.sel = 0;
                true
            }
            KeyCode::Char(c) if plain => {
                m.query.push(c);
                m.sel = 0;
                true
            }
            _ => {
                self.focus.draft(id).menu = None;
                return false;
            }
        };
        self.focus.draft(id).menu = open.then_some(m);
        true
    }

    /// Row `n` of `id`'s model or thinking list was picked: switch to it. Claude Code takes `/model` and `/effort`
    /// as they come; Codex takes `/model <model> --reasoning=<level>` between turns (an older Codex, only its own
    /// menu, which shows in the terminal).
    pub(crate) fn focus_pick(&mut self, id: PaneId, kind: MenuKind, n: usize) {
        let rows = self.focus_menu_rows(id, kind);
        self.focus.draft(id).menu = None;
        let Some(row) = rows.get(n).filter(|r| !r.current && !r.id.is_empty()).cloned() else { return };
        self.focus_pick_row(id, kind, row);
    }

    fn focus_pick_row(&mut self, id: PaneId, kind: MenuKind, row: MenuRow) {
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return };
        let Some(harness) = t.meta.harness else { return };
        let version = t.insights().and_then(|s| s.conversation).and_then(|c| c.version.clone());
        let said = self.focus_said(id);
        let (_, effort) = self.focus_running_on(id);
        // half a change still on its way (a model picked, then its thinking) stays with the new half
        let mut change = match self.focus.drafts.get(&id).and_then(|d| d.change.clone()) {
            Some(c) if change_matches(&c, &said) => Change { sent: None, ..c },
            _ => Change { model: None, effort: None, was: said.clone(), sent: None },
        };
        let command = match kind {
            MenuKind::Model => {
                // a thinking level the new model doesn't have becomes its own starting one
                if harness == Harness::Codex
                    && let Some(m) = self.focus_models(id).and_then(|ms| ms.iter().find(|m| m.id == row.id).cloned())
                    && !m.levels.is_empty()
                    && !effort.as_deref().is_some_and(|e| m.levels.iter().any(|l| l.id.eq_ignore_ascii_case(e)))
                {
                    change.effort = m.default_level.clone();
                }
                change.model = Some(row.id.clone());
                format!("/model {}", row.id)
            }
            MenuKind::Thinking => {
                change.effort = Some(row.id.clone());
                format!("/effort {}", row.id)
            }
        };
        if harness == Harness::Codex && version.is_some() && !codex_takes_arguments(version.as_deref()) {
            if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                t.type_command("/model");
            }
            self.focus.peek.insert(id);
            self.toast(Kind::Info, "this Codex picks its model in its own menu: here's the terminal");
            return;
        }
        let items = self.focus_items(id);
        if harness == Harness::Claude {
            if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                t.type_command(&command);
            }
            change.sent = Some(items);
        }
        self.focus.draft(id).change = Some(change);
        self.focus_settle(id);
    }

    /// A change Codex couldn't take while it worked goes in once it's free; a change the transcript has caught
    /// up with (or that didn't take) goes.
    pub(crate) fn focus_settle(&mut self, id: PaneId) {
        let said = self.focus_said(id);
        let items = self.focus_items(id);
        let busy = self.panes.get(&id).is_none_or(|p| p.busy());
        let Some(d) = self.focus.drafts.get_mut(&id) else { return };
        let Some(c) = d.change.clone() else { return };
        if !change_matches(&c, &said) || c.sent.is_some_and(|n| items > n + 1) {
            d.change = None;
            return;
        }
        if c.sent.is_some() || busy {
            return;
        }
        let (Some(model), effort) = (c.model.clone().or(said.0.clone()), c.effort.clone().or(said.1.clone())) else {
            return;
        };
        let version = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).and_then(|c| c.version.clone());
        if !codex_takes_arguments(version.as_deref()) { return; }
        let command = match effort {
            Some(e) => format!("/model {model} --reasoning={e}"),
            None => format!("/model {model}"),
        };
        d.change = Some(Change { sent: Some(items), was: said, ..c });
        if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
            t.type_command(&command);
        }
    }

    /// Follow conversation resets and settle every session's waiting change, once a loop.
    pub(crate) fn focus_settle_all(&mut self) {
        let ids: Vec<PaneId> = self.focus.drafts.keys().copied().collect();
        for id in ids {
            self.focus_follow_session(id);
            if self.focus.drafts[&id].change.is_some() {
                self.focus_settle(id);
            }
        }
    }
}
