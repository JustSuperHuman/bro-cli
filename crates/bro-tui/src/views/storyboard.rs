//! The Storyboard: a Create Slop project's film as timed scenes, beside the session that is making it
//! ([`bro_core::storyboard`]). Each scene shows what is heard (its script), what is seen (its direction), its
//! keyframes and its takes. The agent fills it in through bro's storyboard tools; you change it here: reword a
//! line, choose the keyframe a scene is filmed from, approve a scene, or ask for a change. What you changed
//! waits on the board for the agent, and **Send changes** tells its session to read it.
//!
//! The board is the file `slop/storyboard.json` in the project. A background thread watches it and hands each
//! new version over ([`Shared`]); every change made here is written by another, so nothing on the UI thread
//! touches the disk. The demo's board is a fixture kept in memory.
//!
//! This file is the state, the keys and the terminal UI's drawing; `storyboard/gui.rs` draws it in the window.

#[cfg(feature = "gui")]
pub mod gui;

use crate::pane::{Action, Cx, Pane, Waker};
use crate::textarea::TextArea;
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::storyboard::{self, Board, Field, Scene, Stage, clock, seconds};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use parking_lot::Mutex;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use ratatui::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// How often the board's file is looked at.
const WATCH_EVERY: Duration = Duration::from_millis(400);

/// What the watcher and the writers hand the view.
#[derive(Default)]
struct Shared {
    /// the board as last read or written (None: the project has none yet)
    board: Mutex<Option<Board>>,
    /// goes up each time `board` is replaced
    seq: AtomicU64,
    /// why the last write failed
    error: Mutex<Option<String>>,
    /// the view is gone
    stop: AtomicBool,
}

/// Something you changed, by the scene's id (the agent may have moved scenes about since it was drawn).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change {
    /// the film's title or treatment (`None`), or a scene's text
    Text(Option<String>, Field, String),
    Approve(String, bool),
    Keyframe(String, usize),
    Take(String, usize),
}

impl Change {
    fn apply(&self, b: &mut Board) -> bool {
        let at = |b: &Board, id: &str| b.scenes.iter().position(|s| s.id == id);
        match self {
            Change::Text(None, field, text) => b.set_text(None, *field, text),
            Change::Text(Some(id), field, text) => at(b, id).is_some_and(|i| b.set_text(Some(i), *field, text)),
            Change::Approve(id, on) => at(b, id).is_some_and(|i| b.approve(i, *on)),
            Change::Keyframe(id, n) => at(b, id).is_some_and(|i| b.pick_keyframe(i, *n)),
            Change::Take(id, n) => at(b, id).is_some_and(|i| b.pick_take(i, *n)),
        }
    }
}

/// A text being edited in place.
pub struct Editing {
    /// the scene it belongs to (None: the film)
    pub scene: Option<String>,
    pub field: Field,
    pub area: TextArea,
    /// how many characters fit across the field as last drawn (the editor wraps by them)
    pub cols: usize,
}

pub struct StoryboardView {
    pub project: PathBuf,
    /// the session it was opened beside: where **Send changes** types
    pub session: Option<String>,
    /// what that session runs on (it decides how the agent is told to read the board)
    pub harness: Option<Harness>,
    /// the film is made with SLOPCANNON: its limits on a scene (5 to 15 seconds, a direction of 2,800
    /// characters) are checked
    pub slopcannon: bool,
    shared: Arc<Shared>,
    seen: u64,
    watching: bool,
    /// the demo's board lives here and nowhere else
    memory: bool,
    pub board: Option<Board>,
    /// the selected scene
    pub sel: usize,
    pub edit: Option<Editing>,
    /// the edits that were on the board when its session was last told (how many, the newest one's time)
    told: Option<(usize, i64)>,
    /// The window's: how far the scene list is scrolled, and whether the next draw brings the selection into view.
    pub(crate) scroll: f32,
    pub(crate) reveal: bool,
    /// The window's: how far the selected scene's side is scrolled.
    pub(crate) detail_scroll: f32,
}

impl StoryboardView {
    /// The storyboard of `project`, beside the session with this id.
    pub fn new(project: PathBuf, session: Option<String>, harness: Option<Harness>) -> StoryboardView {
        StoryboardView { project, session, harness, slopcannon: false, shared: Arc::default(), seen: 0, watching: false, memory: false, board: None, sel: 0, edit: None, told: None, scroll: 0.0, reveal: true, detail_scroll: 0.0 }
    }

