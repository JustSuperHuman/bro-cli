//! Focus mode in the desktop app ([`crate::focus`]): an agent session drawn as its conversation instead of its
//! terminal, a document in big type with the reply box under it. Where the session works heads the
//! conversation; what the agent is doing (and how its thinking is going) is in the card's header
//! ([`header_status`]); the model it runs and how hard it thinks sit under the reply box. Links and file paths
//! open as they do in the terminal (the session's own file lookups find them), and pictures you paste or the
//! agent shows are thumbnails. The reply box's footer switches the model and how hard it thinks, and typing `/`
//! lists the commands the agent takes. The terminal still runs under it at the card's size.
//!
//! The subagents a session starts show where it started them, live, and each opens its own conversation in place of
//! the session's ([`subagents`]); those at work stay in sight in a strip over the reply box, with the background
//! tasks it left running (commands, watches, workflows: [`bro_core::sessions::tasks`]), and the header says they're
//! at work once its turn is over.
//!
//! The text is the transcript's ([`bro_core::sessions::conversation`]), the reply box and what you sent are the
//! app's ([`crate::focus::Draft`]); this keeps only layout and motion ([`View`]).

mod arrive;
mod markdown;
mod pixels;
mod sound;
mod quips;
mod subagents;

use super::actions::Action;
use super::kit::breadcrumbs::{crumb_plan, path_parts};
use super::kit::{Cursor, Style, TextBox, Ui, radius, size, weight, widgets};
use super::kit::motion::Spring;
use super::apps::{Apps, FIRST_HEIGHT, Page, Want};
use super::media::{Media, Thumbnail};
use super::{Drag, Gui};
use crate::app::App;
use crate::app::focus::{MenuRow, level_name, same_model};
use crate::focus::{Choice, Command, MenuKind, Scroll, holds, model_name, visible};
use crate::layout::PaneId;
use crate::pane::Activity;
use bro_bridge::file_links::{FileLinkMatch, find_file_links, split_location};
use bro_core::Harness;
use bro_core::sessions::conversation::{Conversation, Item, McpApp, NoteKind, Step, StepKind, StepState, Thought};
use bro_core::sessions::subagents::Subagent;
use bro_core::sessions::tasks::Task;
use subagents::Subs;
use bro_gfx::text::{Align, Span as GSpan, TextOpts};
use bro_gfx::{Color, Fill, Point, Quad, Rect};
use markdown::{Block, Mark, Span};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

// click targets
const AREA: usize = 0;
const BOX: usize = 1;
const SEND: usize = 2;
const STOP: usize = 3;
const LATEST: usize = 4;
/// show the session's terminal (or its conversation again)
pub(super) const TERMINAL: usize = 5;
const EARLIER: usize = 6;
/// the footer's model and thinking: open their lists
const MODEL_CHIP: usize = 7;
const THINKING_CHIP: usize = 8;
/// from a subagent's own conversation back to the session's
const BACK: usize = 9;
const ANSWER_QUEUED: usize = 10;
const DICTATE: usize = 11;
const CANCEL_DICTATION: usize = 12;
const SKIP_CLEANUP: usize = 13;
const OPTION: usize = 100;
/// + a picture's place in the reply box: its popover's Remove, and Open
const UNATTACH: usize = 200;
const SHOW_ATTACHED: usize = 300;
/// + a row of the model or thinking list, of the commands list
const MENU_ROW: usize = 400;
const COMMAND_ROW: usize = 500;
/// + an item's index: open or close its steps (a message: show it whole)
const OPEN: usize = 10_000;
/// + an item's index: copy what it says
const COPY: usize = 100_000;
/// + a link's index this frame
const LINK: usize = 1_000_000;
/// + a picture's index this frame
const PICTURE: usize = 2_000_000;
/// + a failed MCP app card's index this frame: read its page again
const APP_RETRY: usize = 20;
const APP_RETRY_END: usize = 100;
/// + a sound card's index this frame: its play button, its waveform (plays from where it's clicked)
const SOUND: usize = 3_000_000;
pub(super) const SOUND_AT: usize = 4_000_000;
/// + a subagent's index this frame (its card, its chip, its note): show its own conversation
const SUBAGENT: usize = 5_000_000;
const SUBAGENT_END: usize = 6_000_000;

/// Body text line height, as a multiple of its size.
const LINE: f32 = 1.62;
/// A message of yours shows this many lines until you open it.
const YOU_LINES: usize = 14;
/// Code shows this many lines.
const CODE_LINES: usize = 40;
/// The conversation shows this many of its latest items, and this many more for each "Earlier".
const HISTORY: usize = 80;
/// A turn that ended while you watched says "Done" this long (seconds), then nothing.
const DONE_SHOWS: f32 = 8.0;
/// Pictures in your messages, in what the agent did, and waiting in the reply box (logical px).
const SHOT: (f32, f32) = (132.0, 96.0);
const SHOWN: (f32, f32) = (84.0, 64.0);
const ATTACHED: f32 = 60.0;
/// An MCP app's card: its corners, the room above it, and where its page sits inside it (clear of the corners)
const APP_RADIUS: f32 = 12.0;
const APP_GAP: f32 = 10.0;
const APP_INSET: (f32, f32) = (2.0, 7.0);
/// The tallest a card's page grows (a taller page scrolls inside it)
const APP_MAX: f32 = 3000.0;

/// What the agent is up to, as focus mode says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Working,
    Blocked,
    Done,
}

/// What the window keeps per session in focus mode: layout and motion (the text is the conversation's, the
/// reply box the app's).
pub struct View {
    /// px scrolled from the top of the conversation
    top: f32,
    /// new messages scroll into view (you're at the bottom)
    follow: bool,
    /// the conversation's height and the room it had, last frame
    total: f32,
    room: f32,
    line_height: f32,
    /// laid-out items by identity (the item itself is kept, so its address can't be reused while cached)
    cache: HashMap<usize, Cached>,
    cached_skip: usize,
    gallery_revision: u64,
    narration: Vec<bool>,
    message_pictures: Vec<Vec<PathBuf>>,
    /// items whose steps are open (a long message of yours shown whole), by identity
    open: HashSet<usize>,
    /// this frame's links and pictures (their targets)
    links: Vec<String>,
    pictures: Vec<PathBuf>,
    /// this frame's sound cards: the file, and where its waveform is
    sounds: Vec<(PathBuf, Rect)>,
    /// where the reply box's text sat (a click puts the caret under it)
    geo: Option<BoxGeo>,
    /// the mood and since when (the header says "Done" a while; thinking counts from it)
    mood: Option<(Mood, Instant)>,
    /// the choice on screen as last read (a frame that couldn't read the screen keeps it)
    choice: Option<Choice>,
    /// the caret's spot for the input method, logical px
    pub caret: Option<Rect>,
    /// how many of the latest items show ("Earlier" adds more)
    shown: usize,
    /// the picture in the reply box whose popover is open, and where the popover was (the pointer can cross to it)
    pop: Option<usize>,
    pop_rect: Option<Rect>,
    /// the commands list's first row shown, and the model list's
    command_top: usize,
    menu_top: usize,
    quip_fields: [Option<quips::FieldGeo>; 3],
    quip_preview: quips::PreviewState,
    /// the pixel mark of a session with nothing said yet, while it shows
    mark: Option<pixels::Play>,
    /// MCP apps' cards drawn this frame (the call, the app, where its page goes: laid over them once the frame
    /// is drawn), the failed ones (a click reads them again) and the part of the window the pages may show in
    cards: Vec<(String, Arc<McpApp>, Rect)>,
    retry: Vec<Arc<McpApp>>,
    clip: Rect,
    /// a list or a popover is open over the conversation: the pages hide under it
    popup: bool,
    /// the conversation's text as drawn last frame, in reading order: what a press or a drag lands on
    runs: Vec<Run>,
    /// what's selected in the conversation
    sel: Option<Sel>,
    /// the text of every run the selection has been drawn over, kept while it lasts (one scrolled away still
    /// copies)
    sel_text: BTreeMap<RunKey, RunText>,
    /// a press in the reply box: what it took (a double-click's word), which a drag grows from
    box_origin: Option<(usize, usize)>,
    /// where the conversation was last frame (a drag past its edge scrolls)
    text_area: Rect,
    /// the conversation drawn last frame (the session's, or a subagent's): what a click on an item means
    drawn: Option<Arc<Conversation>>,
    /// the subagents that open with a click this frame (cards, chips, notes), by the call that started each
    opened: Vec<String>,
    /// the subagent shown instead of the conversation last frame, and the conversation's scroll while it shows
    lens: Option<String>,
    saved: Option<(f32, bool, usize, HashSet<usize>)>,
    /// a subagent's conversation made up of its brief and its report, when it left no transcript to read
    made: Option<(String, usize, Arc<Conversation>)>,
    /// how many items the conversation shown had last frame (those past it arrive: they rise and fade in), and
    /// which one that was (the session's, or a subagent's: None before its first frame)
    met: Option<usize>,
    shown_lens: Option<Option<String>>,
    /// what the strip over the reply box last held, to fade out once nothing is at work
    dock_last: Option<(Vec<Arc<Subagent>>, Vec<Task>)>,
    /// items that arrived and are still landing (by place in the conversation)
    arriving: Vec<usize>,
    /// how tall the reply box is when it's done growing (what it draws is on its way there)
    box_full: f32,
}

/// A piece of the conversation's text: the item it's in, and its place among that item's pieces.
type RunKey = (usize, u32);

/// A place in the conversation's text: a byte of one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    key: RunKey,
    byte: usize,
}

/// One laid-out piece of the conversation's text (a paragraph, a list item, a line of code, your message), as
/// drawn: `at` its top-left, logical px.
#[derive(Clone)]
struct Run {
    key: RunKey,
    at: Point,
    layout: Arc<bro_gfx::text::Layout>,
    scale: f32,
    /// what goes before it when it's copied after the run before it, and the mark it's drawn with (a list's)
    gap: &'static str,
    lead: String,
}

impl Run {
    fn rect(&self) -> Rect {
        Rect::new(self.at.x, self.at.y, self.layout.width() / self.scale, self.layout.height() / self.scale)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RunText {
    gap: &'static str,
    lead: String,
    text: String,
}

/// A selection in the conversation: what the press took (a place, a double-click's word, a triple-click's
/// piece), and the ends now (the drag grows it from what the press took).
#[derive(Clone, Debug)]
struct Sel {
    origin: (Pos, Pos),
    anchor: Pos,
    head: Pos,
    /// where the press was, and what it would have done as a click (a link, a long message opening): done on
    /// release when the pointer didn't move
    press: Point,
    click: Option<usize>,
    moved: bool,
}

impl Sel {
    /// Its ends in reading order; None when nothing is selected.
    fn span(&self) -> Option<(Pos, Pos)> {
        (self.anchor != self.head).then(|| (self.anchor.min(self.head), self.anchor.max(self.head)))
    }
}

impl Default for View {
    fn default() -> View {
        View {
            top: 0.0,
            follow: true,
            total: 0.0,
            room: 0.0,
            line_height: 30.0,
            cache: HashMap::new(),
            cached_skip: 0,
            gallery_revision: 0,
            narration: Vec::new(),
            message_pictures: Vec::new(),
            open: HashSet::new(),
            links: vec![],
            pictures: vec![],
            sounds: vec![],
            geo: None,
            mood: None,
            choice: None,
            caret: None,
            shown: HISTORY,
            pop: None,
            pop_rect: None,
            command_top: 0,
            menu_top: 0,
            quip_fields: [None; 3],
            quip_preview: quips::PreviewState::default(),
            mark: None,
            cards: vec![],
            retry: vec![],
            clip: Rect::new(0.0, 0.0, 0.0, 0.0),
            popup: false,
            runs: vec![],
            sel: None,
            sel_text: BTreeMap::new(),
            box_origin: None,
            text_area: Rect::new(0.0, 0.0, 0.0, 0.0),
            drawn: None,
            opened: vec![],
            lens: None,
            saved: None,
            made: None,
            met: None,
            shown_lens: None,
            dock_last: None,
            arriving: vec![],
            box_full: 0.0,
        }
    }
}

struct Cached {
    item: Arc<Item>,
    /// a reply's blocks, parsed once
    blocks: Option<Arc<Vec<Block>>>,
    /// (width, body size, open, narration) → height
    h: Option<(f32, f32, bool, bool, f32)>,
}

#[derive(Clone, Copy)]
struct BoxGeo {
    origin: Point,
    adv: f32,
    cols: usize,
    row_h: f32,
    first: usize,
}

/// Sizes and colours for one frame.
#[derive(Clone)]
struct Typo {
    body: f32,
    lh: f32,
    small: f32,
    adv: f32,
    text: Color,
    narration: Color,
    muted: Color,
    faint: Color,
    code: Color,
    link: Color,
    well: Color,
    bubble: Color,
    /// the agent waits on you (its running step is waiting for your answer)
    waiting: bool,
    /// the agent is working (a step still "running" in a session that isn't was left behind: no clock for it)
    working: bool,
    /// now, as transcripts stamp time
    now_ms: i64,
    /// a subagent's own conversation: what asks it things is this agent, not you ("Claude")
    asker: Option<&'static str>,
}

impl Typo {
    fn new(ui: &mut Ui, body: f32) -> Typo {
        let t = ui.t;
        Typo {
            body,
            lh: (body * LINE).round(),
            small: (body * 0.82).max(size::SMALL),
            adv: advance(ui, body),
            text: t.text,
            narration: t.text.mix(t.muted, 0.55),
            muted: t.muted,
            faint: t.faint,
            code: t.role("codeForeground", t.accent.mix(t.text, 0.42)),
            link: t.accent.mix(t.text, 0.2),
            well: t.role("codeBackground", if t.light { t.text.with_alpha(0.045) } else { Color::BLACK.with_alpha(0.22) }),
            bubble: t.role("messageSurface", t.text.with_alpha(if t.light { 0.06 } else { 0.07 })),
            waiting: false,
            working: false,
            now_ms: crate::util::now_ms(),
            asker: None,
        }
    }
}

/// One cell of the terminal font at `size` (the reply box wraps by cells).
fn advance(ui: &mut Ui, size: f32) -> f32 {
    const PROBE: &str = "0000000000000000000000000000000000000000";
    (ui.measure(PROBE, Style::mono(size, ui.t.text)) / PROBE.len() as f32).max(1.0)
}

/// Styled runs wrapped at `w`, `lh` the line height as a multiple of `size`.
fn lay(ui: &mut Ui, spans: &[(String, Style)], size: f32, lh: f32, w: f32, max_lines: Option<usize>) -> TextBox {
    let mut o = TextOpts::new(size * ui.scale);
    o.line_height = (size * lh * ui.scale).round();
    o.max_width = Some((w * ui.scale).max(1.0));
    o.wrap = true;
    o.ellipsis = max_lines.is_some();
    o.max_lines = max_lines;
    let spans: Vec<GSpan> = spans.iter().map(|(s, st)| GSpan { text: s.clone(), face: st.face, weight: st.weight, italic: st.italic, color: st.color, size: (st.size != size).then_some(st.size * ui.scale) }).collect();
    let layout = ui.text.layout(&spans, &o);
    TextBox { w: layout.width() / ui.scale, h: layout.height() / ui.scale, layout }
}

fn agent_name(h: Option<Harness>) -> &'static str {
    match h {
        Some(Harness::Codex) => "Codex",
        Some(Harness::Pi) => "Pi",
        Some(Harness::Omp) => "omp",
        _ => "Claude",
    }
}

/// "42s", "1m 05s", "1h 02m".
fn clock(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

// ------------------------------------------------------------------------------------------- what's going on

/// What a frame of a session's focus view needs from the workspace.
struct Seen {
    harness: Option<Harness>,
    conv: Option<Arc<Conversation>>,
    activity: Option<Activity>,
    queued_questions: Option<bro_core::terminal_prompt::QueuedQuestions>,
    limit: Option<bro_core::sessions::limits::UsageLimit>,
    done: bool,
    /// the step running now, if the transcript says
    running: Option<Step>,
    /// the newest transcript record, ms
    last_ms: Option<i64>,
    /// where it works (the transcript's word, else where it was started) and its git branch
    cwd: PathBuf,
    branch: Option<String>,
    thought: Option<Thought>,
    /// a message you sent from here isn't in the transcript yet (its newest line is from before it)
    sending: bool,
    /// the subagents it started, as its worker follows them
    subagents: Option<Arc<Vec<Arc<Subagent>>>>,
    /// the background tasks at work now, the oldest first
    tasks: Vec<Task>,
}

impl Seen {
    /// How many of its subagents are at work.
    fn subagents_working(&self) -> usize {
        self.subagents.as_ref().map_or(0, |l| l.iter().filter(|s| s.state.running()).count())
    }
}

fn seen(app: &App, id: PaneId) -> Option<Seen> {
    let p = app.panes.get(&id)?;
    let term = p.as_term_ref()?;
    let snap = term.insights();
    let conv = snap.as_ref().and_then(|s| s.conversation.clone());
    let facts = snap.as_ref().and_then(|s| s.facts.clone());
    let tasks = term.background_tasks(snap.as_ref());
    Some(Seen {
        harness: term.meta.harness,
        activity: p.activity(),
        queued_questions: term.focus_queued_questions(),
        limit: term.limit_warning(),
        done: app.done.contains(&id),
        running: conv.as_ref().and_then(|c| c.running().cloned()),
        last_ms: facts.as_ref().and_then(|f| f.last_activity_ms),
        cwd: conv.as_ref().and_then(|c| c.cwd.clone()).map(PathBuf::from).unwrap_or_else(|| term.meta.cwd.clone()),
        branch: conv.as_ref().and_then(|c| c.branch.clone()).or_else(|| facts.as_ref().and_then(|f| f.branch.clone())),
        thought: conv.as_ref().and_then(|c| c.thought.clone()),
        sending: app.focus.drafts.get(&id).is_some_and(|d| d.sent.iter().any(|s| !matches!(s.delivery.state(), crate::panes::term::DeliveryState::Failed(_)))),
        subagents: snap.as_ref().and_then(|s| s.subagents.clone()),
        tasks,
        conv,
    })
}

/// The mood the session is in, `prev` the last frame's: a turn that just ended reads as done a while, watched or not.
fn mood_of(seen: &Seen, prev: Option<(Mood, Instant)>, now: Instant) -> Mood {
    if seen.limit.is_some() { return Mood::Blocked; }
    let finished = prev.is_some_and(|(m, at)| m == Mood::Done && now.saturating_duration_since(at).as_secs_f32() < DONE_SHOWS);
    match seen.activity {
        Some(Activity::Working) => Mood::Working,
        Some(Activity::Blocked) => Mood::Blocked,
        Some(Activity::Idle) if seen.done || finished || prev.is_some_and(|(m, _)| m == Mood::Working) => Mood::Done,
        _ => Mood::Idle,
    }
}

/// How the agent's thinking is going, in words: its own heading for it when it gave one (Codex), else how long
/// it's been at it ("Thinking", "Still thinking", "Deep in thought"), with the time once it's been a while.
/// `working` is how long it has been working (seconds), for when the transcript hasn't caught up.
fn thinking(seen: &Seen, now_ms: i64, working: i64) -> String {
    if let Some(limit) = &seen.limit { return limit.title.clone(); }
    // since the transcript's newest line, when it has this turn (else since it was seen to start)
    let secs = match seen.last_ms {
        Some(t) if !seen.sending => ((now_ms - t) / 1000).max(0),
        _ => working.max(0),
    };
    // its heading, while it's still the newest thing it did
    let heading = seen.thought.as_ref().filter(|t| t.at_ms.zip(seen.last_ms).is_none_or(|(a, l)| a + 1000 >= l)).map(|t| t.text.clone());
    let words = heading.unwrap_or_else(|| {
        match secs {
            0..=11 => "Thinking",
            12..=44 => "Still thinking",
            _ => "Deep in thought",
        }
        .to_string()
    });
    if secs >= 12 { format!("{words} · {}", clock(secs)) } else { words }
}

/// What it's doing right now, in words: the step it's running (and for how long), else how its thinking is going.
fn doing(seen: &Seen, now_ms: i64, working: i64) -> String {
    if let Some(limit) = &seen.limit { return limit.title.clone(); }
    match &seen.running {
        Some(s) => {
            let secs = s.at_ms.map_or(0, |t| (now_ms - t) / 1000);
            if secs >= 5 { format!("{} · {}", s.label, clock(secs)) } else { s.label.clone() }
        }
        None => thinking(seen, now_ms, working),
    }
}

fn working_secs(prev: Option<(Mood, Instant)>, now: Instant) -> i64 {
    prev.filter(|(m, _)| *m == Mood::Working).map_or(0, |(_, at)| now.saturating_duration_since(at).as_secs() as i64)
}

/// What a focus card's header says the agent is doing, its right edge at `right` (centred on its y): working and on
/// what, or how its thinking is going; waiting on you; done. Nothing while it's ready. Returns the width it took.
pub fn header_status(app: &App, ui: &mut Ui, id: PaneId, view: Option<&View>, right: Point, room: f32) -> Option<f32> {
    let seen = seen(app, id)?;
    let prev = view.and_then(|v| v.mood);
    let mood = mood_of(&seen, prev, ui.now);
    let t = ui.t;
    // its turn is over but its subagents or background tasks work on: that's what it's doing
    let crew = seen.subagents_working();
    let busy = crew > 0 || !seen.tasks.is_empty();
    let mood = if busy && matches!(mood, Mood::Idle | Mood::Done) { Mood::Working } else { mood };
    let (words, ink) = match mood {
        Mood::Working if busy && seen.activity != Some(Activity::Working) => (subagents::working(crew, &seen.tasks, crate::util::now_ms()), t.text.mix(t.muted, 0.35)),
        Mood::Working => (doing(&seen, crate::util::now_ms(), working_secs(prev, ui.now)), t.text.mix(t.muted, 0.35)),
        Mood::Blocked => (seen.limit.as_ref().map_or_else(|| seen.queued_questions.map_or_else(|| "Needs you".to_string(), |q| queued_question_label(q.count)), |limit| limit.title.clone()), t.danger),
        Mood::Done => ("Done".to_string(), t.good),
        Mood::Idle => return None,
    };
    let st = Style::new(size::SMALL, weight::MEDIUM, ink);
    let b = ui.line(&words, st, Some((room - 22.0).max(40.0)));
    let w = 22.0 + b.w;
    let done_for = ui.since_on(("focus-done", id), mood == Mood::Done);
    let x = right.x - w;
    let mid = right.y;
    match mood {
        Mood::Working => ui.spinner(Point::new(x + 7.0, mid), 4.6, 1.6, t.accent),
        Mood::Blocked => {
            ui.ticking |= seen.limit.is_none();
            let k = if seen.limit.is_some() { 1.0 } else { 0.7 + 0.3 * ((ui.time * 4.0).sin() as f32 * 0.5 + 0.5) };
            ui.icon(Rect::new(x, mid - 8.0, 14.0, 16.0), "\u{F0026}", 13.0, t.danger.fade(k));
        }
        Mood::Done => {
            if !seen.done && let Some((Mood::Done, since)) = prev {
                let expiry = since + std::time::Duration::from_secs_f32(DONE_SHOWS);
                if expiry > ui.now { ui.wake_after(expiry - ui.now); }
            }
            // it pops in when the turn ends
            let pop = match done_for {
                Some(s) if s < 1.6 => {
                    ui.anim.moving = true;
                    let p = (s / 0.7).min(1.0);
                    ui.ring(Point::new(x + 7.0, mid), 4.0 + 11.0 * p, 1.6 * (1.0 - p) + 0.4, t.good.fade(0.6 * (1.0 - p)));
                    super::kit::motion::Spring::POP.step(s)
                }
                _ => 1.0,
            };
            ui.icon(Rect::new(x, mid - 8.0, 14.0, 16.0), "\u{F012C}", (14.0 * pop).max(1.0), t.good)
        }
        Mood::Idle => {}
    }
    if mood == Mood::Working {
        // a band of light sweeping over what it's doing (Rune's shimmer over a streaming reply)
        let glint = if t.light { t.accent } else { t.text };
        ui.shimmer(Point::new(x + 22.0, mid - b.h / 2.0), &words, st, glint, Some((room - 22.0).max(40.0)));
    } else {
        ui.draw(&b, Point::new(x + 22.0, mid - b.h / 2.0));
    }
    if ui.hovered(Rect::new(x, mid - 12.0, w, 24.0)) && b.layout.truncated {
        widgets::tooltip(ui, Rect::new(x, mid - 12.0, w, 24.0), &words);
    }
    Some(w)
}

// ------------------------------------------------------------------------------------------------- the view

/// Draw `id`'s conversation in `content` (the card under its header).
#[allow(clippy::too_many_arguments)]
pub fn draw(app: &mut App, ui: &mut Ui, id: PaneId, content: Rect, focused: bool, session_location: Option<Rect>, v: &mut View, media: &mut Media, apps: &mut Apps) {
    let Some(seen) = seen(app, id) else { return };
    super::backdrop::aurora(ui,content);
    v.quip_preview.hovered = None;
    // the choice on screen: read when the screen is free, kept while it waits on you
    if seen.limit.is_none() && seen.activity == Some(Activity::Blocked) {
        v.choice = app.focus_choice(id);
    } else {
        v.choice = None;
    }
    // what you sent that the transcript now has goes; the rest shows as sent
    if let Some(conv) = &seen.conv {
        let items = &conv.items;
        if let Some(d) = app.focus.drafts.get_mut(&id) {
            d.reconcile(|s| items.iter().skip(s.after.min(items.len())).any(|it| matches!(it.as_ref(), Item::You { text, .. } if holds(text, &s.text))));
        }
    }
    let mood = mood_of(&seen, v.mood, ui.now);
    if v.mood.is_none_or(|(m, _)| m != mood) {
        v.mood = Some((mood, ui.now));
    }
    v.links.clear();
    v.pictures.clear();
    v.caret = None;
    // the keys are here only while nothing else has them
    let focused = focused && !app.side_focus && !app.overlay.is_open() && app.renaming.is_none() && !app.prefix_armed
        && app.theme_editor.as_ref().is_none_or(|e|e.minimized || e.field==crate::theme::editor::Field::None);
    ui.hit_with(content, Action::Focus(id, AREA), Cursor::Default);
    // a comfortable measure, centred: big type, never a wall of it
    let avail = (content.w - 48.0).max(120.0);
    let body = (avail / 44.0).clamp(14.0, 19.0);
    let mut c = Typo::new(ui, body);
    c.waiting = seen.limit.is_none() && mood == Mood::Blocked;
    c.working = mood == Mood::Working;
    v.line_height = c.lh;
    let measure = avail.min((c.adv * 76.0).round());
    let mx = content.x + (content.w - measure) / 2.0;
    // its subagents, and the one shown instead of the conversation (one no longer known goes back to it)
    let last = v.lens.as_ref().and(v.drawn.clone());
    let subs = Subs::new(seen.subagents.as_ref(), &[seen.conv.as_deref(), last.as_deref()]);
    let lens = app.focus.subagent.get(&id).and_then(|call| subs.get(call).cloned());
    if lens.is_none() {
        app.focus.subagent.remove(&id);
    }
    v.look_at(lens.as_ref().map(|s| s.call.as_str()));
    // in a subagent's conversation, what asks it things is the session's agent, and its turn is its own
    let mut lc = c.clone();
    if let Some(s) = &lens {
        lc.working = s.state.running();
        lc.waiting = c.waiting && s.state.running();
        lc.asker = Some(agent_name(seen.harness));
    }
    // the reply box at the foot, the choice it waits on over it, the subagents at work over that, the
    // conversation over them all
    let full_h = reply_height(app, id, &c, measure);
    // the box grows and shrinks as lines come and go, what's over it riding along
    let box_h = ui.spring(("focus-box-h", id), full_h, arrive::GROW, full_h);
    v.box_full = full_h;
    let box_r = Rect::new(mx, content.bottom() - 18.0 - box_h, measure, box_h);
    let queued = seen.queued_questions.filter(|_| seen.limit.is_none() && v.choice.is_none());
    // a browser page the agent handed you sits where a question would
    let handoff = app.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.browser_handoff.clone())
        .filter(|_| seen.limit.is_none() && v.choice.is_none() && queued.is_none());
    let choice_h = v.choice.as_ref().map(|ch| choice_height(ui, &c, ch, measure)).unwrap_or(if queued.is_some() {
        80.0
    } else if let Some(h) = &handoff {
        super::browser::focus_card_height(ui, c.small, c.body, h, measure)
    } else {
        0.0
    });
    let choice_r = Rect::new(mx, box_r.y - 12.0 - choice_h, measure, choice_h);
    let bottom = if choice_h > 0.0 { choice_r.y } else { box_r.y };
    let running = subs.running();
    let at_work = !running.is_empty() || !seen.tasks.is_empty();
    let dock_h = ui.ease(("focus-dock-h", id), if at_work { subagents::DOCK_H + 10.0 } else { 0.0 }, 160.0);
    let dock_r = Rect::new(mx, bottom - dock_h, measure, subagents::DOCK_H);
    let bottom = bottom - dock_h;
    // a little room under the header: the text scrolls away under a fade, not against its line
    let area = Rect::new(content.x, content.y + 6.0, content.w, (bottom - 8.0 - content.y - 6.0).max(0.0));
    conversation(app, ui, id, area, Rect::new(mx, area.y, measure, area.h), if lens.is_some() { &lc } else { &c }, &seen, v, mood, media, apps, &subs, lens.as_ref(), session_location.is_some());
    // the strip keeps what it last held while it fades away
    if at_work {
        v.dock_last = Some((running.clone(), seen.tasks.clone()));
    } else if dock_h <= 4.0 {
        v.dock_last = None;
    }
    let (strip_subs, strip_tasks) = v.dock_last.clone().unwrap_or_default();
    if dock_h > 4.0 && (!strip_subs.is_empty() || !strip_tasks.is_empty()) {
        let band = Rect::new(content.x, dock_r.y, content.w, dock_h);
        ui.clip(band, 0.0);
        let tip = subagents::dock(ui, &c, id, &strip_subs, &strip_tasks, dock_r, lens.as_ref().map(|s| s.call.as_str()), &mut v.opened, &mut v.links);
        ui.veil(band, 0.0, dock_h / (subagents::DOCK_H + 10.0));
        ui.unclip();
        // above the chip, kept inside the card (its edges clip)
        if let Some((anchor, text)) = tip {
            let screen = ui.screen;
            ui.screen = content;
            widgets::tooltip(ui, anchor, &text);
            ui.screen = screen;
        }
    }
    if let Some(ch) = v.choice.clone() {
        draw_choice(ui, id, &c, &ch, choice_r);
    } else if let Some(queued) = queued {
        draw_queued_questions(ui, id, &c, queued, choice_r);
    } else if let Some(h) = &handoff {
        super::browser::draw_focus_card(ui, c.small, c.body, h, choice_r);
    }
    reply_box(app, ui, id, box_r, focused, &c, &seen, mood, v, media);
    v.popup |= v.quip_preview.draw(ui, id);
    // the apps' pages go over their cards once the frame is drawn, unless a list over the box covers them
    let cards = std::mem::take(&mut v.cards);
    let sid = app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone());
    if let Some(sid) = sid.filter(|_| !v.popup) {
        let fill = card_fill(ui);
        for (call, a, rect) in cards {
            apps.want(Want { key: (id, call), sid: sid.clone(), app: a, rect, clip: v.clip, fill, max_h: APP_MAX });
        }
    }
    // Header links share the conversation's targets, which are rebuilt while it draws.
    if let Some(r) = session_location {
        let tokens = ui.t.toolbar();
        ui.with_tokens(&tokens, |ui| {
            ui.screen = Rect { y: r.y, h: content.bottom() - r.y, ..content };
            draw_session_location(app, ui, id, r, v, &seen);
        });
    }
}

