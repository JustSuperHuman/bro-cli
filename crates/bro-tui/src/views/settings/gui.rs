//! The settings view in the desktop app, in the manner of Windows 11's Settings: the sections down the left as a
//! navigation list (a monochrome icon, the name, what each is at), the open one on the right as a page: its title
//! and what it's for, then its settings as cards (name and help on the left, the control on the right), headed
//! groups, and the Claude launch preview in a read-only box.
//!
//! Everything comes from the view itself, so clicks, keys and the terminal UI agree: the copy, the values and
//! their colours from `blocks` and `summary`; a control selects its row and steps it like enter / ← / → do
//! (`step(±1)`; a segment steps straight to its option), ↺ is `reset`, a section in the list is `go`, and a click
//! on a row selects it while a click on the selected row steps it (the terminal UI's mouse). The wheel scrolls the
//! page and the selection follows it, so it always stays in view.

use super::{Block, BroOpt, CaveOpt, IMAGINE_RECHECK, IMAGINE_WATCH_FOR, Item, RAIN, Row, SECTIONS, Section, SettingsView, ToolOpt, UpdOpt, VoiceOpt, WatchOpt, cave_choices, live_mode, rows, team_knob};
use bro_core::config::Material;
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::{self, Mark};
use crate::app::gui::kit::{self, Cursor, Style, TextBox, Tokens, Ui, radius, size, weight, widgets};
use crate::pane::{Cx, PaneId};
use bro_core::Harness;
use bro_core::claude_env::{KNOBS, Source};
use bro_core::config::{RainMode, Settings};
use bro_gfx::text::{Align, Face};
use bro_gfx::{Color, Fill, Point, Quad, Rect};
use crossterm::event::KeyModifiers;
use ratatui::style::{Modifier, Style as TuiStyle};
use ratatui::text::Span;
use std::cell::RefCell;
use std::time::Instant;

// ---------------------------------------------------------------------------------------------- targets

/// The view's background (the wheel scrolls the page; a click does nothing).
const BG: usize = 0;
/// The section list's background (the wheel does nothing there).
const NAV_BG: usize = 1;
/// `NAV + n`: section `n` in the list.
const NAV: usize = 10;
/// `ROW + i`: setting `i` (an index into `rows()`): a click selects it, on the selected one it steps.
const ROW: usize = 100;
/// `DEC + i`: ‹ (step −1).
const DEC: usize = 200;
/// `INC + i`: the control itself (switch, value, button, ›): step +1.
const INC: usize = 300;
/// `RESET + i`: back to the preset / bro's default.
const RESET: usize = 400;
/// `PICK + i * 16 + j`: option `j` of a segmented setting `i`.
const PICK: usize = 1000;

// ---------------------------------------------------------------------------------------------- metrics

const NAV_W: f32 = 232.0;
/// a section in the list, and the step from one to the next
const NAV_ROW: f32 = 36.0;
const NAV_PITCH: f32 = 40.0;
/// the page's widest
const MAX_W: f32 = 780.0;
const TOP: f32 = 14.0;
/// the page's title
const TITLE: f32 = 24.0;
/// inside a setting's card
const PAD_X: f32 = 16.0;
const PAD_Y: f32 = 15.0;
/// between two cards of a group
const CARD_GAP: f32 = 4.0;
/// a group heading: room above it, its line, room under it
const SUB_ABOVE: f32 = 26.0;
const SUB_H: f32 = 18.0;
const SUB_BELOW: f32 = 6.0;
/// controls' height
const CTRL_H: f32 = 32.0;
/// between a setting's words and its control
const GAP: f32 = 24.0;
/// room kept above / below the selection when it's scrolled into view
const MARGIN: f32 = 14.0;
/// how far one wheel notch scrolls
const NOTCH: f32 = 56.0;

/// The most lines wrapped text takes (the kit cuts wrapped text to one line when it isn't told).
const LINES: Option<usize> = Some(16);

const CHEVRON_L: &str = "\u{F0141}";
const CHEVRON_R: &str = "\u{F0142}";
const RESTORE: &str = "\u{F099B}";

/// A section's mark (Material Design glyphs, the same marks the terminal UI uses; Claude Code has Claude's logo).
fn glyph(s: Section) -> &'static str {
    match s {
        Section::Appearance => "\u{F03D8}",
        Section::Sessions => "\u{F018D}",
        Section::Voice => "\u{F036C}",
        Section::Caveman => "\u{F08B7}",
        Section::Watchdog => "\u{F0A43}",
        Section::Claude => "\u{F0674}",
        Section::Integrations => "\u{F0463}",
        Section::Sounds => "\u{F057E}",
        Section::Updates => "\u{F02DA}",
    }
}

/// Headings are sentence case: "next Claude launch sets" → "Next Claude launch sets".
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A settings card's fill: a step up from the view it sits on.
fn card_fill(t: &Tokens) -> Color {
    if t.light { Color::WHITE } else { t.text.with_alpha(0.04).over(t.surface) }
}

/// Width [`widgets::kbd`] takes for `chord` (one keycap per key, 3px apart).
fn kbd_width(ui: &mut Ui, chord: &str) -> f32 {
    let st = Style::new(size::CAPTION, weight::MEDIUM, ui.t.muted);
    let keys = widgets::chord_keys(chord);
    let caps: f32 = keys.iter().map(|k| (ui.measure(k, st) + 10.0).max(18.0)).sum();
    caps + 3.0 * keys.len().saturating_sub(1) as f32
}

// ---------------------------------------------------------------------------------------------- scroll state

/// Where the page is scrolled: the desktop app's own, the selection itself stays the view's (`sel`).
#[derive(Default)]
struct Look {
    pane: Option<PaneId>,
    /// the section the page shows (another one starts at its top)
    section: Option<usize>,
    /// bumped when another section opens, so its scroll doesn't ease over from the last one's
    visit: u32,
    /// how far the page is scrolled (px)
    scroll: f32,
    /// the most it scrolls (from the last frame)
    max: f32,
    /// the wheel moved the page since the selection last moved: leave it where it is
    free: bool,
    /// the wheel turned since the last frame: bring the selection into view
    wheeled: bool,
    /// the selection the page was last fitted to
    fitted: Option<usize>,
}

thread_local! {
    static LOOK: RefCell<Look> = RefCell::new(Look::default());
}

// ---------------------------------------------------------------------------------------------- contract