    /// The demo's: a film part-way made, kept in memory.
    pub fn demo(project: PathBuf, session: Option<String>) -> StoryboardView {
        let mut v = StoryboardView::new(project, session, Some(Harness::Claude));
        (v.board, v.memory, v.watching, v.slopcannon) = (Some(demo_board()), true, true, true);
        v
    }

    /// Start watching the board's file (once), waking this pane whenever it changes.
    pub(crate) fn watch(&mut self, waker: Waker) {
        if std::mem::replace(&mut self.watching, true) {
            return;
        }
        let (shared, project) = (self.shared.clone(), self.project.clone());
        let _ = std::thread::Builder::new().name("bro-storyboard-watch".into()).spawn(move || {
            let file = storyboard::path(&project);
            let mut last = None;
            let mut first = true;
            while !shared.stop.load(Ordering::Relaxed) {
                let now = std::fs::metadata(&file).ok().map(|m| (m.modified().ok(), m.len()));
                if first || now != last {
                    first = false;
                    last = now;
                    let board = crate::services::guard("storyboard::load", || storyboard::load(&project)).ok().flatten();
                    // a half-written file reads as nothing: what was there stays until it reads again
                    if board.is_some() || now.is_none() {
                        *shared.board.lock() = board;
                        shared.seq.fetch_add(1, Ordering::Relaxed);
                        waker.wake();
                    }
                }
                std::thread::sleep(WATCH_EVERY);
            }
        });
    }

    /// Take what the watcher or a writer left since the last look.
    pub(crate) fn sync(&mut self) {
        let seq = self.shared.seq.load(Ordering::Relaxed);
        if self.memory || seq == self.seen {
            return;
        }
        self.seen = seq;
        self.board = self.shared.board.lock().clone();
        let n = self.scenes().len();
        self.sel = self.sel.min(n.saturating_sub(1));
    }

    pub fn scenes(&self) -> &[Scene] {
        self.board.as_ref().map_or(&[], |b| b.scenes.as_slice())
    }

    pub fn scene(&self) -> Option<&Scene> {
        self.scenes().get(self.sel)
    }

    /// Why the last change wasn't saved.
    pub(crate) fn error(&self) -> Option<String> {
        self.shared.error.lock().clone()
    }

    /// Make a change: it shows at once and is written to the project behind.
    pub(crate) fn change(&mut self, change: Change, waker: Waker) {
        let Some(board) = &mut self.board else { return };
        if !change.apply(board) || self.memory {
            return;
        }
        let (shared, project) = (self.shared.clone(), self.project.clone());
        std::thread::spawn(move || {
            let done = crate::services::guard("storyboard::update", || storyboard::update(&project, |b| (change.apply(b), ())));
            match done {
                Ok(Ok((_, board))) => {
                    *shared.board.lock() = Some(board);
                    *shared.error.lock() = None;
                }
                Ok(Err(e)) => *shared.error.lock() = Some(format!("The storyboard couldn't be saved: {e}")),
                Err(e) => *shared.error.lock() = Some(e),
            }
            shared.seq.fetch_add(1, Ordering::Relaxed);
            waker.wake();
        });
    }

    /// Select scene `i` (leaving any text being edited as it was typed).
    pub(crate) fn select(&mut self, i: usize, cx: &Cx) {
        let i = i.min(self.scenes().len().saturating_sub(1));
        if i != self.sel {
            self.commit(cx);
            self.sel = i;
            self.detail_scroll = 0.0;
        }
        self.reveal = true;
    }

    /// The text of `field` as it stands (`scene`: None for the film's).
    fn text_of(&self, scene: Option<usize>, field: Field) -> Option<String> {
        let b = self.board.as_ref()?;
        Some(match (scene, field) {
            (None, Field::Film) => b.title.clone(),
            (None, Field::Concept) => b.concept.clone(),
            (Some(i), f) => {
                let s = b.scenes.get(i)?;
                match f {
                    Field::Title => s.title.clone(),
                    Field::Script => s.script.clone(),
                    Field::Direction => s.direction.clone(),
                    Field::Note => s.note.clone(),
                    Field::Film | Field::Concept => return None,
                }
            }
            _ => return None,
        })
    }