// --------------------------------------------------------------------------------------- links and pictures

/// The session's file lookups: a path the transcript names → its link, once it's known to be there.
type Resolve<'a> = &'a mut dyn FnMut(&FileLinkMatch) -> Option<Arc<str>>;

/// What drawing an item needs past the item itself: whether to draw at all (measuring only), the session's file
/// lookups for its links, the thumbnails for its pictures, and what it made clickable.
struct Paint<'a> {
    draw: bool,
    id: PaneId,
    links: Vec<String>,
    pictures: Vec<PathBuf>,
    sounds: Vec<(PathBuf, Rect)>,
    media: Option<&'a mut Media>,
    resolve: Option<Resolve<'a>>,
    apps: AppPaint<'a>,
    /// the text drawn can be selected (the conversation's items, not the header or what's on its way)
    text: TextPaint,
    /// the subagents the frame knows (the steps that started them get cards), and those a click opens
    subs: Option<&'a Subs>,
    opened: Vec<String>,
    quip_hover: Option<quips::Preview>,
}

/// The conversation's selectable text as it's drawn: which item, the next piece's place in it and what goes
/// before it when copied, the pieces drawn, and the selection to show over them.
#[derive(Default)]
struct TextPaint {
    collect: bool,
    item: usize,
    seq: u32,
    gap: &'static str,
    lead: String,
    runs: Vec<Run>,
    sel: Option<(Pos, Pos)>,
}

impl TextPaint {
    /// The next item's pieces start.
    fn item(&mut self, i: usize) {
        self.item = i;
        self.seq = 0;
        self.gap = "\n\n";
        self.lead.clear();
    }
}

/// The bytes of a run with `key` (`len` long) that the selection `(start, end)` covers. An empty run inside
/// the selection is covered (it shows a sliver: a blank line selected).
fn covered(sel: (Pos, Pos), key: RunKey, len: usize) -> Option<Range<usize>> {
    let (s, e) = sel;
    if key < s.key || key > e.key {
        return None;
    }
    let start = if key == s.key { s.byte.min(len) } else { 0 };
    let end = if key == e.key { e.byte.min(len) } else { len };
    (start < end || (len == 0 && s.key < key && key < e.key)).then_some(start..end)
}

/// x of the caret before byte `at` on `line` (physical px from the layout's left).
fn caret_in(layout: &bro_gfx::text::Layout, line: &bro_gfx::text::LLine, at: usize) -> f32 {
    let glyphs = &layout.glyphs[line.glyphs.clone()];
    for g in glyphs {
        if at <= g.start {
            return g.x;
        }
        if at < g.end {
            return g.x + g.w * (at - g.start) as f32 / (g.end - g.start).max(1) as f32;
        }
    }
    glyphs.last().map_or(0.0, |g| g.x + g.w)
}

/// The rectangles a selected byte range of a layout covers, a line each (logical px, from `at`). `on` says the
/// selection goes on past the range's end: each line it runs off gets a sliver for its line break.
fn selection_rects(layout: &bro_gfx::text::Layout, at: Point, scale: f32, range: &Range<usize>, on: bool) -> Vec<Rect> {
    let mut out = vec![];
    let sliver = (layout.lines.first().map_or(16.0, |l| l.height) * 0.3).max(3.0);
    let n = layout.lines.len();
    for (i, line) in layout.lines.iter().enumerate() {
        let lt = &line.text;
        let a = range.start.max(lt.start);
        let b = range.end.min(lt.end);
        // runs off this line: on into the next line, or past the run's end
        let off = range.end > lt.end || (on && i + 1 == n && range.end >= lt.end);
        if a > b {
            continue;
        }
        // none of this line's text: only a line break the selection runs over shows (not the end of the line
        // before the one it starts on)
        if a == b && (!off || (range.start >= lt.end && i + 1 < n)) {
            continue;
        }
        let x0 = caret_in(layout, line, a);
        let mut x1 = caret_in(layout, line, b);
        if off {
            x1 += sliver;
        }
        out.push(Rect::new(at.x + x0 / scale, at.y + line.top / scale, ((x1 - x0) / scale).max(1.0), line.height / scale));
    }
    if out.is_empty() && n == 0 && range.start == range.end && on {
        // a blank piece inside the selection
        out.push(Rect::new(at.x, at.y, sliver / scale, (layout.height() / scale).max(4.0)));
    }
    out
}

/// The MCP apps' cards an item draws: their pages and heights by call (none: no cards), and what it drew (the
/// cards whose pages go over them, the failed ones a click reads again).
#[derive(Default)]
struct AppPaint<'a> {
    cards: Option<&'a HashMap<String, Card>>,
    drawn: Vec<(String, Arc<McpApp>, Rect)>,
    retry: Vec<Arc<McpApp>>,
    /// the item being drawn is a card whose steps are opening or closing: how far open (None: as `open` says)
    reveal: Option<f32>,
}

/// An MCP app's card this frame: its page as far as it's read, how tall the page is and whether it has said so,
/// and what its server is called.
struct Card {
    page: Page,
    h: f32,
    sized: bool,
    name: String,
}

impl Paint<'_> {
    fn measuring<'b>(id: PaneId) -> Paint<'b> {
        Paint { draw: false, id, links: vec![], pictures: vec![], sounds: vec![], media: None, resolve: None, apps: AppPaint::default(), text: TextPaint::default(), subs: None, opened: vec![], quip_hover: None }
    }

    /// Laid-out text about to be drawn at `at`: the selection goes under it, and a drag can land on it.
    fn run(&mut self, ui: &mut Ui, b: &TextBox, at: Point) {
        let tp = &mut self.text;
        if !self.draw || !tp.collect {
            return;
        }
        let key = (tp.item, tp.seq);
        tp.seq += 1;
        // the pieces after the first in a block (a code block's lines, a table's rows) follow on the next line
        let gap = std::mem::replace(&mut tp.gap, "\n");
        let lead = std::mem::take(&mut tp.lead);
        if let Some(sel) = tp.sel
            && let Some(range) = covered(sel, key, b.layout.text.len())
        {
            let on = sel.1.key > key;
            for r in selection_rects(&b.layout, at, ui.scale, &range, on) {
                ui.round(r, 2.0, ui.t.selection);
            }
        }
        tp.runs.push(Run { key, at, layout: b.layout.clone(), scale: ui.scale, gap, lead });
    }

    fn resolve(&mut self, m: &FileLinkMatch) -> Option<Arc<str>> {
        self.resolve.as_mut().and_then(|r| r(m))
    }
}

fn is_web(target: &str) -> bool {
    let t = target.trim_start().to_ascii_lowercase();
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("mailto:")
}

/// Where the links in `spans` (laid out as one text) are and where they go: Markdown links and web addresses as
/// written, and the file paths the session's lookups find (as the terminal links them).
fn link_ranges(spans: &[Span], p: &mut Paint) -> Vec<(Range<usize>, String)> {
    let mut out: Vec<(Range<usize>, String)> = vec![];
    let mut text = String::new();
    for s in spans {
        let r = text.len()..text.len() + s.text.len();
        if let Some(target) = &s.link {
            if is_web(target) || target.starts_with("file:") {
                out.push((r.clone(), target.clone()));
            } else {
                // a link to a file the session knows
                let (path, line, column) = split_location(target.trim());
                let m = FileLinkMatch { start: r.start, end: r.end, path, line, column, label: s.text.clone() };
                if let Some(uri) = p.resolve(&m) {
                    out.push((r.clone(), uri.to_string()));
                }
            }
        }
        text.push_str(&s.text);
    }
    for m in find_file_links(&text) {
        if is_web(&m.path) || out.iter().any(|(r, _)| m.start < r.end && m.end > r.start) {
            continue;
        }
        if let Some(uri) = p.resolve(&m) {
            out.push((m.start..m.end, uri.to_string()));
        }
    }
    out
}

/// The rectangles a byte range of laid-out text covers, a line each (logical px, from the text's top-left `at`).
fn range_rects(b: &TextBox, at: Point, range: &Range<usize>, scale: f32) -> Vec<Rect> {
    let mut out = vec![];
    for line in &b.layout.lines {
        let glyphs: Vec<_> = b.layout.glyphs[line.glyphs.clone()].iter().filter(|g| g.start >= range.start && g.end <= range.end).collect();
        let (Some(first), Some(last)) = (glyphs.first(), glyphs.last()) else { continue };
        let x0 = at.x + first.x / scale;
        let x1 = at.x + (last.x + last.w) / scale;
        out.push(Rect::new(x0, at.y + line.top / scale, (x1 - x0).max(1.0), line.height / scale));
    }
    out
}

/// A picture's thumbnail in `r` (covering it), clickable to show it in the media panel.
fn picture(ui: &mut Ui, p: &mut Paint, path: &Path, r: Rect, rad: f32) {
    if !p.draw {
        return;
    }
    let t = ui.t;
    ui.round(r, rad, t.text.with_alpha(0.06));
    match p.media.as_deref_mut().map(|m| m.thumb(path)) {
        Some(Thumbnail::Ready(tex, w, h)) => {
            let (iw, ih) = (w.max(1) as f32, h.max(1) as f32);
            let (ra, ia) = (r.w / r.h.max(1.0), iw / ih);
            let uv = if ia > ra {
                let k = ra / ia;
                [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, 1.0]
            } else {
                let k = ia / ra;
                [0.0, (1.0 - k) / 2.0, 1.0, (1.0 + k) / 2.0]
            };
            ui.image(tex, r, uv, rad, 1.0);
        }
        Some(Thumbnail::Loading) => ui.spinner(r.center(), 5.0, 1.8, t.faint),
        _ => ui.icon(r, "\u{F02E9}", 20.0, t.faint),
    }
    let hot = ui.hover_amount(("focus-pic", p.id, path), r);
    if hot > 0.01 {
        ui.outline_over(r, rad, 1.0, t.text.with_alpha(0.35 * hot));
    }
    ui.hit(r, Action::Focus(p.id, PICTURE + p.pictures.len()));
    p.pictures.push(path.to_path_buf());
}

/// The pictures a run of steps showed you or looked at (sent, read, viewed, made), by their paths.
fn step_pictures(steps: &[Step]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for s in steps {
        if !matches!(s.kind, StepKind::Read | StepKind::Send | StepKind::Create | StepKind::Edit | StepKind::Other) || s.state == StepState::Failed {
            continue;
        }
        let Some(path) = s.detail.as_deref().and_then(|d| d.lines().next()).map(|d| PathBuf::from(d.trim().trim_start_matches("file:///"))) else { continue };
        if path.is_absolute() && crate::focus::is_picture(&path) && !out.contains(&path) {
            out.push(path);
        }
    }
    out.truncate(6);
    out
}

// ------------------------------------------------------------------------------------------ the conversation

/// A subagent's own conversation, as drawn: what its transcript says, or (when it left none bro can read) its brief
/// and its report, made once.
fn lens_conversation(v: &mut View, s: &Subagent, harness: Option<Harness>) -> Arc<Conversation> {
    if let Some(conv) = &s.conversation {
        return conv.clone();
    }
    let size = s.brief.as_ref().map_or(0, String::len) * 31 + s.result.as_ref().map_or(0, String::len);
    if let Some((call, n, conv)) = &v.made
        && *call == s.call
        && *n == size
    {
        return conv.clone();
    }
    let mut conv = Conversation::subagent(harness.unwrap_or(Harness::Claude));
    if let Some(brief) = &s.brief {
        conv.items.push(Arc::new(Item::You { text: brief.clone(), images: 0, pictures: vec![], command: false, at_ms: s.started_ms }));
    }
    if let Some(result) = s.result.as_ref().filter(|_| !s.state.running()) {
        conv.items.push(Arc::new(Item::Said { text: result.clone(), at_ms: s.ended_ms }));
    }
    let conv = Arc::new(conv);
    v.made = Some((s.call.clone(), size, conv.clone()));
    conv
}

/// Keep parsed items across frames and amend the last affected turn when output arrives.
fn update_history(v: &mut View, shown: &Option<Arc<Conversation>>, skip: usize, switched: bool, media: &Media, in_lens: bool) {
    let items = shown.as_ref().map(|c| c.items.as_slice()).unwrap_or_default();
    let changed = match (&v.drawn, shown) {
        (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
        (None, None) => false,
        _ => true,
    };
    if changed || switched || v.cached_skip != skip {
        let mut cache: HashMap<usize, Cached> = HashMap::with_capacity(items.len() - skip);
        for it in &items[skip..] {
            let k = Arc::as_ptr(it) as usize;
            let e = v.cache.remove(&k).filter(|e| Arc::ptr_eq(&e.item, it)).unwrap_or_else(|| Cached {
                item: it.clone(),
                blocks: match it.as_ref() {
                    Item::Said { text, .. } => Some(Arc::new(markdown::parse(text))),
                    _ => None,
                },
                h: None,
            });
            cache.insert(k, e);
        }
        v.cache = cache;
        v.open.retain(|k| v.cache.contains_key(k));
        v.cached_skip = skip;
    }
    if changed || switched || v.gallery_revision != media.gallery_revision {
        // A later tool can turn earlier speech within its turn into narration.
        // Shared item identities identify the untouched prefix, including across appends.
        let previous = v.drawn.as_ref().map(|c| c.items.as_slice()).unwrap_or_default();
        let shared = if switched || v.gallery_revision != media.gallery_revision { 0 } else {
            items.iter().zip(previous).take_while(|(new, old)| Arc::ptr_eq(new, old)).count()
        };
        let start = items[..shared].iter().rposition(|it| matches!(it.as_ref(), Item::You { .. })).unwrap_or(0);
        if v.gallery_revision != media.gallery_revision {
            for entry in v.cache.values_mut() {
                if matches!(entry.item.as_ref(), Item::You { .. }) { entry.h = None; }
            }
        }
        v.narration.resize(items.len(), false);
        let mut has_step = false;
        for i in (start..items.len()).rev() {
            v.narration[i] = has_step && matches!(items[i].as_ref(), Item::Said { .. });
            match items[i].as_ref() {
                Item::Did { .. } => has_step = true,
                Item::You { .. } => has_step = false,
                _ => {}
            }
        }
        let mut prev_you = items[..start].iter().rev().find_map(|it| match it.as_ref() {
            Item::You { at_ms, .. } => Some(*at_ms),
            _ => None,
        }).flatten();
        v.message_pictures.truncate(start);
        let pastes: Vec<(i64, PathBuf)> = media
            .items
            .iter()
            // a subagent's brief came from its agent, not from your pastes
            .filter(|it| it.source == super::media::Source::Pasted && !in_lens)
            .map(|it| (it.at.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64), it.path.clone()))
            .collect();
        v.message_pictures.extend(items[start..]
            .iter()
            .map(|it| match it.as_ref() {
                Item::You { images, pictures, at_ms, .. } => {
                    let mut out: Vec<PathBuf> = pictures.iter().map(PathBuf::from).filter(|p| p.is_absolute()).collect();
                    if *images > out.len()
                        && let Some(at) = *at_ms
                    {
                        let mut found: Vec<&(i64, PathBuf)> = pastes.iter().filter(|(ts, _)| *ts <= at + 3000 && prev_you.is_none_or(|p| *ts > p)).collect();
                        found.sort_by_key(|(ts, _)| *ts);
                        let need = *images - out.len();
                        out.extend(found.iter().rev().take(need).rev().map(|(_, p)| p.clone()));
                    }
                    prev_you = *at_ms;
                    out
                }
                _ => vec![],
            }));
        v.gallery_revision = media.gallery_revision;
    }
    v.drawn = shown.clone();
}

#[cfg(test)]
#[test]
fn history_cache_follows_edits_and_turn_boundaries() {
    let mut view = View::default();
    let gallery = Media::new(Arc::new(|| {}));
    let said = Arc::new(Item::Said { text: "checking".into(), at_ms: None });
    let you = Arc::new(Item::You { text: "hi".into(), images: 0, pictures: vec![], command: false, at_ms: None });
    let did = Arc::new(Item::Did { steps: vec![] });
    let mut history = Arc::new(Conversation::subagent(Harness::Codex));
    Arc::make_mut(&mut history).items = vec![you.clone(), said.clone()];
    update_history(&mut view, &Some(history.clone()), 0, true, &gallery, false);
    assert_eq!(view.narration, [false, false]);
    Arc::make_mut(&mut history).items.extend([did.clone(), you, said]);
    update_history(&mut view, &Some(history.clone()), 1, false, &gallery, false);
    assert_eq!(view.narration, [false, true, false, false, false]);
    assert_eq!(view.message_pictures.len(), 5);
    Arc::make_mut(&mut history).items[2] = Arc::new(Item::Said { text: "finished".into(), at_ms: None });
    update_history(&mut view, &Some(history), 0, false, &gallery, false);
    assert_eq!(view.narration, [false; 5]);
    update_history(&mut view, &None, 0, true, &gallery, true);
    assert!(view.cache.is_empty() && view.message_pictures.is_empty());
}

