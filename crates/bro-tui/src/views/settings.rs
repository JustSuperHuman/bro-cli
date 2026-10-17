//! Settings view: bro's own everyday options (theme, icons, auto-resume, auto-approve, recaps, compression), watchdog
//! defaults and the reviewer's prompt, caveman
//! mode (terse replies in new Claude / Codex / Pi sessions), integrations (JustTools, its skill, Quick Install, the JustWatch skill and JustImagine), bro's own updates and the Claude Code environment bro sets on every Claude launch — a preset (Standard, Quiet, Private, Lockdown,
//! Incognito, Presenter, Marathon) plus per-switch overrides. Saved to `~/.bro/v2.toml`; the Claude Code
//! part applies to sessions launched from now on.

use crate::keymap::Act;
use crate::alerts::Kind;
use crate::pane::{Action, Cx, Pane, Place};
use crate::services::justimagine::commits;
use crate::services::justtools::JobKind;
use crate::ui::{self, fg, muted};
use bro_core::caveman::Level;
use bro_core::claude_env::{self, ClaudeEnvSettings, KNOBS, PRESETS, Source};
use bro_core::config::RainMode;
use bro_core::justtools::Step;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// bro's own options, shown above the Claude Code section.
#[derive(Clone, Copy, PartialEq, Debug)]
enum BroOpt {
    Theme,
    Font,
    Rain,
    Screensaver,
    SharedBorders,
    Icons,
    AutoResume,
    AutoApprove,
    Recaps,
    Compression,
}

const BRO_OPTS: [BroOpt; 10] = [BroOpt::Theme, BroOpt::Font, BroOpt::Rain, BroOpt::Screensaver, BroOpt::SharedBorders, BroOpt::Icons, BroOpt::AutoResume, BroOpt::AutoApprove, BroOpt::Recaps, BroOpt::Compression];

/// bro's options this window can use: the font only where the terminal can switch it.
fn bro_opts() -> Vec<BroOpt> {
    BRO_OPTS.into_iter().filter(|o| *o != BroOpt::Font || crate::fonts::available()).collect()
}

const RAIN: [RainMode; 3] = [RainMode::Off, RainMode::Everywhere, RainMode::Working];

/// The summary screensaver's waits, in seconds (0 = off).
const SCREENSAVER: [u64; 7] = [0, 60, 120, 300, 600, 900, 1800];

impl BroOpt {
    fn label(self) -> &'static str {
        match self {
            BroOpt::Theme => "Theme",
            BroOpt::Font => "Font",
            BroOpt::Rain => "Digital rain",
            BroOpt::Screensaver => "Summary screensaver",
            BroOpt::SharedBorders => "Shared tile borders",
            BroOpt::Icons => "Nerd Font icons",
            BroOpt::AutoResume => "Auto-resume",
            BroOpt::AutoApprove => "Auto-approve prompts",
            BroOpt::Recaps => "Recaps",
            BroOpt::Compression => "Request compression",
        }
    }
    fn help(self) -> &'static str {
        match self {
            BroOpt::Theme => "enter opens the picker with a live preview",
            BroOpt::Font => "this window only, switches as you go: ←/→ to try them",
            BroOpt::Rain => "over any theme: everywhere, or only in sessions whose agent is mid-turn",
            BroOpt::Screensaver => "step away and every session turns over to what it's working on; any touch turns them back",
            BroOpt::SharedBorders => "tiles that sit together share one line instead of two; status and × move up beside the title",
            BroOpt::Icons => "turn off if icons show as boxes",
            BroOpt::AutoResume => "types `continue` when an agent stops on an API error or a limit resets",
            BroOpt::AutoApprove => "answers Yes to Claude's \"Do you want to proceed?\" (bypass mode's rm / path safety checks)",
            BroOpt::Recaps => "asks Claude for a /recap of a pane left waiting unseen (a few cents each)",
            BroOpt::Compression => "routes new Claude sessions through bro-proxy to trim requests",
        }
    }
    /// How bro looks (Appearance) rather than what it does (Sessions).
    fn looks(self) -> bool {
        matches!(self, BroOpt::Theme | BroOpt::Font | BroOpt::Rain | BroOpt::Screensaver | BroOpt::SharedBorders | BroOpt::Icons)
    }
    /// The value as shown.
    fn value(self, cx: &Cx) -> String {
        let s = cx.svc.settings();
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        match self {
            BroOpt::Theme => cx.theme.name.clone(),
            BroOpt::Font => s.font.as_deref().and_then(crate::fonts::get).map_or("terminal's own", |f| f.label).to_string(),
            BroOpt::Rain => match s.rain {
                RainMode::Off => "off",
                RainMode::Everywhere => "everywhere",
                RainMode::Working => "working sessions",
            }
            .to_string(),
            BroOpt::Screensaver => match s.summary_screensaver_secs {
                0 => "off".to_string(),
                n if n < 60 => format!("after {n} s"),
                n => format!("after {} min", n / 60),
            },
            BroOpt::SharedBorders => on(s.shared_borders),
            BroOpt::Icons => on(ui::NERD.load(Ordering::Relaxed)),
            BroOpt::AutoResume => on(s.auto_resume.enabled),
            BroOpt::AutoApprove => on(s.auto_approve),
            BroOpt::Recaps => on(s.insights.generate_recaps),
            BroOpt::Compression => on(s.proxy.trim),
        }
    }
    /// Change it: on / off flips either way, a choice steps by `dir`, the theme opens its picker.
    fn change(self, dir: isize, cx: &mut Cx) {
        match self {
            BroOpt::Font => {
                // the terminal's own font, then each bundled one
                let mut s = cx.svc.settings();
                let ids: Vec<Option<&str>> = std::iter::once(None).chain(crate::fonts::FONTS.iter().map(|f| Some(f.id))).collect();
                let i = ids.iter().position(|id| *id == s.font.as_deref()).unwrap_or(0) as isize;
                let next = ids[(i + dir).rem_euclid(ids.len() as isize) as usize];
                crate::fonts::apply(next, cx.tx);
                s.font = next.map(str::to_string);
                cx.svc.save_settings(s);
            }
            BroOpt::Rain => {
                let mut s = cx.svc.settings();
                let i = RAIN.iter().position(|r| *r == s.rain).unwrap_or(0) as isize;
                s.rain = RAIN[(i + dir).rem_euclid(RAIN.len() as isize) as usize];
                cx.svc.save_settings(s);
            }
            BroOpt::Screensaver => {
                let mut s = cx.svc.settings();
                let i = SCREENSAVER.iter().position(|n| *n >= s.summary_screensaver_secs).unwrap_or(0) as isize;
                s.summary_screensaver_secs = SCREENSAVER[(i + dir).rem_euclid(SCREENSAVER.len() as isize) as usize];
                cx.svc.save_settings(s);
            }
            BroOpt::Theme => cx.act(Action::Run(Act::Themes)),
            BroOpt::Icons => cx.act(Action::Run(Act::ToggleIcons)),
            BroOpt::AutoResume => cx.act(Action::Run(Act::ToggleAutoResume)),
            BroOpt::Compression => cx.act(Action::Run(Act::ToggleTrim)),
            BroOpt::AutoApprove => {
                let mut s = cx.svc.settings();
                s.auto_approve = !s.auto_approve;
                cx.svc.save_settings(s);
            }
            BroOpt::SharedBorders => {
                let mut s = cx.svc.settings();
                s.shared_borders = !s.shared_borders;
                cx.svc.save_settings(s);
            }
            BroOpt::Recaps => {
                let mut s = cx.svc.settings();
                s.insights.generate_recaps = !s.insights.generate_recaps;
                cx.svc.save_settings(s);
            }
        }
    }
}