    /// Start editing `field` of the selected scene (or the film's title or treatment).
    pub(crate) fn begin(&mut self, field: Field, cx: &Cx) {
        let scene = (!matches!(field, Field::Film | Field::Concept)).then_some(self.sel);
        if self.edit.as_ref().is_some_and(|e| e.field == field && e.scene == scene.and_then(|i| self.scenes().get(i)).map(|s| s.id.clone())) {
            return;
        }
        self.commit(cx);
        let Some(text) = self.text_of(scene, field) else { return };
        let mut area = TextArea::new(&text);
        // a title is one line: enter saves it. The longer texts take new lines.
        area.newline_on_enter = !matches!(field, Field::Film | Field::Title);
        area.cursor_to(false);
        self.edit = Some(Editing { scene: scene.and_then(|i| self.scenes().get(i)).map(|s| s.id.clone()), field, area, cols: 60 });
    }

    /// Keep what was typed (when it differs) and close the editor.
    pub(crate) fn commit(&mut self, cx: &Cx) {
        let Some(e) = self.edit.take() else { return };
        self.change(Change::Text(e.scene, e.field, e.area.text().to_string()), cx.waker());
    }

    /// Sign the selected scene off, or take that back.
    pub(crate) fn toggle_approved(&mut self, cx: &Cx) {
        if let Some(s) = self.scene() {
            self.change(Change::Approve(s.id.clone(), !s.approved), cx.waker());
        }
    }

    /// Sign off every scene that isn't waiting on a change you asked for.
    pub(crate) fn approve_all(&mut self, cx: &Cx) {
        let ids: Vec<String> = self.scenes().iter().filter(|s| !s.approved && s.note.is_empty()).map(|s| s.id.clone()).collect();
        for id in ids {
            self.change(Change::Approve(id, true), cx.waker());
        }
    }

    pub(crate) fn pick_keyframe(&mut self, n: usize, cx: &Cx) {
        if let Some(s) = self.scene() {
            self.change(Change::Keyframe(s.id.clone(), n), cx.waker());
        }
    }

    pub(crate) fn pick_take(&mut self, n: usize, cx: &Cx) {
        if let Some(s) = self.scene() {
            self.change(Change::Take(s.id.clone(), n), cx.waker());
        }
    }

    /// There are changes of yours the session hasn't been told about.
    pub(crate) fn unsent(&self) -> bool {
        let Some(b) = &self.board else { return false };
        !b.edits.is_empty() && self.told != Some(edits_sig(b))
    }

    /// Tell the session what you changed (typed into it as a message).
    pub(crate) fn send_changes(&mut self, cx: &mut Cx) {
        self.commit(cx);
        let Some(b) = &self.board else { return };
        let (Some(sid), Some(text)) = (self.session.clone(), b.nudge(matches!(self.harness, Some(Harness::Claude | Harness::Codex)))) else { return };
        self.told = Some(edits_sig(b));
        cx.act(Action::Tell(sid, text));
    }

    /// Open a file of the board (a keyframe, a take, the song) in bro's preview.
    pub(crate) fn preview(&self, kept: &str, cx: &mut Cx) {
        cx.act(Action::Preview(storyboard::full_path(&self.project, kept)));
    }

    /// Where the film stands, in a line: "12 scenes · 1:36 · 9 keyframed · 4 filmed · 7 approved".
    pub(crate) fn summary(&self) -> String {
        let Some(b) = self.board.as_ref().filter(|b| !b.scenes.is_empty()) else { return String::new() };
        let n = b.scenes.len();
        let mut parts = vec![format!("{n} scene{}", if n == 1 { "" } else { "s" }), clock(b.runs_ms())];
        if !b.aspect.is_empty() {
            parts.push(b.aspect.clone());
        }
        for (count, word) in [(b.count(Stage::Keyframed), "keyframed"), (b.count(Stage::Filmed), "filmed"), (b.approved(), "approved")] {
            if count > 0 {
                parts.push(format!("{count} {word}"));
            }
        }
        if let Some(c) = b.credits() {
            parts.push(format!("{c:.0} credits quoted"));
        }
        parts.join(" · ")
    }
}

/// The edits on a board, as a signature: how many, and the newest one's time.
fn edits_sig(b: &Board) -> (usize, i64) {
    (b.edits.len(), b.edits.iter().map(|e| e.at).max().unwrap_or(0))
}