/// Draw the view inside its card (`area`: the card's content).
pub(crate) fn draw(v: &mut SettingsView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    prepare(v, cx);
    let t = ui.t;
    let id = cx.id;
    let all = rows();
    v.sel = v.sel.min(all.len() - 1);
    let section = Section::of(all[v.sel]);
    let si = SECTIONS.iter().position(|s| *s == section).unwrap_or(0);
    let st = cx.svc.settings();

    // the wheel works anywhere over the view
    ui.hit_with(area, Action::View(id, BG), Cursor::Default);

    // the sections down the left, when there's room for them beside the page
    let wide = area.w >= 640.0;
    if wide {
        nav(v, ui, Rect { w: NAV_W, ..area }, si, cx);
    }

    // the page
    let hints_h = if area.h >= 380.0 { 40.0 } else { 0.0 };
    let left = if wide { area.x + NAV_W + 20.0 } else { area.x + 14.0 };
    let col_w = (area.right() - left - 22.0).clamp(200.0, MAX_W);
    let view = Rect::new(left - 4.0, area.y, col_w + 8.0, (area.h - hints_h).max(40.0));
    let page = lay_out(v, ui, section, si, col_w, wide, &all, &st, cx);
    let max = (page.h - view.h).max(0.0);

    // keep the selection in view, as the terminal UI does, unless the wheel moved the page since
    let mut look = LOOK.with(|l| std::mem::take(&mut *l.borrow_mut()));
    if look.pane != Some(id) {
        look = Look { pane: Some(id), ..Look::default() };
    }
    if look.section != Some(si) {
        (look.section, look.scroll, look.free, look.fitted) = (Some(si), 0.0, false, None);
        look.visit = look.visit.wrapping_add(1);
    }
    if look.fitted != Some(v.sel) {
        look.free = false;
    }
    let at = |i: usize| page.items.iter().find(|x| x.0 == i).map(|x| (x.1, x.2));
    if look.wheeled {
        look.wheeled = false;
        // the selection follows the page: the nearest setting wholly in view
        let (top, bottom) = (look.scroll.clamp(0.0, max), look.scroll.clamp(0.0, max) + view.h);
        if let Some((y, h)) = at(v.sel)
            && (y < top || y + h > bottom)
        {
            let inside: Vec<usize> = page.items.iter().filter(|x| x.1 >= top && x.1 + x.2 <= bottom).map(|x| x.0).collect();
            if let Some(i) = if y < top { inside.first() } else { inside.last() } {
                v.sel = *i;
            }
        }
        look.fitted = Some(v.sel);
    } else if !look.free {
        // the first setting brings the page to its top, the last one to its end (the Claude launch preview),
        // any other is kept in view
        if let Some((y, h)) = at(v.sel) {
            if page.items.first().map(|x| x.0) == Some(v.sel) {
                look.scroll = 0.0;
            } else if page.items.last().map(|x| x.0) == Some(v.sel) {
                look.scroll = max;
            } else if y - MARGIN < look.scroll {
                look.scroll = y - MARGIN;
            } else if y + h + MARGIN > look.scroll + view.h {
                look.scroll = y + h + MARGIN - view.h;
            }
        }
        look.fitted = Some(v.sel);
    }
    look.scroll = look.scroll.clamp(0.0, max);
    look.max = max;
    let offset = ui.ease(("settings-scroll", id, look.visit), look.scroll, 150.0);
    LOOK.with(|l| *l.borrow_mut() = look);

    ui.clip(view, 0.0);
    draw_page(v, ui, &page, left, col_w, view.y - offset, view, cx.focused, id);
    // more above / below: the page fades out under the edge
    let surface = t.surface;
    if offset > 0.5 {
        let r = ui.px(Rect::new(view.x, view.y, view.w, 22.0));
        ui.scene.top_glass_quad(Quad::new(r, Fill::Vertical(surface, surface.with_alpha(0.0))));
    }
    if offset < max - 0.5 {
        let r = ui.px(Rect::new(view.x, view.bottom() - 22.0, view.w, 22.0));
        ui.scene.top_glass_quad(Quad::new(r, Fill::Vertical(surface.with_alpha(0.0), surface)));
    }
    ui.unclip();
    let over = ui.hovered(view);
    widgets::scrollbar(ui, Rect::new(view.right() - 2.0, view.y, 12.0, view.h), page.h, view.h, offset, if over { 0.9 } else { 0.45 });

    // the keys, quietly
    if hints_h > 0.0 {
        let mut hints = vec![("j/k", "move"), ("tab", "next section"), ("enter/→", "change"), ("←", "back")];
        match v.row() {
            r if team_knob(r) => hints.push(("r", "bro's default")),
            Row::Preset | Row::Knob(_) => hints.push(("r", "reset to preset")),
            Row::Watch(_) | Row::Voice(VoiceOpt::Key | VoiceOpt::Mode | VoiceOpt::Speaker) => hints.push(("r", "bro's default")),
            _ => {}
        }
        let y = area.bottom() - 26.0;
        ui.hline(left, y - 13.0, col_w, t.hairline);
        let mut x = left;
        let label = Style::new(size::SMALL, weight::REGULAR, t.muted);
        for (key, what) in hints {
            let need = kbd_width(ui, key) + 6.0 + ui.measure(what, label);
            if x + need > left + col_w {
                break;
            }
            x += widgets::kbd(ui, Point::new(x, y), key) + 6.0;
            let b = ui.line(what, label, None);
            ui.draw(&b, Point::new(x, y + (18.0 - b.h) / 2.0));
            x += b.w + 18.0;
        }
    }
    true
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut SettingsView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    let last = rows().len() - 1;
    match n {
        BG | NAV_BG => {}
        n if (NAV..NAV + SECTIONS.len()).contains(&n) => v.go(SECTIONS[n - NAV]),
        // the terminal UI's mouse: a click selects, a click on the selected setting changes it
        n if (ROW..DEC).contains(&n) && n - ROW <= last => {
            if n - ROW == v.sel {
                v.step(1, cx);
            }
            v.sel = n - ROW;
        }
        n if (DEC..INC).contains(&n) && n - DEC <= last => {
            v.sel = n - DEC;
            v.step(-1, cx);
        }
        n if (INC..RESET).contains(&n) && n - INC <= last => {
            v.sel = n - INC;
            v.step(1, cx);
        }
        n if (RESET..PICK).contains(&n) && n - RESET <= last => {
            v.sel = n - RESET;
            v.reset(cx);
        }
        n if n >= PICK && (n - PICK) / 16 <= last => pick(v, (n - PICK) / 16, (n - PICK) % 16, cx),
        _ => {}
    }
}

/// The wheel over target `n` (None: elsewhere in the view): scrolls the page; the selection follows it.
pub(crate) fn wheel(_v: &mut SettingsView, n: Option<usize>, down: bool, cx: &mut Cx) {
    if n.is_some_and(|n| n == NAV_BG || (NAV..NAV + SECTIONS.len()).contains(&n)) {
        return;
    }
    LOOK.with(|l| {
        let mut l = l.borrow_mut();
        if l.pane != Some(cx.id) {
            return;
        }
        l.scroll = (l.scroll + if down { NOTCH } else { -NOTCH }).clamp(0.0, l.max);
        l.free = true;
        l.wheeled = true;
    });
}

/// What the terminal UI's render does before it draws: look for JustTools and JustImagine once, follow a
/// JustImagine update running in a tab, and say whether a job's spinner needs ticks (`tick_every`).
fn prepare(v: &mut SettingsView, cx: &mut Cx) {
    if !v.tools_checked {
        v.tools_checked = true;
        cx.svc.check_justtools();
        cx.svc.check_justimagine(true);
    }
    // an update running in a tab: look again until the checkout moves
    if let Some((started, from)) = v.imagine_update.clone() {
        let moved = cx.svc.state().justimagine.installed.as_ref().is_some_and(|f| f.commit != from);
        if moved || started.elapsed() > IMAGINE_WATCH_FOR {
            v.imagine_update = None;
        } else if v.imagine_looked.is_none_or(|l| l.elapsed() >= IMAGINE_RECHECK) {
            v.imagine_looked = Some(Instant::now());
            cx.svc.refresh_justimagine();
        }
    }
    v.tools_busy = {
        let jt = &cx.svc.state().justtools;
        jt.job.is_some() || jt.paste_busy.is_some()
    };
    let u = cx.svc.state().updates.clone();
    v.update_busy = u.job.is_some() || u.checking;
    v.voice_busy = cx.svc.state().voice.capturing.is_some();
}

/// Option `j` of segmented setting `i`: select it and step until it's on that option (one step from a value on
/// the list; a value off the list steps onto it first).
fn pick(v: &mut SettingsView, i: usize, j: usize, cx: &mut Cx) {
    v.sel = i;
    for _ in 0..3 {
        let cur = option(v.row(), &cx.svc.settings());
        if cur == Some(j) {
            break;
        }
        v.step(cur.map_or(1, |c| j as isize - c as isize), cx);
    }
}