/// Caveman mode's rows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CaveOpt {
    Mode,
    Subagents,
}

/// Off, then each level.
fn cave_choices() -> Vec<Option<Level>> {
    std::iter::once(None).chain(Level::ALL.map(Some)).collect()
}

/// JustTools' rows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ToolOpt {
    Install,
    /// opens the Quick Install checklist
    QuickInstall,
    Skill,
    AutoUpdate,
    /// the JustWatch skill (Windows only: JustWatch is a Windows app)
    WatchSkill,
    /// JustImagine: its version, and an update when GitHub has newer commits
    Imagine,
}

/// How often to look at JustImagine while an update started here runs in its tab.
const IMAGINE_RECHECK: Duration = Duration::from_secs(3);
/// Stop looking after this long.
const IMAGINE_WATCH_FOR: Duration = Duration::from_secs(10 * 60);

/// bro's own updates' rows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum UpdOpt {
    /// the running version: enter checks now, or installs a newer release
    Version,
    /// look when bro opens and every 12 hours
    Check,
}

/// What an install is doing at `step`.
pub(crate) fn step_label(step: Step) -> &'static str {
    match step {
        Step::Installer => "fetching the installer",
        Step::Download => "downloading the release",
        Step::Verify => "checking its SHA-256",
        Step::Install => "installing the commands",
        Step::Skill => "adding the skill",
    }
}

/// A spinner frame for a job that began at `started`.
pub(crate) fn spinner(started: Instant) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(started.elapsed().as_millis() / 100) as usize % FRAMES.len()]
}

/// The watchdog section's rows: defaults for new watchdogs and the reviewer's instructions.
#[derive(Clone, Copy, PartialEq, Debug)]
enum WatchOpt {
    /// opens the prompt editor
    Prompt,
    Wake,
    Reviewer,
    Model,
    Budget,
}

const WATCH_OPTS: [WatchOpt; 5] = [WatchOpt::Prompt, WatchOpt::Wake, WatchOpt::Reviewer, WatchOpt::Model, WatchOpt::Budget];

/// A selectable row, in the order the sections run: bro's own options (Appearance, then Sessions), caveman mode,
/// watchdogs, the Claude preset and each [`KNOBS`] switch, the integrations, then bro's updates.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Bro(BroOpt),
    Cave(CaveOpt),
    Tool(ToolOpt),
    Update(UpdOpt),
    Watch(WatchOpt),
    Preset,
    Knob(usize),
}

fn rows() -> Vec<Row> {
    bro_opts()
        .into_iter()
        .map(Row::Bro)
        .chain([Row::Cave(CaveOpt::Mode), Row::Cave(CaveOpt::Subagents)])
        .chain(WATCH_OPTS.map(Row::Watch))
        .chain([Row::Preset])
        .chain((0..KNOBS.len()).map(Row::Knob))
        .chain([Row::Tool(ToolOpt::Install), Row::Tool(ToolOpt::QuickInstall), Row::Tool(ToolOpt::Skill), Row::Tool(ToolOpt::AutoUpdate)])
        .chain(bro_core::justwatch::supported().then_some(Row::Tool(ToolOpt::WatchSkill)))
        .chain([Row::Tool(ToolOpt::Imagine), Row::Update(UpdOpt::Version), Row::Update(UpdOpt::Check)])
        .collect()
}

/// The sections down the left; every row belongs to one.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Section {
    Appearance,
    Sessions,
    Caveman,
    Watchdog,
    Claude,
    Integrations,
    Updates,
}

const SECTIONS: [Section; 7] = [Section::Appearance, Section::Sessions, Section::Caveman, Section::Watchdog, Section::Claude, Section::Integrations, Section::Updates];

impl Section {
    fn of(r: Row) -> Section {
        match r {
            Row::Bro(o) if o.looks() => Section::Appearance,
            Row::Bro(_) => Section::Sessions,
            Row::Cave(_) => Section::Caveman,
            Row::Watch(_) => Section::Watchdog,
            Row::Preset | Row::Knob(_) => Section::Claude,
            Row::Tool(_) => Section::Integrations,
            Row::Update(_) => Section::Updates,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Sessions => "Sessions",
            Section::Caveman => "Caveman",
            Section::Watchdog => "Watchdog",
            Section::Claude => "Claude Code",
            Section::Integrations => "Integrations",
            Section::Updates => "Updates",
        }
    }
    /// Its [`ui::icon`] name.
    fn icon(self) -> &'static str {
        match self {
            Section::Appearance => "theme",
            Section::Sessions => "term",
            Section::Caveman => "caveman",
            Section::Watchdog => "watchdog",
            Section::Claude => "claude",
            Section::Integrations => "rocket",
            Section::Updates => "history",
        }
    }
    /// The line under its title.
    fn blurb(self) -> &'static str {
        match self {
            Section::Appearance => "How bro looks. Every change shows up as you make it.",
            Section::Sessions => "What bro does on its own while your agents work.",
            Section::Caveman => "Terse replies with every fact kept, in new Claude, Codex and Pi sessions.",
            Section::Watchdog => "A fresh reviewer reads a session that went quiet and hands it its next task. Prefix w on a session starts one.",
            Section::Claude => "The environment bro sets on every Claude launch. Sessions launched from now on pick it up.",
            Section::Integrations => "Tools your agents can call. bro installs them and tells you when GitHub has something newer.",
            Section::Updates => "New bro releases from GitHub. Nothing installs until you say so.",
        }
    }
}

/// One setting as drawn: its name and value, what it does, then any status lines under that.
struct Item {
    row: Row,
    label: String,
    /// beside the name: "changed", "+ 2 changes"
    tag: Option<Span<'static>>,
    value: Vec<Span<'static>>,
    help: String,
    notes: Vec<Vec<Span<'static>>>,
}