impl Drop for StoryboardView {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// The demo's film: a music video in six scenes, three of them drawn, one filmed, one waiting on a change.
fn demo_board() -> Board {
    let plan = serde_json::json!({
        "title": "Night Drive", "aspect": "16:9", "durationMs": 58000, "song": "slop/audio/night-drive.mp3",
        "concept": "One night, one car, a city that won't sleep. Claudia drives; Chinese Bone rides shotgun and never says a word. Neon on wet asphalt, long lenses, nothing handheld. It ends at dawn with the engine off.",
        "cast": ["CLAUDIA", "CHINESE BONE"],
        "scenes": [
            { "startMs": 0, "endMs": 8000, "title": "Cold open", "cast": ["CLAUDIA"], "script": "(instrumental: four bars of synth, a key turning)", "direction": "[Shot 1] A parking garage, sodium light. CLAUDIA (profile, facing frame-right; walks left to right, unhurried) crosses to a boxy 1980s coupe. At 00:04.200, a slow push in as she opens the door. End on her hand on the wheel." },
            { "startMs": 8000, "endMs": 20000, "title": "Pulling out", "cast": ["CLAUDIA", "CHINESE BONE"], "script": "We carry the morning home\nWe carry it slow", "direction": "[Shot 1] Inside the car. CLAUDIA (3/4 left, facing the road; static) drives. At 00:01.180, CLAUDIA (S1) sings, <d>[English]We carry the morning home.</d> CHINESE BONE (profile, facing frame-left; static) watches the window and CHINESE BONE's mouth remains closed for the full line. At 00:07.400, neon slides across the windscreen." },
            { "startMs": 20000, "endMs": 30000, "title": "The bridge", "cast": ["CLAUDIA"], "script": "Every light is a year I owe", "direction": "[Shot 1] Wide, from the river: the coupe crosses a suspension bridge right to left, rain in the headlights. [Shot 2] Back inside, CLAUDIA (toward camera; static). At 00:05.050, CLAUDIA (S1) sings, <d>[English]Every light is a year I owe.</d>" },
            { "startMs": 30000, "endMs": 41000, "title": "Diner", "cast": ["CLAUDIA", "CHINESE BONE"], "script": "(spoken) You ever think about stopping?\n(no answer)", "direction": "[Shot 1] A 24-hour diner seen through its own window. CLAUDIA (profile, facing frame-right; static) stirs a coffee. CHINESE BONE (profile, facing frame-left; static) sits opposite, hands folded. At 00:03.300, CLAUDIA (S1) says, <d>[English]You ever think about stopping?</d> CHINESE BONE's mouth remains closed." },
            { "startMs": 41000, "endMs": 50000, "title": "The long straight", "cast": ["CLAUDIA"], "script": "(instrumental build)", "direction": "[Shot 1] Low tracking shot beside the front wheel, road markings strobing. [Shot 2] The speedometer needle climbing. End on the horizon going pale." },
            { "startMs": 50000, "endMs": 58000, "title": "Dawn", "cast": ["CLAUDIA", "CHINESE BONE"], "script": "We carried it home", "direction": "[Shot 1] A cliff road at first light. The coupe is parked, engine off. CLAUDIA (seen from behind, back to camera; static) and CHINESE BONE (seen from behind; static) sit on the bonnet. At 00:02.000, CLAUDIA (S1) sings, <d>[English]We carried it home.</d> Hold on the sky." }
        ]
    });
    let project = Path::new("");
    let mut b = Board::default();
    let _ = b.apply_plan(&plan, project);
    let scene = |b: &mut Board, v: serde_json::Value| {
        let _ = b.apply_scene(&v, project);
    };
    for (n, frames) in [(1, 2), (2, 3), (3, 1)] {
        for k in 0..frames {
            scene(&mut b, serde_json::json!({ "scene": n, "keyframe": format!("slop/keyframes/s{n}-{}.png", (b'a' + k) as char) }));
        }
    }
    scene(&mut b, serde_json::json!({ "scene": 1, "take": "slop/clips/s1-take1.mp4", "credits": 64 }));
    scene(&mut b, serde_json::json!({ "scene": 1, "take": "slop/clips/s1-take2.mp4", "selectKeyframe": 1 }));
    scene(&mut b, serde_json::json!({ "scene": 2, "selectKeyframe": 2, "credits": 96 }));
    b.approve(0, true);
    b.approve(1, true);
    b.set_text(Some(2), Field::Note, "Lose the second shot. Stay wide on the bridge the whole time.");
    b.rev = 7;
    b
}

impl Pane for StoryboardView {
    fn title(&self) -> String {
        match self.board.as_ref().map(|b| b.title.as_str()).filter(|t| !t.is_empty()) {
            Some(title) => format!("storyboard · {title}"),
            None => "storyboard".into(),
        }
    }
    fn icon(&self) -> &'static str {
        "film"
    }
    fn view(&self) -> Option<&'static str> {
        // one a project: `App::show_storyboard` finds it by its project, not by this name
        None
    }
    fn tick_every(&self) -> Option<Duration> {
        None
    }
    fn poll(&mut self, cx: &mut Cx) {
        self.watch(cx.waker());
        self.sync();
    }

    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        if let Some(e) = &mut self.edit {
            let text = if e.area.newline_on_enter { text.replace("\r\n", "\n") } else { text.split_whitespace().collect::<Vec<_>>().join(" ") };
            e.area.insert(&text);
        }
    }

    fn key(&mut self, k: KeyEvent, cx: &mut Cx) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // a text is being edited: it has the keys
        if let Some(e) = &mut self.edit {
            match k.code {
                KeyCode::Esc => self.edit = None,
                KeyCode::Char('s') if ctrl => self.commit(cx),
                KeyCode::Enter if ctrl || !e.area.newline_on_enter => self.commit(cx),
                KeyCode::Tab => {
                    // on to the next text of the scene
                    let next = match e.field {
                        Field::Title => Some(Field::Script),
                        Field::Script => Some(Field::Direction),
                        Field::Direction => Some(Field::Note),
                        _ => None,
                    };
                    self.commit(cx);
                    if let Some(f) = next {
                        self.begin(f, cx);
                    }
                }
                _ => {
                    let cols = e.cols.max(8);
                    e.area.key(k, cols);
                }
            }
            return true;
        }
        let n = self.scenes().len();
        match k.code {
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char('l') if n > 0 => self.select(self.sel + 1, cx),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('h') if n > 0 => self.select(self.sel.saturating_sub(1), cx),
            KeyCode::Home if n > 0 => self.select(0, cx),
            KeyCode::End if n > 0 => self.select(n - 1, cx),
            KeyCode::Char('a') => self.toggle_approved(cx),
            KeyCode::Char('A') => self.approve_all(cx),
            KeyCode::Char('t') => self.begin(Field::Title, cx),
            KeyCode::Char('s') | KeyCode::Enter if n > 0 => self.begin(Field::Script, cx),
            KeyCode::Char('d') => self.begin(Field::Direction, cx),
            KeyCode::Char('c') => self.begin(Field::Note, cx),
            KeyCode::Char('f') => self.begin(Field::Film, cx),
            KeyCode::Char('g') => self.send_changes(cx),
            // 1-9: the keyframe the scene is filmed from
            KeyCode::Char(ch @ '1'..='9') => self.pick_keyframe(ch as usize - '1' as usize, cx),
            KeyCode::Char('o') => {
                if let Some(path) = self.scene().and_then(|s| s.chosen_take().or(s.chosen_keyframe())).map(|s| s.path.clone()) {
                    self.preview(&path, cx);
                }
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        self.watch(cx.waker());
        self.sync();
        let t = cx.theme;
        let editing = self.edit.is_some();
        let keys: &[(&str, &str)] = if editing { &[("ctrl+s", "save"), ("tab", "next text"), ("esc", "cancel")] } else { &[("j/k", "scene"), ("s", "script"), ("d", "direction"), ("c", "ask for a change"), ("a", "approve"), ("1-9", "keyframe"), ("g", "send changes")] };
        let area = ui::hint_line(f, area, keys, t);
        let row = |y: u16| Rect { y, height: 1, ..area };
        let w = area.width as usize;
        let mut y = area.y;
        let title = self.board.as_ref().map(|b| b.title.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Untitled film".into());
        ui::line(f, row(y), vec![Span::styled(" STORYBOARD ", ui::bold_accent(t)), Span::styled(format!(" {title}  "), fg(t.fg).add_modifier(Modifier::BOLD)), Span::styled(self.summary(), muted(t))]);
        y += 2;
        if self.scenes().is_empty() {
            ui::line(f, row(y), vec![Span::styled("  No scenes yet. Tell the agent what you want to make and it writes them here.", muted(t))]);
            return;
        }
        let unsent = self.unsent();
        for (i, s) in self.scenes().iter().enumerate() {
            if y + 1 >= area.bottom() {
                break;
            }
            let on = i == self.sel;
            let mark = match (s.approved, s.note.is_empty()) {
                (_, false) => Span::styled("! ", fg(t.inline)),
                (true, _) => Span::styled("✓ ", fg(t.good)),
                _ => Span::raw("  "),
            };
            let stage = match s.stage() {
                Stage::Filmed => format!("{} take{}", s.takes.len(), if s.takes.len() == 1 { "" } else { "s" }),
                Stage::Keyframed => format!("{} keyframe{}", s.keyframes.len(), if s.keyframes.len() == 1 { "" } else { "s" }),
                Stage::Written => "written".into(),
            };
            let head = format!("{:>2}  {}–{}  {}", i + 1, clock(s.start_ms), clock(s.end_ms), if s.title.is_empty() { "Untitled" } else { &s.title });
            let style = if on { fg(t.fg).add_modifier(Modifier::BOLD) } else { fg(t.fg) };
            ui::line_lr(f, row(y), vec![Span::raw(if on { "▸ " } else { "  " }), mark, Span::styled(ui::fit(&head, w.saturating_sub(24)), style)], vec![Span::styled(format!("{} · {stage} ", seconds(s.ms())), muted(t))]);
            y += 1;
            if !on {
                continue;
            }
            // the selected scene opens: what is heard, what is seen, what you asked for
            let field = self.edit.as_ref().map(|e| e.field);
            for (name, which, text) in [("heard", Field::Script, &s.script), ("seen", Field::Direction, &s.direction), ("change", Field::Note, &s.note)] {
                if y >= area.bottom() || (text.is_empty() && field != Some(which) && which == Field::Note) {
                    continue;
                }
                let shown = match &self.edit {
                    Some(e) if e.field == which => format!("{}▏", e.area.text()),
                    _ if text.is_empty() => "(nothing yet)".to_string(),
                    _ => text.clone(),
                };
                let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ");
                ui::line(f, row(y), vec![Span::styled(format!("      {name:<7}"), muted(t)), Span::styled(ui::fit(&flat, w.saturating_sub(14)), if field == Some(which) { fg(t.accent) } else { muted(t) })]);
                y += 1;
            }
        }
        if unsent && y < area.bottom() {
            ui::line(f, row(area.bottom() - 1), vec![Span::styled("  You changed the board. g sends that to the session.", fg(t.inline))]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{Services, fallback_settings};
    use crossterm::event::KeyModifiers;

    fn key(v: &mut StoryboardView, code: KeyCode, mods: KeyModifiers) -> Vec<Action> {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx.clone());
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        let mut cx = Cx { id: 1, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        v.key(KeyEvent::new(code, mods), &mut cx);
        actions
    }

    fn typed(v: &mut StoryboardView, text: &str) {
        for ch in text.chars() {
            key(v, KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }

    #[test]
    fn the_demo_has_a_film_part_way_made() {
        let v = StoryboardView::demo(PathBuf::from("/films/night"), Some("s1".into()));
        let b = v.board.as_ref().unwrap();
        assert_eq!((b.scenes.len(), b.count(Stage::Keyframed), b.count(Stage::Filmed), b.approved()), (6, 3, 1, 2));
        assert!(b.problems(true).is_empty(), "{:?}", b.problems(true));
        assert_eq!(v.summary(), "6 scenes · 0:58 · 16:9 · 3 keyframed · 1 filmed · 2 approved · 160 credits quoted");
        assert!(v.unsent(), "the change asked for on scene 3 hasn't been sent");
        assert_eq!(v.title(), "storyboard · Night Drive");
    }

    #[test]
    fn a_script_is_edited_in_place_and_sent_to_the_session() {
        let mut v = StoryboardView::demo(PathBuf::from("/films/night"), Some("s1".into()));
        key(&mut v, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(v.sel, 1);
        key(&mut v, KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(v.edit.as_ref().map(|e| (e.field, e.area.text().to_string())), Some((Field::Script, "We carry the morning home\nWe carry it slow".into())));
        typed(&mut v, "er");
        // j types into the text while it is being edited
        typed(&mut v, "j");
        key(&mut v, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(v.sel, 1);
        key(&mut v, KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(v.edit.is_none());
        let b = v.board.as_ref().unwrap();
        assert_eq!(b.scenes[1].script, "We carry the morning home\nWe carry it slower");
        assert!(!b.scenes[1].approved || b.scenes[1].approved, "your own rewording keeps your sign-off");
        assert!(b.edits_said().iter().any(|e| e == "scene 2 (s2): the script was rewritten"));
        // esc throws a change away
        key(&mut v, KeyCode::Char('t'), KeyModifiers::NONE);
        typed(&mut v, "!!!");
        key(&mut v, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(v.board.as_ref().unwrap().scenes[1].title, "Pulling out");
        // a title is one line: enter keeps it
        key(&mut v, KeyCode::Char('t'), KeyModifiers::NONE);
        typed(&mut v, " now");
        key(&mut v, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(v.board.as_ref().unwrap().scenes[1].title, "Pulling out now");

        // approve, choose a keyframe, then tell the session once
        key(&mut v, KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(!v.board.as_ref().unwrap().scenes[1].approved, "it was approved: a takes that back");
        key(&mut v, KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(v.board.as_ref().unwrap().scenes[1].keyframe, 0);
        key(&mut v, KeyCode::Char('9'), KeyModifiers::NONE);
        assert_eq!(v.board.as_ref().unwrap().scenes[1].keyframe, 0, "a keyframe that isn't there");
        let sent = key(&mut v, KeyCode::Char('g'), KeyModifiers::NONE);
        let [Action::Tell(sid, text)] = sent.as_slice() else { panic!("one message to the session") };
        assert_eq!(sid, "s1");
        assert!(text.starts_with("I changed the storyboard: ") && text.contains("storyboard_get"), "{text}");
        assert!(!v.unsent(), "told: nothing new to send");
        assert!(key(&mut v, KeyCode::Char('g'), KeyModifiers::NONE).iter().all(|a| matches!(a, Action::Tell(..))), "sending again is allowed");
        key(&mut v, KeyCode::Char('A'), KeyModifiers::NONE);
        let b = v.board.as_ref().unwrap();
        assert_eq!(b.approved(), 5, "every scene but the one waiting on a change");
        assert!(v.unsent());
    }

    #[test]
    fn a_change_is_written_to_the_project_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_path_buf();
        storyboard::update(&project, |b| (true, b.apply_plan(&serde_json::json!({ "title": "Disk", "scenes": [{ "seconds": 6, "title": "One", "direction": "A door." }] }), &project).unwrap())).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut v = StoryboardView::new(project.clone(), None, None);
        v.watch(Waker::new(1, tx.clone()));
        let wait = |v: &mut StoryboardView, done: &dyn Fn(&StoryboardView) -> bool| {
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while !done(v) && std::time::Instant::now() < until {
                let _ = rx.recv_timeout(Duration::from_millis(200));
                v.sync();
            }
            assert!(done(v));
        };
        wait(&mut v, &|v| v.board.as_ref().is_some_and(|b| b.title == "Disk"));
        v.change(Change::Approve("s1".into(), true), Waker::new(1, tx.clone()));
        assert!(v.scene().unwrap().approved, "it shows at once");
        wait(&mut v, &|v| v.board.as_ref().is_some_and(|b| b.rev == 2));
        assert!(storyboard::load(&project).unwrap().scenes[0].approved);
        assert_eq!(v.error(), None);
        // the agent's write (another process) shows up on its own
        storyboard::update(&project, |b| (true, b.apply_scene(&serde_json::json!({ "scene": 1, "keyframe": "slop/k/a.png" }), &project).unwrap())).unwrap();
        wait(&mut v, &|v| v.scene().is_some_and(|s| s.keyframes.len() == 1));
        // no session: nothing to send to
        let mut actions = vec![];
        let svc = Services::offline(fallback_settings(), tx.clone());
        let theme = crate::theme::get("graphite");
        let mut cx = Cx { id: 1, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        v.send_changes(&mut cx);
        assert!(actions.is_empty());
    }
}