/// The conversation, scrolled, in `area` (`col` the text column inside it): the session's, or the subagent's in
/// `lens` (its own head, its own turn, and none of what you sent).
#[allow(clippy::too_many_arguments)]
fn conversation(app: &mut App, ui: &mut Ui, id: PaneId, area: Rect, col: Rect, c: &Typo, seen: &Seen, v: &mut View, mood: Mood, media: &mut Media, apps: &mut Apps, subs: &Subs, lens: Option<&Arc<Subagent>>, header_has_location: bool) {
    let t = ui.t;
    let shown: Option<Arc<Conversation>> = match lens {
        Some(s) => Some(lens_conversation(v, s, seen.harness)),
        None => seen.conv.clone(),
    };
    let items = shown.as_ref().map(|c| c.items.as_slice()).unwrap_or_default();
    // going into a subagent's conversation or back: what shows is new, so it fades in (the first frame is just there)
    let lens_name = lens.map(|s| s.call.as_str());
    let switched = v.shown_lens.as_ref().is_none_or(|l| l.as_deref() != lens_name);
    if switched {
        if v.shown_lens.is_some() {
            ui.spring_set(("focus-lens-in", id), 0.0);
        }
        v.shown_lens = Some(lens_name.map(str::to_string));
        v.met = None;
        v.arriving.clear();
    }
    let lens_in = ui.spring(("focus-lens-in", id), 1.0, Spring::ARRIVE, 1.0);
    let sent: Vec<(String, crate::panes::term::DeliveryState, bool, Vec<PathBuf>, usize)> = app
        .focus
        .drafts
        .get(&id)
        .filter(|_| lens.is_none())
        .map(|d| d.sent.iter().map(|s| (s.text.clone(), s.delivery.state(), s.queued, s.pictures.clone(), s.after)).collect())
        .unwrap_or_default();
    // the latest items show; earlier ones wait behind a button (a long session stays quick to lay out)
    let skip = items.len().saturating_sub(v.shown.max(HISTORY));
    update_history(v, &shown, skip, switched, media, lens.is_some());
    let narration = &v.narration;
    let pics = &v.message_pictures;
    // the MCP apps' cards: their pages as far as they're read (the first look starts reading one), and how tall
    // each page says it is
    let sid = app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap_or_default();
    let mut cards: HashMap<String, Card> = HashMap::new();
    for it in &items[skip..] {
        let Item::Did { steps } = it.as_ref() else { continue };
        for s in steps {
            let Some(a) = &s.app else { continue };
            let page = match apps.unavailable() {
                Some(e) => Page::Failed(e.to_string()),
                // no web view on this system: the step alone
                None if !Apps::available() => Page::Failed(String::new()),
                None => apps.page(&app.svc, &sid, a),
            };
            let key = (id, s.id.clone());
            let h = apps.height(&key).unwrap_or(FIRST_HEIGHT).min(APP_MAX);
            let h = ui.ease(("focus-app", id, &s.id), h, 180.0);
            cards.insert(s.id.clone(), Card { page, h, sized: apps.sized(&key), name: app_name(&a.server) });
        }
    }
    let gap = c.lh.round();
    let intro_h = if header_has_location && lens.is_none() { 0.0 } else { intro_height(c) };
    let earlier_h = if skip > 0 { 44.0 } else { 0.0 };
    let top_pad = 18.0 + intro_h + earlier_h;
    // heights first (cached per width), so the scroll knows where everything is
    let mut heights = Vec::with_capacity(items.len() - skip);
    // the steps of a card opening or closing: how far open, while it's on its way
    let mut reveals: HashMap<usize, f32> = HashMap::new();
    for (i, it) in items.iter().enumerate().skip(skip) {
        let k = Arc::as_ptr(it) as usize;
        let open = v.open.contains(&k);
        if matches!(it.as_ref(), Item::Did { .. }) {
            let want = if open { 1.0 } else { 0.0 };
            let now = ui.spring(("focus-open", id, lens_name, k), want, arrive::FOLD, want);
            if now != want {
                reveals.insert(i, now.clamp(0.0, 1.0));
            }
        }
        let e = v.cache.get(&k);
        // an app's card grows as its page says, and the subagents a turn started get their cards as they're
        // known (no height kept)
        let app = matches!(it.as_ref(), Item::Did { steps } if steps.iter().any(|s| s.app.is_some() || subs.get(&s.id).is_some()));
        let moving = reveals.contains_key(&i);
        let h = match e.and_then(|e| e.h).filter(|_| !moving) {
            Some((w, s, o, n, h)) if !app && (w - col.w).abs() < 0.5 && (s - c.body).abs() < 0.01 && o == open && n == narration[i] => h,
            _ => {
                let blocks = e.and_then(|e| e.blocks.clone());
                let mut p = Paint::measuring(id);
                p.apps.cards = Some(&cards);
                p.apps.reveal = reveals.get(&i).copied();
                p.subs = Some(subs);
                let h = item(ui, c, it, blocks.as_deref(), &pics[i], i, Point::new(col.x, 0.0), col.w, open, narration[i], &mut p);
                if !moving && let Some(e) = v.cache.get_mut(&k) {
                    e.h = Some((col.w, c.body, open, narration[i], h));
                }
                h
            }
        };
        heights.push(h);
    }
    let sent_hs: Vec<f32> = sent.iter().map(|(text, _, _, pics, _)| you(ui, c, text, pics.len(), pics, false, Point::new(col.x, 0.0), col.w, false, Some(""), &mut Paint::measuring(id))).collect();
    // what you sent goes where you sent it (after what the conversation had then), until the transcript has it
    // where the agent read it
    let sent_at: Vec<usize> = sent.iter().map(|s| s.4.clamp(skip, items.len())).collect();
    // at the end: how its thinking is going while it runs no step (a subagent's own), or how a subagent's work ended
    let working_line = match lens {
        Some(s) => s.state.running() && shown.as_ref().is_none_or(|c| c.running().is_none()),
        None => mood == Mood::Working && seen.running.is_none(),
    };
    let ending = lens.and_then(|s| subagents::ending(s, c.now_ms));
    let limit_box = seen.limit.as_ref().filter(|_| lens.is_none() && (!items.is_empty() || !sent.is_empty())).map(|limit| lay(ui, &[(limit.message.clone(), Style::mono(c.small, t.danger))], c.small, 1.6, (col.w - 26.0).max(80.0), None));
    let tail_h = limit_box.as_ref().map_or_else(|| if working_line || ending.is_some() { c.lh + gap } else { 0.0 }, |b| b.h + gap + 8.0);
    let total = top_pad + heights.iter().filter(|h| **h > 0.0).map(|h| h + gap).sum::<f32>() + sent_hs.iter().map(|h| h + gap).sum::<f32>() + tail_h + 8.0;
    let room = area.h;
    let max = (total - room).max(0.0);
    // the keys' scrolls
    if let Some(d) = app.focus.drafts.get_mut(&id)
        && let Some(s) = d.scroll.take()
    {
        match s {
            Scroll::PageUp => {
                v.top = (v.top - room * 0.85).max(0.0);
                v.follow = false;
            }
            Scroll::PageDown => {
                v.top = (v.top + room * 0.85).min(max);
                v.follow = v.top >= max - 1.0;
            }
            Scroll::Top => {
                v.top = 0.0;
                v.follow = max <= 0.0;
            }
            Scroll::Bottom => v.follow = true,
        }
    }
    if v.follow {
        v.top = max;
    }
    v.top = v.top.clamp(0.0, max);
    v.total = total;
    v.room = room;
    // each conversation (the session's, each subagent's) scrolls on its own, on a spring: a wheel notch while it's
    // still moving carries on from its speed, and what lands at the bottom lifts the text up rather than jumping
    let scroll_key = ("focus-scroll", id, lens_name);
    if switched {
        ui.spring_set(scroll_key, v.top);
    }
    let offset = ui.spring(scroll_key, v.top, arrive::SCROLL, v.top);
    // what arrived since last frame rises and fades in (what you sent shows by itself, and the transcript's copy of
    // it takes its place unseen)
    let met = v.met.unwrap_or(items.len()).min(items.len());
    v.met = Some(items.len());
    for i in met.max(skip)..items.len() {
        if !matches!(items[i].as_ref(), Item::You { .. }) && !v.arriving.contains(&i) {
            v.arriving.push(i);
        }
    }
    let mut arrive: HashMap<usize, f32> = HashMap::new();
    v.arriving.retain(|&i| {
        let k = ui.spring(("focus-arrive", id, lens_name, i), 1.0, Spring::ARRIVE, 0.0);
        let landed = k == 1.0 || i >= items.len();
        if !landed {
            arrive.insert(i, k);
        }
        !landed
    });
    // the session's file lookups, for its links (what isn't known yet is looked up off this thread, and the
    // pane wakes when it's in)
    let waker = crate::pane::Waker::new(id, app.tx.clone());
    let term = app.panes.get(&id).and_then(|p| p.as_term_ref());
    let mut lookup = term.map(|t| t.file_cache().lookup());
    let mut resolve = |m: &FileLinkMatch| lookup.as_mut().and_then(|l| l.get(m));
    // a selection whose ends left the conversation (it was replaced) goes
    if v.sel.as_ref().is_some_and(|s| s.anchor.key.0.max(s.head.key.0) >= items.len()) {
        v.sel = None;
    }
    let selectable = TextPaint { sel: v.sel.as_ref().and_then(Sel::span), ..TextPaint::default() };
    let mut p = Paint { draw: true, id, links: vec![], pictures: vec![], sounds: vec![], media: Some(media), resolve: Some(&mut resolve), apps: AppPaint { cards: Some(&cards), ..AppPaint::default() }, text: selectable, subs: Some(subs), opened: vec![], quip_hover: None };
    ui.clip(area, 0.0);
    let mut y = area.y + 18.0 - offset + (1.0 - lens_in) * arrive::RISE;
    match lens {
        Some(s) => subagents::head(ui, c, id, s, Point::new(col.x, y), col.w),
        None if !header_has_location => intro(ui, c, seen, Point::new(col.x, y), col.w, &mut p),
        None => {}
    }
    y += intro_h;
    if skip > 0 {
        let label = format!("Earlier · {skip} more");
        let st = Style::mono(c.small, c.muted);
        let w = ui.measure(&label, st) + 28.0;
        let br = Rect::new(col.x + (col.w - w) / 2.0, y + 4.0, w, 28.0);
        let hot = ui.hover_amount(("focus-earlier", id), br);
        ui.round(br, radius::CONTROL, ui.t.text.with_alpha(0.04 + 0.05 * hot));
        ui.label_in(br, &label, st.color(c.muted.mix(c.text, hot)), Align::Center);
        ui.hit(br, Action::Focus(id, EARLIER));
        y += earlier_h;
    }
    let mut next = 0;
    for (i, it) in items.iter().enumerate().skip(skip) {
        while next < sent.len() && sent_at[next] <= i {
            let (text, delivery, queued, pics, after) = &sent[next];
            let k = ui.arrival(("focus-sent", id, text, *after), true, Spring::ARRIVE);
            let dy = (1.0 - k) * arrive::RISE;
            you(ui, c, text, pics.len(), pics, false, Point::new(col.x, y + dy), col.w, false, Some(sent_state(delivery, *queued)), &mut p);
            ui.veil(Rect::new(col.x - 10.0, y + dy - 4.0, col.w + 20.0, sent_hs[next] + 8.0), 0.0, k);
            y += sent_hs[next] + gap;
            next += 1;
        }
        let h = heights[i - skip];
        if h <= 0.0 {
            continue;
        }
        if y + h >= area.y - 40.0 && y <= area.bottom() + 40.0 {
            let k = Arc::as_ptr(it) as usize;
            let open = v.open.contains(&k);
            let blocks = v.cache.get(&k).and_then(|e| e.blocks.clone());
            // its words can be selected
            p.text.collect = true;
            p.text.item(i);
            let (born, reveal) = (arrive.get(&i).copied(), reveals.get(&i).copied());
            let dy = born.map_or(0.0, |k| (1.0 - k) * arrive::RISE);
            p.apps.reveal = reveal;
            item(ui, c, it, blocks.as_deref(), &pics[i], i, Point::new(col.x, y + dy), col.w, open, narration[i], &mut p);
            p.apps.reveal = None;
            p.text.collect = false;
            if let Some(k) = born {
                ui.veil(Rect::new(col.x - 10.0, y + dy - 4.0, col.w + 20.0, h + 8.0), 0.0, k);
            }
        }
        y += h + gap;
    }
    for (n, (text, delivery, queued, pics, after)) in sent.iter().enumerate().skip(next) {
        let k = ui.arrival(("focus-sent", id, text, *after), true, Spring::ARRIVE);
        let dy = (1.0 - k) * arrive::RISE;
        you(ui, c, text, pics.len(), pics, false, Point::new(col.x, y + dy), col.w, false, Some(sent_state(delivery, *queued)), &mut p);
        ui.veil(Rect::new(col.x - 10.0, y + dy - 4.0, col.w + 20.0, sent_hs[n] + 8.0), 0.0, k);
        y += sent_hs[n] + gap;
    }
    if let Some(b) = &limit_box {
        ui.icon(Rect::new(col.x, y + 2.0, 15.0, 16.0), "\u{F0026}", 14.0, t.danger);
        ui.draw(b, Point::new(col.x + 24.0, y));
    } else if working_line {
        // how its thinking is going, at the end of what it's said
        ui.spinner(Point::new(col.x + 7.0, y + c.lh / 2.0), 4.6, 1.6, t.accent);
        let words = match lens {
            Some(s) => subagents::thinking(s, c.now_ms),
            None => thinking(seen, c.now_ms, working_secs(v.mood, ui.now)),
        };
        let b = ui.line(&words, Style::mono(c.small, c.muted), Some(col.w - 26.0));
        ui.draw(&b, Point::new(col.x + 22.0, y + (c.lh - b.h) / 2.0));
    } else if let Some(words) = &ending {
        // a subagent's work is over: how it went, quietly
        let b = ui.line(words, Style::mono(c.small, c.faint), Some(col.w));
        ui.draw(&b, Point::new(col.x, y + (c.lh - b.h) / 2.0));
    }
    let (links, pictures, sounds) = (std::mem::take(&mut p.links), std::mem::take(&mut p.pictures), std::mem::take(&mut p.sounds));
    let (drawn, retry) = (std::mem::take(&mut p.apps.drawn), std::mem::take(&mut p.apps.retry));
    let runs = std::mem::take(&mut p.text.runs);
    v.opened = std::mem::take(&mut p.opened);
    v.quip_preview.hovered = p.quip_hover.take();
    drop(p);
    v.cards = drawn;
    v.retry = retry;
    // what a press lands on next; while something's selected, the text it's drawn over is kept for the copy
    if v.sel.is_some() {
        keep_text(&mut v.sel_text, &runs);
    }
    v.runs = runs;
    v.text_area = area;
    v.clip = area;
    if let Some(l) = lookup {
        l.finish(Some(waker));
    }
    v.links = links;
    v.pictures = pictures;
    v.sounds = sounds;
    // more above / below: the text fades under the edge
    let surface = t.surface;
    let (fade_top, fade_bottom) = (ui.ease(("focus-fade-top", id), if offset > 0.5 { 1.0 } else { 0.0 }, 120.0), ui.ease(("focus-fade-bottom", id), if offset < max - 0.5 { 1.0 } else { 0.0 }, 120.0));
    if fade_top > 0.01 {
        let fr = ui.px(Rect::new(area.x, area.y, area.w, 36.0));
        ui.scene.top_glass_quad(Quad::new(fr, Fill::Vertical(surface.fade(fade_top), surface.with_alpha(0.0))));
    }
    if fade_bottom > 0.01 {
        let fr = ui.px(Rect::new(area.x, area.bottom() - 28.0, area.w, 28.0));
        ui.scene.top_glass_quad(Quad::new(fr, Fill::Vertical(surface.with_alpha(0.0), surface.fade(fade_bottom))));
    }
    ui.unclip();
    let over = ui.hovered(area);
    widgets::scrollbar(ui, Rect::new(area.right() - 10.0, area.y, 12.0, area.h), total, room, offset, if over { 0.9 } else { 0.0 });
    // away from the bottom: a way back to the latest, floating over the text on a layer of its own
    let away = !v.follow && max - v.top > room * 0.4;
    let latest = ui.spring(("focus-latest-in", id, lens_name), if away { 1.0 } else { 0.0 }, Spring::ARRIVE, 0.0);
    if latest > 0.01 {
        let prev = ui.layer();
        let label = "Latest";
        let st = Style::mono(size::SMALL, t.text).weight(weight::MEDIUM);
        let w = ui.measure(label, st) + 40.0;
        let shown = latest.clamp(0.0, 1.0);
        let pr = Rect::new(col.x + (col.w - w) / 2.0, area.bottom() - 42.0 + (1.0 - latest) * 14.0, w, 28.0);
        // an app's page stops short of it (a page is a window of its own, over anything drawn)
        if away {
            v.clip.h = (area.bottom() - 42.0 - 8.0 - area.y).max(0.0);
        }
        let hot = ui.hover_amount(("focus-latest", id), pr);
        ui.shadow(pr, radius::OVERLAY, 12.0, 3.0, t.shadow.fade(0.6 * shown));
        ui.panel(pr, radius::OVERLAY, t.raised.mix(t.text, 0.04 * hot).fade(shown), t.border.fade(shown));
        ui.icon(Rect::new(pr.x + 10.0, pr.y + 6.0, 16.0, 16.0), "\u{F0045}", 14.0, t.muted.fade(shown));
        ui.label(Point::new(pr.x + 28.0, pr.y + 5.0), label, st.color(st.color.fade(shown)));
        if away {
            ui.hit(pr, Action::Focus(id, LATEST));
        }
        ui.set_layer(prev);
    }
    ui.veil(area, 0.0, lens_in);
    match lens {
        // a subagent with nothing to show yet says so (one at work shows its thinking line)
        Some(s) if items.is_empty() && !s.state.running() => {
            let b = ui.line("No transcript of its work to show", Style::mono(c.small, c.faint), Some(col.w));
            ui.draw(&b, Point::new(col.x + (col.w - b.w) / 2.0, area.y + intro_h + 18.0 + (area.h - intro_h) * 0.3));
        }
        Some(_) => v.mark = None,
        None if items.is_empty() && sent.is_empty() => empty(ui, c, seen, Rect::new(col.x, area.y + intro_h + 18.0, col.w, (area.h - intro_h - 18.0).max(0.0)), &mut v.mark),
        None => v.mark = None,
    }
}

/// The caption under a message on its way.
fn sent_state(delivery: &crate::panes::term::DeliveryState, queued: bool) -> &'static str {
    use crate::panes::term::DeliveryState;
    match delivery {
        DeliveryState::Sending => "Sending…",
        DeliveryState::Submitted if queued => "Queued · waiting for the agent",
        DeliveryState::Submitted => "Waiting for the agent…",
        DeliveryState::Failed(_) => "Not sent · check the terminal",
    }
}

fn intro_height(c: &Typo) -> f32 {
    (c.small * 1.7).round() + 18.0
}

/// Draw the working folder and Git branch next to the chat session's title.
fn draw_session_location(app: &App, ui: &mut Ui, id: PaneId, r: Rect, v: &mut View, seen: &Seen) {
    let Some(term) = app.panes.get(&id).and_then(|p| p.as_term_ref()) else { return };
    let mut lookup = term.file_cache().lookup();
    let full = seen.cwd.to_string_lossy().into_owned();
    let path = FileLinkMatch { start: 0, end: full.len(), label: full.clone(), path: full.clone(), line: None, column: None };
    let action = lookup.get(&path).map(|uri| {
        let target = LINK + v.links.len();
        v.links.push(uri.to_string());
        Action::Focus(id, target)
    });
    lookup.finish(Some(crate::pane::Waker::new(id, app.tx.clone())));
    breadcrumb_bar(ui, seen, r, &full, action);
}

fn breadcrumb_bar(ui: &mut Ui, seen: &Seen, r: Rect, full: &str, action: Option<Action>) {
    let t = ui.t;
    let mid = r.y + r.h / 2.0;
    let branch = seen.branch.as_deref().filter(|b| !b.is_empty()).map(|branch| {
        let ink = Style::new(size::SMALL, weight::MEDIUM, t.muted);
        let budget = (r.w * 0.45 - 32.0).max(1.0);
        (branch, ui.line(branch, ink, Some(budget)))
    });
    let end = r.right() - branch.as_ref().map_or(0.0, |(_, text)| text.w + 40.0);

    let parts = path_parts(&crate::util::strip_verbatim(seen.cwd.clone()));
    let Some(last) = parts.len().checked_sub(1) else { return };
    let quiet = Style::new(size::BODY, weight::MEDIUM, t.muted);
    let current = Style::new(size::BODY, weight::SEMIBOLD, t.text);
    let style = |i| if i == last { current } else { quiet };
    let widths: Vec<_> = parts.iter().enumerate().map(|(i, part)| ui.measure(part, style(i))).collect();
    let ellipsis = ui.measure("…", quiet);
    const GAP: f32 = 16.0;
    let text_x = r.x + 27.0;
    let room = (end - text_x - 8.0).max(1.0);
    let plan = crumb_plan(&widths, ellipsis, GAP, room);
    let used: f32 = plan.iter().map(|i| i.map_or(ellipsis, |i| widths[i])).sum::<f32>()
        + GAP * plan.len().saturating_sub(1) as f32;
    let hit = Rect::new(r.x, mid - 14.0, (text_x - r.x) + used.min(room) + 8.0, 28.0);
    if let Some(action) = action {
        let hot = ui.hover_amount("session-breadcrumbs", hit);
        ui.round(hit, 7.0, t.text.with_alpha(0.065 * hot));
        ui.hit(hit, action);
    } else {
        ui.block(hit);
    }
    if ui.hovered(hit) {
        widgets::tooltip_under(ui, hit, full, (ui.screen.w - 32.0).min(480.0));
    }
    ui.icon(Rect::new(r.x + 5.0, r.y, 17.0, r.h), "\u{F0256}", 15.0, t.muted);
    let mut x = text_x;
    for (j, part) in plan.into_iter().enumerate() {
        if j > 0 {
            ui.icon(Rect::new(x, r.y, GAP, r.h), "\u{F0142}", 10.0, t.faint);
            x += GAP;
        }
        let (label, ink) = part.map_or(("…", quiet), |i| (parts[i].as_str(), style(i)));
        let text = ui.line(label, ink, Some((text_x + room - x).max(1.0)));
        ui.draw(&text, Point::new(x, mid - text.h / 2.0));
        x += text.w;
    }
    if let Some((label, text)) = branch {
        let chip = Rect::new(hit.right() + 8.0, mid - 12.0, text.w + 32.0, 24.0);
        ui.panel(chip, 12.0, t.text.with_alpha(0.04), t.hairline);
        ui.icon(Rect::new(chip.x + 7.0, chip.y, 14.0, chip.h), "\u{F062C}", 12.0, t.muted);
        ui.draw(&text, Point::new(chip.x + 22.0, mid - text.h / 2.0));
        if text.layout.truncated && ui.hovered(chip) {
            widgets::tooltip_under(ui, chip, label, (ui.screen.w - 32.0).min(360.0));
        }
    }
}

/// Where the session works, at the head of its conversation: the folder (it opens, as a path does in the
/// terminal) and its git branch.
fn intro(ui: &mut Ui, c: &Typo, seen: &Seen, at: Point, w: f32, p: &mut Paint) {
    let t = ui.t;
    let row_h = (c.small * 1.7).round();
    let mid = at.y + row_h / 2.0;
    ui.icon(Rect::new(at.x, mid - 8.0, 16.0, 16.0), "\u{F0256}", c.small + 1.0, c.faint);
    let place = crate::util::short_path(&seen.cwd, 64);
    let st = Style::mono(c.small, c.muted);
    let b = ui.line(&place, st, Some((w - 140.0).max(60.0)));
    let x = at.x + 24.0;
    ui.draw(&b, Point::new(x, mid - b.h / 2.0));
    let full = seen.cwd.to_string_lossy().to_string();
    let m = FileLinkMatch { start: 0, end: full.len(), path: full, line: None, column: None, label: place.clone() };
    if let Some(uri) = p.resolve(&m) {
        let r = Rect::new(x, mid - b.h / 2.0, b.w, b.h);
        let hot = ui.hover_amount(("focus-cwd", p.id), r);
        if hot > 0.01 {
            ui.top_fill(Rect::new(x, r.bottom() - 1.0, b.w, 1.0), 0.0, c.muted.with_alpha(0.6 * hot));
        }
        ui.hit(r, Action::Focus(p.id, LINK + p.links.len()));
        p.links.push(uri.to_string());
    }
    if let Some(branch) = &seen.branch {
        let bx = x + b.w + 18.0;
        ui.icon(Rect::new(bx, mid - 8.0, 16.0, 16.0), "\u{E725}", c.small + 1.0, c.faint);
        let bb = ui.line(branch, st, Some((at.x + w - bx - 22.0).max(20.0)));
        ui.draw(&bb, Point::new(bx + 20.0, mid - bb.h / 2.0));
    }
    ui.hline(at.x, at.y + row_h + 8.0, w, t.hairline);
}

/// A session with nothing said yet: its mark and an invitation. Claude's and Codex's marks are their logos in
/// pixels that come alive as it opens (see [`pixels`]); with too little room, and for the other agents, the plain
/// mark.
fn empty(ui: &mut Ui, c: &Typo, seen: &Seen, r: Rect, play: &mut Option<pixels::Play>) {
    let t = ui.t;
    if let Some(limit) = &seen.limit {
        let b = lay(ui, &[(limit.message.clone(), Style::mono(c.body, t.danger))], c.body, 1.6, r.w.max(80.0), None);
        ui.draw(&b, Point::new(r.x, r.y + (r.h - b.h).max(0.0) * 0.35));
        return;
    }
    // the pixel mark as big as the room over the words allows; the words come down a little under a big one
    let side = ((r.h * 0.4 - 28.0) / 0.75).min(88.0);
    let cy = match pixels::logo(seen.harness).filter(|_| side >= 40.0) {
        Some(logo) => {
            let cy = r.y + r.h * 0.4 + side * 0.25;
            let play = play.get_or_insert_with(|| pixels::Play::new(ui.time));
            pixels::draw(ui, logo, Point::new(r.x + r.w / 2.0, cy - 24.0 - side / 2.0), side, seen.activity == Some(Activity::Working), play);
            cy
        }
        None => {
            let cy = r.y + r.h * 0.4;
            let mark = super::chrome::harness_mark(seen.harness);
            ui.mark(mark, Rect::new(r.x + (r.w - 44.0) / 2.0, cy - 64.0, 44.0, 44.0), 40.0, Some(t.text.with_alpha(0.22)));
            cy
        }
    };
    let words = match seen.activity {
        Some(Activity::Working) => format!("{} is getting ready.", agent_name(seen.harness)),
        _ => "What should we work on?".to_string(),
    };
    let st = Style::mono((c.body * 1.2).round(), t.text.mix(t.muted, 0.35)).weight(weight::MEDIUM);
    let b = ui.line(&words, st, Some(r.w));
    ui.draw(&b, Point::new(r.x + (r.w - b.w) / 2.0, cy));
    let hint = ui.line("Write below · enter sends", Style::mono(c.small, t.faint), Some(r.w));
    ui.draw(&hint, Point::new(r.x + (r.w - hint.w) / 2.0, cy + b.h + 10.0));
}

/// Measure (and with `p.draw`, draw) item `i` at `at`, `w` wide: its height (0 for nothing to show). The sound
/// files it names get a card each under it, to play them there (see [`sound`]).
#[allow(clippy::too_many_arguments)]
fn item(ui: &mut Ui, c: &Typo, it: &Item, blocks: Option<&Vec<Block>>, pics: &[PathBuf], i: usize, at: Point, w: f32, open: bool, narration: bool, p: &mut Paint) -> f32 {
    let h = item_body(ui, c, it, blocks, pics, i, at, w, open, narration, p);
    let sounds = if h > 0.0 { sound::sounds_of(it) } else { vec![] };
    if sounds.is_empty() {
        return h;
    }
    if p.draw {
        sound::cards(ui, c, p.id, &sounds, Point::new(at.x, at.y + h), w, SOUND, SOUND_AT, &mut p.sounds);
    }
    h + sound::height(sounds.len())
}

#[allow(clippy::too_many_arguments)]
fn item_body(ui: &mut Ui, c: &Typo, it: &Item, blocks: Option<&Vec<Block>>, pics: &[PathBuf], i: usize, at: Point, w: f32, open: bool, narration: bool, p: &mut Paint) -> f32 {
    let id = p.id;
    match it {
        Item::You { text, command: true, .. } if setting(text).is_some() => {
            let h = (c.small * 1.8).round();
            if p.draw {
                let b = ui.line(&setting(text).unwrap_or_default(), Style::mono(c.small, c.faint), Some(w));
                ui.draw(&b, Point::new(at.x, at.y + (h - b.h) / 2.0));
            }
            h
        }
        Item::You { text, images, command, .. } => {
            if p.draw {
                // a long message opens with a click (its links and pictures, drawn after, take theirs first)
                let h = you(ui, c, text, *images, pics, *command, at, w, open, None, &mut Paint::measuring(id));
                ui.hit_with(Rect::new(at.x + w * 0.14, at.y, w * 0.86, h), Action::Focus(id, OPEN + i), Cursor::Default);
            }
            let h = you(ui, c, text, *images, pics, *command, at, w, open, None, p);
            // in a subagent's conversation, what it's asked comes from the session's agent, not from you
            match c.asker {
                Some(who) => {
                    if p.draw {
                        let b = ui.line(&format!("From {who}"), Style::mono(c.small, c.faint), None);
                        ui.draw(&b, Point::new(at.x + w - b.w - 4.0, at.y + h + 6.0));
                    }
                    h + (c.small * 1.8).round()
                }
                None => h,
            }
        }
        Item::Said { .. } => {
            let ink = if narration { c.narration } else { c.text };
            let h = said(ui, c, blocks.map(Vec::as_slice).unwrap_or_default(), at, w, ink, narration, p);
            if p.draw && !narration {
                // copy what it said, on hover
                let full = Rect::new(at.x, at.y, w, h);
                let br = Rect::new(at.x + w - 26.0, at.y - 2.0, 26.0, 26.0);
                if ui.hovered(full) || ui.hovered(br) {
                    let hot = ui.hover_amount(("focus-copy", id, i), br);
                    ui.round(br, radius::CONTROL, ui.t.surface.mix(ui.t.text, 0.06 + 0.06 * hot));
                    ui.icon(br, "\u{F018F}", 14.0, c.muted.mix(c.text, hot));
                    ui.hit(br, Action::Focus(id, COPY + i));
                    if hot > 0.5 {
                        widgets::tooltip(ui, br, "Copy");
                    }
                }
            }
            h
        }
        Item::Did { steps } => did(ui, c, steps, i, at, w, open, p),
        Item::Note { kind: NoteKind::Subagent, text, .. } => subagents::note(ui, c, text, i, at, w, p),
        Item::Note { kind, text, .. } => note(ui, c, *kind, text, at, w, p.draw),
    }
}

/// A command that switched what the agent runs on, said plainly: `/model claude-sonnet-5-5` → "Now on Sonnet
/// 5.5", `/effort high` → "Thinking high".
fn setting(text: &str) -> Option<String> {
    let mut w = text.split_whitespace();
    match (w.next()?, w.next()) {
        ("/model", Some(m)) => Some(format!("Now on {}", model_name(m))),
        ("/effort", Some(e)) => Some(format!("Thinking {}", level_name(e))),
        _ => None,
    }
}