impl Item {
    fn new(row: Row, label: &str, value: Vec<Span<'static>>, help: impl Into<String>) -> Item {
        Item { row, label: label.into(), tag: None, value, help: help.into(), notes: vec![] }
    }
    fn note(mut self, spans: Vec<Span<'static>>) -> Item {
        self.notes.push(spans);
        self
    }
}

/// What a section is drawn from.
enum Block {
    Item(Item),
    /// a heading inside a section
    Sub(String),
    /// a line that isn't a setting
    Text(Vec<Span<'static>>),
}

/// "● on" / "○ off"; `live` false dims an on that does nothing yet.
fn switch(on: bool, live: bool, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    match (on, live) {
        (true, true) => vec![Span::styled("● on", fg(t.good).add_modifier(Modifier::BOLD))],
        (true, false) => vec![Span::styled("● on", muted(t))],
        _ => vec![Span::styled("○ off", muted(t))],
    }
}

/// "‹ value ›": ←/→ step through the choices.
fn choice(v: impl Into<String>, style: Style, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    vec![Span::styled("‹ ", muted(t)), Span::styled(v.into(), style), Span::styled(" ›", muted(t))]
}

/// A value enter acts on rather than steps.
fn shown(v: impl Into<String>, style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(v.into(), style)]
}

/// `text` word-wrapped to `w` columns; a word wider than that is cut across lines.
fn wrap(text: &str, w: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let w = w.max(8);
    let mut out = vec![];
    let mut cur = String::new();
    for word in text.split(' ') {
        let mut word = word.to_string();
        while ui::width(&word) > w {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let mut used = 0;
            let cut = word.char_indices().find(|(_, c)| {
                used += c.width().unwrap_or(0);
                used > w
            });
            let at = cut.map_or(word.len(), |(i, _)| i);
            out.push(word[..at].to_string());
            word = word[at..].to_string();
        }
        if !cur.is_empty() && ui::width(&cur) + 1 + ui::width(&word) > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `left`, then `right` against the far edge of `w` columns (the left side gives way when it's tight).
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let rw: usize = right.iter().map(|s| ui::width(&s.content)).sum();
    let room = w.saturating_sub(rw + 1);
    let mut out = vec![];
    let mut used = 0;
    for s in left {
        let text = ui::fit(&s.content, room.saturating_sub(used));
        if text.is_empty() {
            break;
        }
        used += ui::width(&text);
        out.push(Span::styled(text, s.style));
    }
    out.push(Span::raw(" ".repeat(w.saturating_sub(used + rw))));
    out.extend(right);
    out
}

/// `spans` padded to `w` columns on the selection's background.
fn lit(mut spans: Vec<Span<'static>>, w: usize, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(|s| ui::width(&s.content)).sum();
    spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
    spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(t.frame))).collect()
}

/// The settings pane.
#[derive(Default)]
pub struct SettingsView {
    sel: usize,
    scroll: usize,
    /// the section `scroll` is for: another one starts at its top
    scrolled: Option<usize>,
    /// (screen row, selectable row) from the last render, for clicks
    hits: Vec<(u16, usize)>,
    /// (screen row, section) down the left, and the column it ends at, from the last render
    nav_hits: Vec<(u16, Section)>,
    nav_end: u16,
    /// The upstream Caveman plugin is installed in `~/.claude` (checked when the view opens)
    upstream_caveman: bool,
    /// JustTools has been looked for since the view opened
    tools_checked: bool,
    /// a JustTools job is running: keep its spinner and timer moving
    tools_busy: bool,
    /// bro is updating itself or looking for a release: keep the spinner moving
    update_busy: bool,
    /// a JustImagine update started here: (when, the commit it started from), until the checkout moves
    imagine_update: Option<(Instant, Option<String>)>,
    /// when the last look at JustImagine during that update was asked for
    imagine_looked: Option<Instant>,
}

impl SettingsView {
    pub fn new() -> SettingsView {
        SettingsView { upstream_caveman: bro_core::caveman::upstream_installed(&bro_core::paths::claude_local_dir()), ..SettingsView::default() }
    }

    fn row(&self) -> Row {
        rows()[self.sel.min(rows().len() - 1)]
    }

    /// Select the first row of the section `dir` away (wrapping round).
    fn jump(&mut self, dir: isize) {
        let at = SECTIONS.iter().position(|s| *s == Section::of(self.row())).unwrap_or(0) as isize;
        self.go(SECTIONS[(at + dir).rem_euclid(SECTIONS.len() as isize) as usize]);
    }

    fn go(&mut self, s: Section) {
        self.sel = rows().iter().position(|r| Section::of(*r) == s).unwrap_or(0);
    }

    /// Enter on a JustTools row: open Quick Install (JustTools isn't installed), update or check for a release;
    /// switch its skill or auto-update.
    fn tool(&mut self, o: ToolOpt, cx: &mut Cx) {
        let jt = cx.svc.state().justtools.clone();
        let mut s = cx.svc.settings();
        match o {
            ToolOpt::AutoUpdate => {
                s.justtools.auto_update = !s.justtools.auto_update;
                cx.svc.save_settings(s);
            }
            ToolOpt::QuickInstall => cx.act(Action::Run(Act::QuickInstall)),
            ToolOpt::Imagine => self.imagine(cx),
            ToolOpt::WatchSkill => {
                cx.svc.set_justwatch_skill_on(!s.justwatch.skill);
                if s.justwatch.skill {
                    cx.toast(Kind::Info, "JustWatch skill off · bro took its copies back out");
                }
            }
            _ if jt.job.is_some() => {}
            // a first install starts from the checklist, with JustTools ticked
            ToolOpt::Install if jt.installed.is_none() => cx.act(Action::Run(Act::QuickInstall)),
            ToolOpt::Install if cx.svc.is_demo() => cx.toast(Kind::Info, "the demo doesn't install anything"),
            ToolOpt::Install if jt.update().is_some() => cx.svc.install_justtools(),
            ToolOpt::Install => cx.svc.check_justtools(),
            ToolOpt::Skill if s.justtools.skill => {
                cx.svc.set_justtools_skill_on(false);
                cx.toast(Kind::Info, "JustTools skill off · bro took its copies back out");
            }
            ToolOpt::Skill if jt.installed.is_none() && !cx.svc.is_demo() => cx.toast(Kind::Info, "install JustTools first: its skill needs the commands"),
            ToolOpt::Skill => cx.svc.set_justtools_skill_on(true),
        }
    }

