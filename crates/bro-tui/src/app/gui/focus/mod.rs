//! Focus mode in the desktop app ([`crate::focus`]): an agent session drawn as its conversation instead of its
//! terminal, a document in big type with the reply box under it. Where the session works heads the
//! conversation; what the agent is doing (and how its thinking is going) is in the card's header
//! ([`header_status`]); the model it runs and how hard it thinks sit under the reply box. Links and file paths
//! open as they do in the terminal (the session's own file lookups find them), and pictures you paste or the
//! agent shows are thumbnails. The reply box's footer switches the model and how hard it thinks, and typing `/`
//! lists the commands the agent takes. The terminal still runs under it at the card's size.
//!
//! The text is the transcript's ([`bro_core::sessions::conversation`]), the reply box and what you sent are the
//! app's ([`crate::focus::Draft`]); this keeps only layout and motion ([`View`]).

mod markdown;

use super::actions::Action;
use super::kit::{Cursor, Style, TextBox, Ui, radius, size, weight, widgets};
use super::media::{Media, Thumbnail};
use super::Gui;
use crate::app::App;
use crate::app::focus::{MenuRow, level_name, same_model};
use crate::focus::{Choice, Command, MenuKind, Scroll, holds, model_name, visible};
use crate::layout::PaneId;
use crate::pane::Activity;
use bro_bridge::file_links::{FileLinkMatch, find_file_links, split_location};
use bro_core::Harness;
use bro_core::sessions::conversation::{Conversation, Item, NoteKind, Step, StepKind, StepState, Thought};
use bro_gfx::text::{Align, Span as GSpan, TextOpts};
use bro_gfx::{Color, Fill, Point, Quad, Rect};
use markdown::{Block, Mark, Span};
use std::collections::{HashMap, HashSet};
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
    /// laid-out items by identity (the item itself is kept, so its address can't be reused while cached)
    cache: HashMap<usize, Cached>,
    /// items whose steps are open (a long message of yours shown whole), by identity
    open: HashSet<usize>,
    /// this frame's links and pictures (their targets)
    links: Vec<String>,
    pictures: Vec<PathBuf>,
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
    /// the commands list's first row shown
    command_top: usize,
}