/// One of your messages: a bubble on the right, its pictures over its words. `state` adds a caption under it
/// (a message on its way).
#[allow(clippy::too_many_arguments)]
fn you(ui: &mut Ui, c: &Typo, text: &str, images: usize, pics: &[PathBuf], command: bool, at: Point, w: f32, open: bool, state: Option<&str>, p: &mut Paint) -> f32 {
    let (short, quip_badges) = crate::quips::collapse_with_expansions(text);
    let text = short.as_str();
    let t = ui.t;
    let max_w = (w * 0.86).max(120.0);
    let (px, py) = (16.0, 11.0);
    let inner = max_w - 2.0 * px;
    let spans = [Span { text: text.to_string(), code: command, ..Span::default() }];
    let mut runs = styled(c, &spans, c.body, t.role("messageForeground", c.text), weight::REGULAR, false);
    if !quip_badges.is_empty() {
        let normal = runs[0].1;
        runs.clear();
        let mut start = 0;
        for badge in &quip_badges {
            if start < badge.range.start { runs.push((text[start..badge.range.start].into(), normal)); }
            runs.push((text[badge.range.clone()].into(), normal.weight(weight::SEMIBOLD).color(quips::badge_ink(ui))));
            start = badge.range.end;
        }
        if start < text.len() { runs.push((text[start..].into(), normal)); }
    }
    let tb = (!text.is_empty()).then(|| lay(ui, &runs, c.body, LINE, inner, (!open).then_some(YOU_LINES)));
    // its pictures in rows over the words; a tile where there's no picture to show
    let per_row = (((inner + 8.0) / (SHOT.0 + 8.0)).floor() as usize).max(1);
    let shots = images.max(pics.len());
    let rows = shots.div_ceil(per_row);
    let shots_w = if shots > 0 { (shots.min(per_row) as f32) * (SHOT.0 + 8.0) - 8.0 } else { 0.0 };
    let shots_h = if rows > 0 { rows as f32 * (SHOT.1 + 8.0) - 8.0 } else { 0.0 };
    let text_w = tb.as_ref().map_or(0.0, |b| b.w);
    let text_h = tb.as_ref().map_or(0.0, |b| b.h);
    let bw = (text_w.max(shots_w) + 2.0 * px).min(max_w).max(48.0);
    let between = if tb.is_some() && shots > 0 { 10.0 } else { 0.0 };
    let bh = shots_h + between + text_h + 2.0 * py;
    let cap_h = if state.is_some() { (c.small * 1.8).round() } else { 0.0 };
    if p.draw {
        let x = at.x + w - bw;
        let alpha = if state.is_some() { 0.6 } else { 1.0 };
        ui.round(Rect::new(x, at.y, bw, bh), radius::OVERLAY, c.bubble.fade(alpha));
        ui.theme_region(Rect::new(x,at.y,bw,bh),"messageSurface");
        if ui.theme.aurora(){ui.outline(Rect::new(x,at.y,bw,bh),16.0,1.0,t.border);}
        let mut y = at.y + py;
        for n in 0..shots {
            let (row, col) = (n / per_row, n % per_row);
            let r = Rect::new(x + px + col as f32 * (SHOT.0 + 8.0), y + row as f32 * (SHOT.1 + 8.0), SHOT.0, SHOT.1);
            match pics.get(n) {
                Some(path) => picture(ui, p, path, r, radius::CONTROL + 2.0),
                None => {
                    ui.round(r, radius::CONTROL + 2.0, t.text.with_alpha(0.05));
                    ui.icon(r, "\u{F02E9}", 22.0, t.faint);
                }
            }
        }
        y += shots_h + between;
        if let Some(b) = &tb {
            p.run(ui, b, Point::new(x + px, y));
            ui.draw_faded(b, Point::new(x + px, y), alpha);
            for badge in &quip_badges {
                if let Some(preview) = quips::badge(ui, b, Point::new(x + px, y), &badge.range, &text[badge.range.clone()], &badge.instructions, c.body) {
                    p.quip_hover = Some(preview);
                }
            }
            links_over(ui, c, b, &spans, Point::new(x + px, y), p);
        }
        if let Some(s) = state.filter(|s| !s.is_empty()) {
            let b = ui.line(s, Style::mono(c.small, t.faint), None);
            ui.draw(&b, Point::new(at.x + w - b.w - 4.0, at.y + bh + 6.0));
        }
    }
    bh + cap_h
}

/// Markdown spans as styled runs.
fn styled(c: &Typo, spans: &[Span], size: f32, ink: Color, w8: u16, italic: bool) -> Vec<(String, Style)> {
    spans
        .iter()
        .map(|s| {
            let mut st = Style::mono(size, ink).weight(if s.bold { weight::BOLD } else { w8 });
            st.italic = italic || s.italic;
            if s.code {
                st.color = c.code;
            }
            if s.link.as_deref().is_some_and(is_web) {
                st.color = c.link;
            }
            if s.strike {
                st.color = c.faint;
            }
            (s.text.clone(), st)
        })
        .collect()
}

/// The links in laid-out `spans` (drawn at `at`): underlined (web addresses in the link colour, always; paths
/// quietly, brighter under the pointer) and clickable.
fn links_over(ui: &mut Ui, c: &Typo, b: &TextBox, spans: &[Span], at: Point, p: &mut Paint) {
    if !p.draw {
        return;
    }
    for (range, target) in link_ranges(spans, p) {
        let web = is_web(&target);
        let rects = range_rects(b, at, &range, ui.scale);
        let hot = rects.iter().any(|r| ui.hovered(*r));
        let n = p.links.len();
        for r in &rects {
            let base = r.y + r.h - (r.h - c.body).max(0.0) * 0.38;
            let ink = if web { c.link.with_alpha(if hot { 0.9 } else { 0.55 }) } else { c.muted.with_alpha(if hot { 0.85 } else { 0.32 }) };
            ui.top_fill(Rect::new(r.x, base, r.w, 1.0), 0.0, ink);
            ui.hit(*r, Action::Focus(p.id, LINK + n));
        }
        if !rects.is_empty() {
            p.links.push(target);
        }
    }
}

/// Wrapped text at `at`: its height. Links get an underline and a target.
#[allow(clippy::too_many_arguments)]
fn text_block(ui: &mut Ui, c: &Typo, spans: &[Span], at: Point, w: f32, size: f32, ink: Color, w8: u16, italic: bool, p: &mut Paint) -> f32 {
    let runs = styled(c, spans, size, ink, w8, italic);
    let b = lay(ui, &runs, size, LINE, w, None);
    if p.draw {
        p.run(ui, &b, at);
        ui.draw(&b, at);
        links_over(ui, c, &b, spans, at, p);
    }
    b.h
}

/// A reply's blocks at `at`: their height.
#[allow(clippy::too_many_arguments)]
fn said(ui: &mut Ui, c: &Typo, blocks: &[Block], at: Point, w: f32, ink: Color, narration: bool, p: &mut Paint) -> f32 {
    let mut y = at.y;
    let mut prev: Option<&Block> = None;
    for b in blocks {
        // headings breathe above; list items sit close together
        y += match (prev, b) {
            (None, _) => 0.0,
            (Some(Block::Item { .. }), Block::Item { .. }) => (c.lh * 0.18).round(),
            (Some(_), Block::Heading(..)) => (c.lh * 0.95).round(),
            (Some(Block::Heading(..)), _) => (c.lh * 0.3).round(),
            _ => (c.lh * 0.55).round(),
        };
        // copied, the blocks keep their shape: a blank line between paragraphs, list items one to a line, each
        // with its mark
        p.text.gap = if matches!((prev, b), (Some(Block::Item { .. }), Block::Item { .. })) { "\n" } else { "\n\n" };
        p.text.lead = match b {
            Block::Item { depth, mark, .. } => {
                let mark = match mark {
                    Mark::Bullet => "-".to_string(),
                    Mark::Number(n) => format!("{n}."),
                    Mark::Check(done) => (if *done { "- [x]" } else { "- [ ]" }).to_string(),
                };
                format!("{}{mark} ", "  ".repeat(*depth))
            }
            Block::Quote(_) => "> ".to_string(),
            _ => String::new(),
        };
        let h = match b {
            Block::Para(spans) => text_block(ui, c, spans, Point::new(at.x, y), w, c.body, ink, weight::REGULAR, false, p),
            Block::Heading(level, spans) => {
                let s = c.body
                    * match level {
                        1 => 1.3,
                        2 => 1.16,
                        _ => 1.04,
                    };
                text_block(ui, c, spans, Point::new(at.x, y), w, s.round(), if narration { ink } else { c.text }, weight::BOLD, false, p)
            }
            Block::Item { depth, mark, spans } => {
                let indent = *depth as f32 * c.adv * 2.0;
                let (glyph, cells) = match mark {
                    Mark::Bullet => ("›".to_string(), 2.0),
                    Mark::Number(n) => (format!("{n}."), n.len() as f32 + 2.0),
                    Mark::Check(done) => ((if *done { "[x]" } else { "[ ]" }).to_string(), 4.0),
                };
                let mark_w = c.adv * cells;
                if p.draw {
                    let mb = ui.line(&glyph, Style::mono(c.body, c.muted), None);
                    ui.draw(&mb, Point::new(at.x + indent, y + (c.lh - mb.h) / 2.0 - (c.lh - c.body * LINE).max(0.0) / 2.0));
                }
                let done = matches!(mark, Mark::Check(true));
                text_block(ui, c, spans, Point::new(at.x + indent + mark_w, y), (w - indent - mark_w).max(40.0), c.body, if done { c.muted } else { ink }, weight::REGULAR, false, p)
            }
            Block::Quote(spans) => text_block(ui, c, spans, Point::new(at.x + c.adv * 2.0, y), (w - c.adv * 2.0).max(40.0), c.body, c.muted, weight::REGULAR, true, p),
            Block::Code { lang, lines } => code(ui, c, lang, lines, Point::new(at.x, y), w, p),
            Block::Rule => {
                if p.draw {
                    ui.hline(at.x, y + c.lh / 2.0, w, ui.t.hairline);
                }
                c.lh
            }
            Block::Table { rows, header } => table(ui, c, rows, *header, Point::new(at.x, y), w, ink, p),
        };
        y += h;
        prev = Some(b);
    }
    y - at.y
}

fn code(ui: &mut Ui, c: &Typo, lang: &str, lines: &[String], at: Point, w: f32, p: &mut Paint) -> f32 {
    let size = (c.body * 0.86).round();
    let lh = (size * 1.5).round();
    let shown = lines.len().min(CODE_LINES);
    let more = lines.len() - shown;
    let pad = 14.0;
    let h = shown.max(1) as f32 * lh + 2.0 * pad + if more > 0 { lh } else { 0.0 };
    if p.draw {
        let r = Rect::new(at.x, at.y, w, h);
        ui.round(r, radius::OVERLAY, c.well);
        ui.theme_region(r,"codeBackground");
        if ui.theme.aurora(){ui.outline(r,16.0,1.0,ui.t.border);}
        ui.clip(r, radius::OVERLAY);
        let st = Style::mono(size, ui.t.role("codeForeground", c.text.mix(c.code, 0.25)));
        for (i, l) in lines.iter().take(shown).enumerate() {
            let b = ui.line(l, st, Some(w - 2.0 * pad));
            let at = Point::new(r.x + pad, r.y + pad + i as f32 * lh + (lh - b.h) / 2.0);
            // each line selects on its own, and copies on a line of its own
            p.run(ui, &b, at);
            ui.draw(&b, at);
        }
        if more > 0 {
            let b = ui.line(&format!("… {more} more line{}", if more == 1 { "" } else { "s" }), Style::mono(size, c.faint), None);
            ui.draw(&b, Point::new(r.x + pad, r.y + pad + shown as f32 * lh + (lh - b.h) / 2.0));
        }
        if !lang.is_empty() {
            let b = ui.line(lang, Style::mono(c.small, c.faint), None);
            ui.draw(&b, Point::new(r.right() - pad - b.w, r.y + 8.0));
        }
        ui.unclip();
    }
    h
}

/// A table: its columns lined up on the terminal font's grid when they fit, else a row per line.
#[allow(clippy::too_many_arguments)]
fn table(ui: &mut Ui, c: &Typo, rows: &[Vec<Vec<Span>>], header: bool, at: Point, w: f32, ink: Color, p: &mut Paint) -> f32 {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in rows {
        for (i, cell) in r.iter().enumerate() {
            widths[i] = widths[i].max(markdown::plain(cell).chars().count());
        }
    }
    let total = widths.iter().sum::<usize>() + 3 * cols.saturating_sub(1);
    let mut y = at.y;
    if (total as f32) * c.adv <= w {
        for (ri, r) in rows.iter().enumerate() {
            let mut spans: Vec<Span> = vec![];
            for (i, cell) in r.iter().enumerate() {
                let len = markdown::plain(cell).chars().count();
                for s in cell {
                    let mut s = s.clone();
                    s.bold |= header && ri == 0;
                    spans.push(s);
                }
                if i + 1 < cols {
                    spans.push(Span { text: " ".repeat(widths[i].saturating_sub(len) + 3), ..Span::default() });
                }
            }
            let h = text_block(ui, c, &spans, Point::new(at.x, y), w + c.adv * 4.0, c.body, ink, weight::REGULAR, false, p);
            y += h;
            if header && ri == 0 {
                if p.draw {
                    ui.hline(at.x, y + 3.0, (total as f32 * c.adv).min(w), ui.t.hairline);
                }
                y += 6.0;
            }
        }
    } else {
        for (ri, r) in rows.iter().enumerate() {
            let mut spans: Vec<Span> = vec![];
            for (i, cell) in r.iter().enumerate() {
                if i > 0 {
                    spans.push(Span { text: "  ·  ".into(), ..Span::default() });
                }
                for s in cell {
                    let mut s = s.clone();
                    s.bold |= header && ri == 0;
                    spans.push(s);
                }
            }
            y += text_block(ui, c, &spans, Point::new(at.x, y), w, c.body, ink, weight::REGULAR, false, p) + 4.0;
        }
    }
    y - at.y
}

fn step_icon(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Read => "\u{F0214}",
        StepKind::Edit | StepKind::Create | StepKind::Delete => "\u{F03EB}",
        StepKind::Run => "\u{F018D}",
        StepKind::Search => "\u{F0349}",
        StepKind::Web => "\u{F059F}",
        StepKind::Agent => "\u{F06A9}",
        StepKind::Plan => "\u{F0756}",
        StepKind::Ask => "\u{F0625}",
        StepKind::Send => "\u{F048A}",
        StepKind::Skill => "\u{F0674}",
        StepKind::Mcp => "\u{F1425}",
        StepKind::Other => "\u{F05B7}",
    }
}

/// What a skill or an MCP tool is about, by the words its name starts with, and its icon. The first that fits
/// wins: a voice before a generator, a click before the web.
const ABOUT: &[(&[&str], &str)] = &[
    (&["voice", "speech", "speak", "tts", "narrat", "audio", "sound", "podcast"], "\u{F036C}"),
    (&["music", "song", "melod"], "\u{F0387}"),
    (&["video", "clip", "film", "movie", "reel", "animat"], "\u{F0567}"),
    (&["image", "picture", "photo", "draw", "illustrat", "art", "sticker", "emote", "logo", "thumbnail", "meme"], "\u{F02E9}"),
    (&["screenshot", "snapshot", "screen", "capture"], "\u{F0E51}"),
    (&["click", "tap", "press", "type", "fill", "select", "swipe", "scroll", "hover", "drag"], "\u{F0CFD}"),
    (&["javascript", "script", "exec", "eval", "console", "shell", "command", "terminal"], "\u{F018D}"),
    (&["navigate", "browse", "page", "tab", "url", "web", "site", "chrome"], "\u{F059F}"),
    (&["upload"], "\u{F0552}"),
    (&["download"], "\u{F01DA}"),
    (&["search", "find", "query", "history", "research"], "\u{F0349}"),
    (&["pdf"], "\u{F0226}"),
    (&["docx", "word", "document", "doc", "memo"], "\u{F0219}"),
    (&["xlsx", "excel", "sheet", "spreadsheet", "csv"], "\u{F021B}"),
    (&["pptx", "slide", "deck", "powerpoint", "presentation"], "\u{F0227}"),
    (&["deploy", "release", "ship", "publish", "launch"], "\u{F14DE}"),
    (&["commit", "git", "branch", "merge"], "\u{F0718}"),
    (&["test"], "\u{F0096}"),
    (&["design", "theme", "style", "palette", "colour", "color"], "\u{F0E0C}"),
    (&["workout", "fitness", "exercise", "gains"], "\u{F01E6}"),
    (&["schedule", "cron", "remind", "wait", "timer", "loop"], "\u{F0150}"),
    (&["notif", "alert"], "\u{F009C}"),
    (&["message", "chat", "email", "mail", "reply"], "\u{F036A}"),
    (&["connect", "device", "phone", "android"], "\u{F011C}"),
    (&["read", "get", "list", "fetch", "show", "view", "inspect"], "\u{F06D0}"),
    (&["create", "generate", "make", "write", "new", "build"], "\u{F0674}"),
];

/// A step's own icon: a skill's or an MCP tool's by what it's about ("Generate voice" a microphone, "Click
/// element" a pointer, "Navigate" the web), else its kind's.
fn step_glyph(s: &Step) -> &'static str {
    // a tool's name, not what it was given (an upload of photo.png is an upload)
    let name = s.label.split(" · ").next().unwrap_or_default().to_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let about = |keys: &[&str]| keys.iter().any(|k| words.iter().any(|w| w.starts_with(k)));
    match s.kind {
        StepKind::Skill | StepKind::Mcp => ABOUT.iter().find(|(keys, _)| about(keys)).map_or(step_icon(s.kind), |(_, glyph)| *glyph),
        StepKind::Other if about(&["notif"]) => "\u{F009C}",
        kind => step_icon(kind),
    }
}

/// A step's label as runs: what it did in `ink`, and what follows its " · " (where it went, how far along) in
/// `rest`.
fn label_runs(label: &str, ink: Color, rest: Color, size: f32) -> Vec<(String, Style)> {
    match label.split_once(" · ") {
        Some((what, more)) => vec![(what.to_string(), Style::mono(size, ink)), (format!(" · {more}"), Style::mono(size, rest))],
        None => vec![(label.to_string(), Style::mono(size, ink))],
    }
}

/// "Read 4 files · Changed 2 files · Ran 3 commands", or the step itself when there's one (the running one, while
/// one runs, and for how long: only while the agent is `working`).
fn summary(steps: &[Step], now_ms: i64, working: bool) -> String {
    if let Some(s) = steps.iter().rev().find(|s| s.state == StepState::Running) {
        if !working {
            return s.label.clone();
        }
        let secs = s.at_ms.map_or(0, |t| (now_ms - t) / 1000);
        return if secs >= 5 { format!("{} · {}", s.label, clock(secs)) } else { format!("{}…", s.label) };
    }
    if let [one] = steps {
        return one.label.clone();
    }
    let count = |f: &dyn Fn(StepKind) -> bool| steps.iter().filter(|s| f(s.kind)).count();
    let n = |k: usize, one: &str, many: &str| match k {
        0 => None,
        1 => Some(one.to_string()),
        k => Some(many.replace('#', &k.to_string())),
    };
    // a skill it used leads: the rest was its doing
    let skills: Vec<&Step> = steps.iter().filter(|s| s.kind == StepKind::Skill).collect();
    let skill = match skills.as_slice() {
        [] => None,
        [one] => Some(one.label.clone()),
        many => Some(format!("Used {} skills", many.len())),
    };
    // an MCP server's tools by the server, when they're all one's
    let mcp: Vec<&str> = steps.iter().filter(|s| s.kind == StepKind::Mcp).map(|s| s.detail.as_deref().unwrap_or("a server")).collect();
    let server = match mcp.as_slice() {
        [] => None,
        [one] => Some(format!("Used {one}")),
        [first, rest @ ..] if rest.iter().all(|s| s == first) => Some(format!("Used {first} {} times", mcp.len())),
        many => Some(format!("Used {} tools", many.len())),
    };
    // the rest in the order it happened: what it did first, first
    let first = |f: &dyn Fn(StepKind) -> bool| steps.iter().position(|s| f(s.kind)).unwrap_or(usize::MAX);
    let mut rest: Vec<(usize, Option<String>)> = vec![
        (first(&|k| k == StepKind::Read), n(count(&|k| k == StepKind::Read), "Read a file", "Read # files")),
        (first(&|k| matches!(k, StepKind::Edit | StepKind::Create | StepKind::Delete)), n(count(&|k| matches!(k, StepKind::Edit | StepKind::Create | StepKind::Delete)), "Changed a file", "Changed # files")),
        (first(&|k| k == StepKind::Run), n(count(&|k| k == StepKind::Run), "Ran a command", "Ran # commands")),
        (first(&|k| matches!(k, StepKind::Search | StepKind::Web)), n(count(&|k| matches!(k, StepKind::Search | StepKind::Web)), "Searched once", "Searched # times")),
        // the subagents it started have cards of their own: what's left is talking to them, waiting on them
        (first(&|k| k == StepKind::Agent), n(count(&|k| k == StepKind::Agent), "Checked in with a subagent", "Checked in with subagents # times")),
        (first(&|k| k == StepKind::Mcp), server),
        (first(&|k| matches!(k, StepKind::Plan | StepKind::Ask | StepKind::Send | StepKind::Other)), n(count(&|k| matches!(k, StepKind::Plan | StepKind::Ask | StepKind::Send | StepKind::Other)), "Used a tool", "Used # tools")),
    ];
    rest.sort_by_key(|(at, _)| *at);
    skill.into_iter().chain(rest.into_iter().filter_map(|(_, said)| said)).collect::<Vec<_>>().join(" · ")
}

/// The file a step was about, as a link the session can open (at its line): what it read, changed or sent.
fn step_file(s: &Step, p: &mut Paint) -> Option<String> {
    if !matches!(s.kind, StepKind::Read | StepKind::Edit | StepKind::Create | StepKind::Send) || !p.draw {
        return None;
    }
    let path = s.detail.as_deref()?.lines().next()?.trim().to_string();
    let m = FileLinkMatch { start: 0, end: path.len(), label: path.clone(), path, line: s.line, column: None };
    p.resolve(&m).map(|u| u.to_string())
}

/// What the agent did: one quiet line, the pictures it showed you or looked at under it, the subagents it started
/// (a card each, live), its steps when open (each opens its file).
#[allow(clippy::too_many_arguments)]
fn did(ui: &mut Ui, c: &Typo, steps: &[Step], i: usize, at: Point, w: f32, open: bool, p: &mut Paint) -> f32 {
    let t = ui.t;
    let id = p.id;
    let row_h = (c.small * 1.9).round();
    // the subagents these steps started have cards of their own: the line sums up the rest (no rest, no line)
    let launched: Vec<Arc<Subagent>> = p.subs.map(|s| steps.iter().filter_map(|st| s.get(&st.id).cloned()).collect()).unwrap_or_default();
    let rest: Vec<Step> = if launched.is_empty() { vec![] } else { steps.iter().filter(|st| !launched.iter().any(|l| l.call == st.id)).cloned().collect() };
    let line: &[Step] = if launched.is_empty() { steps } else { &rest };
    // how far open its steps are: all the way or not at all, or on their way
    let k = p.apps.reveal.unwrap_or(if open { 1.0 } else { 0.0 });
    let unfolded = open || k > 0.001;
    let line_h = if line.is_empty() && !unfolded { 0.0 } else { row_h };
    let crew_h = if launched.is_empty() { 0.0 } else { subagents::cards_height(c, launched.len()) + if line_h > 0.0 { 8.0 } else { 0.0 } };
    let shots = step_pictures(steps);
    let shots_h = if shots.is_empty() { 0.0 } else { SHOWN.1 + 10.0 };
    // its MCP apps' cards, under the line
    let all = p.apps.cards;
    let cards: Vec<(&Step, &Card)> = steps.iter().filter(|s| s.app.is_some()).filter_map(|s| all.and_then(|a| a.get(&s.id)).map(|c| (s, c))).filter(|(_, c)| card_height(c) > 0.0).collect();
    let cards_h: f32 = cards.iter().map(|(_, c)| APP_GAP + card_height(c)).sum();
    let mut h = line_h + shots_h + cards_h + crew_h + if cards.is_empty() { 0.0 } else { 4.0 };
    let steps_h = (steps.len() as f32 * row_h + 4.0) * k;
    h += steps_h;
    if !p.draw {
        return h;
    }
    let x = at.x + 26.0;
    if line_h > 0.0 {
        let row = Rect::new(at.x - 8.0, at.y, w + 16.0, row_h);
        let hot = ui.hover_amount(("focus-did", id, i), row);
        if hot > 0.01 {
            ui.round(row, radius::CONTROL, t.text.with_alpha(0.045 * hot));
        }
        ui.hit(row, Action::Focus(id, OPEN + i));
        let running = line.iter().any(|s| s.state == StepState::Running) && (c.working || c.waiting);
        // a step that finished: its mark pops in, a flush of the finish colour fading as it lands
        let done = ui.since_on(("did-done", id, i), !running);
        let pop = ui.pop_scale(done);
        let failed = line.iter().filter(|s| s.state == StepState::Failed).count();
        let (added, removed) = line.iter().fold((0, 0), |(a, r), s| (a + s.added, r + s.removed));
        let mid = at.y + row_h / 2.0;
        let quiet = Style::mono(c.small, c.muted);
        let ir = Rect::new(at.x, mid - 8.0, 16.0, 16.0);
        // a step waiting for your answer isn't working: it pauses, in the colour of needing you
        if running && c.waiting {
            ui.icon(ir, "\u{F03E4}", c.small + 1.0, t.danger);
        } else if running {
            ui.spinner(Point::new(at.x + 8.0, mid), 4.4, 1.5, t.accent);
        } else if let Some(lead) = line.iter().find(|s| s.kind == StepKind::Skill).or(line.last()).or(steps.last()) {
            // the skill it used when it used one, else what it did last
            let flush = done.map_or(0.0, |s| (1.0 - s * 1.4).max(0.0));
            ui.icon(ir, step_glyph(lead), (c.small + 1.0) * pop, c.faint.mix(t.good, 0.8 * flush).fade(pop));
        }
        // the counts on the right: lines added and removed, then the chevron
        let mut right = at.x + w;
        // the chevron turns over as the steps open: one fades out as the other comes in
        let chev = Rect::new(right - 16.0, mid - 8.0, 16.0, 16.0);
        ui.icon(chev, "\u{F0142}", 14.0, c.faint.fade(1.0 - k));
        if k > 0.0 {
            ui.icon(chev, "\u{F0140}", 14.0, c.faint.fade(k));
        }
        right -= 22.0;
        if removed > 0 {
            let rb = ui.line(&format!("−{removed}"), Style::mono(c.small, t.danger.mix(c.muted, 0.35)), None);
            ui.draw(&rb, Point::new(right - rb.w, mid - rb.h / 2.0));
            right -= rb.w + 8.0;
        }
        if added > 0 {
            let ab = ui.line(&format!("+{added}"), Style::mono(c.small, t.good.mix(c.muted, 0.35)), None);
            ui.draw(&ab, Point::new(right - ab.w, mid - ab.h / 2.0));
            right -= ab.w + 8.0;
        }
        right -= 4.0;
        let what = match line.iter().rev().find(|s| s.state == StepState::Running) {
            Some(s) if c.waiting => format!("{} · waiting for you", s.label),
            // open with nothing but the steps that started its subagents: say so
            _ if line.is_empty() => if launched.len() == 1 { "Started a subagent".to_string() } else { format!("Started {} subagents", launched.len()) },
            _ => summary(line, c.now_ms, c.working),
        };
        let mut spans = if running && c.waiting {
            vec![(what, Style::mono(c.small, c.text.mix(c.muted, 0.2)))]
        } else if !running && line.iter().any(|s| s.kind == StepKind::Skill) {
            // the skill's name, a shade above the rest
            label_runs(&what, c.text.mix(c.muted, 0.35), c.muted, c.small)
        } else {
            vec![(what, quiet)]
        };
        if failed > 0 && !running {
            spans.push((format!(" · {failed} failed"), Style::mono(c.small, t.danger)));
        }
        let b = ui.rich(&spans, c.small, Some((right - x).max(20.0)), false, Some(1));
        ui.draw(&b, Point::new(x, mid - b.h / 2.0));
        // one step about a file: its name opens it, as a path in the terminal does
        if let [one] = line
            && let Some(uri) = step_file(one, p)
        {
            let r = Rect::new(x, mid - b.h / 2.0, b.w, b.h);
            let on = ui.hovered(r);
            ui.top_fill(Rect::new(x, r.bottom() - 2.0, b.w.min(right - x), 1.0), 0.0, c.muted.with_alpha(if on { 0.85 } else { 0.3 }));
            ui.hit(r, Action::Focus(id, LINK + p.links.len()));
            p.links.push(uri);
        }
    }
    let mut y = at.y + line_h;
    if !launched.is_empty() {
        if line_h > 0.0 {
            y += 8.0;
        }
        subagents::cards(ui, c, p, &launched, Point::new(at.x, y), w);
        y += subagents::cards_height(c, launched.len());
    }
    if !shots.is_empty() {
        y += 6.0;
        for (n, path) in shots.iter().enumerate() {
            let r = Rect::new(x + n as f32 * (SHOWN.0 + 8.0), y, SHOWN.0, SHOWN.1);
            if r.right() > at.x + w {
                break;
            }
            picture(ui, p, path, r, radius::CONTROL + 2.0);
        }
        y += SHOWN.1 + 4.0;
    }
    for (s, card) in &cards {
        y += APP_GAP;
        let ch = card_height(card);
        app_card(ui, c, p, s, card, Rect::new(at.x, y, w, ch));
        y += ch;
    }
    if !cards.is_empty() {
        y += 4.0;
    }
    if unfolded {
        y += 2.0;
        let block = Rect::new(at.x - 8.0, y, w + 16.0, (steps_h - 2.0).max(0.0));
        if k < 1.0 {
            ui.clip(block, 0.0);
        }
        for s in steps {
            let m = y + row_h / 2.0;
            let sr = Rect::new(at.x + 18.0, y, w - 18.0, row_h);
            let file = step_file(s, p);
            if let Some(uri) = &file {
                let hot = ui.hover_amount(("focus-step", id, i, &s.id), sr);
                if hot > 0.01 {
                    ui.round(sr, radius::CONTROL, t.text.with_alpha(0.05 * hot));
                }
                ui.hit(sr, Action::Focus(id, LINK + p.links.len()));
                p.links.push(uri.clone());
            }
            // each by its own icon: spinning while it runs, a cross when it failed
            let icon = Rect::new(at.x + 26.0, m - 8.0, 16.0, 16.0);
            match s.state {
                StepState::Running if c.waiting => ui.icon(icon, "\u{F03E4}", c.small, t.danger),
                StepState::Running if c.working => ui.spinner(Point::new(at.x + 34.0, m), 4.0, 1.4, t.accent),
                StepState::Failed => ui.icon(icon, "\u{F0156}", c.small, t.danger),
                _ => ui.icon(icon, step_glyph(s), c.small, c.faint),
            }
            // on the right, quietly: the skill or server it used, the lines it added and removed
            let mut right = at.x + w;
            let tag = match s.kind {
                StepKind::Skill => Some("skill"),
                StepKind::Mcp => s.detail.as_deref(),
                _ => None,
            };
            if let Some(tag) = tag {
                let tb = ui.line(tag, Style::mono(c.small, c.faint), Some(160.0));
                ui.draw(&tb, Point::new(right - tb.w, m - tb.h / 2.0));
                right -= tb.w + 14.0;
            }
            for (n, sign, ink) in [(s.removed, "−", t.danger), (s.added, "+", t.good)] {
                if n > 0 {
                    let nb = ui.line(&format!("{sign}{n}"), Style::mono(c.small, ink.mix(c.muted, 0.35)), None);
                    ui.draw(&nb, Point::new(right - nb.w, m - nb.h / 2.0));
                    right -= nb.w + 8.0;
                }
            }
            let lx = at.x + 50.0;
            let ink = if s.kind == StepKind::Skill { c.text } else { c.text.mix(c.muted, 0.3) };
            let mut runs = label_runs(&s.label, ink, c.muted, c.small);
            // what it was done to, after it (a server's name is the tag)
            let detail = if s.kind == StepKind::Mcp { None } else { s.detail.as_deref() };
            if let Some(d) = detail.map(|d| d.lines().next().unwrap_or_default()).filter(|d| !d.is_empty() && !s.label.contains(*d)) {
                let d = match s.line.filter(|_| file.is_some()) {
                    Some(l) => format!("{d}:{l}"),
                    None => d.to_string(),
                };
                runs.push((format!("   {d}"), Style::mono(c.small, c.faint)));
            }
            let b = ui.rich(&runs, c.small, Some((right - 8.0 - lx).max(20.0)), false, Some(1));
            ui.draw(&b, Point::new(lx, m - b.h / 2.0));
            y += row_h;
        }
        if k < 1.0 {
            ui.veil(block, 0.0, k);
            ui.unclip();
        }
    }
    h
}