    /// Enter on JustImagine: open Quick Install (it isn't installed), update it in a tab, or look again.
    fn imagine(&mut self, cx: &mut Cx) {
        let ji = cx.svc.state().justimagine.clone();
        match &ji.installed {
            _ if ji.checking || self.imagine_update.is_some() => {}
            _ if cx.svc.is_demo() => cx.toast(Kind::Info, "the demo doesn't install anything"),
            None => cx.act(Action::Run(Act::QuickInstall)),
            Some(found) if ji.update().is_some() => {
                let cwd = std::env::current_dir().unwrap_or_default();
                cx.act(Action::Open(Box::new(super::quick_install::install_term(cx.svc, cwd, &["justimagine", "--force"])), Place::Tab));
                self.imagine_update = Some((Instant::now(), found.commit.clone()));
            }
            // running and up to date: open it, signed in (its address needs the token bro reads off disk)
            Some(found) if found.url.is_some() => {
                let tx = cx.tx.clone();
                std::thread::spawn(move || {
                    // the agent skills come along each time, as with `bro imagine`
                    let _ = bro_core::justimagine::install_skills();
                    let result = bro_core::justimagine::open_url().ok_or_else(|| "JustImagine stopped answering".to_string()).and_then(|u| crate::panes::links::open(&u));
                    if let Err(e) = result {
                        let _ = tx.send(crate::pane::Event::Toast(Kind::Error, format!("couldn't open JustImagine: {e}")));
                    }
                });
            }
            Some(_) => cx.svc.check_justimagine(false),
        }
    }

    /// Step the selected row's value by `dir` (+1 / -1).
    fn step(&mut self, dir: isize, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        let env = &mut s.claude_env;
        match self.row() {
            Row::Bro(o) => return o.change(dir, cx),
            Row::Tool(o) => return self.tool(o, cx),
            Row::Update(UpdOpt::Version) => {
                let u = cx.svc.state().updates.clone();
                match () {
                    _ if u.job.is_some() => {}
                    _ if u.available().is_some() => cx.svc.install_bro_update(),
                    _ => cx.svc.check_bro_update(false),
                }
                return;
            }
            Row::Update(UpdOpt::Check) => {
                s.updates.check = !s.updates.check;
                if s.updates.check {
                    cx.svc.check_bro_update(true);
                }
            }
            Row::Cave(CaveOpt::Mode) => {
                let all = cave_choices();
                let i = all.iter().position(|c| *c == s.caveman.active()).unwrap_or(0) as isize;
                match all[(i + dir).rem_euclid(all.len() as isize) as usize] {
                    Some(l) => {
                        s.caveman.level = l;
                        s.caveman.set_on(true);
                    }
                    None => s.caveman.set_on(false),
                }
            }
            Row::Cave(CaveOpt::Subagents) => s.caveman.subagents = !s.caveman.subagents,
            Row::Watch(WatchOpt::Prompt) => return cx.act(Action::Run(Act::WatchdogPrompt)),
            Row::Watch(WatchOpt::Wake) => s.watchdog.idle_secs = crate::watchdog::step_choice(&crate::watchdog::IDLE_CHOICES, s.watchdog.idle_secs, dir),
            Row::Watch(WatchOpt::Budget) => s.watchdog.max_wakes = crate::watchdog::step_choice(&crate::watchdog::BUDGET_CHOICES, s.watchdog.max_wakes, dir),
            Row::Watch(WatchOpt::Model) => {
                let all = crate::watchdog::MODEL_CHOICES;
                let i = all.iter().position(|m| *m == s.watchdog.model).map_or(0, |i| i as isize + dir);
                s.watchdog.model = all[i.rem_euclid(all.len() as isize) as usize].to_string();
            }
            Row::Watch(WatchOpt::Reviewer) => {
                let all = reviewer_choices(cx);
                let i = all.iter().position(|r| *r == s.watchdog.reviewer).unwrap_or(0) as isize;
                s.watchdog.reviewer = all[(i + dir).rem_euclid(all.len() as isize) as usize].clone();
            }
            Row::Preset => {
                let i = PRESETS.iter().position(|p| p.id == claude_env::preset(&env.preset).id).unwrap_or(0);
                env.use_preset(PRESETS[(i as isize + dir).rem_euclid(PRESETS.len() as isize) as usize].id);
            }
            Row::Knob(k) => {
                let k = &KNOBS[k];
                let i = env.choice(k).0;
                env.set_choice(k, (i as isize + dir).rem_euclid(k.options.len() as isize) as usize);
            }
        }
        cx.svc.save_settings(s);
    }

    fn reset(&self, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        match self.row() {
            Row::Bro(_) | Row::Cave(_) | Row::Tool(_) | Row::Update(_) => return,
            Row::Watch(o) => {
                let d = bro_core::config::WatchdogSettings::default();
                match o {
                    WatchOpt::Prompt => s.watchdog.prompt = None,
                    WatchOpt::Wake => s.watchdog.idle_secs = d.idle_secs,
                    WatchOpt::Reviewer => s.watchdog.reviewer = d.reviewer,
                    WatchOpt::Model => s.watchdog.model = d.model,
                    WatchOpt::Budget => s.watchdog.max_wakes = d.max_wakes,
                }
            }
            Row::Preset => s.claude_env.overrides.clear(),
            Row::Knob(k) => {
                s.claude_env.overrides.remove(KNOBS[k].id);
            }
        }
        cx.svc.save_settings(s);
    }
}

/// "auto", then every signed-in Claude and Codex login.
fn reviewer_choices(cx: &Cx) -> Vec<String> {
    let mut all = vec!["auto".to_string()];
    if let Some(ps) = cx.svc.state().profiles.ready() {
        all.extend(ps.iter().filter(|p| p.authenticated && (p.is_claude() || p.is_codex())).map(|p| p.id.clone()));
    }
    let current = cx.svc.settings().watchdog.reviewer;
    if !all.contains(&current) {
        all.push(current);
    }
    all
}

/// What the next direct launch sets: each `K=V`, then the `--settings` it passes.
fn env_parts(env: &ClaudeEnvSettings) -> Vec<String> {
    // never empty: `claude_env::ALWAYS` is on every launch
    let mut parts: Vec<String> = env.vars(false).iter().map(|(k, v)| format!("{k}={v}")).collect();
    let settings = env.settings();
    if !settings.is_empty() {
        parts.push(format!("--settings {}", serde_json::Value::Object(settings)));
    }
    parts
}