// ---------------------------------------------------------------------------------------------- the model

/// Which of the theme's colours a terminal-UI style names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tone {
    Text,
    Muted,
    Good,
    Inline,
    Shine,
    Danger,
    Accent,
    Other,
}

/// A colour and weight from the terminal UI's style for a value.
#[derive(Clone, Copy)]
struct Ink {
    color: Color,
    bold: bool,
    tone: Tone,
}

impl Ink {
    fn style(self, size: f32) -> Style {
        Style::new(size, if self.bold { weight::SEMIBOLD } else { weight::MEDIUM }, self.color)
    }

    /// A control's value: the terminal UI colours its values to set them apart (inline, shine, the accent); in a
    /// window the control does that, so they're the text colour. Muted (off, Claude's default) and status colours
    /// stay.
    fn value(self, t: &Tokens) -> Ink {
        match self.tone {
            Tone::Inline | Tone::Shine | Tone::Accent | Tone::Other => Ink { color: t.text, tone: Tone::Text, ..self },
            _ => self,
        }
    }

    /// A status beside a button (a version, "not installed"): plain values are the text colour, highlights
    /// (a release waiting, a job running) and status colours stay.
    fn status(self, t: &Tokens) -> Ink {
        match self.tone {
            Tone::Inline | Tone::Accent => Ink { color: t.text, tone: Tone::Text, ..self },
            _ => self,
        }
    }
}

/// The theme colour a terminal-UI style names, as the window's colour.
fn ink(st: TuiStyle, ui: &Ui) -> Ink {
    let (th, t) = (ui.theme, ui.t);
    let (color, tone) = match st.fg {
        None | Some(ratatui::style::Color::Reset) => (t.text, Tone::Text),
        Some(c) if c == th.muted => (t.muted, Tone::Muted),
        Some(c) if c == th.good => (t.good, Tone::Good),
        Some(c) if c == th.inline => (t.inline, Tone::Inline),
        Some(c) if c == th.shine => (t.shine, Tone::Shine),
        Some(c) if c == th.danger => (t.danger, Tone::Danger),
        Some(c) if c == th.accent => (t.accent, Tone::Accent),
        Some(c) => (kit::color(c, t.text, &t.ansi), Tone::Other),
    };
    Ink { color, bold: st.add_modifier.contains(Modifier::BOLD), tone }
}

/// A value that starts with a braille spinner frame: the rest of it.
fn spinning(text: &str) -> Option<&str> {
    let mut chars = text.chars();
    chars.next().filter(|c| ('\u{2800}'..='\u{28FF}').contains(c)).map(|_| chars.as_str().trim_start())
}

/// How a setting is changed.
enum Ctl {
    /// on / off; `live` false dims an on that does nothing yet
    Switch { on: bool, live: bool },
    /// a few choices side by side; `current` None when the value is off the list; `mark` the logo of whose
    /// choices they are
    Seg { options: Vec<String>, current: Option<usize>, ink: Ink, value: String, mark: Option<Mark> },
    /// ‹ value ›; `mark` the logo of the login it names
    Step { value: String, ink: Ink, mono: bool, mark: Option<Mark> },
    /// a value enter acts on ("graphite ›", "open ›")
    Link { value: String, ink: Ink },
    /// what something is at (a version, "checking…"), and a button doing what enter does
    Status { value: String, ink: Ink, spin: bool, button: Option<(&'static str, widgets::Kind)> },
}

fn rain_label(r: RainMode) -> &'static str {
    match r {
        RainMode::Off => "off",
        RainMode::Everywhere => "everywhere",
        RainMode::Working => "working sessions",
    }
}

/// The option a segmented setting is on.
fn option(row: Row, st: &Settings) -> Option<usize> {
    match row {
        Row::Bro(BroOpt::Rain) => RAIN.iter().position(|r| *r == st.rain),
        Row::Bro(BroOpt::Material) => Material::ALL.iter().position(|m| *m == st.material),
        Row::Cave(CaveOpt::Mode) => cave_choices().iter().position(|c| *c == st.caveman.active()),
        Row::Voice(VoiceOpt::Mode) => Some(usize::from(!live_mode(&st.voice))),
        Row::Watch(WatchOpt::Model) => crate::watchdog::MODEL_CHOICES.iter().position(|m| *m == st.watchdog.model),
        Row::Knob(k) => Some(st.claude_env.choice(&KNOBS[k]).0),
        _ => None,
    }
}

/// What `r` goes back to, when it isn't there already (the tooltip).
fn resettable(row: Row, st: &Settings) -> Option<&'static str> {
    let w = &st.watchdog;
    let d = bro_core::config::WatchdogSettings::default();
    let changed = match row {
        Row::Preset => return (st.claude_env.preset_changes() > 0).then_some("reset to preset · r"),
        Row::Knob(k) => {
            let label = if team_knob(row) { "bro's default · r" } else { "reset to preset · r" };
            return (st.claude_env.choice(&KNOBS[k]).1 == Source::Override).then_some(label);
        }
        Row::Watch(WatchOpt::Prompt) => w.prompt.is_some(),
        Row::Watch(WatchOpt::Wake) => w.idle_secs != d.idle_secs,
        Row::Watch(WatchOpt::Reviewer) => w.reviewer != d.reviewer,
        Row::Watch(WatchOpt::Model) => w.model != d.model,
        Row::Watch(WatchOpt::Budget) => w.max_wakes != d.max_wakes,
        Row::Voice(VoiceOpt::Key) => st.voice.hotkey.is_some(),
        Row::Voice(VoiceOpt::Speaker) => st.voice.live_voice.is_some(),
        Row::Voice(VoiceOpt::Mode) => !live_mode(&st.voice),
        _ => false,
    };
    changed.then_some("bro's default · r")
}

/// The logo of the login a reviewer setting names (none for "auto", or a login bro doesn't know).
fn login_mark(id: &str, cx: &Cx) -> Option<Mark> {
    let st = cx.svc.state();
    let p = st.profiles.ready()?.iter().find(|p| p.id == id)?;
    let h = match () {
        _ if p.is_claude() => Harness::Claude,
        _ if p.is_codex() => Harness::Codex,
        _ => return None,
    };
    Some(brand::harness(h))
}