/// How tall an MCP app's card is: its page, or a line saying it's opening or why it can't (a page that can't be
/// had for no reason worth saying, as the demo's without it, shows no card).
fn card_height(card: &Card) -> f32 {
    match &card.page {
        Page::Ready(_) => card.h + 2.0 * APP_INSET.1,
        Page::Reading => 64.0,
        Page::Failed(why) if why.is_empty() => 0.0,
        Page::Failed(_) => 76.0,
    }
}

/// What an MCP app's card is filled with: solid, as its page sits on it.
fn card_fill(ui: &Ui) -> Color {
    let t = ui.t;
    t.raised.over(t.surface).over(t.bg).with_alpha(1.0)
}

/// What an app's server is called: `claude_ai_JustGains` (a claude.ai connector) is "JustGains", `my_server` "my
/// server".
fn app_name(server: &str) -> String {
    server.strip_prefix("claude_ai_").unwrap_or(server).replace('_', " ")
}

/// An MCP app's card: its page (laid over it once the frame is drawn), and until the page shows, a line saying
/// it's opening, or why it can't.
fn app_card(ui: &mut Ui, c: &Typo, p: &mut Paint, s: &Step, card: &Card, r: Rect) {
    let t = ui.t;
    let Some(app) = &s.app else { return };
    ui.shadow(r, APP_RADIUS, 18.0, 4.0, t.shadow.fade(0.22));
    ui.panel(r, APP_RADIUS, card_fill(ui), t.border);
    let opening = |ui: &mut Ui| {
        let mid = r.y + 32.0;
        ui.spinner(Point::new(r.x + 26.0, mid), 5.0, 1.6, t.accent);
        let b = ui.line(&format!("Opening {}…", card.name), Style::mono(c.small, c.muted), Some((r.w - 64.0).max(20.0)));
        ui.draw(&b, Point::new(r.x + 44.0, mid - b.h / 2.0));
    };
    match &card.page {
        Page::Ready(_) => {
            // under the page until it shows (once it has said how tall it is)
            if !card.sized {
                opening(ui);
            }
            let pr = Rect::new(r.x + APP_INSET.0, r.y + APP_INSET.1, r.w - 2.0 * APP_INSET.0, card.h);
            p.apps.drawn.push((s.id.clone(), app.clone(), pr));
        }
        Page::Reading => opening(ui),
        Page::Failed(why) => {
            let label = "Try again";
            let bw = widgets::button_width(ui, label);
            let br = Rect::new(r.right() - 18.0 - bw, r.y + (r.h - 32.0) / 2.0, bw, 32.0);
            if p.apps.retry.len() < APP_RETRY_END - APP_RETRY {
                widgets::button(ui, br, label, widgets::Kind::Secondary, Action::Focus(p.id, APP_RETRY + p.apps.retry.len()));
                p.apps.retry.push(app.clone());
            }
            ui.icon(Rect::new(r.x + 18.0, r.y + 18.0, 18.0, 18.0), "\u{F0028}", c.small + 3.0, t.warn);
            let x = r.x + 46.0;
            let room = (br.x - 18.0 - x).max(40.0);
            let title = ui.line(&format!("{} didn't open here", card.name), Style::mono(c.small, c.text).weight(weight::MEDIUM), Some(room));
            ui.draw(&title, Point::new(x, r.y + 16.0));
            let first = why.lines().next().unwrap_or_default().trim();
            let reason = ui.rich(&[(first.to_string(), Style::mono(c.small, c.muted))], c.small, Some(room), false, Some(1));
            ui.draw(&reason, Point::new(x, r.y + 16.0 + title.h + 4.0));
        }
    }
}

fn note(ui: &mut Ui, c: &Typo, kind: NoteKind, text: &str, at: Point, w: f32, draw: bool) -> f32 {
    let t = ui.t;
    let st = Style::mono(c.small, c.faint);
    match kind {
        // drawn with the subagents ([`subagents::note`]), which know who reported back
        NoteKind::Subagent => 0.0,
        NoteKind::Worked => {
            // only turns that took a while say how long
            let long = text.contains('m') || text.contains('h') || text.trim_end_matches('s').parse::<u32>().is_ok_and(|s| s >= 20);
            if !long {
                return 0.0;
            }
            let h = (c.small * 1.6).round();
            if draw {
                let b = ui.line(&format!("Worked {text}"), st, None);
                ui.draw(&b, Point::new(at.x, at.y + (h - b.h) / 2.0));
            }
            h
        }
        NoteKind::Interrupted | NoteKind::Compacted => {
            let label = if kind == NoteKind::Interrupted { "Stopped" } else { "Earlier conversation summarised" };
            let h = (c.small * 1.8).round();
            if draw {
                let b = ui.line(label, st, None);
                let x = at.x + (w - b.w) / 2.0;
                let mid = at.y + h / 2.0;
                ui.draw(&b, Point::new(x, mid - b.h / 2.0));
                let line_w = ((w - b.w) / 2.0 - 16.0).clamp(0.0, 120.0);
                ui.hline(x - 12.0 - line_w, mid, line_w, t.hairline);
                ui.hline(x + b.w + 12.0, mid, line_w, t.hairline);
            }
            h
        }
        NoteKind::Error => {
            let text = if text.is_empty() { "The turn stopped on an error" } else { text };
            let b = lay(ui, &[(text.to_string(), Style::mono(c.small, t.danger))], c.small, 1.5, w - 26.0, Some(4));
            if draw {
                ui.icon(Rect::new(at.x, at.y + 1.0, 16.0, 16.0), "\u{F0026}", c.small + 1.0, t.danger);
                ui.draw(&b, Point::new(at.x + 26.0, at.y));
            }
            b.h
        }
    }
}

// ------------------------------------------------------------------------------------------------ the choice

fn queued_question_label(count: usize) -> String {
    if count == 1 { "1 question waiting".to_string() } else { format!("{count} questions waiting") }
}

fn draw_queued_questions(ui: &mut Ui, id: PaneId, c: &Typo, queued: bro_core::terminal_prompt::QueuedQuestions, r: Rect) {
    use bro_core::terminal_prompt::QuestionShortcut;
    let t = ui.t;
    ui.panel(r, radius::OVERLAY, t.warn.with_alpha(if t.light { 0.035 } else { 0.05 }), t.hairline.mix(t.warn, 0.35));
    let inner = Rect::new(r.x + 12.0, r.y + 10.0, (r.w - 24.0).max(0.0), r.h - 20.0);
    let label = ui.line(&queued_question_label(queued.count), Style::mono(c.small, t.text).weight(weight::SEMIBOLD), Some(inner.w));
    ui.draw(&label, Point::new(inner.x, inner.y));
    let button = Rect::new(inner.x, r.bottom() - 40.0, widgets::button_width(ui, "Answer").min(inner.w), 30.0);
    widgets::button(ui, button, "Answer", widgets::Kind::Primary, Action::Focus(id, ANSWER_QUEUED));
    let shortcut = match queued.shortcut { QuestionShortcut::ShiftLeft => "Shift+←", QuestionShortcut::AltUp => "Alt+↑" };
    let hint = ui.line(shortcut, Style::mono(c.small, t.muted), None);
    if hint.w + 12.0 <= inner.right() - button.right() {
        ui.draw(&hint, Point::new(button.right() + 12.0, button.y + (button.h - hint.h) / 2.0));
    }
}

fn choice_height(ui: &mut Ui, c: &Typo, ch: &Choice, w: f32) -> f32 {
    let inner = w - 32.0;
    let q = lay(ui, &[(ch.question.clone(), Style::mono(c.body, ui.t.text).weight(weight::SEMIBOLD))], c.body, 1.4, inner - 26.0, Some(3));
    let ctx = ch.context.iter().rev().take(3).count() as f32 * (c.small * 1.5).round();
    let opts: f32 = ch.options.iter().map(|o| option_height(c, o.detail.is_some())).sum::<f32>() + 4.0 * ch.options.len() as f32;
    16.0 + q.h + if ctx > 0.0 { 8.0 + ctx } else { 0.0 } + 12.0 + opts + 8.0 + (c.small * 1.6).round() + 12.0
}

fn option_height(c: &Typo, detail: bool) -> f32 {
    (c.body * 2.2).round() + if detail { (c.small * 1.4).round() } else { 0.0 }
}

fn draw_choice(ui: &mut Ui, id: PaneId, c: &Typo, ch: &Choice, r: Rect) {
    let t = ui.t;
    ui.panel(r, radius::OVERLAY, t.danger.with_alpha(if t.light { 0.035 } else { 0.05 }), t.danger.with_alpha(0.35));
    let inner = Rect::new(r.x + 16.0, r.y + 16.0, r.w - 32.0, r.h - 32.0);
    let mut y = inner.y;
    ui.icon(Rect::new(inner.x, y + 1.0, 16.0, 18.0), "\u{F0026}", c.body, t.danger);
    let q = lay(ui, &[(ch.question.clone(), Style::mono(c.body, t.text).weight(weight::SEMIBOLD))], c.body, 1.4, inner.w - 26.0, Some(3));
    ui.draw(&q, Point::new(inner.x + 26.0, y));
    y += q.h;
    let ctx: Vec<&String> = ch.context.iter().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
    if !ctx.is_empty() {
        y += 8.0;
        for l in ctx {
            let b = ui.line(l, Style::mono(c.small, c.muted), Some(inner.w - 26.0));
            ui.draw(&b, Point::new(inner.x + 26.0, y));
            y += (c.small * 1.5).round();
        }
    }
    y += 12.0;
    for (i, o) in ch.options.iter().enumerate() {
        let oh = option_height(c, o.detail.is_some());
        let or = Rect::new(inner.x, y, inner.w, oh);
        let hot = ui.hover_amount(("focus-opt", id, i), or);
        let fill = if o.focused { t.selected } else { t.text.with_alpha(0.04 + 0.05 * hot) };
        ui.round(or, radius::CONTROL, fill);
        if o.focused {
            ui.outline(or, radius::CONTROL, 1.0, t.accent.with_alpha(0.55));
        }
        let kw = widgets::kbd_width(ui, &o.key);
        widgets::kbd(ui, Point::new(or.x + 12.0, or.y + ((c.body * 2.2).round() - 18.0) / 2.0), &o.key);
        let lx = or.x + 12.0 + kw + 12.0;
        let top_h = (c.body * 2.2).round();
        let b = ui.line(&o.label, Style::mono(c.body, t.text).weight(if o.focused { weight::SEMIBOLD } else { weight::REGULAR }), Some(or.right() - lx - 12.0));
        ui.draw(&b, Point::new(lx, or.y + (top_h - b.h) / 2.0));
        if let Some(d) = &o.detail {
            let db = ui.line(d, Style::mono(c.small, c.muted), Some(or.right() - lx - 12.0));
            ui.draw(&db, Point::new(lx, or.y + top_h - 6.0));
        }
        ui.hit(or, Action::Focus(id, OPTION + i));
        y += oh + 4.0;
    }
    y += 8.0;
    let hint = "↑ ↓ to move · enter or number to pick · esc to cancel";
    let b = ui.line(hint, Style::mono(c.small, c.faint), Some(inner.w));
    ui.draw(&b, Point::new(inner.x, y));
}

// -------------------------------------------------------------------------------------------- the reply box

/// The reply box's padding and its footer's height (logical px).
const BOX_PAD: (f32, f32) = (18.0, 15.0);
/// The footer row: its pills and the round send button, with the air above and below them.
const FOOTER: f32 = 48.0;
/// The composer's corners (softer than a field's: it's the view's one place to write).
const BOX_RADIUS: f32 = 16.0;
/// The send / stop button.
const SEND_D: f32 = 30.0;
/// A picture waiting in the composer.
const THUMB_RADIUS: f32 = 10.0;

/// How tall the reply box is: the pictures going with it, up to eight rows of text, its footer.
fn reply_height(app: &mut App, id: PaneId, c: &Typo, w: f32) -> f32 {
    let cols = (((w - 2.0 * BOX_PAD.0) / c.adv).floor() as usize).max(8);
    let d = app.focus.draft(id);
    d.cols = cols;
    let rows = d.area.rows(cols).len().clamp(1, 8);
    let pics = if d.attachments.is_empty() { 0.0 } else { ATTACHED + 10.0 };
    BOX_PAD.1 + pics + rows as f32 * c.lh + FOOTER
}

/// The reply box: the pictures going with the message (a popover over the one under the pointer), what you're
/// writing (the pictures' tokens take no room), and a footer: the model and how hard it thinks on the left, each a
/// list to switch from, the key that acts on the right. Its lists (those two, the commands as you type `/`) float
/// over the conversation.
#[allow(clippy::too_many_arguments)]
fn reply_box(app: &mut App, ui: &mut Ui, id: PaneId, r: Rect, focused: bool, c: &Typo, seen: &Seen, mood: Mood, v: &mut View, media: &mut Media) {
    let choosing = v.choice.is_some();
    let dictating = app.dictation.phase(id);
    let focused = focused && !choosing;
    let t = ui.t;
    let (pad_x, pad_y) = BOX_PAD;
    let hot = ui.hover_amount(("focus-box", id), r);
    let lit = ui.ease(("focus-box-keys", id), if focused { 1.0 } else { 0.0 }, 140.0);
    // a surface a step up from the card, its edge a hairline: depth from a whisper of shadow, and a soft ring
    // round it while the keys are here
    let rest = if t.light { Color::WHITE } else { t.surface.mix(t.raised, 0.55) };
    let fill = rest.mix(t.raised, 0.6 * hot.max(lit));
    let edge = t.hairline.mix(t.border, hot.max(lit)).mix(t.accent, 0.3 * lit);
    ui.shadow(r, BOX_RADIUS, 28.0, 10.0, t.shadow.fade(if t.light { 0.55 } else { 0.7 }));
    // while the agent works, light flows round the box in its colour (Rune's aura): it flares as the turn starts,
    // settles to a glow, and fades when the turn ends
    let working = mood == Mood::Working;
    let flare = match v.mood {
        Some((Mood::Working, since)) => 1.6 * (-ui.now.saturating_duration_since(since).as_secs_f32() * 2.2).exp(),
        _ => 0.0,
    };
    let glow = ui.ease(("focus-aura", id), if working { 1.0 } else { 0.0 }, 420.0);
    let own = super::chrome::harness_mark(seen.harness).color(t);
    let aura = [if own == t.text { Color::hex(0x22d3ee) } else { own }, Color::hex(0x7c5cff), Color::hex(0x22d3ee)];
    ui.aura(r, BOX_RADIUS, aura, glow * (1.0 + flare));
    ui.shadow(r, BOX_RADIUS, 3.0, 1.0, t.shadow.fade(0.3));
    if lit > 0.01 {
        ui.outline(r.outset(3.0), BOX_RADIUS + 3.0, 3.0, t.accent.with_alpha(0.13 * lit));
    }
    ui.panel(r, BOX_RADIUS, fill, edge);
    ui.aura_rim(r, BOX_RADIUS, aura, glow * (1.0 + flare));
    ui.hit_with(r, Action::Focus(id, BOX), Cursor::Text);
    // what the footer and the lists need, before the box itself is borrowed
    let (model, effort) = app.focus_running_on(id);
    let models = app.focus_models(id);
    let levels = app.focus_levels(id);
    let waits = app.focus_change_waits(id);
    let commands = if choosing || dictating.is_some() { vec![] } else { app.focus_command_rows(id) };
    let quip_rows = if choosing || dictating.is_some() { vec![] } else { app.focus_quip_rows(id) };
    let quip_open = focused && (app.focus.drafts.get(&id).is_some_and(|d| d.quips.open || d.quips.editor.is_some()) || !quip_rows.is_empty());
    let menu = app.focus.drafts.get(&id).and_then(|d| d.menu.clone()).filter(|_| !choosing);
    let menu_rows: Vec<MenuRow> = menu.as_ref().map(|m| app.focus_menu_rows(id, m.kind)).unwrap_or_default();
    let Some(d) = app.focus.drafts.get_mut(&id) else { return };
    let attachments: Vec<PathBuf> = d.attachments.iter().map(|a| a.path.clone()).collect();
    let cols = d.cols.max(8);
    let rows = d.area.rows(cols);
    let text = d.area.text().to_string();
    let cur = d.area.cursor();
    let selected = d.area.selection();
    let quip_matches: Vec<_> = app.focus.quips.matches(&text).into_iter().map(|(r, q)| (r, q.instructions.clone())).collect();
    let blank = d.blank();
    let command_sel = d.command.min(commands.len().saturating_sub(1));
    // the pictures going with it; the one under the pointer opens its popover, which stays while the pointer
    // crosses to it
    // laid out as tall as the box will be, anchored to its foot: while it grows or shrinks, its edge moves over it
    let full_h = if v.box_full > 0.0 { v.box_full } else { r.h };
    let mut top = r.bottom() - full_h + pad_y;
    let growing = (full_h - r.h).abs() > 0.05;
    if growing {
        ui.clip(r, BOX_RADIUS);
    }
    let mut thumbs: Vec<Rect> = vec![];
    if !attachments.is_empty() {
        let mut p = Paint { draw: true, id, links: vec![], pictures: std::mem::take(&mut v.pictures), sounds: vec![], media: Some(&mut *media), resolve: None, apps: AppPaint::default(), text: TextPaint::default(), subs: None, opened: vec![], quip_hover: None };
        for (n, path) in attachments.iter().enumerate() {
            let pr = Rect::new(r.x + pad_x + n as f32 * (ATTACHED + 8.0), top, ATTACHED, ATTACHED);
            if pr.right() > r.right() - pad_x {
                break;
            }
            picture(ui, &mut p, path, pr, THUMB_RADIUS);
            ui.outline_over(pr, THUMB_RADIUS, 1.0, if v.pop == Some(n) { t.accent.with_alpha(0.75) } else { t.hairline });
            thumbs.push(pr);
        }
        v.pictures = std::mem::take(&mut p.pictures);
        top += ATTACHED + 10.0;
    }
    let over = thumbs.iter().position(|pr| ui.hovered(*pr));
    let stays = v.pop_rect.is_some_and(|pr| ui.hovered(pr.outset(12.0)));
    v.pop = over.or(v.pop.filter(|n| stays && *n < thumbs.len()));
    // what you're writing
    let room = 8usize;
    let crow = rows.iter().rposition(|&(s, e)| cur >= s && cur <= e).unwrap_or(0);
    let first = crow.saturating_sub(room - 1).min(rows.len().saturating_sub(room.min(rows.len())));
    let origin = Point::new(r.x + pad_x, top - first as f32 * c.lh);
    v.geo = Some(BoxGeo { origin, adv: c.adv, cols, row_h: c.lh, first });
    ui.clip(Rect::new(r.x, top - 2.0, r.w, rows.len().clamp(1, room) as f32 * c.lh + 4.0), 0.0);
    let st = Style::mono(c.body, t.text);
    if visible(&text).is_empty() {
        let ph = if v.choice.is_some() {
            "Pick an answer above…".to_string()
        } else if !attachments.is_empty() {
            "Add a few words, or just send it…".to_string()
        } else {
            format!("Reply to {}…", agent_name(seen.harness))
        };
        let b = ui.line(&ph, Style::mono(c.body, t.faint), Some(r.w - 2.0 * pad_x));
        // a hair past the caret, so the caret doesn't sit on its first letter
        let nudge = if focused { 2.0 } else { 0.0 };
        ui.draw(&b, Point::new(r.x + pad_x + nudge, top + (c.lh - b.h) / 2.0));
    }
    let mut caret_at = None;
    for (n, &(s, e)) in rows.iter().enumerate().skip(first).take(room) {
        let y = origin.y + n as f32 * c.lh;
        let b = ui.line(&visible(&text[s..e]), st, None);
        let at = Point::new(origin.x, y + (c.lh - b.h) / 2.0);
        // the selection, under the words (quieter while the box doesn't have the keys)
        if let Some(sel) = &selected {
            let (a, z) = (sel.start.max(s), sel.end.min(e));
            // the line break after this row is selected (the next row starts past it): a sliver shows it
            let off = rows.get(n + 1).is_some_and(|r| r.0 > e) && sel.start <= e && sel.end > e;
            if a < z || off {
                let x0 = b.layout.caret_x(visible(&text[s..a]).len()) / ui.scale;
                let x1 = b.layout.caret_x(visible(&text[s..z]).len()) / ui.scale + if off { c.adv * 0.5 } else { 0.0 };
                let ink = if focused { t.selection } else { t.selection.fade(0.55) };
                ui.round(Rect::new(origin.x + x0, y + 1.0, (x1 - x0).max(1.0), c.lh - 2.0), 2.0, ink);
            }
        }
        for (range, instructions) in &quip_matches {
            let start = range.start.max(s);
            let end = range.end.min(e);
            if start < end {
                let local = visible(&text[s..start]).len()..visible(&text[s..end]).len();
                if let Some(preview) = quips::badge(ui, &b, at, &local, &text[range.clone()], instructions, c.body) {
                    v.quip_preview.hovered = Some(preview);
                }
            }
        }
        ui.draw(&b, at);
        for (range, _) in &quip_matches {
            let start = range.start.max(s);
            let end = range.end.min(e);
            if start < end {
                let prefix = ui.measure(&visible(&text[s..start]), st);
                let mark = ui.line(&visible(&text[start..end]), st.weight(weight::SEMIBOLD).color(quips::badge_ink(ui)), None);
                ui.draw(&mark, Point::new(origin.x + prefix, at.y));
            }
        }
        if n == crow {
            caret_at = Some(Point::new(origin.x + b.layout.caret_x(visible(&text[s..cur]).len()) / ui.scale, y));
        }
    }
    ui.unclip();
    // a steady caret, as the terminals' cursor is (a blink would keep the window drawing)
    if focused && dictating.is_none() && let Some(at) = caret_at {
        ui.top_fill(Rect::new(at.x, at.y + (c.lh - c.body * 1.25) / 2.0, 1.8, c.body * 1.25), 0.9, t.accent);
        v.caret = Some(Rect::new(at.x, at.y, 2.0, c.lh));
    }
    // the footer: a round button that acts on the right, the model and how hard it thinks on the left
    let fmid = r.bottom() - 10.0 - SEND_D / 2.0;
    let working = mood == Mood::Working || seen.limit.as_ref().is_some_and(|limit| limit.is_retrying());
    let act = if choosing {
        Go::Idle
    } else if !commands.is_empty() {
        Go::Run
    } else if dictating.is_some() || !blank || !attachments.is_empty() {
        Go::Send
    } else if working {
        Go::Stop
    } else {
        Go::Idle
    };
    let send = Rect::new(r.right() - 10.0 - SEND_D, fmid - SEND_D / 2.0, SEND_D, SEND_D);
    send_button(ui, id, send, act);
    let right = if dictating.is_some() || (!choosing && !app.dictation.active()) {
        dictation_controls(ui, id, send.x - 8.0, fmid, dictating)
    } else {
        send.x - 8.0
    };
    let later = if waits { " · Codex switches when this turn ends" } else { "" };
    let mut x = r.x + pad_x;
    let mut anchors: [Option<Rect>; 2] = [None, None];
    if (model.is_some() || models.is_some()) && right - x > 64.0 {
        let name = model.as_deref().map_or_else(|| "Model".to_string(), |m| models.as_ref().and_then(|ms| ms.iter().find(|c| same_model(m, c))).map_or_else(|| model_name(m), |c| c.name.clone()));
        let open = menu.as_ref().is_some_and(|m| m.kind == MenuKind::Model);
        let cr = chip(ui, id, Point::new(x, fmid), &name, MODEL_CHIP, models.is_some(), open, right - x);
        if !open && ui.hovered(cr) {
            let tip = if models.is_some() { format!("Switch the model{later}") } else { format!("{name} · bro has no list of this provider's models yet") };
            widgets::tooltip(ui, cr, &tip);
        }
        anchors[0] = Some(cr);
        x = cr.right() + 14.0;
    }
    if (levels.is_some() || effort.is_some()) && right - x > 100.0 {
        let label = effort.as_deref().map_or_else(|| "Thinking".to_string(), thinking_label);
        let open = menu.as_ref().is_some_and(|m| m.kind == MenuKind::Thinking);
        let cr = chip(ui, id, Point::new(x, fmid), &label, THINKING_CHIP, levels.is_some(), open, right - x);
        if !open && ui.hovered(cr) && levels.is_some() {
            widgets::tooltip(ui, cr, &format!("How hard it thinks{later}"));
        }
        anchors[1] = Some(cr);
    }
    if growing {
        ui.unclip();
    }
    // the lists, over everything (they slide in)
    let k = ui.ease(("focus-menu", id), if menu.is_some() { 1.0 } else { 0.0 }, 120.0);
    if let Some(m) = menu.as_ref()
        && let Some(anchor) = anchors[if m.kind == MenuKind::Model { 0 } else { 1 }]
        && (!menu_rows.is_empty() || !m.query.is_empty())
    {
        let title = if m.kind == MenuKind::Model { "Model" } else { "Thinking" };
        // a provider's long list searches as you type
        let searchable = m.kind == MenuKind::Model && models.as_ref().is_some_and(|ms| ms.len() > 8);
        let note = waits.then_some("Codex switches when this turn ends");
        pick_menu(ui, id, title, note, &menu_rows, m.sel, &m.query, searchable, anchor, k, &mut v.menu_top);
    }
    let kc = ui.ease(("focus-commands", id), if commands.is_empty() { 0.0 } else { 1.0 }, 120.0);
    if !commands.is_empty() {
        commands_list(ui, id, c, &commands, command_sel, r, v, kc);
    }
    if quip_open {
        quips::draw(app, ui, id, r, &quip_rows, v);
    }
    v.pop_rect = None;
    if let Some(n) = v.pop
        && let (Some(path), Some(anchor)) = (attachments.get(n), thumbs.get(n))
    {
        v.pop_rect = Some(attachment_popover(ui, id, path, n, *anchor, media));
    }
    v.popup = quip_open || menu.is_some() || !commands.is_empty() || v.pop_rect.is_some();
}