/// Section `s` as lines `w` columns wide: its title and blurb, then its blocks. Each line carries the selectable
/// row it belongs to (None: headings, text and gaps).
fn lay_out(s: Section, blocks: Vec<Block>, sel: usize, w: usize, t: &crate::theme::Theme) -> Vec<(Option<usize>, Vec<Span<'static>>)> {
    let all = rows();
    let idx = |r: Row| all.iter().position(|x| *x == r).unwrap_or(0);
    let mut lines = vec![(None, vec![Span::styled(format!("{}{}", ui::lead(s.icon()), s.label()), ui::bold_accent(t))])];
    lines.extend(wrap(s.blurb(), w).into_iter().map(|l| (None, vec![Span::styled(l, muted(t))])));
    lines.push((None, vec![Span::styled("─".repeat(w), fg(t.frame))]));
    for b in blocks {
        match b {
            Block::Item(it) => {
                lines.push((None, vec![]));
                let i = idx(it.row);
                let on = i == sel;
                // the selection is a bar down the setting's left edge, its name, help and notes all lit
                let bar = || Span::styled(if on { "▌ " } else { "  " }, fg(t.accent));
                let mut left = vec![bar(), Span::styled(it.label, if on { ui::bold() } else { Style::default() })];
                left.extend(it.tag);
                let mut right = it.value;
                right.push(Span::raw(" "));
                let mut block = vec![spread(left, right, w)];
                block.extend(wrap(&it.help, w.saturating_sub(3)).into_iter().map(|l| vec![bar(), Span::styled(l, muted(t))]));
                for n in it.notes {
                    match n.as_slice() {
                        // plain text wraps like the help; a progress bar stays on its line
                        [one] => block.extend(wrap(&one.content, w.saturating_sub(3)).into_iter().map(|l| vec![bar(), Span::styled(l, one.style)])),
                        _ => block.push(std::iter::once(bar()).chain(n).collect()),
                    }
                }
                lines.extend(block.into_iter().map(|l| (Some(i), if on { lit(l, w, t) } else { l })));
            }
            Block::Sub(text) => {
                lines.push((None, vec![]));
                let head = format!("  {} ", text.to_uppercase());
                let rest = w.saturating_sub(ui::width(&head));
                lines.push((None, vec![Span::styled(head, muted(t).add_modifier(Modifier::BOLD)), Span::styled("─".repeat(rest), fg(t.frame))]));
            }
            Block::Text(spans) => match spans.as_slice() {
                [one] => lines.extend(wrap(&one.content, w.saturating_sub(2)).into_iter().map(|l| (None, vec![Span::raw("  "), Span::styled(l, one.style)]))),
                _ => lines.push((None, std::iter::once(Span::raw("  ")).chain(spans).collect())),
            },
        }
    }
    lines
}

impl SettingsView {
    /// What section `s` is drawn from.
    fn blocks(&self, s: Section, cx: &Cx) -> Vec<Block> {
        let t = cx.theme;
        let st = cx.svc.settings();
        let bold = |c| fg(c).add_modifier(Modifier::BOLD);
        match s {
            Section::Appearance | Section::Sessions => bro_opts()
                .into_iter()
                .filter(|o| o.looks() == (s == Section::Appearance))
                .map(|o| {
                    let v = o.value(cx);
                    let value = match o {
                        BroOpt::Theme => shown(format!("{v} ›"), fg(t.inline)),
                        BroOpt::Font | BroOpt::Rain | BroOpt::Screensaver => {
                            let style = if v == "off" { muted(t) } else { fg(t.inline) };
                            choice(v, style, t)
                        }
                        _ => switch(v == "on", true, t),
                    };
                    // the font row says what the current pick is like
                    let help = match st.font.as_deref().and_then(crate::fonts::get) {
                        Some(f) if o == BroOpt::Font => format!("{} · ←/→ to try another", f.blurb),
                        _ => o.help().to_string(),
                    };
                    Block::Item(Item::new(Row::Bro(o), o.label(), value, help))
                })
                .collect(),
            Section::Caveman => {
                let cave = &st.caveman;
                let (value, help) = match cave.active() {
                    Some(l) => (choice(l.id(), bold(t.shine), t), l.help()),
                    None => (choice("off", muted(t), t), "agents write normally · rules from github.com/JuliusBrussee/caveman"),
                };
                let mut mode = Item::new(Row::Cave(CaveOpt::Mode), "Caveman mode", value, help);
                if self.upstream_caveman {
                    mode = mode.note(vec![Span::styled("the Caveman plugin is installed in ~/.claude too: bro keeps it quiet while this is on", muted(t))]);
                }
                let subagents = Item::new(Row::Cave(CaveOpt::Subagents), "Claude subagents too", switch(cave.subagents, cave.on, t), "their reports back to the main agent get shorter too");
                vec![Block::Item(mode), Block::Item(subagents)]
            }
            Section::Watchdog => {
                let w = &st.watchdog;
                WATCH_OPTS
                    .iter()
                    .map(|&o| {
                        let (label, value, help) = match o {
                            WatchOpt::Prompt => match &w.prompt {
                                Some(_) => ("Watchdog prompt", shown("customized ›", bold(t.shine)), "your instructions for every reviewer · enter edits · r goes back to bro's"),
                                None => ("Watchdog prompt", shown("bro's default ›", fg(t.inline)), "what every reviewer is told before its brief · enter opens the editor"),
                            },
                            WatchOpt::Wake => ("Wake after", choice(format!("{} idle", crate::watchdog::human(w.idle_secs)), fg(t.inline), t), "how long a session sits quiet after it stops before its watchdog wakes"),
                            WatchOpt::Reviewer if w.reviewer == "auto" => ("Reviewer", choice("auto", fg(t.inline), t), "the login with the most usage left, Claude first, Codex when Claude is out"),
                            WatchOpt::Reviewer => ("Reviewer", choice(w.reviewer.clone(), bold(t.shine), t), "always this login, whatever its usage"),
                            WatchOpt::Model => ("Reviewer model", choice(w.model.clone(), fg(t.inline), t), "the Claude model reviewers run on · a Codex reviewer uses its login's own"),
                            WatchOpt::Budget => ("Wake-up budget", choice(format!("{} wake-ups", w.max_wakes), fg(t.inline), t), "per watchdog, then it stands down and tells you"),
                        };
                        Block::Item(Item::new(Row::Watch(o), label, value, help))
                    })
                    .collect()
            }
            Section::Claude => {
                let env = &st.claude_env;
                let p = claude_env::preset(&env.preset);
                let mut preset = Item::new(Row::Preset, "Preset", choice(p.label, bold(t.shine), t), p.help);
                preset.tag = match env.overrides.len() {
                    0 => None,
                    n => Some(Span::styled(format!("  + {n} change{}", if n == 1 { "" } else { "s" }), fg(t.inline))),
                };
                let mut out = vec![Block::Item(preset)];
                let mut group = None;
                for (k_i, k) in KNOBS.iter().enumerate() {
                    if group != Some(k.group) {
                        group = Some(k.group);
                        out.push(Block::Sub(k.group.label().into()));
                    }
                    let (o, src) = env.choice(k);
                    let style = match src {
                        Source::Override => bold(t.shine),
                        Source::Preset => fg(t.inline),
                        Source::Default => muted(t),
                    };
                    let mut item = Item::new(Row::Knob(k_i), k.label, choice(k.options[o].label, style, t), k.help);
                    if matches!(src, Source::Override) {
                        item.tag = Some(Span::styled("  changed", fg(t.shine)));
                    }
                    out.push(Block::Item(item));
                }
                out.push(Block::Sub("next Claude launch sets".into()));
                out.push(Block::Text(vec![]));
                out.extend(env_parts(env).into_iter().map(|p| Block::Text(vec![Span::styled(p, fg(t.inline))])));
                out.push(Block::Text(vec![]));
                out.push(Block::Text(vec![Span::styled("more variables: [claude_env.extra] in ~/.bro/v2.toml", muted(t))]));
                out
            }
            Section::Integrations => self.integrations(cx),
            Section::Updates => {
                let u = cx.svc.state().updates.clone();
                let current = bro_core::self_update::CURRENT;
                let (v, style, help): (String, Style, String) = match (&u.job, &u.installed, u.available()) {
                    (Some(job), _, _) => (format!("{} updating", spinner(job.started)), bold(t.shine), String::new()),
                    (_, Some(new), _) => (format!("{new} installed"), fg(t.good), "restart bro to run it".into()),
                    _ if u.checking => ("checking…".into(), muted(t), String::new()),
                    (_, _, Some(_)) if bro_core::self_update::target().is_none() => (current.into(), fg(t.inline), "a new release is out; there's no prebuilt bro for this platform, so build it from source".into()),
                    (_, _, Some(new)) => (format!("{current} → {new}"), bold(t.shine), "a new release is out · enter installs it (checked against its SHA-256)".into()),
                    _ if u.offline => (current.into(), fg(t.inline), "couldn't reach GitHub to look for a newer release · enter tries again".into()),
                    _ if u.latest.is_some() => (current.into(), fg(t.inline), "the newest release · enter checks again".into()),
                    _ => (current.into(), fg(t.inline), "enter checks GitHub for a newer release now".into()),
                };
                let mut version = Item::new(Row::Update(UpdOpt::Version), "bro version", shown(v, style), help);
                if let Some(job) = &u.job {
                    let at = bro_core::self_update::Step::ALL.iter().position(|s| *s == job.step).unwrap_or(0) + 1;
                    let all = bro_core::self_update::Step::ALL.len();
                    version = version.note(progress(at, all, &format!("{} · {}s", job.step.label(), job.started.elapsed().as_secs()), t));
                } else if let Some(e) = &u.error {
                    version = version.note(vec![Span::styled(format!("✗ {e} · enter tries again"), fg(t.danger))]);
                }
                let check = Item::new(
                    Row::Update(UpdOpt::Check),
                    "Check for updates",
                    switch(st.updates.check, true, t),
                    "when bro opens and every 12 hours; a new release is announced once, nothing installs on its own",
                );
                vec![Block::Item(version), Block::Item(check)]
            }
        }
    }