/// The button beside JustTools, JustImagine, Smart Paste or bro's version: what enter would do there (see
/// `SettingsView::tool`, `imagine` and `step`), None while it would do nothing or a look is under way.
fn button(v: &SettingsView, row: Row, cx: &Cx) -> Option<(&'static str, widgets::Kind)> {
    use widgets::Kind::{Primary, Secondary};
    let jt = cx.svc.state().justtools.clone();
    match row {
        Row::Tool(ToolOpt::Install) => match () {
            _ if jt.job.is_some() || jt.checking || !jt.checked => None,
            _ if jt.installed.is_none() => Some(("Install", Primary)),
            _ if jt.update().is_some() => Some(("Update", Primary)),
            _ => Some((if jt.offline { "Try again" } else { "Check" }, Secondary)),
        },
        Row::Tool(ToolOpt::SmartPaste) => match () {
            _ if jt.paste_busy.is_some() || jt.checking || !jt.checked || jt.paste.is_some() => None,
            _ if jt.installed.is_none() => Some(("Install", Primary)),
            _ if jt.update().is_some() && jt.job.is_none() => Some(("Update", Primary)),
            _ => None,
        },
        Row::Tool(ToolOpt::Imagine) => {
            let ji = cx.svc.state().justimagine.clone();
            match &ji.installed {
                _ if ji.checking || !ji.checked || v.imagine_update.is_some() => None,
                None => Some(("Install", Primary)),
                Some(_) if ji.update().is_some() => Some(("Update", Primary)),
                Some(f) if f.url.is_some() => Some(("Open", Secondary)),
                Some(_) => Some((if ji.error.is_some() { "Try again" } else { "Check" }, Secondary)),
            }
        }
        Row::Voice(VoiceOpt::Key) if bro_voice::Voice::can_capture_hotkey() => {
            cx.svc.state().voice.capturing.is_none().then_some(("Set key", Secondary))
        }
        Row::Voice(VoiceOpt::ApiKey) => {
            let vs = cx.svc.state().voice.clone();
            match vs.key_found {
                Some(false) => Some(("Look again", Primary)),
                _ => Some(("Check", Secondary)),
            }
        }
        Row::Update(UpdOpt::Version) => {
            let u = cx.svc.state().updates.clone();
            match () {
                _ if u.job.is_some() || u.checking => None,
                _ if u.available().is_some() => Some(("Install", Primary)),
                _ => Some((if u.offline || u.error.is_some() { "Try again" } else { "Check" }, Secondary)),
            }
        }
        _ => None,
    }
}

/// The control for `it`, from its value as the terminal UI shows it.
fn control(v: &SettingsView, it: &Item, ui: &Ui, st: &Settings, cx: &Cx) -> Ctl {
    let t = ui.t;
    let text: String = it.value.iter().map(|s| s.content.as_ref()).collect();
    let first = it.value.first().map_or(Ink { color: t.text, bold: false, tone: Tone::Text }, |s| ink(s.style, ui));
    // "‹ value ›": the value in the middle
    let (value, vink) = match it.value.as_slice() {
        [_, mid, _] => (mid.content.to_string(), ink(mid.style, ui)),
        _ => (text.trim_end_matches(" ›").to_string(), first),
    };
    let switch = || Ctl::Switch { on: text.starts_with('●'), live: it.value.first().is_some_and(|s| s.style.fg == Some(cx.theme.good)) };
    let seg = |options: Vec<String>, mark: Option<Mark>| Ctl::Seg { options, current: option(it.row, st), ink: vink.value(t), value: value.clone(), mark };
    let step = |mark: Option<Mark>| Ctl::Step { value: value.clone(), ink: vink.value(t), mono: false, mark };
    match it.row {
        Row::Bro(BroOpt::Theme) => Ctl::Link { value, ink: vink.value(t) },
        Row::Bro(BroOpt::Font) => Ctl::Step { value, ink: vink.value(t), mono: true, mark: None },
        Row::Watch(WatchOpt::Reviewer) => step(login_mark(&value, cx)),
        Row::Bro(BroOpt::Screensaver | BroOpt::Tint) | Row::Watch(WatchOpt::Wake | WatchOpt::Budget) | Row::Preset | Row::Voice(VoiceOpt::Speaker) => step(None),
        Row::Voice(VoiceOpt::Key) if !bro_voice::Voice::can_capture_hotkey() => step(None),
        Row::Voice(VoiceOpt::Mode) => seg(vec!["out loud".into(), "as text".into()], None),
        Row::Bro(BroOpt::Material) => seg(Material::ALL.iter().map(|m| m.id().to_string()).collect(), None),
        Row::Bro(BroOpt::Rain) => seg(RAIN.iter().map(|r| rain_label(*r).to_string()).collect(), None),
        Row::Cave(CaveOpt::Mode) => seg(cave_choices().iter().map(|c| c.map_or("off", |l| l.id()).to_string()).collect(), None),
        // Claude's models: Claude's logo beside them
        Row::Watch(WatchOpt::Model) => seg(crate::watchdog::MODEL_CHOICES.iter().map(|m| m.to_string()).collect(), Some(brand::harness(Harness::Claude))),
        // agent teams: a switch, as the terminal UI draws it
        Row::Knob(k) if KNOBS[k].id == bro_core::claude_env::AGENT_TEAMS => switch(),
        Row::Knob(k) => seg(KNOBS[k].options.iter().map(|o| o.label.to_string()).collect(), None),
        Row::Watch(WatchOpt::Prompt) | Row::Tool(ToolOpt::QuickInstall | ToolOpt::Sounds | ToolOpt::PeonPing) => Ctl::Link { value, ink: vink.value(t) },
        Row::Tool(ToolOpt::SoundsOn) => Ctl::Switch { on: text.starts_with('●'), live: true },
        _ if text.starts_with('●') || text.starts_with('○') => switch(),
        row => {
            let (value, spin) = match spinning(&value) {
                Some(rest) => (rest.to_string(), true),
                None => (value, false),
            };
            Ctl::Status { value, ink: vink.status(t), spin, button: button(v, row, cx) }
        }
    }
}

/// A line under a setting: text, or a job's progress (one segment per step).
enum Note {
    Text(TextBox),
    Progress { at: usize, of: usize, text: TextBox },
}

impl Note {
    fn h(&self) -> f32 {
        match self {
            Note::Text(b) => b.h,
            Note::Progress { text, .. } => text.h + 8.0 + 6.0,
        }
    }
}

/// The terminal UI's progress note ("━━━━━━━━────  2 of 5 · what · 3s"): steps done, steps in all, the words.
fn progress_of<'a>(n: &'a [Span<'static>]) -> Option<(usize, usize, &'a str)> {
    let [done, rest, what] = n else { return None };
    let bar = |s: &Span, c: char| s.content.chars().all(|x| x == c).then(|| s.content.chars().count() / 4);
    let at = bar(done, '━')?;
    let left = bar(rest, '─')?;
    (at + left > 0).then(|| (at, at + left, what.content.trim()))
}

/// One setting, laid out.
struct Laid {
    /// its index in `rows()`
    i: usize,
    /// top and height on the page
    y: f32,
    h: f32,
    label: TextBox,
    /// beside the name ("changed", "+ 2 changes"), quiet text
    tag: Option<TextBox>,
    help: Option<TextBox>,
    notes: Vec<Note>,
    ctl: Ctl,
    ctl_w: f32,
    /// ↺'s tooltip, when it has somewhere to go back to
    reset: Option<&'static str>,
    /// the name and help together (the control sits centred beside them)
    text_h: f32,
    progress: bool,
}

enum Piece {
    /// a heading inside a section
    Sub { y: f32, text: String },
    /// settings that sit together, a card each
    Group { y: f32, h: f32, items: Vec<Laid> },
    /// lines of code-like values (the Claude launch preview)
    Well { y: f32, h: f32, lines: Vec<TextBox> },
    Caption { y: f32, b: TextBox },
}

struct Page {
    title: TextBox,
    /// "3 of 7" when the list down the left isn't shown
    count: Option<TextBox>,
    blurb: TextBox,
    pieces: Vec<Piece>,
    /// every setting's (row, top, height) on the page, in order
    items: Vec<(usize, f32, f32)>,
    h: f32,
}