pub(super) fn dictation_controls(ui: &mut Ui, id: PaneId, right: f32, mid: f32, phase: Option<crate::app::dictation::Phase>) -> f32 {
    use crate::app::dictation::Phase;
    let t = ui.t;
    let action = Rect::new(right - 26.0, mid - 13.0, 26.0, 26.0);
    let Some(phase) = phase else {
        if widgets::icon_button(ui, action, "\u{F036C}", t.muted, Action::Focus(id, DICTATE)) {
            let label = if cfg!(target_os = "macos") { "Dictate (⌘⇧D)" } else { "Dictate (Ctrl+Shift+D)" };
            widgets::tooltip(ui, action, label);
        }
        return action.x - 8.0;
    };
    let cancel = Rect::new(action.x - 28.0, action.y, action.w, action.h);
    if widgets::icon_button(ui, cancel, "\u{F0156}", t.muted, Action::Focus(id, CANCEL_DICTATION)) {
        widgets::tooltip(ui, cancel, "Cancel dictation (Esc)");
    }
    let label = match phase {
        Phase::Recording(level) => {
            if widgets::icon_button(ui, action, "\u{F012C}", t.good, Action::Focus(id, DICTATE)) {
                widgets::tooltip(ui, action, "Finish recording");
            }
            for (i, weight) in [0.5, 0.8, 1.0, 0.8, 0.5].into_iter().enumerate() {
                let height = 3.0 + level * weight * 14.0;
                ui.round(Rect::new(cancel.x - 24.0 + i as f32 * 4.0, mid - height / 2.0, 2.0, height), 1.0, t.danger);
            }
            return cancel.x - 32.0;
        }
        Phase::Transcribing => "Transcribing…",
        Phase::Cleaning => {
            if widgets::icon_button(ui, action, "\u{F04AD}", t.muted, Action::Focus(id, SKIP_CLEANUP)) {
                widgets::tooltip(ui, action, "Skip cleanup");
            }
            "Cleaning up…"
        }
    };
    let text = ui.line(label, Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
    let x = cancel.x - text.w - 6.0;
    ui.draw(&text, Point::new(x, mid - text.h / 2.0));
    x - 8.0
}

/// What the composer's round button does now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Go {
    /// a `/command` is picked: run it
    Run,
    /// words or pictures to send
    Send,
    /// nothing written while the agent works: stop it
    Stop,
    /// nothing to do: the button rests, quiet
    Idle,
}

/// "High thinking", "Extra high thinking", "Thinking off".
fn thinking_label(level: &str) -> String {
    let name = level_name(level);
    if name == "off" {
        return "Thinking off".into();
    }
    let mut chars = name.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    format!("{first}{} thinking", chars.as_str())
}

/// The composer's round button: send (accent), stop (ink, a square), or resting when there's nothing to do. Its
/// tooltip names the key that does the same.
fn send_button(ui: &mut Ui, id: PaneId, r: Rect, go: Go) {
    let t = ui.t;
    let live = go != Go::Idle;
    let hot = if live { ui.hover_amount(("focus-go", id), r) } else { 0.0 };
    let shown = ui.ease(("focus-go-live", id), if live { 1.0 } else { 0.0 }, 120.0);
    let center = Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
    let soft = t.text.with_alpha(if t.light { 0.07 } else { 0.09 });
    let fill = match go {
        // a soft disc with a solid square: unlike send's solid accent in every theme, even one whose accent is ink
        Go::Stop => t.text.with_alpha((if t.light { 0.09 } else { 0.12 }) + 0.06 * hot),
        Go::Run | Go::Send => soft.mix(t.accent.mix(t.text, 0.14 * hot), shown),
        // at rest: a faint disc, as a disabled button is
        Go::Idle => soft,
    };
    ui.circle(center, r.w / 2.0, fill);
    match go {
        Go::Stop => ui.round(Rect::new(center.x - 5.0, center.y - 5.0, 10.0, 10.0), 2.5, t.text),
        Go::Run | Go::Send => ui.icon(Rect::new(center.x - 9.0, center.y - 9.0, 18.0, 18.0), "\u{F005D}", 16.0, t.on_accent),
        Go::Idle => ui.icon(Rect::new(center.x - 9.0, center.y - 9.0, 18.0, 18.0), "\u{F005D}", 16.0, t.faint),
    }
    if live {
        let target = if go == Go::Stop { STOP } else { SEND };
        ui.hit(r, Action::Focus(id, target));
        if ui.hovered(r) {
            let tip = match go {
                Go::Run => "Run · Enter",
                Go::Send => "Send · Enter",
                _ => "Stop · Esc",
            };
            widgets::tooltip(ui, r, tip);
        }
    }
}

/// A quiet pill in the composer's footer, at `at` (its label's left, centred on its line) and at most `room` wide,
/// that opens a list when it's `pickable`: the model, how hard it thinks. Returns where it is.
#[allow(clippy::too_many_arguments)]
fn chip(ui: &mut Ui, id: PaneId, at: Point, label: &str, target: usize, pickable: bool, open: bool, room: f32) -> Rect {
    let t = ui.t;
    let arrow = if pickable { 16.0 } else { 0.0 };
    let b = ui.line(label, Style::new(size::BODY, weight::MEDIUM, t.muted), Some((room - arrow - 20.0).max(24.0)));
    let h = 28.0;
    let r = Rect::new(at.x - 10.0, at.y - h / 2.0, b.w + arrow + 20.0, h);
    let hot = if pickable { ui.hover_amount(("focus-chip", id, target), r) } else { 0.0 };
    let lit = if open { 1.0 } else { hot };
    if lit > 0.01 {
        ui.round(r, h / 2.0, t.text.with_alpha((if t.light { 0.055 } else { 0.07 }) * lit));
    }
    let ink = t.muted.mix(t.text, 0.85 * lit);
    ui.draw_color(&b, Point::new(at.x, at.y - b.h / 2.0), ink);
    if pickable {
        ui.icon(Rect::new(at.x + b.w + 2.0, at.y - 7.0, 14.0, 14.0), if open { "\u{F0143}" } else { "\u{F0140}" }, 13.0, ink.fade(0.8));
        ui.hit(r, Action::Focus(id, target));
    }
    r
}

/// The model or thinking list over its word in the footer: each row its name and what it's for, a check on what
/// it runs on, the keys' row lit. What's typed narrows it (`searchable`: a provider's long list says so), and it
/// scrolls eight rows at a time. On a layer of its own, `k` of the way in.
#[allow(clippy::too_many_arguments)]
fn pick_menu(ui: &mut Ui, id: PaneId, title: &str, note: Option<&str>, rows: &[MenuRow], sel: usize, query: &str, searchable: bool, anchor: Rect, k: f32, top: &mut usize) {
    let t = ui.t;
    let prev = ui.layer();
    let detail = rows.iter().any(|r| r.detail.is_some());
    let row_h = if detail { 48.0 } else { 34.0 };
    let shown = rows.len().clamp(1, 8);
    let sel = sel.min(rows.len().saturating_sub(1));
    if sel < *top {
        *top = sel;
    } else if sel >= *top + shown {
        *top = sel + 1 - shown;
    }
    *top = (*top).min(rows.len().saturating_sub(shown));
    let (head, foot) = (32.0, if note.is_some() { 28.0 } else { 0.0 });
    let w = if searchable { 380.0f32 } else { 330.0 }.min(ui.screen.w - 16.0);
    let h = head + shown as f32 * row_h + foot + 6.0;
    let x = anchor.x.min(ui.screen.right() - w - 8.0).max(ui.screen.x + 8.0);
    let y = (anchor.y - 8.0 - h).max(ui.screen.y + 8.0) + (1.0 - k) * 8.0;
    let r = Rect::new(x, y, w, h);
    widgets::card(ui, r, radius::OVERLAY);
    let tb = ui.line(title, Style::new(size::SMALL, weight::SEMIBOLD, t.muted), None);
    ui.draw(&tb, Point::new(r.x + 14.0, r.y + (head - tb.h) / 2.0 + 1.0));
    // what's typed, at the top's right
    if searchable || !query.is_empty() {
        let (text, ink) = if query.is_empty() { ("type to search", t.faint) } else { (query, t.text) };
        let qb = ui.line(text, Style::mono(size::SMALL, ink), Some(r.w - tb.w - 64.0));
        let qx = r.right() - 14.0 - qb.w - if query.is_empty() { 0.0 } else { 3.0 };
        ui.icon(Rect::new(qx - 20.0, r.y + (head - 16.0) / 2.0 + 1.0, 16.0, 16.0), "\u{F0349}", 13.0, t.faint);
        ui.draw(&qb, Point::new(qx, r.y + (head - qb.h) / 2.0 + 1.0));
        if !query.is_empty() {
            ui.top_fill(Rect::new(qx + qb.w + 1.0, r.y + (head - 14.0) / 2.0 + 1.0, 1.5, 14.0), 0.5, t.accent);
        }
    }
    let mut ry = r.y + head;
    if rows.is_empty() {
        let nb = ui.line(&format!("Nothing matches “{query}”"), Style::new(size::SMALL, weight::REGULAR, t.muted), Some(r.w - 28.0));
        ui.draw(&nb, Point::new(r.x + 14.0, ry + (row_h - nb.h) / 2.0));
        ry += row_h;
    }
    for (n, row) in rows.iter().enumerate().skip(*top).take(shown) {
        let rr = Rect::new(r.x + 4.0, ry, r.w - 8.0, row_h);
        widgets::row(ui, rr, ("focus-menu-row", id, n), n == sel, Action::Focus(id, MENU_ROW + n));
        let lb = ui.line(&row.label, Style::new(size::BODY, if row.current { weight::SEMIBOLD } else { weight::REGULAR }, t.text), Some(rr.w - 64.0));
        let ly = if detail { rr.y + 7.0 } else { rr.y + (row_h - lb.h) / 2.0 };
        ui.draw(&lb, Point::new(rr.x + 10.0, ly));
        if row.default && !row.current {
            let db = ui.line("default", Style::new(size::CAPTION, weight::MEDIUM, t.faint), None);
            ui.draw(&db, Point::new(rr.x + 10.0 + lb.w + 8.0, ly + (lb.h - db.h) / 2.0 + 1.0));
        }
        if let Some(dt) = &row.detail {
            let db = ui.line(dt, Style::new(size::SMALL, weight::REGULAR, t.muted), Some(rr.w - 48.0));
            ui.draw(&db, Point::new(rr.x + 10.0, ly + lb.h + 3.0));
        }
        if row.current {
            ui.icon(Rect::new(rr.right() - 30.0, rr.y + (row_h - 18.0) / 2.0, 18.0, 18.0), "\u{F012C}", 16.0, t.accent);
        }
        ry += row_h;
    }
    if rows.len() > shown {
        let list = shown as f32 * row_h;
        widgets::scrollbar(ui, Rect::new(r.right() - 9.0, r.y + head, 6.0, list), rows.len() as f32 * row_h, list, *top as f32 * row_h, 0.7);
    }
    if let Some(note) = note {
        ui.hline(r.x + 10.0, ry + 3.0, r.w - 20.0, t.hairline);
        let nb = ui.line(note, Style::new(size::CAPTION, weight::REGULAR, t.faint), Some(r.w - 28.0));
        ui.draw(&nb, Point::new(r.x + 14.0, ry + 3.0 + (foot - nb.h) / 2.0));
    }
    ui.set_layer(prev);
}

/// The commands matching what's typed after `/`, over the reply box: each its name, what follows it and what it
/// does, the keys' row lit. Eight at a time, scrolled to keep the keys' row in view; `k` of the way in.
#[allow(clippy::too_many_arguments)]
fn commands_list(ui: &mut Ui, id: PaneId, c: &Typo, rows: &[Command], sel: usize, over: Rect, v: &mut View, k: f32) {
    let t = ui.t;
    let prev = ui.layer();
    let n = rows.len();
    let shown = n.min(8);
    if sel < v.command_top {
        v.command_top = sel;
    } else if sel >= v.command_top + shown {
        v.command_top = sel + 1 - shown;
    }
    v.command_top = v.command_top.min(n - shown);
    let (row_h, foot) = (34.0, 30.0);
    let h = 8.0 + shown as f32 * row_h + foot;
    let r = Rect::new(over.x, over.y - 8.0 - h + (1.0 - k) * 8.0, over.w, h);
    widgets::card(ui, r, radius::OVERLAY);
    let mono = Style::mono(c.small, t.text);
    let window = &rows[v.command_top..v.command_top + shown];
    let col = window
        .iter()
        .map(|cmd| ui.measure(&format!("/{}", cmd.name), mono) + cmd.hint.as_deref().map_or(0.0, |h| 8.0 + ui.measure(h, mono)))
        .fold(0.0, f32::max)
        .min(r.w * 0.5);
    let mut y = r.y + 4.0;
    for (i, cmd) in rows.iter().enumerate().skip(v.command_top).take(shown) {
        let rr = Rect::new(r.x + 4.0, y, r.w - 8.0, row_h);
        widgets::row(ui, rr, ("focus-command", id, i), i == sel, Action::Focus(id, COMMAND_ROW + i));
        let nb = ui.line(&format!("/{}", cmd.name), mono.weight(weight::MEDIUM), Some(col.max(24.0)));
        ui.draw(&nb, Point::new(rr.x + 10.0, rr.y + (row_h - nb.h) / 2.0));
        if let Some(hint) = &cmd.hint
            && col - nb.w > 32.0
        {
            let hb = ui.line(hint, Style::mono(c.small, t.faint), Some(col - nb.w - 8.0));
            ui.draw(&hb, Point::new(rr.x + 10.0 + nb.w + 8.0, rr.y + (row_h - hb.h) / 2.0));
        }
        let dx = rr.x + 10.0 + col + 24.0;
        if let Some(d) = &cmd.detail
            && rr.right() - 10.0 - dx > 40.0
        {
            let db = ui.line(d, Style::new(size::SMALL, weight::REGULAR, t.muted), Some(rr.right() - 10.0 - dx));
            ui.draw(&db, Point::new(dx, rr.y + (row_h - db.h) / 2.0));
        }
        y += row_h;
    }
    if n > shown {
        widgets::scrollbar(ui, Rect::new(r.right() - 9.0, r.y + 4.0, 6.0, shown as f32 * row_h), n as f32 * row_h, shown as f32 * row_h, v.command_top as f32 * row_h, 0.7);
    }
    let fy = r.bottom() - foot;
    ui.hline(r.x + 10.0, fy, r.w - 20.0, t.hairline);
    let hint = if n == 1 { "tab completes · enter runs · esc closes".to_string() } else { format!("{n} commands · tab completes · enter runs · esc closes") };
    let hb = ui.line(&hint, Style::new(size::CAPTION, weight::REGULAR, t.faint), Some(r.w - 28.0));
    ui.draw(&hb, Point::new(r.x + 14.0, fy + (foot - hb.h) / 2.0));
    ui.set_layer(prev);
}

/// The popover over a picture waiting in the reply box: the picture bigger, its name, and the ways to look at it
/// or take it off the message (deleting its place in the text doesn't). Returns where it is.
fn attachment_popover(ui: &mut Ui, id: PaneId, path: &Path, n: usize, anchor: Rect, media: &mut Media) -> Rect {
    let t = ui.t;
    let prev = ui.layer();
    let (w, img_h) = (248.0, 156.0);
    let h = 10.0 + img_h + 10.0 + 18.0 + 10.0 + 28.0 + 10.0;
    let x = (anchor.x - 10.0).min(ui.screen.right() - w - 8.0).max(ui.screen.x + 8.0);
    let y = (anchor.y - 10.0 - h).max(ui.screen.y + 8.0);
    let r = Rect::new(x, y, w, h);
    widgets::card(ui, r, radius::OVERLAY);
    let well = Rect::new(r.x + 10.0, r.y + 10.0, w - 20.0, img_h);
    ui.round(well, radius::CONTROL + 2.0, t.text.with_alpha(0.05));
    match media.thumb(path) {
        Thumbnail::Ready(tex, iw, ih) => {
            let s = (well.w / iw.max(1) as f32).min(well.h / ih.max(1) as f32).min(2.0);
            let (dw, dh) = (iw as f32 * s, ih as f32 * s);
            ui.image(tex, Rect::new(well.x + (well.w - dw) / 2.0, well.y + (well.h - dh) / 2.0, dw, dh), [0.0, 0.0, 1.0, 1.0], radius::CONTROL, 1.0);
        }
        Thumbnail::Loading => ui.spinner(well.center(), 6.0, 2.0, t.faint),
        Thumbnail::Failed => ui.icon(well, "\u{F02E9}", 26.0, t.faint),
    }
    let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let nb = ui.line(&name, Style::new(size::SMALL, weight::MEDIUM, t.text), Some(w - 20.0));
    ui.draw(&nb, Point::new(r.x + 10.0, well.bottom() + 10.0 + (18.0 - nb.h) / 2.0));
    let by = well.bottom() + 38.0;
    let half = (w - 28.0) / 2.0;
    widgets::button(ui, Rect::new(r.x + 10.0, by, half, 28.0), "Open", widgets::Kind::Secondary, Action::Focus(id, SHOW_ATTACHED + n));
    widgets::button(ui, Rect::new(r.x + 18.0 + half, by, half, 28.0), "Remove", widgets::Kind::Ghost, Action::Focus(id, UNATTACH + n));
    ui.set_layer(prev);
    r
}

// ------------------------------------------------------------------------------- selecting the conversation

/// Keep the text of the runs drawn (a piece of a reply still being written grows in place).
fn keep_text(kept: &mut BTreeMap<RunKey, RunText>, runs: &[Run]) {
    for r in runs {
        let fresh = RunText { gap: r.gap, lead: r.lead.clone(), text: r.layout.text.clone() };
        if kept.get(&r.key) != Some(&fresh) {
            kept.insert(r.key, fresh);
        }
    }
}

/// The byte of `run` nearest `p` (the line under it, else the nearest line; a point past the end is its end).
fn byte_in(run: &Run, p: Point) -> usize {
    let l = &run.layout;
    let (x, y) = ((p.x - run.at.x) * run.scale, (p.y - run.at.y) * run.scale);
    if l.lines.is_empty() {
        return if y < 0.0 { 0 } else { l.text.len() };
    }
    if y < 0.0 {
        return 0;
    }
    if y >= l.height() {
        return l.text.len();
    }
    let line = l.lines.iter().rev().find(|line| line.top <= y).unwrap_or(&l.lines[0]);
    for g in &l.glyphs[line.glyphs.clone()] {
        if x < g.x + g.w * 0.5 {
            return g.start.max(line.text.start);
        }
    }
    line.text.end
}

/// The place in the conversation's text under `p`, or the nearest one: in the piece it's over, else the end of
/// the piece above it (between two paragraphs, a drag takes the first whole), else the start of the first.
fn pos_at(runs: &[Run], p: Point) -> Option<Pos> {
    let first = runs.first()?;
    if let Some(r) = runs.iter().find(|r| r.rect().contains(p)) {
        return Some(Pos { key: r.key, byte: byte_in(r, p) });
    }
    // the last piece starting above the point: on its lines when level with them (left or right of a message),
    // else past its end
    match runs.iter().rev().find(|r| r.at.y <= p.y) {
        Some(r) => Some(Pos { key: r.key, byte: byte_in(r, p) }),
        None => Some(Pos { key: first.key, byte: 0 }),
    }
}

/// What a press with `count` clicks takes at `pos`: nothing yet, the word under it, or its whole piece.
fn press_span(runs: &[Run], pos: Pos, count: u32) -> (Pos, Pos) {
    let text = runs.iter().find(|r| r.key == pos.key).map(|r| r.layout.text.as_str()).unwrap_or_default();
    let range = match count {
        1 => return (pos, pos),
        2 => crate::textarea::word_range(text, pos.byte),
        _ => 0..text.len(),
    };
    (Pos { key: pos.key, byte: range.start }, Pos { key: pos.key, byte: range.end })
}

/// The selection grown from what the press took to `to`: (anchor, head).
fn grow(origin: (Pos, Pos), to: Pos) -> (Pos, Pos) {
    if to < origin.0 {
        (origin.1, to)
    } else if to > origin.1 {
        (origin.0, to)
    } else {
        origin
    }
}

/// The selected text as plain text: each piece's words, list items with their marks, paragraphs a blank line
/// apart. An item the selection crosses that was never drawn (scrolled past at speed) copies as the transcript
/// has it (`raw`).
fn selected_text(sel: (Pos, Pos), kept: &BTreeMap<RunKey, RunText>, raw: &dyn Fn(usize) -> Option<String>) -> String {
    let (s, e) = sel;
    let mut out = String::new();
    let mut last_item: Option<usize> = None;
    let push = |out: &mut String, gap: &str, piece: &str| {
        if !out.is_empty() && !piece.is_empty() {
            out.push_str(gap);
        }
        out.push_str(piece);
    };
    for (key, rt) in kept.range(s.key..=e.key) {
        if let Some(li) = last_item {
            for i in li + 1..key.0 {
                if let Some(text) = raw(i) {
                    push(&mut out, "\n\n", &text);
                }
            }
        }
        let len = rt.text.len();
        let mut start = if *key == s.key { s.byte.min(len) } else { 0 };
        let mut end = if *key == e.key { e.byte.min(len) } else { len };
        while !rt.text.is_char_boundary(start) {
            start -= 1;
        }
        while !rt.text.is_char_boundary(end) {
            end -= 1;
        }
        let gap = if last_item.is_some_and(|li| li != key.0) { "\n\n" } else { rt.gap };
        last_item = Some(key.0);
        if start >= end && !(start == 0 && len == 0 && s.key < *key && *key < e.key) {
            continue;
        }
        let lead = if start == 0 { rt.lead.as_str() } else { "" };
        let piece = format!("{lead}{}", &rt.text[start..end]);
        if piece.is_empty() {
            // a blank line of code
            if !out.is_empty() {
                out.push_str(gap);
            }
            continue;
        }
        push(&mut out, gap, &piece);
    }
    out.trim_end().to_string()
}

/// The byte a click at `p` in the reply box puts the caret on.
fn byte_at(ta: &crate::textarea::TextArea, g: &BoxGeo, p: Point, ui_measure: &mut dyn FnMut(&str) -> f32) -> usize {
    let rows = ta.rows(g.cols);
    let n = (((p.y - g.origin.y) / g.row_h).floor().max(0.0) as usize).clamp(g.first, rows.len().saturating_sub(1).max(g.first)).min(rows.len().saturating_sub(1));
    let Some(&(s, e)) = rows.get(n) else { return ta.text().len() };
    let text = ta.text();
    let mut best = e;
    for (i, _) in text[s..e].char_indices().chain(std::iter::once((e - s, ' '))) {
        let w = ui_measure(&text[s..s + i]);
        if g.origin.x + w >= p.x - g.adv / 2.0 {
            best = s + i;
            break;
        }
    }
    best
}