    /// JustTools, Quick Install, the skills and JustImagine.
    fn integrations(&self, cx: &Cx) -> Vec<Block> {
        let t = cx.theme;
        let bold = |c| fg(c).add_modifier(Modifier::BOLD);
        let jt = cx.svc.state().justtools.clone();
        let st = cx.svc.settings();
        let jts = &st.justtools;
        let note = |text: String, style: Style| vec![Span::styled(text, style)];
        let installing = jt.job.as_ref().filter(|j| j.kind != JobKind::Skill);
        let version = jt.installed.as_ref().map(|f| f.version.clone().unwrap_or_else(|| "installed".into()));
        let (v, style, help): (String, Style, &str) = match (installing, &version, jt.update()) {
            (Some(job), _, _) => (format!("{} {}", spinner(job.started), if job.kind == JobKind::Update { "updating" } else { "installing" }), bold(t.shine), ""),
            _ if jt.checking || !jt.checked => ("checking…".into(), muted(t), ""),
            (_, None, _) => ("not installed".into(), muted(t), "enter opens Quick Install · image, video, PDF and port commands in one binary"),
            (_, Some(v), Some(new)) => (format!("{v} → {new}"), bold(t.shine), "a newer release is out · enter updates"),
            (_, Some(v), None) if jt.offline => (v.clone(), fg(t.inline), "couldn't reach GitHub to look for a newer release · enter tries again"),
            (_, Some(v), None) => (v.clone(), fg(t.inline), "the newest release · enter checks again"),
        };
        let mut tools = Item::new(Row::Tool(ToolOpt::Install), "JustTools", shown(v, style), help);
        if let Some(job) = installing {
            // one segment per step, then where it is and how long it has taken
            let at = Step::ALL.iter().position(|s| *s == job.step).unwrap_or(0) + 1;
            tools = tools.note(progress(at, Step::ALL.len(), &format!("{} · {}s", step_label(job.step), job.started.elapsed().as_secs()), t));
        } else if let Some(e) = &jt.error {
            let retry = if version.is_none() || jt.update().is_some() { " · enter tries again" } else { "" };
            tools = tools.note(note(format!("✗ {e}{retry}"), fg(t.danger)));
        } else if let Some(done) = &jt.done {
            tools = tools.note(note(format!("✓ {done}"), fg(t.good)));
        } else if let Some(found) = jt.installed.as_ref().filter(|f| !f.on_path()) {
            tools = tools.note(note(format!("in {} · restart bro so new sessions find it on PATH", found.bin.display()), muted(t)));
        } else if jt.checked && version.is_none() {
            tools = tools.note(note("github.com/JustGains/JustTools · the download is checked against its SHA-256 before it installs".into(), muted(t)));
        }
        let quick = Item::new(
            Row::Tool(ToolOpt::QuickInstall),
            "Quick install",
            shown("open ›", fg(t.inline)),
            "tick what to install: JustTools, JustWatch, JustImagine, Git, Node.js, Bun, Python, .NET 9 / 10 / 11, Claude Code, Codex, Pi, omp…",
        );
        let value = match &jt.job {
            Some(job) if job.kind == JobKind::Skill => shown(format!("{} adding", spinner(job.started)), bold(t.shine)),
            _ => switch(jts.skill, true, t),
        };
        let help = if jt.own_skill { "~/.claude has its own copy, which bro leaves alone" } else { "tells new Claude and Codex sessions when to use the just* commands" };
        let skill = Item::new(Row::Tool(ToolOpt::Skill), "JustTools skill", value, help);
        let auto = Item::new(Row::Tool(ToolOpt::AutoUpdate), "Auto-update JustTools", switch(jts.auto_update, true, t), "when bro opens, looks on GitHub for a newer release and installs it");
        let mut out = vec![Block::Item(tools), Block::Item(quick), Block::Item(skill), Block::Item(auto)];
        if bro_core::justwatch::supported() {
            out.push(Block::Item(Item::new(
                Row::Tool(ToolOpt::WatchSkill),
                "JustWatch skill",
                switch(st.justwatch.skill, true, t),
                "lets new Claude and Codex sessions check, restart and tail your dev servers through JustWatch",
            )));
        }

        let ji = cx.svc.state().justimagine.clone();
        let (v, style, help): (String, Style, String) = match (&ji.installed, ji.update()) {
            _ if self.imagine_update.is_some() => ("updating in a tab".into(), bold(t.shine), "pulling from GitHub, then npm install and a service restart".into()),
            _ if ji.checking || !ji.checked => ("checking…".into(), muted(t), String::new()),
            (None, _) => ("not installed".into(), muted(t), "enter opens Quick Install · image, video, song and voice studio that runs in the background".into()),
            (Some(f), Some(n)) => (format!("+{}", commits(n)), bold(t.shine), format!("{} is behind GitHub · enter pulls them and restarts the service", f.describe())),
            (Some(f), None) if f.dir.is_none() => (f.describe(), fg(t.inline), "runs from a copy bro doesn't manage, so bro leaves its updates to you".into()),
            (Some(f), None) if ji.error.is_some() => (f.describe(), fg(t.inline), format!("couldn't compare with GitHub: {} · enter tries again", ji.error.as_deref().unwrap_or_default())),
            (Some(f), None) if f.url.is_some() => (f.describe(), fg(t.inline), "up to date with GitHub · enter opens it in your browser, signed in".into()),
            (Some(f), None) => (f.describe(), fg(t.inline), "up to date with GitHub · enter checks again".into()),
        };
        let mut imagine = Item::new(Row::Tool(ToolOpt::Imagine), "JustImagine", shown(v, style), help);
        match ji.installed.as_ref() {
            Some(f) if f.url.is_some() => imagine = imagine.note(note(format!("open at {} · starts at login through {}", f.url.as_deref().unwrap_or_default(), bro_core::justimagine::mechanism()), muted(t))),
            Some(_) => imagine = imagine.note(note("its service isn't answering · `bro install justimagine --force` reinstalls and starts it".into(), fg(t.danger))),
            None => {}
        }
        out.push(Block::Item(imagine));
        out
    }