#[allow(clippy::too_many_arguments)]
fn lay_out(v: &SettingsView, ui: &mut Ui, s: Section, si: usize, w: f32, wide: bool, all: &[Row], st: &Settings, cx: &Cx) -> Page {
    let t = ui.t;
    let title = ui.line(s.label(), Style::new(TITLE, weight::SEMIBOLD, t.text), None);
    let count = (!wide).then(|| ui.line(&format!("{} of {}", si + 1, SECTIONS.len()), Style::new(size::SMALL, weight::MEDIUM, t.muted), None));
    let blurb = ui.para(s.blurb(), Style::new(size::BODY, weight::REGULAR, t.muted), w, LINES);
    let mut y = TOP + title.h + 4.0 + blurb.h + 22.0;
    let mut pieces = vec![];
    let mut group: Option<(f32, Vec<Laid>)> = None;
    let mut text: Vec<Vec<Span<'static>>> = vec![];

    fn close(group: &mut Option<(f32, Vec<Laid>)>, pieces: &mut Vec<Piece>, y: f32) {
        if let Some((gy, items)) = group.take() {
            pieces.push(Piece::Group { y: gy, h: y - gy, items });
        }
    }
    // a run of lines: paragraphs between the blank ones; code-like ones (all in the value colour) in a box
    let flush = |text: &mut Vec<Vec<Span<'static>>>, pieces: &mut Vec<Piece>, y: &mut f32, ui: &mut Ui| {
        let lines = std::mem::take(text);
        let mut first = true;
        for para in lines.split(|l| l.is_empty()).filter(|p| !p.is_empty()) {
            *y += if first { 0.0 } else { 12.0 };
            first = false;
            let code = para.iter().all(|l| l.iter().all(|s| s.style.fg == Some(cx.theme.inline)));
            if code {
                let mut out = vec![];
                let mut h = 24.0;
                for l in para {
                    let line: String = l.iter().map(|s| s.content.as_ref()).collect();
                    let b = ui.para(&line, Style::mono(size::SMALL, ui.t.text.mix(ui.t.muted, 0.2)), w - 32.0, LINES);
                    h += b.h + 3.0;
                    out.push(b);
                }
                pieces.push(Piece::Well { y: *y, h: h - 3.0, lines: out });
                *y += h - 3.0;
            } else {
                for l in para {
                    let spans: Vec<(String, Style)> = l
                        .iter()
                        .map(|s| {
                            let k = ink(s.style, ui);
                            (s.content.to_string(), Style::new(size::SMALL, if k.bold { weight::SEMIBOLD } else { weight::REGULAR }, k.color))
                        })
                        .collect();
                    let b = ui.rich(&spans, size::SMALL, Some(w - 8.0), true, LINES);
                    let h = b.h;
                    pieces.push(Piece::Caption { y: *y, b });
                    *y += h + 2.0;
                }
            }
        }
    };

    for b in v.blocks(s, cx) {
        match b {
            Block::Item(it) => {
                flush(&mut text, &mut pieces, &mut y, ui);
                if group.is_none() {
                    if !pieces.is_empty() && !matches!(pieces.last(), Some(Piece::Sub { .. })) {
                        y += 20.0;
                    }
                    group = Some((y, vec![]));
                } else {
                    y += CARD_GAP;
                }
                let l = lay_item(v, ui, it, w, y, all, st, cx);
                y += l.h;
                if let Some((_, items)) = &mut group {
                    items.push(l);
                }
            }
            Block::Sub(head) => {
                close(&mut group, &mut pieces, y);
                flush(&mut text, &mut pieces, &mut y, ui);
                y += SUB_ABOVE;
                pieces.push(Piece::Sub { y, text: sentence(&head) });
                y += SUB_H + SUB_BELOW;
            }
            Block::Text(spans) => {
                close(&mut group, &mut pieces, y);
                text.push(spans);
            }
        }
    }
    close(&mut group, &mut pieces, y);
    flush(&mut text, &mut pieces, &mut y, ui);
    let items = pieces
        .iter()
        .flat_map(|p| match p {
            Piece::Group { items, .. } => items.iter().map(|l| (l.i, l.y, l.h)).collect(),
            _ => vec![],
        })
        .collect();
    Page { title, count, blurb, pieces, items, h: y + 28.0 }
}

#[allow(clippy::too_many_arguments)]
fn lay_item(v: &SettingsView, ui: &mut Ui, it: Item, w: f32, y: f32, all: &[Row], st: &Settings, cx: &Cx) -> Laid {
    let t = ui.t;
    let i = all.iter().position(|r| *r == it.row).unwrap_or(0);
    let mut ctl = control(v, &it, ui, st, cx);
    let mut ctl_w = ctl_width(ui, &ctl);
    let reset = resettable(it.row, st);
    let inner = w - 2.0 * PAD_X;
    // too narrow for the choices side by side: ‹ value › instead
    if let Ctl::Seg { value, ink, mark, .. } = &ctl
        && inner - ctl_w - GAP < 200.0
    {
        ctl = Ctl::Step { value: value.clone(), ink: *ink, mono: false, mark: *mark };
        ctl_w = ctl_width(ui, &ctl);
    }
    let text_w = (inner - ctl_w - GAP - if reset.is_some() { 36.0 } else { 0.0 }).max(140.0);
    let tag = it.tag.as_ref().map(|s| {
        let k = ink(s.style, ui);
        // the terminal UI's inline (a count of changes) is a quiet note here; shine (changed) stays a highlight
        let c = if k.tone == Tone::Inline { t.muted } else { k.color };
        ui.line(s.content.trim(), Style::new(size::SMALL, weight::MEDIUM, c), None)
    });
    let tag_w = tag.as_ref().map_or(0.0, |b| b.w + 10.0);
    let label = ui.line(&it.label, Style::new(size::TITLE, weight::REGULAR, t.text), Some((text_w - tag_w).max(40.0)));
    let help = (!it.help.is_empty()).then(|| ui.para(&it.help, Style::new(size::SMALL, weight::REGULAR, t.muted), text_w, LINES));
    let text_h = label.h + help.as_ref().map_or(0.0, |h| 1.0 + h.h);
    let mut notes = vec![];
    let mut progress = false;
    for n in &it.notes {
        if let Some((at, of, what)) = progress_of(n) {
            progress = true;
            let b = ui.line(what, Style::new(size::SMALL, weight::MEDIUM, t.muted), Some(inner - 20.0));
            notes.push(Note::Progress { at, of, text: b });
        } else {
            let spans: Vec<(String, Style)> = n
                .iter()
                .map(|s| {
                    let k = ink(s.style, ui);
                    (s.content.to_string(), Style::new(size::SMALL, if k.bold { weight::SEMIBOLD } else { weight::REGULAR }, k.color))
                })
                .collect();
            notes.push(Note::Text(ui.rich(&spans, size::SMALL, Some(inner), true, LINES)));
        }
    }
    let notes_h: f32 = notes.iter().map(|n| 8.0 + n.h()).sum();
    let h = PAD_Y + text_h.max(CTRL_H) + notes_h + PAD_Y;
    Laid { i, y, h, label, tag, help, notes, ctl, ctl_w, reset, text_h, progress }
}

// ---------------------------------------------------------------------------------------------- drawing

