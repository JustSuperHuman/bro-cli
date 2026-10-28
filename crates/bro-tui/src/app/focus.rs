//! Focus mode's keys and pastes (see [`crate::focus`]): what you type goes to the session's reply box, Enter
//! sends it, Esc stops a working agent, and while the session shows a choice the keys that answer it go to the
//! session itself, so its own menu moves and picks as it does in the terminal. The box's lists (the commands as
//! you type `/`, the models and thinking levels in its footer) take the keys that move and pick in them first.

use super::App;
use crate::alerts::Kind;
use crate::focus::{Change, Choice, Command, Draft, Menu, MenuKind, Scroll, Sent, choice, command_query, matching, visible};
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

/// Agents whose transcript bro follows (Pi and omp, shells, a resume still in line and a failed start keep
/// their terminal).
pub(crate) fn focusable(t: &Term) -> bool {
    matches!(t.meta.harness, Some(Harness::Claude | Harness::Codex)) && t.queued.is_none() && t.spawn_error().is_none()
}

/// A row of the model or thinking list in the reply box's footer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MenuRow {
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

/// The models Claude Code switches between (`/model <id>`).
fn claude_models() -> Arc<Vec<ModelChoice>> {
    static MODELS: OnceLock<Arc<Vec<ModelChoice>>> = OnceLock::new();
    MODELS.get_or_init(|| Arc::new(bro_core::catalogue::claude_choices())).clone()
}