    /// What a section is at, beside its name down the left.
    fn summary(&self, s: Section, cx: &Cx) -> Span<'static> {
        let t = cx.theme;
        let st = cx.svc.settings();
        match s {
            Section::Appearance => Span::styled(t.name.clone(), muted(t)),
            Section::Sessions => {
                let opts: Vec<BroOpt> = bro_opts().into_iter().filter(|o| !o.looks()).collect();
                let on = opts.iter().filter(|o| o.value(cx) == "on").count();
                Span::styled(format!("{on} of {} on", opts.len()), muted(t))
            }
            Section::Caveman => match st.caveman.active() {
                Some(l) => Span::styled(l.id(), fg(t.shine)),
                None => Span::styled("off", muted(t)),
            },
            Section::Watchdog => Span::styled(format!("{} idle", crate::watchdog::human(st.watchdog.idle_secs)), muted(t)),
            Section::Claude => {
                let label = claude_env::preset(&st.claude_env.preset).label;
                match st.claude_env.overrides.len() {
                    0 => Span::styled(label, muted(t)),
                    n => Span::styled(format!("{label} +{n}"), fg(t.inline)),
                }
            }
            Section::Integrations => {
                let (jt, ji) = (cx.svc.state().justtools.clone(), cx.svc.state().justimagine.clone());
                match &jt.job {
                    Some(job) => Span::styled(spinner(job.started), fg(t.shine)),
                    None if self.imagine_update.is_some() => Span::styled("updating", fg(t.shine)),
                    None if jt.update().is_some() || ji.update().is_some() => Span::styled("update", fg(t.shine).add_modifier(Modifier::BOLD)),
                    None => Span::raw(""),
                }
            }
            Section::Updates => {
                let u = cx.svc.state().updates.clone();
                match (&u.job, u.available()) {
                    (Some(job), _) => Span::styled(spinner(job.started), fg(t.shine)),
                    (None, Some(new)) => Span::styled(format!("→ {new}"), fg(t.shine).add_modifier(Modifier::BOLD)),
                    (None, None) => Span::styled(bro_core::self_update::CURRENT, muted(t)),
                }
            }
        }
    }
}

/// An install's progress: one segment per step done, the rest thin, then "3 of 5 · what · how long".
fn progress(at: usize, of: usize, what: &str, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled("━━━━".repeat(at), fg(t.accent)),
        Span::styled("────".repeat(of.saturating_sub(at)), muted(t)),
        Span::styled(format!("  {at} of {of} · {what}"), muted(t)),
    ]
}