/// The sections, each with what it's at; where the settings are kept at the bottom.
fn nav(v: &SettingsView, ui: &mut Ui, r: Rect, current: usize, cx: &Cx) {
    let t = ui.t;
    let id = cx.id;
    ui.hit_with(r, Action::View(id, NAV_BG), Cursor::Default);
    widgets::section(ui, Rect::new(r.x + 14.0, r.y + TOP, r.w - 28.0, 18.0), "Settings");
    let mut y = r.y + TOP + 28.0;
    for (n, s) in SECTIONS.into_iter().enumerate() {
        let row = Rect::new(r.x + 4.0, y, r.w - 12.0, NAV_ROW);
        if row.bottom() > r.bottom() - 4.0 {
            break;
        }
        let on = n == current;
        let hot = widgets::row(ui, row, ("settings-nav", id, n), on, Action::View(id, NAV + n));
        // icons stand alone, in the text's colour (a touch quieter until hovered or chosen)
        let mark = if on { t.text } else { t.text.mix(t.muted, 0.3 * (1.0 - hot)) };
        let ir = Rect::new(row.x + 10.0, row.y, 20.0, row.h);
        match s {
            Section::Claude => ui.mark(brand::harness(Harness::Claude), ir, 15.0, Some(mark)),
            _ => ui.icon(ir, glyph(s), 16.0, mark),
        }
        let lb = ui.line(s.label(), Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::REGULAR }, t.text), None);
        let lx = row.x + 40.0;
        ui.draw(&lb, Point::new(lx, row.y + (row.h - lb.h) / 2.0));
        // what it's at
        let sum = v.summary(s, cx);
        let k = ink(sum.style, ui);
        let right = row.right() - 12.0;
        if spinning(&sum.content).is_some() {
            ui.spinner(Point::new(right - 6.0, row.y + row.h / 2.0), 5.0, 2.2, k.color);
        } else if !sum.content.is_empty() {
            let room = right - (lx + lb.w + 8.0);
            if room > 24.0 {
                // quiet unless it's news: a value off the preset in the text colour, an update in its highlight
                let c = match k.tone {
                    Tone::Inline | Tone::Accent | Tone::Other => t.text,
                    _ => k.color,
                };
                let st = Style::new(size::SMALL, if k.bold { weight::SEMIBOLD } else { weight::REGULAR }, c);
                let b = ui.line(&sum.content, st, Some(room));
                ui.draw(&b, Point::new(right - b.w, row.y + (row.h - b.h) / 2.0));
            }
        }
        y += NAV_PITCH;
    }
    if r.bottom() - 34.0 > y + 8.0 {
        let spans = [("saved to ".to_string(), Style::new(size::SMALL, weight::REGULAR, t.faint)), ("~/.bro/v2.toml".to_string(), Style::mono(size::SMALL, t.faint))];
        let b = ui.rich(&spans, size::SMALL, None, false, None);
        ui.draw(&b, Point::new(r.x + 14.0, r.bottom() - 14.0 - b.h));
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_page(v: &SettingsView, ui: &mut Ui, p: &Page, x: f32, w: f32, top: f32, view: Rect, focused: bool, id: PaneId) {
    let t = ui.t;
    // the title and what the section is for (no icon: Windows' Settings pages lead with their name)
    let ty = top + TOP;
    ui.draw(&p.title, Point::new(x, ty));
    if let Some(c) = &p.count {
        ui.draw(c, Point::new(x + p.title.w + 12.0, ty + (p.title.h - c.h) / 2.0 + 2.0));
    }
    ui.draw(&p.blurb, Point::new(x, ty + p.title.h + 4.0));

    let card = card_fill(t);
    for piece in &p.pieces {
        match piece {
            Piece::Sub { y, text } => widgets::section(ui, Rect::new(x + 1.0, top + y, w - 1.0, SUB_H), text),
            Piece::Group { y, h, items } => {
                if top + y + h < view.y || top + y > view.bottom() {
                    continue;
                }
                for it in items {
                    let r = Rect::new(x, top + it.y, w, it.h);
                    if r.bottom() < view.y || r.y > view.bottom() {
                        continue;
                    }
                    item(ui, it, r, it.i == v.sel, focused, id, card);
                }
            }
            Piece::Well { y, h, lines } => {
                // read-only: a sunken box in the terminal font
                let r = Rect::new(x, top + y, w, *h);
                ui.glass_panel(r, radius::CONTROL, t.sunken, t.hairline);
                let mut ly = r.y + 12.0;
                for b in lines {
                    ui.draw(b, Point::new(r.x + 16.0, ly));
                    ly += b.h + 3.0;
                }
            }
            Piece::Caption { y, b } => ui.draw(b, Point::new(x + 1.0, top + y)),
        }
    }
}

/// One setting as a card: its name and tag, help and notes on the left, its control on the right. `card`: the
/// card's fill.
#[allow(clippy::too_many_arguments)]
fn item(ui: &mut Ui, it: &Laid, r: Rect, selected: bool, focused: bool, id: PaneId, card: Color) {
    let t = ui.t;
    let hot = ui.hover_amount(("settings-row", id, it.i), r);
    // the keyboard's place: the selection's fill (lighter than a list row's: it covers a whole card), outlined
    // while the view has the keys
    let fill = if selected { t.selected.fade(0.65).over(card) } else { t.hover.fade(0.8 * hot).over(card) };
    let edge = if selected && focused { t.accent.with_alpha(0.8) } else { t.hairline };
    ui.glass_panel(r, radius::CONTROL, fill, edge);
    ui.hit_with(r, Action::View(id, ROW + it.i), Cursor::Default);

    let x = r.x + PAD_X;
    let body = it.text_h.max(CTRL_H);
    let ty = r.y + PAD_Y + (body - it.text_h) / 2.0;
    ui.draw(&it.label, Point::new(x, ty));
    if let Some(tag) = &it.tag {
        ui.draw(tag, Point::new(x + it.label.w + 10.0, ty + (it.label.h - tag.h) / 2.0 + 0.5));
    }
    if let Some(h) = &it.help {
        ui.draw(h, Point::new(x, ty + it.label.h + 1.0));
    }

    let cy = r.y + PAD_Y + (body - CTRL_H) / 2.0;
    let cr = Rect::new(r.right() - PAD_X - it.ctl_w, cy, it.ctl_w, CTRL_H);
    ctl(ui, cr, &it.ctl, it.i, id, it.progress);
    if let Some(tip) = it.reset {
        let b = Rect::new(cr.x - 36.0, cy + 2.0, 28.0, 28.0);
        if widgets::icon_button(ui, b, RESTORE, t.muted, Action::View(id, RESET + it.i)) {
            widgets::tooltip(ui, b, tip);
        }
    }

    let mut ny = r.y + PAD_Y + body;
    for n in &it.notes {
        ny += 8.0;
        match n {
            Note::Text(b) => ui.draw(b, Point::new(x, ny)),
            Note::Progress { at, of, text } => {
                ui.spinner(Point::new(x + 5.0, ny + text.h / 2.0), 4.5, 2.2, t.accent);
                ui.draw(text, Point::new(x + 16.0, ny));
                // one segment per step, as the terminal UI's ━━━━────
                let mw = (r.w - 2.0 * PAD_X).min(360.0);
                let n = (*of).max(1);
                widgets::segments(ui, Rect::new(x, ny + text.h + 8.0, mw, 6.0), *at as f32 / n as f32, n, t.accent);
            }
        }
        ny += n.h();
    }
}

fn seg_w(ui: &mut Ui, o: &str) -> f32 {
    ui.measure(o, Style::new(size::SMALL, weight::SEMIBOLD, ui.t.text)) + 24.0
}

/// A control's value, in the interface font or the terminal's.
fn value_style(ink: Ink, mono: bool) -> Style {
    let mut st = Style::new(size::BODY, weight::REGULAR, ink.color);
    if mono {
        st.face = Face::Mono;
    }
    st
}

fn value_w(ui: &mut Ui, value: &str, ink: Ink, mono: bool) -> f32 {
    ui.measure(value, value_style(ink, mono))
}

/// The words beside a switch.
fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

/// A logo before a control, and the room it takes.
const MARK: f32 = 16.0;
const MARK_W: f32 = MARK + 10.0;

fn ctl_width(ui: &mut Ui, c: &Ctl) -> f32 {
    match c {
        Ctl::Switch { .. } => {
            let st = Style::new(size::BODY, weight::REGULAR, ui.t.text);
            ui.measure("On", st).max(ui.measure("Off", st)) + 12.0 + widgets::SWITCH_W
        }
        Ctl::Seg { options, mark, .. } => 4.0 + options.iter().map(|o| seg_w(ui, o)).sum::<f32>() + if mark.is_some() { MARK_W } else { 0.0 },
        // a long value (a reviewer's login) is cut rather than crowding the help out
        Ctl::Step { value, ink, mono, mark } => {
            let m = if mark.is_some() { MARK + 6.0 } else { 0.0 };
            (value_w(ui, value, *ink, *mono).min(220.0) + m + 80.0).max(150.0)
        }
        Ctl::Link { value, ink } => value_w(ui, value, *ink, false).max(48.0) + 12.0 + 8.0 + 16.0 + 8.0,
        Ctl::Status { value, ink, spin, button } => {
            let mut w = ui.measure(value, ink.style(size::BODY)).min(260.0);
            if *spin {
                w += 18.0;
            }
            if let Some((label, _)) = button {
                w += 12.0 + widgets::button_width(ui, label).max(88.0);
            }
            w
        }
    }
}

/// Draw control `c` of setting `i` in `r` (its full width, `CTRL_H` high). `progress`: a note under the setting
/// already shows the job's spinner.
fn ctl(ui: &mut Ui, r: Rect, c: &Ctl, i: usize, id: PaneId, progress: bool) {
    let t = ui.t;
    let act = |n: usize| Action::View(id, n);
    match c {
        Ctl::Switch { on, live } => {
            // WinUI's toggle: what it's at, then the switch, against the right edge
            let at = Point::new(r.right() - widgets::SWITCH_W, r.y + (r.h - widgets::SWITCH_H) / 2.0);
            let st = Style::new(size::BODY, weight::REGULAR, if *on && !*live { t.muted } else { t.text });
            let words = Rect::new(r.x, r.y, at.x - 12.0 - r.x, r.h);
            ui.label_in(words, on_off(*on), st, Align::Right);
            ui.hit(words, act(INC + i));
            switch(ui, at, *on, *live, act(INC + i));
        }
        Ctl::Seg { options, current, mark, .. } => {
            let mut r = r;
            if let Some(m) = mark {
                ui.mark(*m, Rect::new(r.x, r.y, MARK, r.h), MARK, None);
                r = Rect { x: r.x + MARK_W, w: r.w - MARK_W, ..r };
            }
            widgets::control(ui, r, t.control);
            let mut x = r.x + 2.0;
            for (j, o) in options.iter().enumerate() {
                let c = Rect::new(x, r.y + 2.0, seg_w(ui, o), r.h - 4.0);
                let on = *current == Some(j);
                if on {
                    ui.panel(c, radius::CONTROL - 1.0, if t.light { Color::WHITE } else { t.text.with_alpha(0.09) }, t.stroke);
                } else {
                    let hot = ui.hover_amount(("settings-seg", id, i, j), c);
                    if hot > 0.0 {
                        ui.round(c, radius::CONTROL - 1.0, t.text.with_alpha(0.045 * hot));
                    }
                }
                let st = Style::new(size::SMALL, if on { weight::SEMIBOLD } else { weight::MEDIUM }, if on { t.text } else { t.muted });
                ui.label_in(c, o, st, Align::Center);
                ui.hit(c, act(PICK + i * 16 + j));
                x += c.w;
            }
        }
        Ctl::Step { value, ink, mono, mark } => {
            let hot = ui.hover_amount(("settings-step-box", id, i), r);
            widgets::control(ui, r, t.control.mix(t.control_hover, hot));
            let l = Rect::new(r.x + 4.0, r.y + 4.0, 24.0, r.h - 8.0);
            let rr = Rect::new(r.right() - 28.0, r.y + 4.0, 24.0, r.h - 8.0);
            for (b, g, n) in [(l, CHEVRON_L, DEC + i), (rr, CHEVRON_R, INC + i)] {
                let hot = ui.hover_amount(("settings-step", id, n), b);
                if hot > 0.0 {
                    let down = ui.pressing && hot > 0.5;
                    ui.round(b, radius::CONTROL, t.text.with_alpha(if down { 0.04 } else { 0.07 * hot }));
                }
                ui.icon(b, g, size::BODY, t.muted.mix(t.text, hot));
                ui.hit(b, act(n));
            }
            let mid = Rect::new(l.right() + 4.0, r.y, rr.x - l.right() - 8.0, r.h);
            let st = value_style(*ink, *mono);
            match mark {
                Some(m) => {
                    // the login's logo, then its name, centred together
                    let b = ui.line(value, st, Some((mid.w - MARK - 6.0).max(10.0)));
                    let x = mid.x + (mid.w - MARK - 6.0 - b.w) / 2.0;
                    ui.mark(*m, Rect::new(x, r.y, MARK, r.h), MARK - 2.0, None);
                    ui.draw(&b, Point::new(x + MARK + 6.0, r.y + (r.h - b.h) / 2.0));
                }
                None => {
                    ui.label_in(mid, value, st, Align::Center);
                }
            }
            ui.hit(mid, act(INC + i));
        }
        Ctl::Link { value, ink } => {
            // a button that opens something: the value, then a chevron
            let hot = ui.hover_amount(("settings-link", id, i), r);
            let down = ui.pressing && hot > 0.5;
            widgets::control(ui, r, if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) });
            let b = ui.line(value, value_style(*ink, false), Some((r.w - 44.0).max(10.0)));
            ui.draw(&b, Point::new(r.x + 12.0, r.y + (r.h - b.h) / 2.0));
            ui.icon(Rect::new(r.right() - 28.0, r.y, 20.0, r.h), CHEVRON_R, size::BODY, t.muted.mix(t.text, hot));
            ui.hit(r, act(INC + i));
        }
        Ctl::Status { value, ink, spin, button } => {
            let mut right = r.right();
            if let Some((label, kind)) = button {
                let bw = widgets::button_width(ui, label).max(88.0);
                let br = Rect::new(right - bw, r.y, bw, r.h);
                widgets::button(ui, br, label, *kind, act(INC + i));
                right = br.x - 12.0;
            }
            let b = ui.line(value, ink.style(size::BODY), Some(260.0));
            let x = right - b.w;
            ui.draw(&b, Point::new(x, r.y + (r.h - b.h) / 2.0));
            if *spin && !progress {
                ui.spinner(Point::new(x - 11.0, r.y + r.h / 2.0), 4.5, 2.2, ink.color);
            }
        }
    }
}