/// Codex takes `/model <model> --reasoning=<level>` from 0.165 on (before that, only its own menu).
fn codex_takes_arguments(version: Option<&str>) -> bool {
    let Some(v) = version else { return false };
    let mut n = v.trim_start_matches('v').split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().unwrap_or(0));
    let (major, minor) = (n.next().unwrap_or(0), n.next().unwrap_or(0));
    (major, minor) >= (0, 165)
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
        self.focus.on && !self.focus.peek.contains(&id) && self.panes.get(&id).and_then(|p| p.as_term_ref()).is_some_and(focusable)
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
        choice(&p.as_term_ref()?.try_screen_rows()?)
    }

    /// What you've said in `id`, oldest first: the conversation's messages, then those on their way.
    fn your_messages(&self, id: PaneId) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        if let Some(conv) = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation) {
            for it in &conv.items {
                if let Item::You { text, .. } = it.as_ref() {
                    out.push(text.clone());
                }
            }
        }
        if let Some(d) = self.focus.drafts.get(&id) {
            out.extend(d.sent.iter().map(|s| s.text.clone()));
        }
        out.dedup();
        out
    }

    /// A key for `id`'s conversation.
    pub(crate) fn focus_key(&mut self, id: PaneId, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        // a list open over the box moves and picks first
        if self.focus_menu_key(id, k) || self.focus_commands_key(id, k) {
            return;
        }
        let blank = self.focus.drafts.get(&id).is_none_or(|d| d.blank());
        // a choice on screen takes the keys that answer it while the box is empty: its own menu moves and picks
        if blank && self.focus_choice(id).is_some() {
            let answers = matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' '))
                || matches!(k.code, KeyCode::Char(c) if c.is_ascii_digit() && !ctrl && !alt);
            if answers {
                self.with_pane(id, |p, cx| p.key(k, cx));
                return;
            }
        }
        match k.code {
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
        let d = self.focus.draft(id);
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
        let after = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).map_or(0, |c| c.items.len());
        let d = self.focus.draft(id);
        let text = visible(d.area.text()).trim().to_string();
        if text.is_empty() && d.attachments.is_empty() {
            return;
        }
        let parts = d.parts();
        let pictures: Vec<PathBuf> = d.attachments.drain(..).map(|a| a.path).collect();
        d.area.set_text("");
        d.recall = None;
        d.dismissed = None;
        d.menu = None;
        d.scroll = Some(Scroll::Bottom);
        let queued = self.panes.get(&id).is_some_and(|p| p.busy());
        let command = pictures.is_empty() && !text.contains('\n') && (text.starts_with('/') || text.starts_with('!'));
        let screen = command && crate::focus::opens_screen(&text);
        // a command that opens one of the agent's own screens isn't a message: it shows the terminal for it
        if !screen {
            self.focus.draft(id).sent.push(Sent { text: text.clone(), at: Instant::now(), after, queued, pictures: pictures.clone() });
        }
        if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
            if command {
                t.type_command(&text);
            } else if pictures.is_empty() {
                t.send_message(&text);
            } else {
                t.send_parts(&parts);
            }
        }
        if screen {
            self.focus.peek.insert(id);
            let key = self.keymap.primary(Act::Focus);
            let back = if key.is_empty() { String::new() } else { format!(" · {key} for the conversation") };
            self.toast(Kind::Info, format!("{} opens a screen of its own: here's the terminal{back}", text.split_whitespace().next().unwrap_or(&text)));
        }
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
        (conv.as_ref().and_then(|c| c.model.clone()).or_else(|| t.meta.model.clone()), conv.and_then(|c| c.effort.clone()))
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
            Some(c) if c.was == said && c.sent.is_none_or(|n| items <= n + 1) => (c.model.clone().or(said.0), c.effort.clone().or(said.1)),
            _ => said,
        }
    }

    /// The change picked for `id` hasn't gone in yet (Codex takes it when its turn ends).
    pub(crate) fn focus_change_waits(&self, id: PaneId) -> bool {
        self.focus.drafts.get(&id).and_then(|d| d.change.as_ref()).is_some_and(|c| c.sent.is_none())
    }

    /// The models `id` can switch to: Claude Code's, or the Codex login's own (read off this thread the first time
    /// they're wanted). None for a session on bro's route (it was started on its model) and until they're read.
    pub(crate) fn focus_models(&mut self, id: PaneId) -> Option<Arc<Vec<ModelChoice>>> {
        let t = self.panes.get(&id)?.as_term_ref()?;
        if t.meta.route_id.is_some() || t.launch_spec.as_ref().is_some_and(|s| s.provider_id.is_some()) {
            return None;
        }
        match t.meta.harness? {
            Harness::Claude => Some(claude_models()),
            Harness::Codex => {
                let demo = self.svc.is_demo() || cfg!(test);
                let home = if demo { PathBuf::new() } else { t.config_home().unwrap_or_else(bro_core::paths::codex_local_dir) };
                if let Some(m) = self.focus.lists.lock().models.get(&home) {
                    return Some(m.clone());
                }
                // the demo and tests touch no login: Codex's usual models
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
                None
            }
            _ => None,
        }
    }

    /// The thinking levels `id` can pick from: its model's, else its agent's usual ones (a session on bro's
    /// route keeps Claude's, which Claude Code sends whatever the model). With the model's own starting level.
    pub(crate) fn focus_levels(&mut self, id: PaneId) -> Option<(Vec<Level>, Option<String>)> {
        let harness = self.panes.get(&id)?.as_term_ref()?.meta.harness?;
        let models = self.focus_models(id);
        let (model, _) = self.focus_running_on(id);
        if let Some(m) = models.as_ref().and_then(|ms| model.as_deref().and_then(|cur| ms.iter().find(|m| same_model(cur, m)))) {
            return (!m.levels.is_empty()).then(|| (m.levels.clone(), m.default_level.clone()));
        }
        match harness {
            Harness::Claude => claude_models().first().map(|m| (m.levels.clone(), None)),
            // a Codex model bro can't name: no levels to offer it
            _ => None,
        }
    }

    /// The model `id` runs on isn't one it can switch to (an older one): the model list shows it first.
    fn focus_runs_other(&mut self, id: PaneId) -> bool {
        let (model, _) = self.focus_running_on(id);
        model.is_some_and(|cur| self.focus_models(id).is_some_and(|ms| !ms.iter().any(|m| same_model(&cur, m))))
    }

    /// The rows of `id`'s model or thinking list.
    pub(crate) fn focus_menu_rows(&mut self, id: PaneId, kind: MenuKind) -> Vec<MenuRow> {
        let (model, effort) = self.focus_running_on(id);
        match kind {
            MenuKind::Model => {
                let mut rows: Vec<MenuRow> = self
                    .focus_models(id)
                    .map(|ms| ms.iter().map(|m| MenuRow { label: m.name.clone(), detail: m.detail.clone(), current: model.as_deref().is_some_and(|cur| same_model(cur, m)), default: false }).collect())
                    .unwrap_or_default();
                if self.focus_runs_other(id)
                    && let Some(cur) = model.as_deref()
                {
                    rows.insert(0, MenuRow { label: crate::focus::model_name(cur), detail: Some("What it runs on now".into()), current: true, default: false });
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
                                label: chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default(),
                                detail: l.detail.clone(),
                                current: effort.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(&l.id)),
                                default: default.as_deref() == Some(l.id.as_str()),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Open (or close) `id`'s model or thinking list, the keys on what it runs on now.
    pub(crate) fn focus_toggle_menu(&mut self, id: PaneId, kind: MenuKind) {
        if self.focus.drafts.get(&id).and_then(|d| d.menu).is_some_and(|m| m.kind == kind) {
            self.focus.draft(id).menu = None;
            return;
        }
        let rows = self.focus_menu_rows(id, kind);
        if rows.is_empty() {
            return;
        }
        let sel = rows.iter().position(|r| r.current).unwrap_or(0);
        self.focus.draft(id).menu = Some(Menu { kind, sel });
    }

    /// The model and thinking lists' keys: up and down move, enter picks, esc closes. Any other key closes the
    /// list and does what it does. True when the key was the list's.
    fn focus_menu_key(&mut self, id: PaneId, k: KeyEvent) -> bool {
        let Some(menu) = self.focus.drafts.get(&id).and_then(|d| d.menu) else { return false };
        let n = self.focus_menu_rows(id, menu.kind).len().max(1);
        let d = self.focus.draft(id);
        match k.code {
            KeyCode::Up => d.menu = Some(Menu { sel: (menu.sel + n - 1) % n, ..menu }),
            KeyCode::Down | KeyCode::Tab => d.menu = Some(Menu { sel: (menu.sel + 1) % n, ..menu }),
            KeyCode::Esc => d.menu = None,
            KeyCode::Enter => self.focus_pick(id, menu.kind, menu.sel),
            _ => {
                d.menu = None;
                return false;
            }
        }
        true
    }

    /// Row `n` of `id`'s model or thinking list was picked: switch to it. Claude Code takes `/model` and `/effort`
    /// as they come; Codex takes `/model <model> --reasoning=<level>` between turns (an older Codex, only its own
    /// menu, which shows in the terminal).
    pub(crate) fn focus_pick(&mut self, id: PaneId, kind: MenuKind, n: usize) {
        self.focus.draft(id).menu = None;
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return };
        let Some(harness) = t.meta.harness else { return };
        let version = t.insights().and_then(|s| s.conversation).and_then(|c| c.version.clone());
        let said = self.focus_said(id);
        let (model, effort) = self.focus_running_on(id);
        let models = self.focus_models(id);
        let levels = self.focus_levels(id);
        // half a change still on its way (a model picked, then its thinking) stays with the new half
        let mut change = match self.focus.drafts.get(&id).and_then(|d| d.change.clone()) {
            Some(c) if c.was == said => Change { sent: None, ..c },
            _ => Change { model: None, effort: None, was: said.clone(), sent: None },
        };
        let other = self.focus_runs_other(id);
        let command = match kind {
            MenuKind::Model => {
                // the first row is what it runs on now when that isn't one of the list's
                let Some(m) = n.checked_sub(other as usize).and_then(|n| models.as_ref().and_then(|ms| ms.get(n))).cloned() else { return };
                if model.as_deref().is_some_and(|cur| same_model(cur, &m)) {
                    return;
                }
                // a thinking level the new model doesn't have becomes its own starting one
                if harness == Harness::Codex && !effort.as_deref().is_some_and(|e| m.levels.iter().any(|l| l.id.eq_ignore_ascii_case(e))) {
                    change.effort = m.default_level.clone();
                }
                change.model = Some(m.id.clone());
                format!("/model {}", m.id)
            }
            MenuKind::Thinking => {
                let Some(l) = levels.as_ref().and_then(|(ls, _)| ls.get(n)).cloned() else { return };
                if effort.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(&l.id)) {
                    return;
                }
                change.effort = Some(l.id.clone());
                format!("/effort {}", l.id)
            }
        };
        if harness == Harness::Codex && !codex_takes_arguments(version.as_deref()) {
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
        if c.was != said || c.sent.is_some_and(|n| items > n + 1) {
            d.change = None;
            return;
        }
        if c.sent.is_some() || busy {
            return;
        }
        let (Some(model), effort) = (c.model.clone().or(said.0), c.effort.clone().or(said.1)) else {
            d.change = None;
            return;
        };
        let command = match effort {
            Some(e) => format!("/model {model} --reasoning={e}"),
            None => format!("/model {model}"),
        };
        d.change = Some(Change { sent: Some(items), ..c });
        if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
            t.type_command(&command);
        }
    }

    /// Every session's waiting change, once a loop.
    pub(crate) fn focus_settle_all(&mut self) {
        let ids: Vec<PaneId> = self.focus.drafts.iter().filter(|(_, d)| d.change.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            self.focus_settle(id);
        }
    }
}