impl Pane for SettingsView {
    fn title(&self) -> String {
        "settings".into()
    }
    fn icon(&self) -> &'static str {
        "settings"
    }
    fn view(&self) -> Option<&'static str> {
        Some("settings")
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        if !self.tools_checked {
            self.tools_checked = true;
            cx.svc.check_justtools();
            cx.svc.check_justimagine(true);
        }
        // an update running in a tab: look again until the checkout moves
        if let Some((started, from)) = self.imagine_update.clone() {
            let moved = cx.svc.state().justimagine.installed.as_ref().is_some_and(|f| f.commit != from);
            if moved || started.elapsed() > IMAGINE_WATCH_FOR {
                self.imagine_update = None;
            } else if self.imagine_looked.is_none_or(|l| l.elapsed() >= IMAGINE_RECHECK) {
                self.imagine_looked = Some(Instant::now());
                cx.svc.refresh_justimagine();
            }
        }
        // spinners keep moving whichever section is open: the left side shows them too
        self.tools_busy = cx.svc.state().justtools.job.is_some();
        let u = cx.svc.state().updates.clone();
        self.update_busy = u.job.is_some() || u.checking;

        let row = self.row();
        let mut hints = vec![("j/k", "move"), ("tab", "next section"), ("enter/→", "change"), ("←", "back")];
        match row {
            Row::Preset | Row::Knob(_) => hints.push(("r", "reset to preset")),
            Row::Watch(_) => hints.push(("r", "bro's default")),
            _ => {}
        }
        let area = ui::hint_line(f, area, &hints, t);
        let section = Section::of(row);

        // the sections down the left, when there's room for them beside the settings
        let nav_w: u16 = if area.width >= 76 { 28 } else { 0 };
        self.nav_hits.clear();
        self.nav_end = area.x + nav_w;
        if nav_w > 0 {
            let nav = Rect { width: nav_w - 2, ..area };
            let w = nav.width as usize;
            ui::line(f, Rect { height: 1, ..nav }, vec![Span::styled(" SETTINGS", ui::bold_accent(t))]);
            for (n, s) in SECTIONS.into_iter().enumerate() {
                let y = nav.y + 2 + n as u16;
                if y >= nav.bottom() {
                    break;
                }
                let on = s == section;
                let left = vec![
                    Span::styled(if on { "▌" } else { " " }, fg(t.accent)),
                    Span::styled(format!(" {}{}", ui::lead(s.icon()), s.label()), if on { ui::bold() } else { Style::default() }),
                ];
                let spans = spread(left, vec![self.summary(s, cx), Span::raw(" ")], w);
                ui::line(f, Rect { y, height: 1, ..nav }, if on { lit(spans, w, t) } else { spans });
                self.nav_hits.push((y, s));
            }
            if nav.height as usize > SECTIONS.len() + 4 {
                ui::line(f, Rect { y: nav.bottom() - 1, height: 1, ..nav }, vec![Span::styled(" saved to ~/.bro/v2.toml", muted(t))]);
            }
            for y in area.y..area.bottom() {
                ui::line(f, Rect { x: area.x + nav_w - 1, y, width: 1, height: 1 }, vec![Span::styled("│", fg(t.frame))]);
            }
        }

        let body = Rect { x: area.x + nav_w + 1, width: area.width.saturating_sub(nav_w + 2), ..area };
        let w = (body.width as usize).min(80);
        let mut lines = lay_out(section, self.blocks(section, cx), self.sel, w, t);
        let at = SECTIONS.iter().position(|s| *s == section);
        if nav_w == 0 {
            // no list down the left: the title says where this section sits
            lines[0].1.push(Span::styled(format!("  {} of {}", at.unwrap_or(0) + 1, SECTIONS.len()), muted(t)));
        }

        // Keep the selected setting on screen; another section starts at its top.
        let room = body.height as usize;
        if self.scrolled != at {
            self.scrolled = at;
            self.scroll = 0;
        }
        let first = lines.iter().position(|(s, _)| *s == Some(self.sel)).unwrap_or(0);
        let last = lines.iter().rposition(|(s, _)| *s == Some(self.sel)).unwrap_or(first);
        let all = rows();
        if all.iter().position(|r| Section::of(*r) == section) == Some(self.sel) {
            self.scroll = 0;
        } else if all.iter().rposition(|r| Section::of(*r) == section) == Some(self.sel) {
            // the last setting brings what's under it into view (the Claude launch preview)
            self.scroll = lines.len().saturating_sub(room);
        } else if first <= self.scroll {
            self.scroll = first.saturating_sub(1);
        } else if last >= self.scroll + room {
            self.scroll = last + 1 - room;
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        let (above, below) = (self.scroll > 0, lines.len() > self.scroll + room);
        self.hits.clear();
        for (n, (sel, spans)) in lines.into_iter().skip(self.scroll).take(room).enumerate() {
            let r = Rect { y: body.y + n as u16, height: 1, width: w as u16, ..body };
            if let Some(s) = sel {
                self.hits.push((r.y, s));
            }
            ui::line(f, r, spans);
        }
        // more above / below, at the right edge
        let edge = Rect { x: body.x + w as u16, y: body.y, width: 1, height: 1 };
        if above && edge.x < area.right() {
            ui::line(f, edge, vec![Span::styled("▲", muted(t))]);
        }
        if below && edge.x < area.right() && body.height > 0 {
            ui::line(f, Rect { y: body.bottom() - 1, ..edge }, vec![Span::styled("▼", muted(t))]);
        }
    }
    fn tick_every(&self) -> Option<Duration> {
        if self.tools_busy || self.update_busy {
            return Some(Duration::from_millis(100));
        }
        self.imagine_update.as_ref().map(|_| IMAGINE_RECHECK)
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        let last = rows().len() - 1;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sel = (self.sel + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.sel = last,
            // shift+tab arrives as Tab+SHIFT on some paths (Windows console input)
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.jump(-1),
            KeyCode::Tab => self.jump(1),
            KeyCode::BackTab => self.jump(-1),
            KeyCode::Enter | KeyCode::Char(' ' | 'l') | KeyCode::Right => self.step(1, cx),
            KeyCode::Char('h') | KeyCode::Left => self.step(-1, cx),
            KeyCode::Char('r') => self.reset(cx),
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        let last = rows().len() - 1;
        match ev.kind {
            MouseEventKind::ScrollDown => self.sel = (self.sel + 1).min(last),
            MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) if ev.column < self.nav_end => {
                if let Some(&(_, s)) = self.nav_hits.iter().find(|(y, _)| *y == ev.row) {
                    self.go(s);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, s)) = self.hits.iter().find(|(y, _)| *y == ev.row) {
                    if s == self.sel {
                        self.step(1, cx);
                    }
                    self.sel = s;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_lists_vars_and_settings() {
        let mut env = ClaudeEnvSettings::default();
        assert_eq!(env_parts(&env), ["CLAUDE_CODE_NO_FLICKER=1", r#"--settings {"viewMode":"focus","spinnerTipsEnabled":false}"#]);
        env.use_preset("presenter");
        let p = env_parts(&env);
        assert!(p.iter().any(|v| v == "IS_DEMO=1") && p.iter().any(|v| v == "CLAUDE_CODE_HIDE_CWD=1"), "{p:?}");
    }

    #[test]
    fn rows_run_in_section_order() {
        let r = rows();
        assert_eq!(r[0], Row::Bro(BroOpt::Theme));
        let n = bro_opts().len();
        assert_eq!(r[n], Row::Cave(CaveOpt::Mode));
        assert_eq!(r[n + 2], Row::Watch(WatchOpt::Prompt));
        assert_eq!(r[n + 2 + WATCH_OPTS.len()], Row::Preset);
        let tools = n + 3 + WATCH_OPTS.len() + KNOBS.len();
        assert_eq!(r[tools], Row::Tool(ToolOpt::Install));
        assert_eq!(r[tools + 1], Row::Tool(ToolOpt::QuickInstall));
        let watch = usize::from(bro_core::justwatch::supported());
        assert_eq!(r[tools + 4 + watch], Row::Tool(ToolOpt::Imagine));
        assert_eq!(r[tools + 5 + watch], Row::Update(UpdOpt::Version));
        assert_eq!(r.len(), tools + 7 + watch);
        // each section's rows sit together, in the order the sections are listed
        let order: Vec<usize> = r.iter().map(|row| SECTIONS.iter().position(|s| *s == Section::of(*row)).unwrap()).collect();
        assert!(order.is_sorted(), "{order:?}");
        assert!((0..SECTIONS.len()).all(|i| order.contains(&i)), "every section has a row");
    }

    #[test]
    fn wrap_breaks_at_spaces_and_cuts_long_words() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(wrap("abcdefghijkl x", 8), ["abcdefgh", "ijkl x"]);
        assert!(wrap("", 20).is_empty());
    }

    #[test]
    fn caveman_choices_start_off() {
        assert_eq!(cave_choices(), vec![None, Some(Level::Caveman), Some(Level::Ultracave), Some(Level::Megacave)]);
    }
}