/// An on / off switch, the kit's (40×20, WinUI's); `live` false shows an on that does nothing yet in grey.
fn switch(ui: &mut Ui, at: Point, on: bool, live: bool, action: Action) {
    if live || !on {
        widgets::switch(ui, at, on, action);
        return;
    }
    // the kit's switch, its accent swapped for grey (the same keys, so it doesn't jump when it comes alive)
    let t = ui.t;
    let r = Rect::new(at.x, at.y, widgets::SWITCH_W, widgets::SWITCH_H);
    let k = ui.ease(("switch", at.x as i32, at.y as i32), 1.0, 140.0);
    let hot = ui.hover_amount(("switch-hover", at.x as i32, at.y as i32), r.outset(3.0));
    let lit = t.text.with_alpha(if t.light { 0.28 } else { 0.3 });
    ui.round(r, r.h / 2.0, t.text.with_alpha(0.04 * hot).mix(lit, k));
    if k < 1.0 {
        ui.outline(r, r.h / 2.0, 1.0, t.muted.fade(1.0 - k));
    }
    let knob = Point::new(r.x + 10.0 + 20.0 * k, r.y + r.h / 2.0);
    ui.circle(knob, 6.0 + hot, t.muted.mix(if t.light { Color::WHITE } else { t.text.mix(t.muted, 0.4) }, k));
    ui.hit(r.outset(3.0), action);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_terminal_ui_progress_note() {
        let t = crate::theme::get("graphite");
        let note = super::super::progress(3, 5, "checking its SHA-256 · 12s", &t);
        assert_eq!(progress_of(&note), Some((3, 5, "3 of 5 · checking its SHA-256 · 12s")));
        let all_done = super::super::progress(3, 3, "replacing bro · 9s", &t);
        assert_eq!(progress_of(&all_done).map(|p| (p.0, p.1)), Some((3, 3)));
        assert_eq!(progress_of(&[Span::raw("✓ installed")]), None);
    }

    #[test]
    fn spinner_frames_come_off_the_value() {
        assert_eq!(spinning(&format!("{} updating", super::super::spinner(Instant::now()))), Some("updating"));
        assert_eq!(spinning("2.0.29 → 2.1.0"), None);
        assert_eq!(spinning(""), None);
    }

    #[test]
    fn segments_know_their_option_and_resets_their_target() {
        let mut st = Settings::default();
        assert_eq!(option(Row::Bro(BroOpt::Rain), &st), RAIN.iter().position(|r| *r == st.rain));
        assert_eq!(option(Row::Watch(WatchOpt::Model), &st), Some(0));
        st.watchdog.model = "claude-opus-4-1".into();
        assert_eq!(option(Row::Watch(WatchOpt::Model), &st), None, "a model off the list has no segment");
        assert_eq!(resettable(Row::Watch(WatchOpt::Model), &st), Some("bro's default · r"));
        assert_eq!(resettable(Row::Watch(WatchOpt::Wake), &st), None);
        assert_eq!(resettable(Row::Preset, &st), None);
        let k = KNOBS.iter().position(|k| k.id == "telemetry").unwrap();
        st.claude_env.set_choice(&KNOBS[k], 1);
        assert_eq!(option(Row::Knob(k), &st), Some(1));
        assert_eq!(resettable(Row::Knob(k), &st), Some("reset to preset · r"));
        assert_eq!(resettable(Row::Preset, &st), Some("reset to preset · r"));
    }

    #[test]
    fn headings_are_sentence_case() {
        assert_eq!(sentence("next Claude launch sets"), "Next Claude launch sets");
        assert_eq!(sentence("long runs"), "Long runs");
        assert_eq!(sentence(""), "");
    }

    /// A frame's drawing context with no window.
    fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
        let theme = crate::theme::get("graphite");
        let tokens = Tokens::from_theme(&theme);
        let mono = crate::fonts::get("jetbrains-mono").map(|f| f.data).unwrap_or(&[]);
        let mut text = bro_gfx::Text::new(mono, crate::fonts::SYMBOLS.data, false);
        text.set_mono(14.0, 1.18);
        let mut scene = bro_gfx::Scene::new();
        let mut hits = vec![];
        let mut anim = kit::Anims::default();
        let mut ui = Ui {
            scene: &mut scene,
            text: &mut text,
            t: &tokens,
            theme: &theme,
            scale: 1.0,
            pointer: None,
            pressing: false,
            hits: &mut hits,
            top_layer: 0,
            anim: &mut anim,
            now: Instant::now(),
            time: 0.0,
            ticking: false,
            screen: Rect::new(0.0, 0.0, 1200.0, 800.0),
        };
        f(&mut ui)
    }

    fn laid(page: &Page, row: Row) -> &Laid {
        let i = rows().iter().position(|r| *r == row).unwrap();
        page.pieces
            .iter()
            .flat_map(|p| match p {
                Piece::Group { items, .. } => items.iter().collect(),
                _ => vec![],
            })
            .find(|l| l.i == i)
            .unwrap()
    }

    /// What the demo never shows: a JustTools update, Smart Paste switching and bro updating itself turn into a
    /// spinner with the step's words and one segment per step, the sections' summaries spin, and no button offers
    /// what enter would ignore meanwhile.
    #[test]
    fn running_jobs_show_spinners_and_steps() {
        use crate::services::justtools::{Job, JobKind};
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(Settings::default(), tx.clone());
        svc.update(|st| {
            st.justtools.checked = true;
            st.justtools.job = Some(Job { kind: JobKind::Update, step: bro_core::justtools::Step::Verify, started: Instant::now() });
            st.justtools.paste_busy = Some(Instant::now());
            st.updates.job = Some(crate::services::updates::Job { step: bro_core::self_update::Step::Verify, started: Instant::now() });
        });
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        let cx = Cx { id: 1, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        let v = SettingsView::default();
        let st = svc.settings();
        let all = rows();
        with_ui(|ui| {
            let page = lay_out(&v, ui, Section::Integrations, 5, 700.0, true, &all, &st, &cx);
            let tools = laid(&page, Row::Tool(ToolOpt::Install));
            assert!(matches!(&tools.ctl, Ctl::Status { value, spin: true, button: None, .. } if value == "updating"));
            assert!(tools.progress, "the note under it carries the spinner");
            match tools.notes.as_slice() {
                [Note::Progress { at: 3, of: 5, .. }] => {}
                _ => panic!("one progress note, step 3 of 5"),
            }
            if bro_core::justpaste::supported() {
                let paste = laid(&page, Row::Tool(ToolOpt::SmartPaste));
                assert!(matches!(&paste.ctl, Ctl::Status { value, spin: true, button: None, .. } if value == "switching"));
            }
            let page = lay_out(&v, ui, Section::Updates, 6, 700.0, true, &all, &st, &cx);
            let version = laid(&page, Row::Update(UpdOpt::Version));
            assert!(matches!(&version.ctl, Ctl::Status { value, spin: true, button: None, .. } if value == "updating"));
            match version.notes.as_slice() {
                [Note::Progress { at: 2, of: 3, .. }] => {}
                _ => panic!("one progress note, step 2 of 3"),
            }
        });
        assert!(spinning(&v.summary(Section::Integrations, &cx).content).is_some(), "the list spins beside Integrations");
        assert!(spinning(&v.summary(Section::Updates, &cx).content).is_some(), "and beside Updates");
    }

    /// The skill downloading on its own: its row spins "adding" while JustTools itself stays put.
    #[test]
    fn a_skill_download_spins_on_its_own_row() {
        use crate::services::justtools::{Job, JobKind};
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(Settings::default(), tx.clone());
        svc.update(|st| {
            st.justtools.checked = true;
            st.justtools.job = Some(Job { kind: JobKind::Skill, step: bro_core::justtools::Step::Skill, started: Instant::now() });
        });
        let theme = crate::theme::get("graphite");
        let mut actions = vec![];
        let cx = Cx { id: 1, theme: &theme, svc: &svc, tx: &tx, actions: &mut actions, focused: true };
        let v = SettingsView::default();
        let st = svc.settings();
        let all = rows();
        with_ui(|ui| {
            let page = lay_out(&v, ui, Section::Integrations, 5, 700.0, true, &all, &st, &cx);
            let skill = laid(&page, Row::Tool(ToolOpt::Skill));
            assert!(matches!(&skill.ctl, Ctl::Status { value, spin: true, .. } if value == "adding"));
            let tools = laid(&page, Row::Tool(ToolOpt::Install));
            assert!(!tools.progress && matches!(&tools.ctl, Ctl::Status { spin: false, .. }));
        });
    }
}