// --------------------------------------------------------------------------------------------- the mouse

pub(super) fn draw_quips_overlay(app: &mut App, ui: &mut Ui, id: PaneId, window: Rect) {
    quips::draw_overlay(app, ui, id, window);
}

pub(super) fn terminal_quips_height(app: &App, id: PaneId) -> f32 {
    if app.focus.drafts.get(&id).is_some_and(|d| !d.terminal_quips.pending.is_empty()) { 34.0 } else { 0.0 }
}

pub(super) fn draw_terminal_quips(app: &mut App, ui: &mut Ui, id: PaneId, content: Rect, focused: bool, input: Option<Rect>, view: &mut View) {
    quips::draw_terminal(app, ui, id, content, focused, input, view);
}

/// The reply box's byte under `p` (as last drawn).
fn box_byte(g: &mut Gui, id: PaneId, p: Point) -> Option<usize> {
    let geo = g.panes.get(&id).and_then(|st| st.focus.geo)?;
    let adv = geo.adv;
    let d = g.app.focus.draft(id);
    // the terminal font's cells: as many as the characters before the click
    let mut measure = |s: &str| unicode_width::UnicodeWidthStr::width(s) as f32 * adv;
    Some(byte_at(&d.area, &geo, p, &mut measure))
}

/// A press in the reply box: the caret goes there (a double-click takes the word, a triple the line;
/// shift grows the selection to it), and a drag selects from it.
fn press_box(g: &mut Gui, id: PaneId, p: Point, count: u32) {
    let Some(at) = box_byte(g, id, p) else { return };
    let shift = g.mods.shift_key();
    let d = g.app.focus.draft(id);
    let origin = if shift && count == 1 {
        let anchor = d.area.anchor();
        d.area.select(anchor, at);
        (anchor, anchor)
    } else {
        let r = match count {
            1 => at..at,
            2 => crate::textarea::word_range(d.area.text(), at),
            _ => crate::textarea::line_range(d.area.text(), at),
        };
        d.area.select(r.start, r.end);
        (r.start, r.end)
    };
    if let Some(st) = g.panes.get_mut(&id) {
        st.focus.box_origin = Some(origin);
    }
    g.drag = Some(Drag::FocusBox { pane: id });
}

/// The pointer dragged from a press in the reply box: the selection runs to it (rows past the box's edge
/// scroll into it, as the caret follows).
pub(super) fn drag_box(g: &mut Gui, id: PaneId, p: Point) {
    let Some(origin) = g.panes.get(&id).and_then(|st| st.focus.box_origin) else { return };
    let Some(at) = box_byte(g, id, p) else { return };
    let (anchor, head) = if at < origin.0 {
        (origin.1, at)
    } else if at > origin.1 {
        (origin.0, at)
    } else {
        origin
    };
    g.app.focus.draft(id).area.select(anchor, head);
    g.dirty = true;
}

/// A press on the conversation: a selection starts there (a double-click takes the word, a triple the whole
/// piece; shift grows the one there). `click` is what the press does as a plain click (a link opens, a long
/// message shows whole): on release, if the pointer didn't move.
fn press_text(g: &mut Gui, id: PaneId, p: Point, count: u32, click: Option<usize>) {
    let shift = g.mods.shift_key();
    let Some(v) = g.panes.get_mut(&id).map(|st| &mut st.focus) else { return };
    let Some(pos) = pos_at(&v.runs, p) else {
        // no text to select: it's just the click
        if let Some(n) = click {
            clicked(g, id, n, p);
        }
        return;
    };
    match v.sel.as_mut() {
        Some(sel) if shift && count == 1 => {
            sel.origin = (sel.anchor, sel.anchor);
            sel.head = pos;
            sel.press = p;
            sel.click = None;
            sel.moved = true;
        }
        _ => {
            let origin = press_span(&v.runs, pos, count);
            v.sel = Some(Sel { origin, anchor: origin.0, head: origin.1, press: p, click: click.filter(|_| count == 1), moved: false });
            v.sel_text.clear();
            keep_text(&mut v.sel_text, &v.runs);
        }
    }
    g.drag = Some(Drag::FocusText { pane: id });
}

/// The pointer dragged from a press on the conversation: the selection runs to it. Past the top or the bottom
/// the conversation scrolls, and the selection with it.
pub(super) fn drag_text(g: &mut Gui, id: PaneId, p: Point) {
    let Some(v) = g.panes.get_mut(&id).map(|st| &mut st.focus) else { return };
    let Some(sel) = v.sel.as_ref() else { return };
    if !sel.moved && (p.x - sel.press.x).abs().max((p.y - sel.press.y).abs()) < 4.0 {
        return;
    }
    let a = v.text_area;
    if p.y < a.y + 6.0 {
        v.scroll_pixels(((a.y + 6.0 - p.y) * 0.5).clamp(4.0, 60.0));
    } else if p.y > a.bottom() - 6.0 {
        v.scroll_pixels(-((p.y - a.bottom() + 6.0) * 0.5).clamp(4.0, 60.0));
    }
    let Some(pos) = pos_at(&v.runs, p) else { return };
    if let Some(sel) = v.sel.as_mut() {
        sel.moved = true;
        sel.click = None;
        (sel.anchor, sel.head) = grow(sel.origin, pos);
    }
    g.dirty = true;
}

/// The press on the conversation let go: a click that didn't move does what it was for; a selection is
/// copied, as a terminal's is.
pub(super) fn release_text(g: &mut Gui, id: PaneId, p: Point) {
    let Some(v) = g.panes.get_mut(&id).map(|st| &mut st.focus) else { return };
    let Some(sel) = v.sel.clone() else { return };
    if !sel.moved
        && let Some(n) = sel.click
    {
        v.sel = None;
        clicked(g, id, n, p);
        return;
    }
    if sel.span().is_none() {
        v.sel = None;
        return;
    }
    if let Some(text) = output_text(g, id) {
        super::window::set_clipboard(&text);
        copied_toast(g, &text);
    }
}

fn copied_toast(g: &mut Gui, text: &str) {
    let n = text.chars().count();
    g.app.toast(crate::alerts::Kind::Info, format!("copied {n} character{}", if n == 1 { "" } else { "s" }));
}

/// What's selected in `id`'s conversation, as it copies.
fn output_text(g: &Gui, id: PaneId) -> Option<String> {
    let v = &g.panes.get(&id)?.focus;
    let span = v.sel.as_ref()?.span()?;
    let conv = shown_conversation(g, id);
    let raw = |i: usize| match conv.as_ref().and_then(|c| c.items.get(i)).map(|it| it.as_ref()) {
        Some(Item::Said { text, .. } | Item::You { text, .. }) => Some(text.clone()),
        _ => None,
    };
    Some(selected_text(span, &v.sel_text, &raw)).filter(|t| !t.is_empty())
}

/// The conversation `id` shows (the session's, or the subagent's open in its place), as last drawn: what an item's
/// place in it means.
fn shown_conversation(g: &Gui, id: PaneId) -> Option<Arc<Conversation>> {
    g.panes.get(&id).and_then(|st| st.focus.drawn.clone()).or_else(|| g.app.panes.get(&id)?.as_term_ref()?.insights()?.conversation)
}

/// The session in focus mode whose reply box has the keys.
fn box_owner(g: &Gui) -> Option<PaneId> {
    let app = &g.app;
    let id = app.focused()?;
    (app.focus_view(id) && !app.overlay.is_open() && !app.side_focus && app.renaming.is_none() && !app.prefix_armed).then_some(id)
}

pub(super) fn dictation_modifiers() -> winit::keyboard::ModifiersState {
    use winit::keyboard::ModifiersState as M;
    let command = if cfg!(target_os = "macos") { M::SUPER } else { M::CONTROL };
    command | M::SHIFT
}

pub(super) fn dictation_key(g: &mut Gui, event: &winit::event::KeyEvent) -> bool {
    use winit::{event::ElementState, keyboard::{Key, NamedKey}};
    let pressed = event.state == ElementState::Pressed;
    let letter = matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("d"));
    if !pressed && letter {
        return g.app.dictation.release_shortcut();
    }
    if !pressed || g.app.overlay.is_open() || g.media.expanded || g.media.focus
        || g.app.theme_editor.as_ref().is_some_and(|editor| !editor.minimized)
    {
        return false;
    }
    if g.app.dictation.active() && event.logical_key == Key::Named(NamedKey::Escape) {
        g.app.dictation.cancel_all();
        return true;
    }
    if !letter || g.mods != dictation_modifiers() {
        return false;
    }
    let app = &g.app;
    let Some(id) = app.focused().filter(|id| app.can_dictate(*id)) else { return false };
    if app.side_focus || app.renaming.is_some() || app.prefix_armed
        || app.focus.drafts.get(&id).is_some_and(|d| d.quips.editor.is_some())
    {
        return false;
    }
    if !event.repeat {
        g.app.toggle_dictation(id, true);
    }
    true
}

/// What's selected in `id`'s reply box (the pictures' tokens left out).
fn box_text(g: &Gui, id: PaneId) -> Option<String> {
    let text = crate::focus::visible(g.app.focus.drafts.get(&id)?.area.selected()?);
    (!text.is_empty()).then_some(text)
}

/// Something in a focus view is selected that ctrl+c would copy.
pub(super) fn has_selection(g: &Gui) -> bool {
    g.panes.values().any(|st| st.focus.sel.as_ref().is_some_and(|s| s.span().is_some())) || box_owner(g).and_then(|id| box_text(g, id)).is_some()
}

/// What a right-click's Copy takes in `id`'s focus view: the conversation's selection, else the reply box's.
pub(super) fn selection_text(g: &Gui, id: PaneId) -> Option<String> {
    output_text(g, id).or_else(|| box_text(g, id))
}

/// Copy a focus view's selection (ctrl+c): the conversation's (it ends, as a terminal's does), else the reply
/// box's (it stays, as a text field's does). True when something was copied.
pub(super) fn copy(g: &mut Gui) -> bool {
    let ids: Vec<PaneId> = g.panes.iter().filter(|(_, st)| st.focus.sel.is_some()).map(|(id, _)| *id).collect();
    for id in ids {
        if let Some(text) = output_text(g, id) {
            super::window::set_clipboard(&text);
            copied_toast(g, &text);
            clear(g, None);
            return true;
        }
    }
    if let Some(text) = box_owner(g).and_then(|id| box_text(g, id)) {
        super::window::set_clipboard(&text);
        copied_toast(g, &text);
        return true;
    }
    false
}

/// Cut the reply box's selection (ctrl+x). True when there was one.
pub(super) fn cut(g: &mut Gui) -> bool {
    let Some(id) = box_owner(g) else { return false };
    if g.app.dictation.phase(id).is_some() {
        return true;
    }
    let Some(text) = box_text(g, id) else { return false };
    super::window::set_clipboard(&text);
    let d = g.app.focus.draft(id);
    d.area.delete_selection();
    d.keep_tokens();
    d.recall = None;
    true
}

/// End the conversations' selections, but `keep`'s.
pub(super) fn clear(g: &mut Gui, keep: Option<PaneId>) {
    for (id, st) in g.panes.iter_mut() {
        if Some(*id) != keep {
            st.focus.sel = None;
        }
    }
}

/// The pointer over the conversation's words is a text cursor.
pub(super) fn cursor(g: &Gui, id: PaneId, n: usize, p: Point) -> Option<Cursor> {
    if n != AREA && !(OPEN..COPY).contains(&n) {
        return None;
    }
    let v = &g.panes.get(&id)?.focus;
    v.runs.iter().any(|r| r.rect().contains(p)).then_some(Cursor::Text)
}

/// What a click on `id`'s target `n` does once it's known to be a click (it let go where it pressed): show a
/// long message whole or a turn's steps, open a link.
fn clicked(g: &mut Gui, id: PaneId, n: usize, _p: Point) {
    match n {
        n if (OPEN..COPY).contains(&n) => {
            let i = n - OPEN;
            let item = shown_conversation(g, id).and_then(|c| c.items.get(i).cloned());
            if let Some(it) = item
                && let Some(st) = g.panes.get_mut(&id)
            {
                let k = Arc::as_ptr(&it) as usize;
                if !st.focus.open.remove(&k) {
                    st.focus.open.insert(k);
                }
            }
        }
        // a link opens as it does from the terminal: files in the preview (pictures in the media panel), the rest
        // with the system
        n if (LINK..PICTURE).contains(&n) => {
            let uri = g.panes.get(&id).and_then(|st| st.focus.links.get(n - LINK).cloned());
            if let Some(uri) = uri {
                super::mouse::open_link(g, id, &uri);
            }
        }
        _ => {}
    }
    g.dirty = true;
}

/// Target `n` of `id`'s focus view was clicked.
pub fn click(g: &mut Gui, id: PaneId, n: usize, p: Point, count: u32) {
    focus_pane(g, id);
    if quips::click(g, id, n, p, count) { return; }
    // a click anywhere but its list (or the word that opened it) closes the model or thinking list
    if !(MENU_ROW..COMMAND_ROW).contains(&n) && n != MODEL_CHIP && n != THINKING_CHIP {
        g.app.focus.draft(id).menu = None;
    }
    match n {
        AREA => {
            let d = g.app.focus.draft(id);
            d.quips.dismissed = Some((d.area.text().into(), d.area.cursor()));
            press_text(g, id, p, count, None);
        }
        BOX => press_box(g, id, p, count),
        DICTATE => g.app.toggle_dictation(id, false),
        CANCEL_DICTATION => g.app.dictation.cancel(id),
        SKIP_CLEANUP => g.app.skip_dictation_cleanup(id),
        // as Enter does: a command picked from the list runs, anything else is sent
        SEND => {
            if g.app.focus_choice(id).is_none() {
                g.app.focus_key(id, crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
            }
        }
        MODEL_CHIP => g.app.focus_toggle_menu(id, MenuKind::Model),
        THINKING_CHIP => g.app.focus_toggle_menu(id, MenuKind::Thinking),
        STOP => {
            if let Some(t) = g.app.panes.get_mut(&id).and_then(|p| p.as_term()) {
                t.send(b"\x1b");
            }
        }
        LATEST => {
            if let Some(st) = g.panes.get_mut(&id) {
                st.focus.follow = true;
            }
        }
        TERMINAL => {
            if !g.app.focus.on {
                g.app.toggle_focus_mode();
            } else {
                g.app.focus.toggle_peek(id);
            }
        }
        ANSWER_QUEUED => { g.app.focus_open_queued_questions(id); }
        BACK => g.app.focus.show_subagent(id, None),
        // a subagent's card, chip or note: its own conversation, in place of the session's
        n if (SUBAGENT..SUBAGENT_END).contains(&n) => {
            let call = g.panes.get(&id).and_then(|st| st.focus.opened.get(n - SUBAGENT).cloned());
            if let Some(call) = call {
                g.app.focus.show_subagent(id, Some(call));
            }
        }
        EARLIER => {
            if let Some(st) = g.panes.get_mut(&id) {
                st.focus.shown += HISTORY;
                st.focus.follow = false;
            }
        }
        n if (OPTION..UNATTACH).contains(&n) => {
            let shown = g.panes.get(&id).and_then(|st| st.focus.choice.clone());
            if let Some(shown) = shown
                && let Some(option) = shown.options.get(n - OPTION)
            {
                g.app.focus_answer(id, &shown, &option.key);
            }
        }
        n if (UNATTACH..SHOW_ATTACHED).contains(&n) => {
            g.app.focus.draft(id).detach(n - UNATTACH);
            if let Some(st) = g.panes.get_mut(&id) {
                st.focus.pop = None;
            }
        }
        n if (SHOW_ATTACHED..MENU_ROW).contains(&n) => {
            let path = g.app.focus.drafts.get(&id).and_then(|d| d.attachments.get(n - SHOW_ATTACHED)).map(|a| a.path.clone());
            if let Some(path) = path {
                let owner = super::media::owner(&g.app, id);
                g.media.add(path.clone(), super::media::Source::Opened, owner, None);
                g.show_media(&path);
            }
        }
        n if (APP_RETRY..APP_RETRY_END).contains(&n) => {
            let a = g.panes.get(&id).and_then(|st| st.focus.retry.get(n - APP_RETRY).cloned());
            let sid = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone());
            if let (Some(a), Some(sid)) = (a, sid) {
                let svc = g.app.svc.clone();
                g.apps.retry(&svc, &sid, &a);
            }
        }
        n if (MENU_ROW..COMMAND_ROW).contains(&n) => {
            if let Some(m) = g.app.focus.drafts.get(&id).and_then(|d| d.menu.clone()) {
                g.app.focus_pick(id, m.kind, n - MENU_ROW);
            }
        }
        n if (COMMAND_ROW..quips::BASE).contains(&n) => {
            let picked = g.app.focus_command_rows(id).get(n - COMMAND_ROW).cloned();
            if let Some(cmd) = picked {
                g.app.focus_complete(id, &cmd, true);
            }
        }
        // a long message, a turn's steps, a link: the press can start a selection, and is the click when it
        // lets go where it pressed
        n if (OPEN..COPY).contains(&n) || (LINK..PICTURE).contains(&n) => press_text(g, id, p, count, Some(n)),
        n if (COPY..LINK).contains(&n) => {
            let i = n - COPY;
            let text = shown_conversation(g, id).and_then(|c| match c.items.get(i).map(|it| it.as_ref()) {
                Some(Item::Said { text, .. }) => Some(text.clone()),
                _ => None,
            });
            if let Some(text) = text {
                super::window::set_clipboard(&text);
                g.app.toast(crate::alerts::Kind::Info, "copied");
            }
        }
        n if (SOUND..SOUND_AT).contains(&n) => {
            if let Some((path, _)) = g.panes.get(&id).and_then(|st| st.focus.sounds.get(n - SOUND).cloned()) {
                sound::toggle(&path);
            }
        }
        n if n >= SOUND_AT => {
            if let Some((path, r)) = g.panes.get(&id).and_then(|st| st.focus.sounds.get(n - SOUND_AT).cloned()) {
                sound::seek(&path, (p.x - r.x) / r.w.max(1.0));
            }
        }
        n if n >= PICTURE => {
            let path = g.panes.get(&id).and_then(|st| st.focus.pictures.get(n - PICTURE).cloned());
            if let Some(path) = path {
                let owner = super::media::owner(&g.app, id);
                g.media.add(path.clone(), super::media::Source::Opened, owner, None);
                g.show_media(&path);
            }
        }
        _ => {}
    }
    g.dirty = true;
}

fn focus_pane(g: &mut Gui, id: PaneId) {
    let app = &mut g.app;
    if app.stacked() {
        app.stack_focus = Some(id);
    } else if let Some(t) = app.tabs.get_mut(app.cur) {
        t.focus = id;
    }
    app.leave_sidebar();
}

/// The wheel over `id`'s conversation: it scrolls (and stops following the newest once you go up).
pub fn wheel(g: &mut Gui, id: PaneId, delta: super::scroll::Wheel) {
    if let Some(st) = g.panes.get_mut(&id)
        && st.focus.quip_preview.wheel(g.pointer, delta.pixels(22.0))
    {
        g.dirty = true;
        return;
    }
    if quips::wheel(g, id, delta.units()) { return; }
    let Some(st) = g.panes.get_mut(&id) else { return };
    let v = &mut st.focus;
    v.scroll_pixels(delta.pixels(v.line_height));
    g.dirty = true;
}

impl View {
    /// The subagent shown now (None: the conversation). Into a subagent, its latest shows; back in the
    /// conversation, it's where it was left. A selection doesn't go across.
    fn look_at(&mut self, call: Option<&str>) {
        if self.lens.as_deref() == call {
            return;
        }
        if self.lens.is_none() {
            self.saved = Some((self.top, self.follow, self.shown, std::mem::take(&mut self.open)));
        }
        match call {
            Some(_) => {
                self.top = 0.0;
                self.follow = true;
                self.shown = HISTORY;
            }
            None => {
                let (top, follow, shown, open) = self.saved.take().unwrap_or((0.0, true, HISTORY, HashSet::new()));
                (self.top, self.follow, self.shown, self.open) = (top, follow, shown, open);
            }
        }
        self.lens = call.map(str::to_string);
        self.sel = None;
        self.sel_text.clear();
        self.made = None;
    }

    pub(super) fn has_selection(&self) -> bool {
        self.sel.is_some()
    }

    pub(super) fn clear_selection(&mut self) {
        self.sel = None;
    }

    pub(super) fn scroll_rows(&mut self, rows: f32) {
        self.scroll_pixels(rows * self.line_height);
    }

    fn scroll_pixels(&mut self, up: f32) {
        let max = (self.total - self.room).max(0.0);
        let top = if self.follow { max } else { self.top };
        self.top = (top - up).clamp(0.0, max);
        self.follow = self.top >= max;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_prompt_holds_focus_and_hides_composer_actions_and_lists() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        let meta = app.panes[&id].as_term_ref().unwrap().meta.clone();
        let screen = include_str!("../../../../tests/fixtures/codex-folder-trust.txt").replace('\n', "\r\n");
        let mut term = crate::panes::term::Term::fixed(meta, &screen, app.svc.clone());
        term.set_screen_status(Some(Activity::Blocked));
        app.panes.insert(id, Box::new(term));
        let theme = app.theme.clone();
        let tokens = super::super::kit::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        let mut media = Media::new(Arc::new(|| {}));
        for width in [480.0, 900.0] {
            let d = app.focus.draft(id);
            d.area.set_text("/mo");
            d.menu = Some(crate::focus::Menu { kind: MenuKind::Model, sel: 0, query: String::new() });
            d.quips.open = true;
            let mut view = View { choice: app.focus_choice(id), ..Default::default() };
            assert!(view.choice.is_some());
            let mut scene = bro_gfx::Scene::new();
            let mut anim = super::super::kit::Anims::default();
            let mut hits = vec![];
            let screen = Rect::new(0.0, 0.0, width, 300.0);
            text.begin_frame();
            let mut ui = Ui {
                scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0,
                pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim,
                now: Instant::now(), time: 0.0, ticking: false, screen,
            };
            let c = Typo::new(&mut ui, 14.0);
            let seen = seen(&app, id).unwrap();
            reply_box(&mut app, &mut ui, id, screen, true, &c, &seen, Mood::Blocked, &mut view, &mut media);
            assert!(view.caret.is_none(), "the reply box must not claim keyboard focus");
            assert!(!view.popup, "saved composer lists must not cover the choice");
            assert!(!hits.iter().any(|h| matches!(h.action, Action::Focus(pane, SEND) if pane == id)), "Send must not act as an answer");
            assert_eq!(app.focus.drafts[&id].area.text(), "/mo");
            assert!(app.focus.drafts[&id].menu.is_some(), "the saved menu can return after the prompt");
        }
    }