impl Default for View {
    fn default() -> View {
        View {
            top: 0.0,
            follow: true,
            total: 0.0,
            room: 0.0,
            cache: HashMap::new(),
            open: HashSet::new(),
            links: vec![],
            pictures: vec![],
            geo: None,
            mood: None,
            choice: None,
            caret: None,
            shown: HISTORY,
            pop: None,
            pop_rect: None,
            command_top: 0,
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
            code: t.accent.mix(t.text, 0.42),
            link: t.accent.mix(t.text, 0.2),
            well: if t.light { t.text.with_alpha(0.045) } else { Color::BLACK.with_alpha(0.22) },
            bubble: t.text.with_alpha(if t.light { 0.06 } else { 0.07 }),
            waiting: false,
            working: false,
            now_ms: crate::util::now_ms(),
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
}

fn seen(app: &App, id: PaneId) -> Option<Seen> {
    let p = app.panes.get(&id)?;
    let term = p.as_term_ref()?;
    let snap = term.insights();
    let conv = snap.as_ref().and_then(|s| s.conversation.clone());
    let facts = snap.as_ref().and_then(|s| s.facts.clone());
    Some(Seen {
        harness: term.meta.harness,
        activity: p.activity(),
        done: app.done.contains(&id),
        running: conv.as_ref().and_then(|c| c.running().cloned()),
        last_ms: facts.as_ref().and_then(|f| f.last_activity_ms),
        cwd: conv.as_ref().and_then(|c| c.cwd.clone()).map(PathBuf::from).unwrap_or_else(|| term.meta.cwd.clone()),
        branch: conv.as_ref().and_then(|c| c.branch.clone()).or_else(|| facts.as_ref().and_then(|f| f.branch.clone())),
        thought: conv.as_ref().and_then(|c| c.thought.clone()),
        sending: app.focus.drafts.get(&id).is_some_and(|d| !d.sent.is_empty()),
        conv,
    })
}

/// The mood the session is in, `prev` the last frame's: a turn that just ended reads as done a while, watched or not.
fn mood_of(seen: &Seen, prev: Option<(Mood, Instant)>, now: Instant) -> Mood {
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
    let (words, ink) = match mood {
        Mood::Working => (doing(&seen, crate::util::now_ms(), working_secs(prev, ui.now)), t.text.mix(t.muted, 0.35)),
        Mood::Blocked => ("Needs you".to_string(), t.danger),
        Mood::Done => ("Done".to_string(), t.good),
        Mood::Idle => return None,
    };
    let st = Style::new(size::SMALL, weight::MEDIUM, ink);
    let b = ui.line(&words, st, Some((room - 22.0).max(40.0)));
    let w = 22.0 + b.w;
    let x = right.x - w;
    let mid = right.y;
    match mood {
        Mood::Working => ui.spinner(Point::new(x + 7.0, mid), 4.6, 1.6, t.accent),
        Mood::Blocked => {
            ui.ticking = true;
            let k = 0.7 + 0.3 * ((ui.time * 4.0).sin() as f32 * 0.5 + 0.5);
            ui.icon(Rect::new(x, mid - 8.0, 14.0, 16.0), "\u{F0026}", 13.0, t.danger.fade(k));
        }
        Mood::Done => ui.icon(Rect::new(x, mid - 8.0, 14.0, 16.0), "\u{F012C}", 14.0, t.good),
        Mood::Idle => {}
    }
    ui.draw(&b, Point::new(x + 22.0, mid - b.h / 2.0));
    if ui.hovered(Rect::new(x, mid - 12.0, w, 24.0)) && b.layout.truncated {
        widgets::tooltip(ui, Rect::new(x, mid - 12.0, w, 24.0), &words);
    }
    Some(w)
}

// ------------------------------------------------------------------------------------------------- the view

/// Draw `id`'s conversation in `content` (the card under its header).
pub fn draw(app: &mut App, ui: &mut Ui, id: PaneId, content: Rect, focused: bool, v: &mut View, media: &mut Media) {
    let Some(seen) = seen(app, id) else { return };
    // the choice on screen: read when the screen is free, kept while it waits on you
    if seen.activity == Some(Activity::Blocked) {
        if let Some(c) = app.focus_choice(id) {
            v.choice = Some(c);
        }
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
    let focused = focused && !app.side_focus && !app.overlay.is_open() && app.renaming.is_none() && !app.prefix_armed;
    ui.hit_with(content, Action::Focus(id, AREA), Cursor::Default);
    // a comfortable measure, centred: big type, never a wall of it
    let avail = (content.w - 48.0).max(120.0);
    let body = (avail / 44.0).clamp(14.0, 19.0);
    let mut c = Typo::new(ui, body);
    c.waiting = mood == Mood::Blocked;
    c.working = mood == Mood::Working;
    let measure = avail.min((c.adv * 76.0).round());
    let mx = content.x + (content.w - measure) / 2.0;
    // the reply box at the foot, the choice it waits on over it, the conversation over both
    let box_h = reply_height(app, id, &c, measure);
    let box_r = Rect::new(mx, content.bottom() - 16.0 - box_h, measure, box_h);
    let choice_h = v.choice.as_ref().map(|ch| choice_height(ui, &c, ch, measure)).unwrap_or(0.0);
    let choice_r = Rect::new(mx, box_r.y - 12.0 - choice_h, measure, choice_h);
    let bottom = if choice_h > 0.0 { choice_r.y } else { box_r.y };
    // a little room under the header: the text scrolls away under a fade, not against its line
    let area = Rect::new(content.x, content.y + 6.0, content.w, (bottom - 8.0 - content.y - 6.0).max(0.0));
    conversation(app, ui, id, area, Rect::new(mx, area.y, measure, area.h), &c, &seen, v, mood, media);
    if let Some(ch) = v.choice.clone() {
        draw_choice(ui, id, &c, &ch, choice_r);
    }
    reply_box(app, ui, id, box_r, focused, &c, &seen, mood, v, media);
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
    media: Option<&'a mut Media>,
    resolve: Option<Resolve<'a>>,
}

impl Paint<'_> {
    fn measuring(id: PaneId) -> Paint<'static> {
        Paint { draw: false, id, links: vec![], pictures: vec![], media: None, resolve: None }
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

/// The conversation, scrolled, in `area` (`col` the text column inside it).
#[allow(clippy::too_many_arguments)]
fn conversation(app: &mut App, ui: &mut Ui, id: PaneId, area: Rect, col: Rect, c: &Typo, seen: &Seen, v: &mut View, mood: Mood, media: &mut Media) {
    let t = ui.t;
    let items: Vec<Arc<Item>> = seen.conv.as_ref().map(|c| c.items.clone()).unwrap_or_default();
    let sent: Vec<(String, f32, bool, Vec<PathBuf>, usize)> = app
        .focus
        .drafts
        .get(&id)
        .map(|d| d.sent.iter().map(|s| (s.text.clone(), ui.now.saturating_duration_since(s.at).as_secs_f32(), s.queued, s.pictures.clone(), s.after)).collect())
        .unwrap_or_default();
    // the latest items show; earlier ones wait behind a button (a long session stays quick to lay out)
    let skip = items.len().saturating_sub(v.shown.max(HISTORY));
    // the cache follows the items: kept by identity, dropped when they go
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
    // which replies are the agent narrating its work (more of it follows in the same turn): those read quieter
    let narration: Vec<bool> = (0..items.len())
        .map(|i| matches!(items[i].as_ref(), Item::Said { .. }) && items[i + 1..].iter().take_while(|it| !matches!(it.as_ref(), Item::You { .. })).any(|it| matches!(it.as_ref(), Item::Did { .. })))
        .collect();
    // the pictures in your messages: their paths when the transcript has them, else the ones you pasted (alt+v)
    // since the message before
    let pastes: Vec<(i64, PathBuf)> = media
        .items
        .iter()
        .filter(|it| it.source == super::media::Source::Pasted)
        .map(|it| (it.at.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64), it.path.clone()))
        .collect();
    let mut prev_you: Option<i64> = None;
    let pics: Vec<Vec<PathBuf>> = items
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
        })
        .collect();
    let gap = c.lh.round();
    let intro_h = intro_height(c);
    let earlier_h = if skip > 0 { 44.0 } else { 0.0 };
    let top_pad = 18.0 + intro_h + earlier_h;
    // heights first (cached per width), so the scroll knows where everything is
    let mut heights = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        if i < skip {
            heights.push(0.0);
            continue;
        }
        let k = Arc::as_ptr(it) as usize;
        let open = v.open.contains(&k);
        let e = v.cache.get(&k);
        let h = match e.and_then(|e| e.h) {
            Some((w, s, o, n, h)) if (w - col.w).abs() < 0.5 && (s - c.body).abs() < 0.01 && o == open && n == narration[i] => h,
            _ => {
                let blocks = e.and_then(|e| e.blocks.clone());
                let mut p = Paint::measuring(id);
                let h = item(ui, c, it, blocks.as_deref(), &pics[i], i, Point::new(col.x, 0.0), col.w, open, narration[i], &mut p);
                if let Some(e) = v.cache.get_mut(&k) {
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
    let working_line = mood == Mood::Working && seen.running.is_none();
    let tail_h = if working_line { c.lh + gap } else { 0.0 };
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
    let offset = ui.ease(("focus-scroll", id), v.top, 160.0);
    // the session's file lookups, for its links (what isn't known yet is looked up off this thread, and the
    // pane wakes when it's in)
    let waker = crate::pane::Waker::new(id, app.tx.clone());
    let term = app.panes.get(&id).and_then(|p| p.as_term_ref());
    let mut lookup = term.map(|t| t.file_cache().lookup());
    let mut resolve = |m: &FileLinkMatch| lookup.as_mut().and_then(|l| l.get(m));
    let mut p = Paint { draw: true, id, links: vec![], pictures: vec![], media: Some(media), resolve: Some(&mut resolve) };
    ui.clip(area, 0.0);
    let mut y = area.y + 18.0 - offset;
    intro(ui, c, seen, Point::new(col.x, y), col.w, &mut p);
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
    for (i, it) in items.iter().enumerate() {
        while next < sent.len() && sent_at[next] <= i {
            let (text, age, queued, pics, _) = &sent[next];
            you(ui, c, text, pics.len(), pics, false, Point::new(col.x, y), col.w, false, Some(sent_state(*age, *queued)), &mut p);
            y += sent_hs[next] + gap;
            next += 1;
        }
        let h = heights[i];
        if h <= 0.0 {
            continue;
        }
        if y + h >= area.y - 40.0 && y <= area.bottom() + 40.0 {
            let k = Arc::as_ptr(it) as usize;
            let open = v.open.contains(&k);
            let blocks = v.cache.get(&k).and_then(|e| e.blocks.clone());
            item(ui, c, it, blocks.as_deref(), &pics[i], i, Point::new(col.x, y), col.w, open, narration[i], &mut p);
        }
        y += h + gap;
    }
    for (n, (text, age, queued, pics, _)) in sent.iter().enumerate().skip(next) {
        you(ui, c, text, pics.len(), pics, false, Point::new(col.x, y), col.w, false, Some(sent_state(*age, *queued)), &mut p);
        y += sent_hs[n] + gap;
    }
    if working_line {
        // how its thinking is going, at the end of what it's said
        ui.spinner(Point::new(col.x + 7.0, y + c.lh / 2.0), 4.6, 1.6, t.accent);
        let words = thinking(seen, c.now_ms, working_secs(v.mood, ui.now));
        let b = ui.line(&words, Style::mono(c.small, c.muted), Some(col.w - 26.0));
        ui.draw(&b, Point::new(col.x + 22.0, y + (c.lh - b.h) / 2.0));
    }
    let (links, pictures) = (std::mem::take(&mut p.links), std::mem::take(&mut p.pictures));
    drop(p);
    if let Some(l) = lookup {
        l.finish(Some(waker));
    }
    v.links = links;
    v.pictures = pictures;
    // more above / below: the text fades under the edge
    let surface = t.surface;
    if offset > 0.5 {
        let fr = ui.px(Rect::new(area.x, area.y, area.w, 36.0));
        ui.scene.top_glass_quad(Quad::new(fr, Fill::Vertical(surface, surface.with_alpha(0.0))));
    }
    if offset < max - 0.5 {
        let fr = ui.px(Rect::new(area.x, area.bottom() - 28.0, area.w, 28.0));
        ui.scene.top_glass_quad(Quad::new(fr, Fill::Vertical(surface.with_alpha(0.0), surface)));
    }
    ui.unclip();
    let over = ui.hovered(area);
    widgets::scrollbar(ui, Rect::new(area.right() - 10.0, area.y, 12.0, area.h), total, room, offset, if over { 0.9 } else { 0.0 });
    // away from the bottom: a way back to the latest, floating over the text on a layer of its own
    if !v.follow && max - v.top > room * 0.4 {
        let prev = ui.layer();
        let label = "Latest";
        let st = Style::mono(size::SMALL, t.text).weight(weight::MEDIUM);
        let w = ui.measure(label, st) + 40.0;
        let pr = Rect::new(col.x + (col.w - w) / 2.0, area.bottom() - 42.0, w, 28.0);
        let hot = ui.hover_amount(("focus-latest", id), pr);
        ui.shadow(pr, radius::OVERLAY, 12.0, 3.0, t.shadow.fade(0.6));
        ui.panel(pr, radius::OVERLAY, t.raised.mix(t.text, 0.04 * hot), t.border);
        ui.icon(Rect::new(pr.x + 10.0, pr.y + 6.0, 16.0, 16.0), "\u{F0045}", 14.0, t.muted);
        ui.label(Point::new(pr.x + 28.0, pr.y + 5.0), label, st);
        ui.hit(pr, Action::Focus(id, LATEST));
        ui.set_layer(prev);
    }
    if items.is_empty() && sent.is_empty() {
        empty(ui, c, seen, Rect::new(col.x, area.y + intro_h + 18.0, col.w, (area.h - intro_h - 18.0).max(0.0)));
    }
}

/// The caption under a message on its way.
fn sent_state(age: f32, queued: bool) -> &'static str {
    if queued {
        "Queued · sending on the next turn"
    } else if age < 2.5 {
        "Sending…"
    } else {
        "Sent"
    }
}

fn intro_height(c: &Typo) -> f32 {
    (c.small * 1.7).round() + 18.0
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

/// A session with nothing said yet: its mark and an invitation.
fn empty(ui: &mut Ui, c: &Typo, seen: &Seen, r: Rect) {
    let t = ui.t;
    let cy = r.y + r.h * 0.4;
    let mark = super::chrome::harness_mark(seen.harness);
    ui.mark(mark, Rect::new(r.x + (r.w - 44.0) / 2.0, cy - 64.0, 44.0, 44.0), 40.0, Some(t.text.with_alpha(0.22)));
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

/// Measure (and with `p.draw`, draw) item `i` at `at`, `w` wide: its height (0 for nothing to show).
#[allow(clippy::too_many_arguments)]
fn item(ui: &mut Ui, c: &Typo, it: &Item, blocks: Option<&Vec<Block>>, pics: &[PathBuf], i: usize, at: Point, w: f32, open: bool, narration: bool, p: &mut Paint) -> f32 {
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
            you(ui, c, text, *images, pics, *command, at, w, open, None, p)
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
    let t = ui.t;
    let max_w = (w * 0.86).max(120.0);
    let (px, py) = (16.0, 11.0);
    let inner = max_w - 2.0 * px;
    let spans = [Span { text: text.to_string(), code: command, ..Span::default() }];
    let runs = styled(c, &spans, c.body, c.text, weight::REGULAR, false);
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
            ui.draw_faded(b, Point::new(x + px, y), alpha);
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
            Block::Code { lang, lines } => code(ui, c, lang, lines, Point::new(at.x, y), w, p.draw),
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

fn code(ui: &mut Ui, c: &Typo, lang: &str, lines: &[String], at: Point, w: f32, draw: bool) -> f32 {
    let size = (c.body * 0.86).round();
    let lh = (size * 1.5).round();
    let shown = lines.len().min(CODE_LINES);
    let more = lines.len() - shown;
    let pad = 14.0;
    let h = shown.max(1) as f32 * lh + 2.0 * pad + if more > 0 { lh } else { 0.0 };
    if draw {
        let r = Rect::new(at.x, at.y, w, h);
        ui.round(r, radius::OVERLAY, c.well);
        ui.clip(r, radius::OVERLAY);
        let st = Style::mono(size, c.text.mix(c.code, 0.25));
        for (i, l) in lines.iter().take(shown).enumerate() {
            let b = ui.line(l, st, Some(w - 2.0 * pad));
            ui.draw(&b, Point::new(r.x + pad, r.y + pad + i as f32 * lh + (lh - b.h) / 2.0));
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
        StepKind::Other => "\u{F05B7}",
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
    [
        n(count(&|k| k == StepKind::Read), "Read a file", "Read # files"),
        n(count(&|k| matches!(k, StepKind::Edit | StepKind::Create | StepKind::Delete)), "Changed a file", "Changed # files"),
        n(count(&|k| k == StepKind::Run), "Ran a command", "Ran # commands"),
        n(count(&|k| matches!(k, StepKind::Search | StepKind::Web)), "Searched once", "Searched # times"),
        n(count(&|k| k == StepKind::Agent), "Asked a helper", "Asked # helpers"),
        n(count(&|k| matches!(k, StepKind::Plan | StepKind::Ask | StepKind::Send | StepKind::Other)), "Used a tool", "Used # tools"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
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

/// What the agent did: one quiet line, the pictures it showed you or looked at under it, its steps when open
/// (each opens its file).
#[allow(clippy::too_many_arguments)]
fn did(ui: &mut Ui, c: &Typo, steps: &[Step], i: usize, at: Point, w: f32, open: bool, p: &mut Paint) -> f32 {
    let t = ui.t;
    let id = p.id;
    let row_h = (c.small * 1.9).round();
    let shots = step_pictures(steps);
    let shots_h = if shots.is_empty() { 0.0 } else { SHOWN.1 + 10.0 };
    let mut h = row_h + shots_h;
    if open {
        h += steps.len() as f32 * row_h + 4.0;
    }
    if !p.draw {
        return h;
    }
    let row = Rect::new(at.x - 8.0, at.y, w + 16.0, row_h);
    let hot = ui.hover_amount(("focus-did", id, i), row);
    if hot > 0.01 {
        ui.round(row, radius::CONTROL, t.text.with_alpha(0.045 * hot));
    }
    ui.hit(row, Action::Focus(id, OPEN + i));
    let running = steps.iter().any(|s| s.state == StepState::Running) && (c.working || c.waiting);
    let failed = steps.iter().filter(|s| s.state == StepState::Failed).count();
    let (added, removed) = steps.iter().fold((0, 0), |(a, r), s| (a + s.added, r + s.removed));
    let mid = at.y + row_h / 2.0;
    let quiet = Style::mono(c.small, c.muted);
    let ir = Rect::new(at.x, mid - 8.0, 16.0, 16.0);
    // a step waiting for your answer isn't working: it pauses, in the colour of needing you
    if running && c.waiting {
        ui.icon(ir, "\u{F03E4}", c.small + 1.0, t.danger);
    } else if running {
        ui.spinner(Point::new(at.x + 8.0, mid), 4.4, 1.5, t.accent);
    } else {
        let kind = steps.last().map_or(StepKind::Other, |s| s.kind);
        ui.icon(ir, step_icon(kind), c.small + 1.0, c.faint);
    }
    // the counts on the right: lines added and removed, then the chevron
    let mut right = at.x + w;
    ui.icon(Rect::new(right - 16.0, mid - 8.0, 16.0, 16.0), if open { "\u{F0140}" } else { "\u{F0142}" }, 14.0, c.faint);
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
    let x = at.x + 26.0;
    let what = match steps.iter().rev().find(|s| s.state == StepState::Running) {
        Some(s) if c.waiting => format!("{} · waiting for you", s.label),
        _ => summary(steps, c.now_ms, c.working),
    };
    let mut spans = vec![(what, if running && c.waiting { Style::mono(c.small, c.text.mix(c.muted, 0.2)) } else { quiet })];
    if failed > 0 && !running {
        spans.push((format!(" · {failed} failed"), Style::mono(c.small, t.danger)));
    }
    let b = ui.rich(&spans, c.small, Some((right - x).max(20.0)), false, Some(1));
    ui.draw(&b, Point::new(x, mid - b.h / 2.0));
    // one step about a file: its name opens it, as a path in the terminal does
    if let [one] = steps
        && let Some(uri) = step_file(one, p)
    {
        let r = Rect::new(x, mid - b.h / 2.0, b.w, b.h);
        let on = ui.hovered(r);
        ui.top_fill(Rect::new(x, r.bottom() - 2.0, b.w.min(right - x), 1.0), 0.0, c.muted.with_alpha(if on { 0.85 } else { 0.3 }));
        ui.hit(r, Action::Focus(id, LINK + p.links.len()));
        p.links.push(uri);
    }
    let mut y = at.y + row_h;
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
    if open {
        y += 2.0;
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
            let (glyph, color) = match s.state {
                StepState::Running if c.waiting => ("\u{F03E4}", t.danger),
                StepState::Running => ("", t.accent),
                StepState::Done => ("\u{F012C}", t.good.mix(c.muted, 0.4)),
                StepState::Failed => ("\u{F0156}", t.danger),
            };
            if s.state == StepState::Running && c.working {
                ui.spinner(Point::new(at.x + 34.0, m), 4.0, 1.4, color);
            } else {
                ui.icon(Rect::new(at.x + 26.0, m - 8.0, 16.0, 16.0), glyph, c.small, color);
            }
            let lx = at.x + 50.0;
            let mut runs = vec![(s.label.clone(), Style::mono(c.small, c.text.mix(c.muted, 0.3)))];
            if let Some(d) = s.detail.as_deref().map(|d| d.lines().next().unwrap_or_default()).filter(|d| !d.is_empty() && !s.label.contains(*d)) {
                let d = match s.line.filter(|_| file.is_some()) {
                    Some(l) => format!("{d}:{l}"),
                    None => d.to_string(),
                };
                runs.push((format!("   {d}"), Style::mono(c.small, c.faint)));
            }
            let b = ui.rich(&runs, c.small, Some((w - (lx - at.x)).max(20.0)), false, Some(1));
            ui.draw(&b, Point::new(lx, m - b.h / 2.0));
            y += row_h;
        }
    }
    h
}

fn note(ui: &mut Ui, c: &Typo, kind: NoteKind, text: &str, at: Point, w: f32, draw: bool) -> f32 {
    let t = ui.t;
    let st = Style::mono(c.small, c.faint);
    match kind {
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
    let hint = "↑ ↓ to move · enter to pick · esc to cancel";
    let b = ui.line(hint, Style::mono(c.small, c.faint), Some(inner.w));
    ui.draw(&b, Point::new(inner.x, y));
}

// -------------------------------------------------------------------------------------------- the reply box

/// The reply box's padding and its footer's height (logical px).
const BOX_PAD: (f32, f32) = (16.0, 13.0);
const FOOTER: f32 = 32.0;

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
    let t = ui.t;
    let (pad_x, pad_y) = BOX_PAD;
    let hot = ui.hover_amount(("focus-box", id), r);
    let fill = if focused { t.field_focus } else { t.control.mix(t.control_hover, hot) };
    ui.panel(r, radius::OVERLAY, fill, if focused { t.accent.with_alpha(0.6) } else { t.stroke });
    ui.hit_with(r, Action::Focus(id, BOX), Cursor::Text);
    // what the footer and the lists need, before the box itself is borrowed
    let (model, effort) = app.focus_running_on(id);
    let models = app.focus_models(id);
    let levels = app.focus_levels(id);
    let waits = app.focus_change_waits(id);
    let commands = app.focus_command_rows(id);
    let menu = app.focus.drafts.get(&id).and_then(|d| d.menu);
    let menu_rows: Vec<MenuRow> = menu.map(|m| app.focus_menu_rows(id, m.kind)).unwrap_or_default();
    let Some(d) = app.focus.drafts.get_mut(&id) else { return };
    let attachments: Vec<PathBuf> = d.attachments.iter().map(|a| a.path.clone()).collect();
    let cols = d.cols.max(8);
    let rows = d.area.rows(cols);
    let text = d.area.text().to_string();
    let cur = d.area.cursor();
    let blank = d.blank();
    let command_sel = d.command.min(commands.len().saturating_sub(1));
    // the pictures going with it; the one under the pointer opens its popover, which stays while the pointer
    // crosses to it
    let mut top = r.y + pad_y;
    let mut thumbs: Vec<Rect> = vec![];
    if !attachments.is_empty() {
        let mut p = Paint { draw: true, id, links: vec![], pictures: std::mem::take(&mut v.pictures), media: Some(&mut *media), resolve: None };
        for (n, path) in attachments.iter().enumerate() {
            let pr = Rect::new(r.x + pad_x + n as f32 * (ATTACHED + 8.0), top, ATTACHED, ATTACHED);
            if pr.right() > r.right() - pad_x {
                break;
            }
            picture(ui, &mut p, path, pr, radius::CONTROL + 2.0);
            if v.pop == Some(n) {
                ui.outline_over(pr, radius::CONTROL + 2.0, 1.5, t.accent.with_alpha(0.75));
            }
            thumbs.push(pr);
        }
        v.pictures = std::mem::take(&mut p.pictures);
        top += ATTACHED + 10.0;
    }
    let pointer = ui.pointer;
    let over = thumbs.iter().position(|pr| pointer.is_some_and(|q| pr.contains(q)));
    let stays = v.pop_rect.is_some_and(|pr| pointer.is_some_and(|q| pr.outset(12.0).contains(q)));
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
        let b = ui.line(&ph, Style::mono(c.body, t.muted), Some(r.w - 2.0 * pad_x));
        ui.draw(&b, Point::new(r.x + pad_x, top + (c.lh - b.h) / 2.0));
    }
    let mut caret_at = None;
    for (n, &(s, e)) in rows.iter().enumerate().skip(first).take(room) {
        let y = origin.y + n as f32 * c.lh;
        let b = ui.line(&visible(&text[s..e]), st, None);
        ui.draw(&b, Point::new(origin.x, y + (c.lh - b.h) / 2.0));
        if n == crow {
            caret_at = Some(Point::new(origin.x + b.layout.caret_x(visible(&text[s..cur]).len()) / ui.scale, y));
        }
    }
    ui.unclip();
    // a steady caret, as the terminals' cursor is (a blink would keep the window drawing)
    if focused && let Some(at) = caret_at {
        ui.top_fill(Rect::new(at.x, at.y + (c.lh - c.body * 1.25) / 2.0, 1.8, c.body * 1.25), 0.9, t.accent);
        v.caret = Some(Rect::new(at.x, at.y, 2.0, c.lh));
    }
    // the footer: the key that acts on the right
    let foot = Rect::new(r.x + pad_x, r.bottom() - FOOTER, r.w - 2.0 * pad_x, FOOTER - 6.0);
    let fmid = foot.y + foot.h / 2.0;
    let (key, what, target) = if !commands.is_empty() {
        ("enter", "run", SEND)
    } else if !blank || !attachments.is_empty() {
        ("enter", "send", SEND)
    } else if mood == Mood::Working {
        ("esc", "stop", STOP)
    } else {
        ("", "", 0)
    };
    let mut right = foot.right();
    if !key.is_empty() {
        let kw = widgets::kbd_width(ui, key);
        let wb = ui.line(what, Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
        let w = kw + 6.0 + wb.w + 16.0;
        let br = Rect::new(right + 8.0 - w, fmid - 12.0, w, 24.0);
        let h = ui.hover_amount(("focus-act", id, target), br);
        if h > 0.01 {
            ui.round(br, radius::CONTROL, t.text.with_alpha(0.06 * h));
        }
        widgets::kbd(ui, Point::new(br.x + 8.0, br.y + 3.0), key);
        ui.draw(&wb, Point::new(br.x + 8.0 + kw + 6.0, br.y + (24.0 - wb.h) / 2.0));
        ui.hit(br, Action::Focus(id, target));
        right = br.x - 8.0;
    }
    // ... and on the left the model and how hard it thinks, each opening its list
    let later = if waits { " · Codex switches when this turn ends" } else { "" };
    let mut x = foot.x;
    let mut anchors: [Option<Rect>; 2] = [None, None];
    if let Some(m) = model.as_deref() {
        let name = models.as_ref().and_then(|ms| ms.iter().find(|c| same_model(m, c))).map_or_else(|| model_name(m), |c| c.name.clone());
        let open = menu.is_some_and(|m| m.kind == MenuKind::Model);
        let cr = chip(ui, id, c, Point::new(x, fmid), &name, MODEL_CHIP, models.is_some(), open, right - x);
        if !open && ui.hovered(cr) {
            let tip = if models.is_some() { format!("Switch the model{later}") } else { format!("{name} · bro's route set it when the session started") };
            widgets::tooltip(ui, cr, &tip);
        }
        anchors[0] = Some(cr);
        x = cr.right() + 2.0;
    }
    if levels.is_some() || effort.is_some() {
        if anchors[0].is_some() {
            let dot = ui.line("·", Style::mono(c.small, t.faint), None);
            ui.draw(&dot, Point::new(x + 4.0, fmid - dot.h / 2.0));
            x += dot.w + 16.0;
        }
        let label = effort.as_deref().map_or_else(|| "thinking".to_string(), |e| format!("thinking {}", level_name(e)));
        let open = menu.is_some_and(|m| m.kind == MenuKind::Thinking);
        let cr = chip(ui, id, c, Point::new(x, fmid), &label, THINKING_CHIP, levels.is_some(), open, right - x);
        if !open && ui.hovered(cr) && levels.is_some() {
            widgets::tooltip(ui, cr, &format!("How hard it thinks{later}"));
        }
        anchors[1] = Some(cr);
    }
    // the lists, over everything (they slide in)
    let k = ui.ease(("focus-menu", id), if menu.is_some() { 1.0 } else { 0.0 }, 120.0);
    if let Some(m) = menu
        && let Some(anchor) = anchors[if m.kind == MenuKind::Model { 0 } else { 1 }]
        && !menu_rows.is_empty()
    {
        let title = if m.kind == MenuKind::Model { "Model" } else { "Thinking" };
        pick_menu(ui, id, title, waits.then_some("Codex switches when this turn ends"), &menu_rows, m.sel, anchor, k);
    }
    let kc = ui.ease(("focus-commands", id), if commands.is_empty() { 0.0 } else { 1.0 }, 120.0);
    if !commands.is_empty() {
        commands_list(ui, id, c, &commands, command_sel, r, v, kc);
    }
    v.pop_rect = None;
    if let Some(n) = v.pop
        && let (Some(path), Some(anchor)) = (attachments.get(n), thumbs.get(n))
    {
        v.pop_rect = Some(attachment_popover(ui, id, path, n, *anchor, media));
    }
}

/// A quiet word in the reply box's footer, at `at` (its left, centred on its line) and at most `room` wide, that
/// opens a list when it's `pickable`: the model, how hard it thinks. Returns where it is.
#[allow(clippy::too_many_arguments)]
fn chip(ui: &mut Ui, id: PaneId, c: &Typo, at: Point, label: &str, target: usize, pickable: bool, open: bool, room: f32) -> Rect {
    let t = ui.t;
    let arrow = if pickable { 18.0 } else { 0.0 };
    let b = ui.line(label, Style::mono(c.small, t.faint), Some((room - arrow - 16.0).max(24.0)));
    let r = Rect::new(at.x - 8.0, at.y - 12.0, b.w + arrow + 16.0, 24.0);
    let hot = if pickable { ui.hover_amount(("focus-chip", id, target), r) } else { 0.0 };
    let lit = if open { 1.0 } else { hot };
    if lit > 0.01 {
        ui.round(r, radius::CONTROL, t.text.with_alpha(0.06 * lit));
    }
    let ink = t.faint.mix(t.text, 0.75 * lit);
    ui.draw_color(&b, Point::new(at.x, at.y - b.h / 2.0), ink);
    if pickable {
        ui.icon(Rect::new(at.x + b.w + 3.0, at.y - 7.0, 14.0, 14.0), if open { "\u{F0143}" } else { "\u{F0140}" }, 12.0, ink);
        ui.hit(r, Action::Focus(id, target));
    }
    r
}

/// The model or thinking list over its word in the footer: each row its name and what it's for, a check on what
/// it runs on, the keys' row lit. On a layer of its own, `k` of the way in.
#[allow(clippy::too_many_arguments)]
fn pick_menu(ui: &mut Ui, id: PaneId, title: &str, note: Option<&str>, rows: &[MenuRow], sel: usize, anchor: Rect, k: f32) {
    let t = ui.t;
    let prev = ui.layer();
    let detail = rows.iter().any(|r| r.detail.is_some());
    let row_h = if detail { 48.0 } else { 34.0 };
    let (head, foot) = (32.0, if note.is_some() { 28.0 } else { 0.0 });
    let w = 330.0f32.min(ui.screen.w - 16.0);
    let h = head + rows.len() as f32 * row_h + foot + 6.0;
    let x = anchor.x.min(ui.screen.right() - w - 8.0).max(ui.screen.x + 8.0);
    let y = (anchor.y - 8.0 - h).max(ui.screen.y + 8.0) + (1.0 - k) * 8.0;
    let r = Rect::new(x, y, w, h);
    widgets::card(ui, r, radius::OVERLAY);
    let tb = ui.line(title, Style::new(size::SMALL, weight::SEMIBOLD, t.muted), None);
    ui.draw(&tb, Point::new(r.x + 14.0, r.y + (head - tb.h) / 2.0 + 1.0));
    let mut ry = r.y + head;
    for (n, row) in rows.iter().enumerate() {
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

/// Target `n` of `id`'s focus view was clicked.
pub fn click(g: &mut Gui, id: PaneId, n: usize, p: Point, count: u32) {
    let _ = count;
    focus_pane(g, id);
    // a click anywhere but its list (or the word that opened it) closes the model or thinking list
    if !(MENU_ROW..COMMAND_ROW).contains(&n) && n != MODEL_CHIP && n != THINKING_CHIP {
        g.app.focus.draft(id).menu = None;
    }
    match n {
        AREA => {}
        BOX => {
            let geo = g.panes.get(&id).and_then(|st| st.focus.geo);
            if let Some(geo) = geo {
                let adv = geo.adv;
                let d = g.app.focus.draft(id);
                // the terminal font's cells: as many as the characters before the click
                let mut measure = |s: &str| unicode_width::UnicodeWidthStr::width(s) as f32 * adv;
                let at = byte_at(&d.area, &geo, p, &mut measure);
                d.area.set_cursor(at);
            }
        }
        // as Enter does: a command picked from the list runs, anything else is sent
        SEND => g.app.focus_key(id, crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE)),
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
        TERMINAL => g.app.focus.toggle_peek(id),
        EARLIER => {
            if let Some(st) = g.panes.get_mut(&id) {
                st.focus.shown += HISTORY;
                st.focus.follow = false;
            }
        }
        n if (OPTION..UNATTACH).contains(&n) => {
            let key = g.panes.get(&id).and_then(|st| st.focus.choice.as_ref()).and_then(|c| c.options.get(n - OPTION)).map(|o| o.key.clone());
            if let Some(key) = key
                && let Some(t) = g.app.panes.get_mut(&id).and_then(|p| p.as_term())
            {
                t.send(format!("{key}\r").as_bytes());
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
        n if (MENU_ROW..COMMAND_ROW).contains(&n) => {
            if let Some(m) = g.app.focus.drafts.get(&id).and_then(|d| d.menu) {
                g.app.focus_pick(id, m.kind, n - MENU_ROW);
            }
        }
        n if (COMMAND_ROW..OPEN).contains(&n) => {
            let picked = g.app.focus_command_rows(id).get(n - COMMAND_ROW).cloned();
            if let Some(cmd) = picked {
                g.app.focus_complete(id, &cmd, true);
            }
        }
        n if (OPEN..COPY).contains(&n) => {
            let i = n - OPEN;
            let item = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).and_then(|c| c.items.get(i).cloned());
            if let Some(it) = item
                && let Some(st) = g.panes.get_mut(&id)
            {
                let k = Arc::as_ptr(&it) as usize;
                if !st.focus.open.remove(&k) {
                    st.focus.open.insert(k);
                }
            }
        }
        n if (COPY..LINK).contains(&n) => {
            let i = n - COPY;
            let text = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.insights()).and_then(|s| s.conversation).and_then(|c| match c.items.get(i).map(|it| it.as_ref()) {
                Some(Item::Said { text, .. }) => Some(text.clone()),
                _ => None,
            });
            if let Some(text) = text {
                super::window::set_clipboard(&text);
                g.app.toast(crate::alerts::Kind::Info, "copied");
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
pub fn wheel(g: &mut Gui, id: PaneId, dy: f32) {
    let Some(st) = g.panes.get_mut(&id) else { return };
    let v = &mut st.focus;
    let max = (v.total - v.room).max(0.0);
    v.top = (v.top - dy * 56.0).clamp(0.0, max);
    v.follow = v.top >= max - 2.0;
    g.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: StepKind, label: &str, state: StepState) -> Step {
        Step { id: label.into(), kind, label: label.into(), detail: None, state, added: 0, removed: 0, line: None, at_ms: None }
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
    fn a_switch_of_model_or_thinking_reads_as_a_line() {
        assert_eq!(setting("/model claude-sonnet-5-5").as_deref(), Some("Now on Sonnet 5.5"));
        assert_eq!(setting("/effort xhigh").as_deref(), Some("Thinking extra high"));
        assert_eq!(setting("/model"), None, "its own menu: a command like any other");
        assert_eq!(setting("/compact"), None);
        assert_eq!(sent_state(30.0, true), "Queued · sending on the next turn");
        assert_eq!(sent_state(0.5, false), "Sending…");
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
        Seen { harness: Some(Harness::Claude), conv: None, activity: Some(Activity::Working), done: false, running: None, last_ms, cwd: PathBuf::from("/x"), branch: None, thought, sending: false }
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
    fn links_are_web_addresses_and_the_files_the_session_finds() {
        let spans = markdown::inline("see [docs](https://docs.rs/x), `src/focus.rs:12` and notes.txt");
        let mut found = |m: &FileLinkMatch| (m.path == "src/focus.rs").then(|| Arc::<str>::from(format!("file:///F:/app/{}#L{}", m.path, m.line.unwrap_or(0)).as_str()));
        let mut p = Paint { draw: true, id: 1, links: vec![], pictures: vec![], media: None, resolve: Some(&mut found) };
        let links = link_ranges(&spans, &mut p);
        let text = markdown::plain(&spans);
        let got: Vec<(&str, &str)> = links.iter().map(|(r, t)| (&text[r.clone()], t.as_str())).collect();
        assert_eq!(got, [("docs", "https://docs.rs/x"), ("src/focus.rs:12", "file:///F:/app/src/focus.rs#L12")], "a file nobody found (notes.txt) isn't a link");
    }
}