    #[test]
    fn queued_questions_have_an_answer_button_at_narrow_and_wide_widths() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let theme = app.theme.clone();
        let tokens = super::super::kit::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        for width in [120.0, 240.0, 480.0, 900.0] {
            let mut scene = bro_gfx::Scene::new();
            let mut anim = super::super::kit::Anims::default();
            let mut hits = vec![];
            let screen = Rect::new(0.0, 0.0, width, 80.0);
            text.begin_frame();
            let mut ui = Ui {
                scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0,
                pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim,
                now: Instant::now(), time: 0.0, ticking: false, screen,
            };
            let c = Typo::new(&mut ui, 14.0);
            draw_queued_questions(&mut ui, 7, &c, bro_core::terminal_prompt::QueuedQuestions { count: 2, shortcut: bro_core::terminal_prompt::QuestionShortcut::ShiftLeft }, screen);
            let button = hits.iter().find(|h| matches!(h.action, Action::Focus(7, ANSWER_QUEUED))).expect("the queue needs a clickable Answer action");
            assert!(button.rect.w >= 48.0 && button.rect.h >= 28.0);
            assert!(button.rect.x >= screen.x && button.rect.right() <= screen.right());
            assert!(button.rect.y >= screen.y && button.rect.bottom() <= screen.bottom());
        }
    }

    #[test]
    fn a_browser_handoff_card_opens_and_returns_its_page_and_stays_inside_its_band() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let theme = app.theme.clone();
        let tokens = super::super::kit::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        let mut handoff = bro_bridge::BrowserHandoff {
            id: "h1".into(), tab_id: 42, title: "GitHub · Sign in".into(), reason: "Sign in with your passkey, then press Done.".into(),
            phase: "requested".into(), viewer_available: true, returning: false, session: Some("s".into()),
        };
        for (width, phase) in [(300.0, "requested"), (720.0, "requested"), (720.0, "human")] {
            handoff.phase = phase.into();
            let mut scene = bro_gfx::Scene::new();
            let mut anim = super::super::kit::Anims::default();
            let mut hits = vec![];
            text.begin_frame();
            let mut ui = Ui {
                scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0,
                pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim,
                now: Instant::now(), time: 0.0, ticking: false, screen: Rect::new(0.0, 0.0, width, 400.0),
            };
            let c = Typo::new(&mut ui, 15.0);
            let h = super::super::browser::focus_card_height(&mut ui, c.small, c.body, &handoff, width);
            let band = Rect::new(0.0, 0.0, width, h);
            super::super::browser::draw_focus_card(&mut ui, c.small, c.body, &handoff, band);
            let buttons: Vec<_> = hits.iter().filter_map(|hit| match &hit.action {
                Action::BrowserHandoff { tab_id: 42, id, finish } if id == "h1" => Some((*finish, hit.rect)),
                _ => None,
            }).collect();
            if phase == "human" {
                assert!(buttons.is_empty(), "with the page in your hands its own Done hands it back");
                continue;
            }
            assert_eq!(buttons.iter().map(|(finish, _)| *finish).collect::<Vec<_>>(), [false, true], "{width}: Open, then Return");
            for (_, r) in buttons {
                assert!(r.h >= 28.0 && r.x >= band.x && r.right() <= band.right() && r.y >= band.y && r.bottom() <= band.bottom(), "{width}: {r:?} in {band:?}");
            }
        }
    }

    #[test]
    fn codex_focus_shows_working_controls_while_the_terminal_looks_idle() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        let mut meta = app.panes[&id].as_term_ref().unwrap().meta.clone();
        meta.harness = Some(Harness::Codex);
        let mut term = crate::panes::term::Term::fixed(meta, "› Ask Codex to do anything", app.svc.clone());
        term.set_screen_status(Some(Activity::Idle));
        term.set_insights(crate::panes::insights::Insights::fixed_transcript(Harness::Codex,
            r#"{"type":"event_msg","payload":{"type":"task_started"}}"#));
        app.panes.insert(id, Box::new(term));
        app.focus.draft(id);
        let seen = seen(&app, id).unwrap();
        assert_eq!(seen.activity, Some(Activity::Working));
        let mood = mood_of(&seen, None, Instant::now());
        assert_eq!(mood, Mood::Working);
        assert_eq!(doing(&seen, crate::util::now_ms(), 0), "Thinking");
        let theme = app.theme.clone();
        let tokens = super::super::kit::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        let mut media = Media::new(Arc::new(|| {}));
        for width in [240.0, 480.0, 900.0] {
            let mut scene = bro_gfx::Scene::new();
            let mut anim = super::super::kit::Anims::default();
            let mut hits = vec![];
            let screen = Rect::new(0.0, 0.0, width, 300.0);
            text.begin_frame();
            let mut ui = Ui {
                scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0,
                pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim,
                now: Instant::now(), time: 0.0, ticking: false, screen,
            };
            assert!(header_status(&app, &mut ui, id, None, Point::new(width, 16.0), width * 0.36).is_some());
            assert!(ui.ticking, "the working spinner must animate at {width}px");
            let c = Typo::new(&mut ui, 18.0);
            reply_box(&mut app, &mut ui, id, screen, true, &c, &seen, mood, &mut View::default(), &mut media);
            assert!(hits.iter().any(|h| matches!(h.action, Action::Focus(pane, STOP) if pane == id)), "Stop must be available at {width}px");
        }
    }

    #[test]
    fn a_fresh_session_has_a_clickable_model_chip_at_narrow_and_wide_widths() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        let mut meta = app.panes[&id].as_term_ref().unwrap().meta.clone();
        meta.model = None;
        app.panes.insert(id, Box::new(crate::panes::term::Term::fixed(meta, "❯ ", app.svc.clone())));
        app.focus.draft(id);
        let theme = app.theme.clone();
        let tokens = super::super::kit::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        let mut media = Media::new(Arc::new(|| {}));
        for width in [240.0, 480.0, 900.0] {
            let mut scene = bro_gfx::Scene::new();
            let mut anim = super::super::kit::Anims::default();
            let mut hits = vec![];
            let screen = Rect::new(0.0, 0.0, width, 300.0);
            text.begin_frame();
            let mut ui = Ui {
                scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0,
                pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim,
                now: Instant::now(), time: 0.0, ticking: false, screen,
            };
            let c = Typo::new(&mut ui, 18.0);
            let seen = seen(&app, id).unwrap();
            reply_box(&mut app, &mut ui, id, screen, true, &c, &seen, Mood::Idle, &mut View::default(), &mut media);
            let chip = hits.iter().find(|h| matches!(h.action, Action::Focus(pane, MODEL_CHIP) if pane == id)).expect("the model picker must be reachable before the model is known");
            assert!(chip.rect.w > 24.0 && chip.rect.h >= 24.0);
            assert!(chip.rect.x >= screen.x && chip.rect.right() <= screen.right());
        }
    }

    #[test]
    fn precise_scrolling_holds_the_reading_position_and_follows_again_at_the_bottom() {
        let mut view = View { top: 800.0, total: 1000.0, room: 200.0, line_height: 30.0, ..Default::default() };
        view.scroll_pixels(0.25);
        assert_eq!(view.top, 799.75);
        assert!(!view.follow, "even a small touchpad movement leaves the live edge");
        view.scroll_pixels(-0.25);
        assert!(view.follow);
        view.scroll_rows(1.0);
        assert_eq!(view.top, 770.0);
        assert!(!view.follow);
        view.scroll_pixels(super::super::scroll::Wheel::Pixels(2.5).pixels(view.line_height));
        assert_eq!(view.top, 767.5);
        view.total = 1100.0;
        assert_eq!(view.top, 767.5, "new output must not pull you down while reading history");
        view.scroll_rows(100.0);
        assert_eq!(view.top, 0.0);
        view.scroll_rows(-100.0);
        assert_eq!(view.top, 900.0);
        assert!(view.follow);
        view.total = 1200.0;
        view.scroll_rows(1.0);
        assert_eq!(view.top, 970.0, "scroll from the newest output when following");
        assert!(!view.follow);
    }

    #[test]
    fn ctrl_shift_arrows_scroll_the_active_conversation_without_editing_its_reply() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        app.focus.on = true;
        app.focus.draft(id).area.set_text("An unsent reply");
        let mut panes = HashMap::new();
        panes.entry(id).or_insert_with(super::super::PaneState::default).focus = View {
            top: 800.0, total: 1000.0, room: 200.0, line_height: 30.0, ..Default::default()
        };
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        assert!(super::super::scroll::key(&mut app, &mut panes, KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT)));
        assert_eq!(panes[&id].focus.top, 770.0);
        assert!(!panes[&id].focus.follow);
        assert_eq!(app.focus.draft(id).area.text(), "An unsent reply");
        assert!(super::super::scroll::key(&mut app, &mut panes, KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT)));
        assert_eq!(panes[&id].focus.top, 800.0);
        assert!(panes[&id].focus.follow);
    }

    fn step(kind: StepKind, label: &str, state: StepState) -> Step {
        Step { id: label.into(), kind, label: label.into(), detail: None, state, added: 0, removed: 0, line: None, at_ms: None, app: None }
    }

    #[test]
    fn a_turn_that_started_subagents_shows_a_card_each_that_opens_it() {
        use bro_core::sessions::subagents::SubagentState;
        let theme = crate::theme::get("graphite");
        let tokens = super::super::kit::Tokens::from_theme(&theme);
        let mut text = bro_gfx::Text::new(crate::fonts::get("jetbrains-mono").unwrap().data, crate::fonts::SYMBOLS.data, false);
        let mut scene = bro_gfx::Scene::new();
        let mut anim = super::super::kit::Anims::default();
        let mut hits = vec![];
        let screen = Rect::new(0.0, 0.0, 720.0, 600.0);
        text.begin_frame();
        let mut ui = Ui { scene: &mut scene, text: &mut text, t: &tokens, theme: &theme, scale: 1.0, pointer: None, pressing: false, hits: &mut hits, top_layer: 0, anim: &mut anim, now: Instant::now(), time: 0.0, ticking: false, screen };
        let c = Typo::new(&mut ui, 16.0);
        let done = Subagent { state: SubagentState::Done, result: Some("Routes are registered in three places.".into()), ..Subagent::started("a1", "Map every route registration".into(), Some(0)) };
        let busy = Subagent { name: Some("route-cleanup".into()), ..Subagent::started("a2", "Check route cleanup on exit".into(), Some(0)) };
        let mut conv = Conversation::new(Harness::Claude);
        conv.subagents = vec![done, busy];
        let subs = Subs::new(None, &[Some(&conv)]);
        let launch = |id: &str| Step { id: id.into(), ..step(StepKind::Agent, "Started", StepState::Done) };
        let steps = [launch("a1"), launch("a2"), step(StepKind::Read, "Read a.rs", StepState::Done)];
        let paint = |draw: bool| Paint { draw, id: 7, links: vec![], pictures: vec![], sounds: vec![], media: None, resolve: None, apps: AppPaint::default(), text: TextPaint::default(), subs: Some(&subs), opened: vec![], quip_hover: None };
        let measured = did(&mut ui, &c, &steps, 3, Point::new(20.0, 0.0), 680.0, false, &mut paint(false));
        let mut p = paint(true);
        let drawn = did(&mut ui, &c, &steps, 3, Point::new(20.0, 0.0), 680.0, false, &mut p);
        assert_eq!(measured, drawn, "measured as tall as it's drawn");
        assert_eq!(p.opened, ["a1", "a2"], "each card opens its subagent");
        let cards: Vec<Rect> = ui.hits.iter().filter(|h| matches!(h.action, Action::Focus(7, n) if (SUBAGENT..SUBAGENT_END).contains(&n))).map(|h| h.rect).collect();
        assert_eq!(cards.len(), 2);
        assert!(cards[0].y < cards[1].y && cards[0].w > 600.0, "a row each, the card's width");
        // a turn that only started subagents has its cards and no line over them
        let only = [launch("a1"), launch("a2")];
        let h = did(&mut ui, &c, &only, 4, Point::new(20.0, 0.0), 680.0, false, &mut paint(false));
        assert_eq!(h, subagents::cards_height(&c, 2));
        // without its subagents known, a turn is the line it always was
        let mut plain = paint(false);
        plain.subs = None;
        assert_eq!(did(&mut ui, &c, &only, 4, Point::new(20.0, 0.0), 680.0, false, &mut plain), (c.small * 1.9).round());
    }

    #[test]
    fn steps_sum_up_plainly() {
        use StepKind::*;
        use StepState::*;
        assert_eq!(summary(&[step(Read, "Read a.rs", Done)], 0, true), "Read a.rs");
        assert_eq!(summary(&[step(Read, "Read a.rs", Done), step(Read, "Read b.rs", Done), step(Edit, "Edited b.rs", Done), step(Run, "Ran cargo test", Done)], 0, true), "Read 2 files · Changed a file · Ran a command");
        assert_eq!(summary(&[step(Read, "Read a.rs", Done), step(Run, "Run the tests", Running)], 0, true), "Run the tests…");
        let mut long = step(Run, "Run the tests", Running);
        long.at_ms = Some(1_000);
        assert_eq!(summary(std::slice::from_ref(&long), 43_000, true), "Run the tests · 42s", "a step that's been running a while says for how long");
        assert_eq!(summary(&[long], 43_000, false), "Run the tests", "one left behind in a session that isn't working has no clock");
    }

    #[test]
    fn a_skill_leads_its_steps_and_a_server_goes_by_name() {
        use StepKind::*;
        use StepState::*;
        let browse = |label: &str| Step { detail: Some("JustBrowse".into()), ..step(Mcp, label, Done) };
        let voice = [step(Skill, "Generate voice", Done), step(Run, "Find cast members with voices", Done), step(Run, "Make the voice-over", Done)];
        assert_eq!(summary(&voice, 0, true), "Generate voice · Ran 2 commands", "what the rest was for comes first");
        assert_eq!(summary(&[step(Skill, "Generate voice", Done), step(Skill, "Generate images", Done)], 0, true), "Used 2 skills");
        assert_eq!(summary(&[browse("Navigate · figma.com"), browse("Screenshot"), browse("Click element")], 0, true), "Used JustBrowse 3 times");
        assert_eq!(summary(&[browse("Navigate · figma.com"), browse("Screenshot"), step(Run, "Pushed main", Done)], 0, true), "Used JustBrowse 2 times · Ran a command", "in the order it happened");
        assert_eq!(summary(&[step(Run, "Ran the tests", Done), step(Read, "Read a.rs", Done), step(Edit, "Edited a.rs", Done)], 0, true), "Ran a command · Read a file · Changed a file");
        // each by what it's about
        assert_eq!(step_glyph(&voice[0]), "\u{F036C}", "a voice skill is a microphone");
        assert_eq!(step_glyph(&step(Skill, "Generate images", Done)), "\u{F02E9}");
        assert_eq!(step_glyph(&step(Skill, "JustGains app deployment", Done)), "\u{F14DE}");
        assert_eq!(step_glyph(&step(Skill, "Reverse engineer anything", Done)), "\u{F0674}", "any other skill is a skill");
        assert_eq!(step_glyph(&browse("Navigate · figma.com")), "\u{F059F}");
        assert_eq!(step_glyph(&browse("Click element")), "\u{F0CFD}");
        assert_eq!(step_glyph(&browse("Upload file · photo.png")), "\u{F0552}", "by the tool, not what it was given");
        assert_eq!(step_glyph(&browse("Handle dialog · dismiss")), "\u{F1425}", "any other tool is a connector's");
        assert_eq!(step_glyph(&step(Run, "Generate the sound", Done)), "\u{F018D}", "a command stays a command");
        assert_eq!(label_runs("Navigate · figma.com", Color::WHITE, Color::BLACK, 12.0).len(), 2, "what it did, then where");
    }

    #[test]
    fn a_switch_of_model_or_thinking_reads_as_a_line() {
        assert_eq!(setting("/model claude-sonnet-5-5").as_deref(), Some("Now on Sonnet 5.5"));
        assert_eq!(setting("/effort xhigh").as_deref(), Some("Thinking extra high"));
        assert_eq!(setting("/model"), None, "its own menu: a command like any other");
        assert_eq!(setting("/compact"), None);
        use crate::panes::term::DeliveryState;
        assert_eq!(sent_state(&DeliveryState::Submitted, true), "Queued · waiting for the agent");
        assert_eq!(sent_state(&DeliveryState::Sending, false), "Sending…");
        assert_eq!(sent_state(&DeliveryState::Submitted, false), "Waiting for the agent…");
        assert_eq!(sent_state(&DeliveryState::Failed("closed".into()), false), "Not sent · check the terminal");
    }

    #[test]
    fn models_read_as_people_say_them() {
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_name("claude-opus-5"), "Opus 5");
        assert_eq!(model_name("gpt-6.1-sol"), "GPT-6.1 Sol");
        assert_eq!(model_name("gpt-5.2-codex"), "GPT-5.2 Codex");
        assert_eq!(model_name("moonshotai/kimi-k2"), "Kimi K2");
        assert_eq!(clock(42), "42s");
        assert_eq!(clock(65), "1m 05s");
        assert_eq!(clock(3720), "1h 02m");
    }

    fn seen_at(last_ms: Option<i64>, thought: Option<Thought>) -> Seen {
        Seen { harness: Some(Harness::Claude), conv: None, activity: Some(Activity::Working), queued_questions: None, limit: None, done: false, running: None, last_ms, cwd: PathBuf::from("/x"), branch: None, thought, sending: false, subagents: None, tasks: vec![] }
    }

    #[test]
    fn thinking_says_how_its_going() {
        let now = 100_000;
        assert_eq!(thinking(&seen_at(Some(now - 3_000), None), now, 60), "Thinking");
        assert_eq!(thinking(&seen_at(Some(now - 20_000), None), now, 60), "Still thinking · 20s");
        assert_eq!(thinking(&seen_at(Some(now - 70_000), None), now, 600), "Deep in thought · 1m 10s");
        let mut sending = seen_at(Some(now - 7_200_000), None);
        sending.sending = true;
        assert_eq!(thinking(&sending, now, 5), "Thinking", "a message on its way: the transcript's newest line is from before it");
        assert_eq!(thinking(&seen_at(None, None), now, 30), "Still thinking · 30s", "nothing in the transcript yet: since it started working");
        let heading = Thought { text: "Planning the fix".into(), at_ms: Some(now - 15_000) };
        assert_eq!(thinking(&seen_at(Some(now - 15_000), Some(heading)), now, 60), "Planning the fix · 15s", "its own heading while it's the newest thing");
    }

    #[test]
    fn provider_limits_outrank_busy_thinking_and_running_steps() {
        let mut seen = seen_at(Some(0), Some(Thought { text: "Deep in thought".into(), at_ms: Some(0) }));
        seen.limit = bro_core::sessions::limits::UsageLimit::from_screen("✻ Weekly limit reached · Retrying in 5h (Nov 1, 7pm) · attempt 1/30");
        assert_eq!(mood_of(&seen, Some((Mood::Working, Instant::now())), Instant::now()), Mood::Blocked);
        assert_eq!(thinking(&seen, 3_600_000, 3600), "Weekly limit reached");
        assert_eq!(doing(&seen, 3_600_000, 3600), "Weekly limit reached");
        seen.limit = None;
        assert_eq!(mood_of(&seen, Some((Mood::Blocked, Instant::now())), Instant::now()), Mood::Working);
    }

    #[test]
    fn links_are_web_addresses_and_the_files_the_session_finds() {
        let spans = markdown::inline("see [docs](https://docs.rs/x), `src/focus.rs:12` and notes.txt");
        let mut found = |m: &FileLinkMatch| (m.path == "src/focus.rs").then(|| Arc::<str>::from(format!("file:///F:/app/{}#L{}", m.path, m.line.unwrap_or(0)).as_str()));
        let mut p = Paint { draw: true, id: 1, links: vec![], pictures: vec![], sounds: vec![], media: None, resolve: Some(&mut found), apps: AppPaint::default(), text: TextPaint::default(), subs: None, opened: vec![], quip_hover: None };
        let links = link_ranges(&spans, &mut p);
        let text = markdown::plain(&spans);
        let got: Vec<(&str, &str)> = links.iter().map(|(r, t)| (&text[r.clone()], t.as_str())).collect();
        assert_eq!(got, [("docs", "https://docs.rs/x"), ("src/focus.rs:12", "file:///F:/app/src/focus.rs#L12")], "a file nobody found (notes.txt) isn't a link");
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::file_view::gui::tests::with_ui;

    fn laid(ui: &mut Ui, text: &str, w: f32) -> TextBox {
        lay(ui, &[(text.to_string(), Style::mono(16.0, ui.t.text))], 16.0, LINE, w, None)
    }

    fn run(ui: &mut Ui, key: RunKey, text: &str, at: Point, w: f32) -> Run {
        let b = laid(ui, text, w);
        Run { key, at, layout: b.layout.clone(), scale: ui.scale, gap: "\n\n", lead: String::new() }
    }

    fn two_runs() -> Vec<Run> {
        with_ui("graphite", 800.0, 600.0, |ui| {
            vec![run(ui, (0, 0), "alpha beta gamma", Point::new(10.0, 100.0), 600.0), run(ui, (1, 0), "second item", Point::new(10.0, 160.0), 600.0)]
        })
        .0
    }

    fn at(key: RunKey, byte: usize) -> Pos {
        Pos { key, byte }
    }

    #[test]
    fn a_press_lands_on_the_nearest_byte_of_the_text_drawn() {
        let runs = two_runs();
        let adv = runs[0].layout.glyphs[1].x - runs[0].layout.glyphs[0].x;
        let mid = 100.0 + runs[0].rect().h / 2.0;
        assert_eq!(pos_at(&runs, Point::new(10.0 + adv * 6.2, mid)), Some(at((0, 0), 6)), "the near half of a letter is before it");
        assert_eq!(pos_at(&runs, Point::new(10.0 + adv * 6.7, mid)), Some(at((0, 0), 7)), "its far half after it");
        assert_eq!(pos_at(&runs, Point::new(790.0, mid)), Some(at((0, 0), 16)), "right of the line: its end");
        assert_eq!(pos_at(&runs, Point::new(0.0, mid)), Some(at((0, 0), 0)), "left of it: its start");
        assert_eq!(pos_at(&runs, Point::new(300.0, 140.0)), Some(at((0, 0), 16)), "between two pieces: the end of the one above");
        assert_eq!(pos_at(&runs, Point::new(300.0, 20.0)), Some(at((0, 0), 0)), "above them all: the start");
        assert_eq!(pos_at(&runs, Point::new(30.0, 590.0)), Some(at((1, 0), 11)), "below them all: the end");
        assert_eq!(pos_at(&[], Point::new(0.0, 0.0)), None, "nothing drawn, nothing to select");
    }

    #[test]
    fn a_double_click_takes_a_word_a_triple_the_piece_and_a_drag_grows_from_what_it_took() {
        let runs = two_runs();
        let p = at((0, 0), 7);
        assert_eq!(press_span(&runs, p, 1), (p, p));
        let word = press_span(&runs, p, 2);
        assert_eq!(word, (at((0, 0), 6), at((0, 0), 10)), "beta");
        assert_eq!(press_span(&runs, p, 3), (at((0, 0), 0), at((0, 0), 16)));
        // dragging back past the word keeps it whole: the anchor moves to its far end
        assert_eq!(grow(word, at((0, 0), 2)), (word.1, at((0, 0), 2)));
        assert_eq!(grow(word, at((1, 0), 3)), (word.0, at((1, 0), 3)), "on into the next piece");
        assert_eq!(grow(word, p), word, "inside the word: just the word");
        let sel = Sel { origin: word, anchor: word.1, head: at((0, 0), 2), press: Point::new(0.0, 0.0), click: None, moved: true };
        assert_eq!(sel.span(), Some((at((0, 0), 2), word.1)), "in reading order, whichever way it was dragged");
    }

    #[test]
    fn the_selection_shows_on_the_text_it_covers_and_runs_on_with_a_sliver() {
        let (s, e) = (at((1, 0), 3), at((1, 2), 2));
        assert_eq!(covered((s, e), (1, 0), 10), Some(3..10));
        assert_eq!(covered((s, e), (1, 1), 5), Some(0..5));
        assert_eq!(covered((s, e), (1, 1), 0), Some(0..0), "a blank line inside it shows");
        assert_eq!(covered((s, e), (1, 2), 9), Some(0..2));
        assert_eq!(covered((s, e), (0, 4), 9), None);
        assert_eq!(covered((s, e), (2, 0), 9), None);
        assert_eq!(covered((s, at((1, 0), 10)), (1, 0), 10), Some(3..10));
        let ((one, on, wrapped, lines, adv), _) = with_ui("graphite", 800.0, 600.0, |ui| {
            let b = laid(ui, "alpha beta", 600.0);
            let adv = b.layout.glyphs[1].x - b.layout.glyphs[0].x;
            let one = selection_rects(&b.layout, Point::new(10.0, 50.0), 1.0, &(6..10), false);
            let on = selection_rects(&b.layout, Point::new(10.0, 50.0), 1.0, &(6..10), true);
            let long = laid(ui, "one two three four five six seven eight", adv * 12.0);
            let wrapped = selection_rects(&long.layout, Point::new(0.0, 0.0), 1.0, &(4..long.layout.text.len()), false);
            (one, on, wrapped, long.layout.lines.len(), adv)
        });
        assert_eq!(one.len(), 1);
        assert!((one[0].x - (10.0 + 6.0 * adv)).abs() < 0.5, "it starts at the selected letter");
        assert!((one[0].w - 4.0 * adv).abs() < 0.5, "and covers just the four selected letters");
        assert!(on[0].w > one[0].w, "running on into the next piece, a sliver shows the line break");
        assert!(lines >= 3);
        assert_eq!(wrapped.len(), lines, "a line each, across the wraps");
        assert!(wrapped[0].x > 0.0 && wrapped[1].x < 1.0, "from the selected word, then from each next line's start");
    }

    #[test]
    fn the_copy_keeps_paragraphs_list_marks_and_lines_of_code() {
        let mut kept = BTreeMap::new();
        for (key, gap, lead, text) in [
            ((2, 0), "\n\n", "", "Here is the plan."),
            ((2, 1), "\n\n", "- ", "ship it"),
            ((2, 2), "\n", "- ", "then docs"),
            ((2, 3), "\n\n", "", "cargo test"),
            ((2, 4), "\n", "", ""),
            ((2, 5), "\n", "", "cargo build"),
            ((5, 0), "\n\n", "", "Done, thanks"),
        ] {
            kept.insert(key, RunText { gap, lead: lead.into(), text: text.into() });
        }
        // item 3 was never drawn while selected (scrolled past at speed): it copies as the transcript has it
        let raw = |i: usize| (i == 3).then(|| "a reply scrolled past".to_string());
        assert_eq!(
            selected_text((at((2, 0), 8), at((5, 0), 4)), &kept, &raw),
            "the plan.\n\n- ship it\n- then docs\n\ncargo test\n\ncargo build\n\na reply scrolled past\n\nDone"
        );
        assert_eq!(selected_text((at((2, 1), 2), at((2, 1), 6)), &kept, &raw), "ip i", "a part of an item has no mark");
        assert_eq!(selected_text((at((2, 1), 0), at((2, 2), 0)), &kept, &raw), "- ship it", "ending where the next piece starts takes none of it");
        assert_eq!(selected_text((at((2, 0), 0), at((2, 0), 99)), &kept, &raw), "Here is the plan.", "an end past the text is its end");
    }

    #[test]
    fn a_reply_lays_out_its_pieces_in_reading_order_and_copies_as_it_reads() {
        let blocks = markdown::parse("Intro line\n\n- one\n- two\n\n```\nlet a = 1;\n\nlet b = 2;\n```\n\n> quoted");
        let (runs, _) = with_ui("graphite", 900.0, 900.0, |ui| {
            let c = Typo::new(ui, 16.0);
            let mut p = Paint::measuring(1);
            p.draw = true;
            p.text.collect = true;
            p.text.item(4);
            said(ui, &c, &blocks, Point::new(0.0, 0.0), 600.0, c.text, false, &mut p);
            std::mem::take(&mut p.text.runs)
        });
        let texts: Vec<&str> = runs.iter().map(|r| r.layout.text.as_str()).collect();
        assert_eq!(texts, ["Intro line", "one", "two", "let a = 1;", "", "let b = 2;", "quoted"]);
        assert!(runs.iter().enumerate().all(|(n, r)| r.key == (4, n as u32)));
        assert!(runs.windows(2).all(|w| w[1].at.y > w[0].at.y), "top to bottom");
        assert_eq!(runs.iter().map(|r| r.gap).collect::<Vec<_>>(), ["\n\n", "\n\n", "\n", "\n\n", "\n", "\n", "\n\n"]);
        assert_eq!(runs.iter().map(|r| r.lead.as_str()).collect::<Vec<_>>(), ["", "- ", "- ", "", "", "", "> "]);
        let mut kept = BTreeMap::new();
        keep_text(&mut kept, &runs);
        let all = (at((4, 0), 0), at((4, 99), 0));
        assert_eq!(selected_text(all, &kept, &|_| None), "Intro line\n\n- one\n- two\n\nlet a = 1;\n\nlet b = 2;\n\n> quoted");
        // the text a drag lands on is the text it copies: from the start of "one" to the middle of "let b"
        let first = pos_at(&runs, Point::new(runs[1].at.x + 1.0, runs[1].at.y + 4.0)).unwrap();
        assert_eq!(first, at((4, 1), 0));
        assert_eq!(selected_text((first, at((4, 5), 5)), &kept, &|_| None), "- one\n- two\n\nlet a = 1;\n\nlet b");
    }

    #[test]
    fn a_picture_dropped_or_pasted_over_a_selection_keeps_the_words() {
        let mut d = crate::focus::Draft::default();
        d.area.selectable = true;
        d.area.set_text("compare these two");
        d.area.select(8, 13);
        d.attach(PathBuf::from("C:/shots/a.png"));
        assert_eq!(crate::focus::visible(d.area.text()), "compare these two");
        assert_eq!(d.attachments.len(), 1);
    }

    #[test]
    fn the_reply_box_selects_with_the_keys_and_typing_or_a_paste_replaces_it() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        app.focus.on = true;
        app.focus.draft(id).area.set_text("fix the flaky test");
        // as wide as a drawn box (home and end go by the rows you see)
        app.focus.draft(id).cols = 80;
        for _ in 0..4 {
            app.focus_key(id, KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        }
        assert_eq!(app.focus.draft(id).area.selected(), Some("test"));
        for ch in "bug".chars() {
            app.focus_key(id, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(app.focus.draft(id).area.text(), "fix the flaky bug");
        app.focus_key(id, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(app.focus.draft(id).area.selected(), Some("fix the flaky bug"), "ctrl+a selects all of it");
        app.focus_key(id, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let d = app.focus.draft(id);
        assert_eq!((d.area.selected(), d.area.text()), (None, "fix the flaky bug"), "esc lets go of the selection, not the words");
        app.focus_key(id, KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT | KeyModifiers::CONTROL));
        app.focus_paste(id, "race");
        assert_eq!(app.focus.draft(id).area.text(), "fix the flaky race", "a paste goes over the selection");
        app.focus_key(id, KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
        app.focus_key(id, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.focus.draft(id).area.text(), "", "backspace takes the selection");
    }
}
