//! Settings view: bro's own everyday options (theme, icons, auto-resume, auto-approve, recaps, compression), push-to-talk
//! voice (its key, set by pressing it), watchdog
//! defaults and the reviewer's prompt, caveman
//! mode (terse replies in new Claude / Codex / Pi sessions), integrations (JustTools, Quick Install, Smart Paste and JustImagine), skills (bro's
//! own, JustTools', JustWatch's and JustImagine's, and the git repos bro watches for more), bro's own updates and the Claude Code environment bro sets on every Claude launch — agent teams (on, the
//! teammates' model, the teams running now), then a preset (Standard, Quiet, Private, Lockdown,
//! Incognito, Presenter, Marathon) plus per-switch overrides. Saved to `~/.bro/v2.toml`; the Claude Code
//! part applies to sessions launched from now on.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::keymap::Act;
use crate::alerts::Kind;
use crate::pane::{Action, Cx, Pane, Place};
use crate::services::justimagine::commits;
use crate::services::justtools::JobKind;
use crate::services::skills::Bundled;
use crate::ui::{self, fg, muted};
use bro_core::caveman::Level;
use bro_core::claude_env::{self, ClaudeEnvSettings, Group, KNOBS, PRESETS, Source};
use bro_core::config::{MATERIAL_TINTS, Material, RainMode};
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
    /// the desktop app's window: solid, acrylic, glass
    Material,
    /// how much of the theme covers a see-through window
    Tint,
    Rain,
    Screensaver,
    SharedBorders,
    ProjectTabs,
    AutoResume,
    AutoApprove,
    Recaps,
    /// sessions share every signed-in login of their agent
    ProxyMode,
    Compression,
}

const BRO_OPTS: [BroOpt; 13] = [
    BroOpt::Theme,
    BroOpt::Font,
    BroOpt::Material,
    BroOpt::Tint,
    BroOpt::Rain,
    BroOpt::Screensaver,
    BroOpt::SharedBorders,
    BroOpt::ProjectTabs,
    BroOpt::AutoResume,
    BroOpt::AutoApprove,
    BroOpt::Recaps,
    BroOpt::ProxyMode,
    BroOpt::Compression,
];

/// bro's options this window can use: the font only where the terminal can switch it, the window's material only
/// in the desktop app (a terminal draws its own window), its tint only once something shows through.
fn bro_opts() -> Vec<BroOpt> {
    BRO_OPTS
        .into_iter()
        .filter(|o| match o {
            BroOpt::Font => crate::fonts::available(),
            BroOpt::Material => desktop_app(),
            BroOpt::Tint => desktop_app() && SEE_THROUGH.with(|s| s.get()),
            _ => true,
        })
        .collect()
}

/// Drawn by the desktop app, which has a window of its own to dress.
fn desktop_app() -> bool {
    #[cfg(test)]
    return crate::fonts::TEST_APP_WINDOW.with(|w| w.get());
    #[cfg(not(test))]
    return crate::fonts::GUI.load(Ordering::Relaxed);
}

const RAIN: [RainMode; 3] = [RainMode::Off, RainMode::Everywhere, RainMode::Working];

/// The summary screensaver's waits, in seconds (0 = off).
const SCREENSAVER: [u64; 7] = [0, 60, 120, 300, 600, 900, 1800];

impl BroOpt {
    fn label(self) -> &'static str {
        match self {
            BroOpt::Theme => "Theme",
            BroOpt::Font => "Font",
            BroOpt::Material => "Window",
            BroOpt::Tint => "Window tint",
            BroOpt::Rain => "Digital rain",
            BroOpt::Screensaver => "Summary screensaver",
            BroOpt::SharedBorders => "Shared tile borders",
            BroOpt::ProjectTabs => "Project tabs",
            BroOpt::AutoResume => "Auto-resume",
            BroOpt::AutoApprove => "Auto-approve prompts",
            BroOpt::Recaps => "Recaps",
            BroOpt::ProxyMode => "Proxy mode",
            BroOpt::Compression => "Request compression",
        }
    }
    fn help(self) -> &'static str {
        match self {
            BroOpt::Theme => "enter opens the picker with a live preview",
            BroOpt::Font => "this window only, switches as you go: ←/→ to try them",
            BroOpt::Material => "let your desktop show through: acrylic frosts it, glass leaves it clear",
            BroOpt::Tint => "how much theme covers a see-through window, evenly from title bar to sessions · text stays solid",
            BroOpt::Rain => "over any theme: everywhere, or only in sessions whose agent is mid-turn",
            BroOpt::Screensaver => "step away and every session turns over to what it's working on; any touch turns them back",
            BroOpt::SharedBorders => "tiles that sit together share one line instead of two; status and × move up beside the title",
            BroOpt::ProjectTabs => "a tab strip over your sessions: All tiles every one, a project's tab tiles just its own, in its color",
            BroOpt::AutoResume => "types `continue` when an agent stops on an API error or a limit resets",
            BroOpt::AutoApprove => "answers Yes to Claude's \"Do you want to proceed?\" (bypass mode's rm / path safety checks)",
            BroOpt::Recaps => "asks Claude for a /recap of a pane left waiting unseen (a few cents each)",
            BroOpt::ProxyMode => "one pool per agent: every signed-in login serves new sessions, and a spent one hands off to the next",
            BroOpt::Compression => "routes new Claude sessions through bro-proxy to trim requests",
        }
    }
    /// How bro looks (Appearance) rather than what it does (Sessions).
    fn looks(self) -> bool {
        matches!(self, BroOpt::Theme | BroOpt::Font | BroOpt::Material | BroOpt::Tint | BroOpt::Rain | BroOpt::Screensaver | BroOpt::SharedBorders | BroOpt::ProjectTabs)
    }
    /// The value as shown.
    fn value(self, cx: &Cx) -> String {
        let s = cx.svc.settings();
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        match self {
            BroOpt::Theme => cx.theme.name.clone(),
            BroOpt::Font => s.font.as_deref().and_then(crate::fonts::get).map_or(if crate::fonts::GUI.load(std::sync::atomic::Ordering::Relaxed) { crate::fonts::FONTS[0].label } else { "terminal's own" }, |f| f.label).to_string(),
            BroOpt::Material => s.material.id().to_string(),
            BroOpt::Tint => format!("{}%", s.material_tint),
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
            BroOpt::ProjectTabs => on(s.project_tabs),
            BroOpt::AutoResume => on(s.auto_resume.enabled),
            BroOpt::AutoApprove => on(s.auto_approve),
            BroOpt::Recaps => on(s.insights.generate_recaps),
            BroOpt::ProxyMode => on(s.proxy.shared),
            BroOpt::Compression => on(s.proxy.trim),
        }
    }
    /// Change it: on / off flips either way, a choice steps by `dir`, the theme opens its picker.
    fn change(self, dir: isize, cx: &mut Cx) {
        match self {
            BroOpt::Font => {
                // the terminal's own font, then each bundled one (the desktop app has no font of its own: its
                // default is the first bundled one)
                let mut s = cx.svc.settings();
                let gui = crate::fonts::GUI.load(std::sync::atomic::Ordering::Relaxed);
                let own = (!gui).then_some(None);
                let ids: Vec<Option<&str>> = own.into_iter().chain(crate::fonts::FONTS.iter().map(|f| Some(f.id))).collect();
                let current = if gui { s.font.as_deref().or(Some(crate::fonts::FONTS[0].id)) } else { s.font.as_deref() };
                let i = ids.iter().position(|id| *id == current).unwrap_or(0) as isize;
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
            BroOpt::Material => {
                let mut s = cx.svc.settings();
                let i = Material::ALL.iter().position(|m| *m == s.material).unwrap_or(0) as isize;
                s.material = Material::ALL[(i + dir).rem_euclid(Material::ALL.len() as isize) as usize];
                cx.svc.save_settings(s);
            }
            BroOpt::Tint => {
                // steps stop at the ends (a tint isn't a loop)
                let mut s = cx.svc.settings();
                let i = MATERIAL_TINTS.iter().position(|t| *t >= s.material_tint).unwrap_or(MATERIAL_TINTS.len() - 1) as isize;
                s.material_tint = MATERIAL_TINTS[(i + dir).clamp(0, MATERIAL_TINTS.len() as isize - 1) as usize];
                cx.svc.save_settings(s);
            }
            BroOpt::Screensaver => {
                let mut s = cx.svc.settings();
                let i = SCREENSAVER.iter().position(|n| *n >= s.summary_screensaver_secs).unwrap_or(0) as isize;
                s.summary_screensaver_secs = SCREENSAVER[(i + dir).rem_euclid(SCREENSAVER.len() as isize) as usize];
                cx.svc.save_settings(s);
            }
            BroOpt::Theme => cx.act(Action::Run(Act::Themes)),
            BroOpt::AutoResume => cx.act(Action::Run(Act::ToggleAutoResume)),
            BroOpt::Compression => cx.act(Action::Run(Act::ToggleTrim)),
            BroOpt::ProxyMode => {
                let on = !cx.svc.settings().proxy.shared;
                cx.svc.set_shared(on);
            }
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
            BroOpt::ProjectTabs => {
                let mut s = cx.svc.settings();
                s.project_tabs = !s.project_tabs;
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
/// Push-to-talk's rows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum VoiceOpt {
    On,
    /// enter, then press the key (where the platform lets bro listen for it)
    Key,
    /// GPT-Live answering out loud, or speech to text
    Mode,
    /// GPT-Live's voice
    Speaker,
    /// whether the OpenAI key is there
    ApiKey,
}

const VOICE_OPTS: [VoiceOpt; 5] = [VoiceOpt::On, VoiceOpt::Key, VoiceOpt::Mode, VoiceOpt::Speaker, VoiceOpt::ApiKey];

/// GPT-Live's voices.
const LIVE_VOICES: [&str; 10] = ["cedar", "marin", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"];

/// Keys ←/→ step through where pressing one to set it doesn't work.
fn key_choices() -> Vec<&'static str> {
    let mut all = vec![bro_voice::default_hotkey()];
    all.extend(
        ["ralt", "rctrl", "rshift", "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20", "f21", "f22", "f23", "f24", "scrolllock", "pause"]
            .into_iter()
            .filter(|k| *k != bro_voice::default_hotkey()),
    );
    all
}

/// The voice key a key that reached bro's window names: function keys (F13–F24 by name) and Insert; keys you
/// type or move with can't be one.
fn window_hotkey(code: KeyCode) -> Option<String> {
    match code {
        KeyCode::F(n @ 13..=24) => Some(format!("f{n}")),
        KeyCode::F(n @ 1..=12) => Some(format!("vk:0x{:02X}", 0x6F + u32::from(n))),
        KeyCode::Insert => Some("insert".into()),
        _ => None,
    }
}

/// The push-to-talk key's name, the platform's default when none is set.
fn voice_key(v: &bro_core::config::VoiceSettings) -> String {
    v.hotkey.clone().unwrap_or_else(|| bro_voice::default_hotkey().to_string())
}

fn live_mode(v: &bro_core::config::VoiceSettings) -> bool {
    v.mode != "transcribe"
}

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
    /// JustPaste's Smart Paste for File Explorer (Windows only; comes with JustTools)
    SmartPaste,
    /// JustImagine: its version, and an update when GitHub has newer commits
    Imagine,
    /// Sounds on / off: bro's hooks in new sessions and peon-ping's own `enabled`
    SoundsOn,
    /// opens Sounds (PeonPing)
    Sounds,
    /// PeonPing's site, where the sounds come from
    PeonPing,
}

/// The Skills section's own rows. The JustTools and JustWatch skills ([`ToolOpt::Skill`], [`ToolOpt::WatchSkill`])
/// are listed there too. The skills themselves, the watched repos and Skill Issue are the Skills view's.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SkillOpt {
    /// enter opens the Skills view
    Manage,
    /// bro's own `bro-open`
    BroOpen,
    /// JustImagine's generate-images, -video and -voice
    Imagine,
    /// pull the repos when bro opens and every 6 hours
    AutoSync,
}

thread_local! {
    /// Whether the window was see-through as the view last read the settings: the tint row [`rows`] lists.
    static SEE_THROUGH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Note what of `s` decides which rows there are (a see-through window), for [`rows`].
fn note_settings(s: &bro_core::config::Settings) {
    SEE_THROUGH.with(|w| w.set(s.material.see_through()));
}

/// "just now", "4 min ago", "2 h ago".
pub(crate) fn ago(at: Instant) -> String {
    match at.elapsed().as_secs() {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
}

/// Where the sound packs come from.
const PEONPING_URL: &str = "https://peonping.com";

/// How often to look at JustImagine while an update started here runs in its tab.
const IMAGINE_RECHECK: Duration = Duration::from_secs(3);
/// Stop looking after this long.
const IMAGINE_WATCH_FOR: Duration = Duration::from_secs(10 * 60);

/// The session server's rows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum HostOpt {
    /// new sessions run in the server and keep running when bro closes
    Keep,
    /// a background bro carries on with them (bridge, proxy, auto-resume, watchdogs) while bro is closed
    Working,
    /// how it is; enter restarts it
    Restart,
    /// it starts when you sign in (for your user, no admin)
    Startup,
}

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

/// bro's Claude Code mod (`[claude_mod]`, see [`bro_core::claude_mod`]): a section of its own.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ModOpt {
    On,
    Chime,
    Timer,
    TimerSecs,
    Ticks,
}

const MOD_OPTS: [ModOpt; 5] = [
    ModOpt::On,
    ModOpt::Chime,
    ModOpt::Timer,
    ModOpt::TimerSecs,
    ModOpt::Ticks,
];

impl ModOpt {
    fn label(self) -> &'static str {
        match self {
            ModOpt::On => "Load the mod",
            ModOpt::Chime => "Question chime",
            ModOpt::Timer => "Question timer",
            ModOpt::TimerSecs => "Answer within",
            ModOpt::Ticks => "Countdown ticks",
        }
    }
    fn help(self) -> &'static str {
        match self {
            ModOpt::On => "new Claude sessions load bro's mod: /bro opens your sessions and limits in a pane · the questions need it",
            ModOpt::Chime => "bro chimes when Claude asks you a question",
            ModOpt::Timer => "a countdown above Claude's questions; at zero Claude goes with its best judgement",
            ModOpt::TimerSecs => "how long a question waits for you while the timer is on",
            ModOpt::Ticks => "a tick a second for the timer's last 15 seconds, firmer for the last five",
        }
    }
    fn get(self, m: &bro_core::claude_mod::ClaudeModSettings) -> bool {
        match self {
            ModOpt::On => m.on,
            ModOpt::Chime => m.chime,
            ModOpt::Timer | ModOpt::TimerSecs => m.timer,
            ModOpt::Ticks => m.ticks,
        }
    }
    fn flip(self, m: &mut bro_core::claude_mod::ClaudeModSettings) {
        let v = match self {
            ModOpt::On => &mut m.on,
            ModOpt::Chime => &mut m.chime,
            ModOpt::Timer => &mut m.timer,
            ModOpt::Ticks => &mut m.ticks,
            // a length, not a switch: stepped in `step`
            ModOpt::TimerSecs => return,
        };
        *v = !*v;
    }
    /// The heading it leads, inside the section.
    fn heading(self) -> Option<&'static str> {
        match self {
            ModOpt::Chime => Some("questions"),
            _ => None,
        }
    }
    /// Takes effect: the questions only while the mod loads, the timer's own only with the timer on.
    fn live(self, m: &bro_core::claude_mod::ClaudeModSettings) -> bool {
        match self {
            ModOpt::On => true,
            ModOpt::TimerSecs | ModOpt::Ticks => m.on && m.timer,
            _ => m.on,
        }
    }
}

/// A selectable row, in the order the sections run: bro's own options (Appearance, then Sessions), caveman mode,
/// watchdogs, agent teams, the Claude preset and each other [`KNOBS`] switch, bro's Claude mod, the integrations,
/// the skills, the sounds, then bro's updates.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Bro(BroOpt),
    Voice(VoiceOpt),
    Cave(CaveOpt),
    Tool(ToolOpt),
    Skill(SkillOpt),
    Update(UpdOpt),
    Host(HostOpt),
    Watch(WatchOpt),
    Mod(ModOpt),
    Preset,
    Knob(usize),
}

fn rows() -> Vec<Row> {
    bro_opts()
        .into_iter()
        .map(Row::Bro)
        .chain([Row::Host(HostOpt::Keep), Row::Host(HostOpt::Working), Row::Host(HostOpt::Restart), Row::Host(HostOpt::Startup)])
        .chain(VOICE_OPTS.map(Row::Voice))
        .chain([Row::Cave(CaveOpt::Mode), Row::Cave(CaveOpt::Subagents)])
        .chain(WATCH_OPTS.map(Row::Watch))
        .chain(knobs(true).map(Row::Knob))
        .chain([Row::Preset])
        .chain(knobs(false).map(Row::Knob))
        .chain(MOD_OPTS.map(Row::Mod))
        .chain([Row::Tool(ToolOpt::Install), Row::Tool(ToolOpt::QuickInstall), Row::Tool(ToolOpt::AutoUpdate)])
        .chain(bro_core::justpaste::supported().then_some(Row::Tool(ToolOpt::SmartPaste)))
        .chain([Row::Tool(ToolOpt::Imagine), Row::Skill(SkillOpt::Manage), Row::Skill(SkillOpt::BroOpen), Row::Tool(ToolOpt::Skill)])
        .chain(bro_core::justwatch::supported().then_some(Row::Tool(ToolOpt::WatchSkill)))
        .chain([Row::Skill(SkillOpt::Imagine), Row::Skill(SkillOpt::AutoSync)])
        .chain([Row::Tool(ToolOpt::SoundsOn), Row::Tool(ToolOpt::Sounds), Row::Tool(ToolOpt::PeonPing), Row::Update(UpdOpt::Version), Row::Update(UpdOpt::Check)])
        .collect()
}

/// The [`KNOBS`] for agent teams (`teams`), drawn in a group of their own above the preset, or all the others.
fn knobs(teams: bool) -> impl Iterator<Item = usize> {
    (0..KNOBS.len()).filter(move |&k| (KNOBS[k].group == Group::Teams) == teams)
}

/// An agent teams switch: reset goes back to bro's default, not to a preset (no preset touches them).
fn team_knob(r: Row) -> bool {
    matches!(r, Row::Knob(k) if KNOBS[k].group == Group::Teams)
}

/// The agent teams running in sessions here, under the switch: one line each (lead, project, teammates, tasks), or
/// how to start one.
fn team_notes(on: bool, teams: &[crate::services::LiveTeam], t: &crate::theme::Theme) -> Vec<Vec<Span<'static>>> {
    if !on {
        return vec![vec![Span::styled("off · sessions you start now can't form a team; ones already running keep theirs", muted(t))]];
    }
    if teams.is_empty() {
        return vec![vec![Span::styled(format!("none running · in a new Claude session, ask: “{}”", crate::team_board::EXAMPLE), muted(t))]];
    }
    let mut out = vec![vec![Span::styled(format!("{} running · prefix M on a session opens its team board", teams.len()), muted(t))]];
    for l in teams.iter().take(5) {
        let c = l.team.counts();
        let mates = l.team.teammates().count();
        let mut line = vec![
            Span::styled(ui::lead("team"), fg(t.accent)),
            Span::styled(ui::fit(&l.lead, 28), ui::bold()),
            Span::styled(format!("  {}", l.project), muted(t)),
            Span::styled(format!("  ·  {mates} teammate{}", if mates == 1 { "" } else { "s" }), fg(t.fg)),
        ];
        if l.team.working() > 0 {
            line.push(Span::styled(format!(", {} working", l.team.working()), fg(t.accent)));
        }
        if c.total > 0 {
            line.push(Span::styled(format!("  ·  {} of {} tasks done", c.done, c.total), fg(if c.done == c.total { t.good } else { t.fg })));
        }
        out.push(line);
    }
    out
}

/// The sections down the left; every row belongs to one.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Section {
    Appearance,
    Sessions,
    Server,
    Voice,
    Caveman,
    Watchdog,
    Claude,
    Mod,
    Integrations,
    Skills,
    Sounds,
    Updates,
}

const SECTIONS: [Section; 12] = [
    Section::Appearance,
    Section::Sessions,
    Section::Server,
    Section::Voice,
    Section::Caveman,
    Section::Watchdog,
    Section::Claude,
    Section::Mod,
    Section::Integrations,
    Section::Skills,
    Section::Sounds,
    Section::Updates,
];

impl Section {
    fn of(r: Row) -> Section {
        match r {
            Row::Bro(o) if o.looks() => Section::Appearance,
            Row::Bro(_) => Section::Sessions,
            Row::Host(_) => Section::Server,
            Row::Voice(_) => Section::Voice,
            Row::Cave(_) => Section::Caveman,
            Row::Watch(_) => Section::Watchdog,
            Row::Preset | Row::Knob(_) => Section::Claude,
            Row::Mod(_) => Section::Mod,
            Row::Tool(ToolOpt::SoundsOn | ToolOpt::Sounds | ToolOpt::PeonPing) => Section::Sounds,
            Row::Tool(ToolOpt::Skill | ToolOpt::WatchSkill) | Row::Skill(_) => Section::Skills,
            Row::Tool(_) => Section::Integrations,
            Row::Update(_) => Section::Updates,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Sessions => "Sessions",
            Section::Server => "Session server",
            Section::Voice => "Voice",
            Section::Caveman => "Caveman",
            Section::Watchdog => "Watchdog",
            Section::Claude => "Claude Code",
            Section::Mod => "bro mod",
            Section::Integrations => "Integrations",
            Section::Skills => "Skills",
            Section::Sounds => "Sounds",
            Section::Updates => "Updates",
        }
    }
    /// Its [`ui::icon`] name.
    fn icon(self) -> &'static str {
        match self {
            Section::Appearance => "theme",
            Section::Sessions => "term",
            Section::Server => "server",
            Section::Voice => "mic",
            Section::Caveman => "caveman",
            Section::Watchdog => "watchdog",
            Section::Claude => "claude",
            Section::Mod => "mod",
            Section::Integrations => "rocket",
            Section::Skills => "skills",
            Section::Sounds => "sound",
            Section::Updates => "history",
        }
    }
    /// The line under its title.
    fn blurb(self) -> &'static str {
        match self {
            Section::Appearance => "How bro looks. Every change shows up as you make it.",
            Section::Sessions => "What bro does on its own while your agents work.",
            Section::Server => "Your sessions run in a background process of bro's, so they keep running when bro closes, crashes or updates, and come back when it opens. The terminal UI and the desktop app both pick them up.",
            Section::Voice => "Hold the key in any app, speak, let go: Hugh hears you and gets it done. bro keeps that key to itself while it runs.",
            Section::Caveman => "Terse replies with every fact kept, in new Claude, Codex and Pi sessions.",
            Section::Watchdog => "A fresh reviewer reads a session that went quiet and hands it its next task. Prefix w on a session starts one.",
            Section::Claude => "Agent teams, and the environment bro sets on every Claude launch. New sessions pick it up.",
            Section::Mod => "bro's mod for Claude Code: your sessions and limits a keystroke away in /bro, and a chime and a clock on Claude's questions. New sessions pick it up.",
            Section::Integrations => "Tools your agents can call. bro installs them and tells you when GitHub has something newer.",
            Section::Skills => "What your agents know how to do. The Skills view lists every skill they have and where each came from, watches repos for more and finds more on Skill Issue; here, the switches for the skills bro brings.",
            Section::Sounds => "Voice lines when a session starts, finishes, fails or needs you, in Claude, Codex, Pi and omp. The sounds and packs are provided by PeonPing.",
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

/// "Start at sign-in": a switch, and what's registered under it.
fn startup_item(hs: &crate::services::host::HostState, t: &crate::theme::Theme) -> Item {
    use bro_host::service::Startup;
    let row = Row::Host(HostOpt::Startup);
    let label = "Start at sign-in";
    let off_outside = if cfg!(windows) { "turned off in Task Manager › Startup apps" } else { "turned off outside bro (systemctl --user disable)" };
    let (value, help) = match (&hs.startup_busy, &hs.startup) {
        (Some(started), _) => (shown(format!("{} switching", spinner(*started)), fg(t.shine).add_modifier(Modifier::BOLD)), String::new()),
        (None, Some(Startup::On { .. })) => (switch(true, true, t), "bro starts in the background when you sign in and stays running, ready for you and your phone · for your user only, no admin".into()),
        (None, Some(Startup::Disabled { .. })) => (switch(false, true, t), format!("{off_outside} · enter turns it back on")),
        (None, _) => (
            switch(false, true, t),
            "start bro in the background when you sign in, and keep it running · for your user only, no admin · off: the server starts when bro opens, and stops 10 minutes after bro closes with no session left".into(),
        ),
    };
    let item = Item::new(row, label, value, help);
    match hs.startup.as_ref().and_then(|s| s.command()) {
        Some(command) if hs.startup_busy.is_none() => item.note(vec![Span::styled(format!("runs {command}"), muted(t))]),
        _ => item,
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
pub(crate) fn wrap(text: &str, w: usize) -> Vec<String> {
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
    /// waiting for the voice key to be pressed: keep the spinner moving
    voice_busy: bool,
    /// a JustImagine update started here: (when, the commit it started from), until the checkout moves
    imagine_update: Option<(Instant, Option<String>)>,
    /// when the last look at JustImagine during that update was asked for
    imagine_looked: Option<Instant>,
    /// the watched repos' clones have been looked at since the view opened
    skills_loaded: bool,
    /// a repo is being pulled: keep its spinner moving
    skills_busy: bool,
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

    /// Select the agent teams switch (the team board's s).
    pub fn show_agent_teams(&mut self) {
        let k = KNOBS.iter().position(|k| k.id == claude_env::AGENT_TEAMS).unwrap_or(0);
        self.sel = rows().iter().position(|r| *r == Row::Knob(k)).unwrap_or(self.sel);
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
            ToolOpt::SoundsOn => {
                let on = !s.peon.on;
                s.peon.on = on;
                cx.svc.save_settings(s);
                if !cx.svc.is_demo() {
                    let mut cfg = bro_core::peon::PeonConfig::load();
                    cfg.set_enabled(on);
                    if let Err(e) = cfg.save() {
                        cx.toast(Kind::Error, format!("couldn't save the sound settings: {e}"));
                    }
                }
                cx.toast(Kind::Info, if on { "sounds enabled · new sessions play them · pick packs under Sound packs" } else { "sounds disabled" });
            }
            ToolOpt::Sounds => cx.act(Action::Run(Act::Sounds)),
            ToolOpt::PeonPing => {
                if let Err(e) = crate::panes::links::open(PEONPING_URL) {
                    cx.toast(Kind::Error, format!("couldn't open {PEONPING_URL}: {e}"));
                }
            }
            ToolOpt::Imagine => self.imagine(cx),
            ToolOpt::WatchSkill => cx.svc.set_bundled(Bundled::JustWatch, !s.justwatch.skill),
            ToolOpt::SmartPaste if jt.paste_busy.is_some() => {}
            ToolOpt::SmartPaste if jt.installed.is_none() && !cx.svc.is_demo() => cx.act(Action::Run(Act::QuickInstall)),
            ToolOpt::SmartPaste if jt.installed.is_some() && jt.paste.is_none() && jt.update().is_some() && jt.job.is_none() => cx.svc.install_justtools(),
            ToolOpt::SmartPaste if jt.installed.is_some() && jt.paste.is_none() => cx.toast(Kind::Info, "this JustTools has no JustPaste · update JustTools first"),
            ToolOpt::SmartPaste => cx.svc.set_smart_paste(!jt.paste.is_some_and(|p| p.running)),
            _ if jt.job.is_some() => {}
            // a first install starts from the checklist, with JustTools ticked
            ToolOpt::Install if jt.installed.is_none() => cx.act(Action::Run(Act::QuickInstall)),
            ToolOpt::Install if cx.svc.is_demo() => cx.toast(Kind::Info, "the demo doesn't install anything"),
            ToolOpt::Install if jt.update().is_some() => cx.svc.install_justtools(),
            ToolOpt::Install => cx.svc.check_justtools(),
            ToolOpt::Skill => cx.svc.set_bundled(Bundled::JustTools, !s.justtools.skill),
        }
    }

    /// Enter on a Skills row: open the Skills view, switch bro-open or auto-sync, or write JustImagine's skills.
    fn skill(&mut self, o: SkillOpt, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        match o {
            SkillOpt::Manage => cx.act(Action::Run(Act::Skills)),
            SkillOpt::BroOpen => cx.svc.set_bundled(Bundled::Open, !s.skills.bro_open),
            SkillOpt::AutoSync => {
                s.skills.auto_sync = !s.skills.auto_sync;
                let on = s.skills.auto_sync;
                cx.svc.save_settings(s);
                if on {
                    cx.svc.sync_skill_repos();
                }
            }
            SkillOpt::Imagine if cx.svc.is_demo() => cx.toast(Kind::Info, "the demo doesn't install anything"),
            SkillOpt::Imagine => cx.svc.install_imagine_skills(),
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

    /// Enter (or ←/→) on a voice row. Every change restarts the listener with it.
    fn voice(&mut self, o: VoiceOpt, dir: isize, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        let v = &mut s.voice;
        match o {
            VoiceOpt::Key => {
                let all = key_choices();
                let i = all.iter().position(|k| *k == voice_key(v)).map_or(0, |i| i as isize + dir);
                v.hotkey = Some(all[i.rem_euclid(all.len() as isize) as usize].to_string());
            }
            VoiceOpt::On => v.enabled = !v.enabled,
            VoiceOpt::Mode => v.mode = if live_mode(v) { "transcribe" } else { "live" }.into(),
            VoiceOpt::Speaker => {
                let current = v.live_voice.clone().unwrap_or_else(|| bro_voice::DEFAULT_LIVE_VOICE.into());
                let i = LIVE_VOICES.iter().position(|x| *x == current).map_or(0, |i| i as isize + dir);
                v.live_voice = Some(LIVE_VOICES[i.rem_euclid(LIVE_VOICES.len() as isize) as usize].into());
            }
            // look for the key again
            VoiceOpt::ApiKey => {}
        }
        if o != VoiceOpt::ApiKey {
            cx.svc.save_settings(s);
        }
        if cx.svc.runs_voice() {
            cx.svc.restart_voice();
        } else if o == VoiceOpt::On || o == VoiceOpt::ApiKey {
            cx.toast(Kind::Info, voice_elsewhere(cx));
        }
    }

    /// Enter on the selected row: the voice key waits for the key you press (where that works); everything
    /// else steps forward.
    fn press(&mut self, cx: &mut Cx) {
        if self.row() == Row::Voice(VoiceOpt::Key) && bro_voice::Voice::can_capture_hotkey() {
            return cx.svc.capture_voice_hotkey();
        }
        self.step(1, cx);
    }

    /// Step the selected row's value by `dir` (+1 / -1).
    fn step(&mut self, dir: isize, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        let env = &mut s.claude_env;
        match self.row() {
            Row::Bro(o) => return o.change(dir, cx),
            Row::Voice(o) => return self.voice(o, dir, cx),
            Row::Tool(o) => return self.tool(o, cx),
            Row::Skill(o) => return self.skill(o, cx),
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
            Row::Host(HostOpt::Keep) => {
                s.server.keep_sessions = !s.server.keep_sessions;
                if s.server.keep_sessions {
                    cx.svc.ensure_host();
                }
            }
            Row::Host(HostOpt::Working) => s.server.keep_working = !s.server.keep_working,
            Row::Host(HostOpt::Restart) => {
                // the app asks first: restarting ends the sessions it runs
                cx.act(Action::Run(Act::RestartServer));
                return;
            }
            Row::Host(HostOpt::Startup) => {
                let on = cx.svc.state().host.startup.as_ref().is_some_and(|s| s.on());
                cx.svc.set_host_startup(!on);
                return;
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
            Row::Mod(ModOpt::TimerSecs) => {
                let m = &mut s.claude_mod;
                m.timer_secs = crate::watchdog::step_choice(&bro_core::claude_mod::TIMER_CHOICES, m.timer_secs, dir);
            }
            Row::Mod(o) => o.flip(&mut s.claude_mod),
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
            Row::Mod(ModOpt::TimerSecs) => s.claude_mod.timer_secs = bro_core::claude_mod::ClaudeModSettings::default().timer_secs,
            Row::Bro(_) | Row::Cave(_) | Row::Tool(_) | Row::Skill(_) | Row::Update(_) | Row::Host(_) | Row::Mod(_) | Row::Voice(VoiceOpt::On | VoiceOpt::ApiKey) => return,
            Row::Voice(o) => {
                match o {
                    VoiceOpt::Key => s.voice.hotkey = None,
                    VoiceOpt::Speaker => s.voice.live_voice = None,
                    _ => s.voice.mode = bro_core::config::VoiceSettings::default().mode,
                }
                cx.svc.save_settings(s);
                cx.svc.restart_voice();
                return;
            }
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
            Row::Preset => s.claude_env.clear_preset_changes(),
            Row::Knob(k) => {
                s.claude_env.overrides.remove(KNOBS[k].id);
            }
        }
        cx.svc.save_settings(s);
    }
}

/// Why this bro isn't the one listening.
fn voice_elsewhere(cx: &Cx) -> &'static str {
    match () {
        _ if cx.svc.is_demo() => "the demo doesn't listen for voice",
        _ if !cx.svc.settings().bridge.enabled => "voice talks to Hugh through the phone bridge, which is off ([bridge] enabled in ~/.bro/v2.toml)",
        _ => "your other bro runs voice: change it there, or restart it to pick this up",
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
    // bro's own hooks, which each launch gets with its folder
    if let Some(scope) = bro_core::peers::Scope::of(env) {
        parts.push(format!("hooks on SendMessage and ListAgents: bro hook peers {}", scope.id()));
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
                        BroOpt::Font | BroOpt::Rain | BroOpt::Screensaver | BroOpt::Material => {
                            let style = if v == "off" || v == "solid" { muted(t) } else { fg(t.inline) };
                            choice(v, style, t)
                        }
                        BroOpt::Tint => choice(v, fg(t.inline), t),
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
            Section::Voice => self.voice_blocks(cx),
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
            Section::Mod => {
                let m = &st.claude_mod;
                let mut out = vec![];
                for o in MOD_OPTS {
                    if let Some(h) = o.heading() {
                        out.push(Block::Sub(h.into()));
                    }
                    let value = match o {
                        ModOpt::TimerSecs => {
                            let style = if o.live(m) { fg(t.inline) } else { muted(t) };
                            choice(crate::watchdog::human(u64::from(m.timer_secs)), style, t)
                        }
                        _ => switch(o.get(m), o.live(m), t),
                    };
                    out.push(Block::Item(Item::new(Row::Mod(o), o.label(), value, o.help())));
                }
                out
            }
            Section::Claude => {
                let env = &st.claude_env;
                let mut out = vec![Block::Sub(Group::Teams.label().into())];
                let teams_on = env.agent_teams();
                for k_i in knobs(true) {
                    let k = &KNOBS[k_i];
                    let (o, src) = env.choice(k);
                    let value = match k.id {
                        claude_env::AGENT_TEAMS => switch(teams_on, true, t),
                        _ => choice(k.options[o].label, if src == Source::Override { bold(t.shine) } else { muted(t) }, t),
                    };
                    let mut item = Item::new(Row::Knob(k_i), k.label, value, k.help);
                    if k.id == claude_env::AGENT_TEAMS {
                        item.notes = team_notes(teams_on, &cx.svc.live_teams(), t);
                    } else if src == Source::Override {
                        item.tag = Some(Span::styled("  changed", fg(t.shine)));
                    }
                    out.push(Block::Item(item));
                }
                out.push(Block::Sub("environment".into()));
                let p = claude_env::preset(&env.preset);
                let mut preset = Item::new(Row::Preset, "Preset", choice(p.label, bold(t.shine), t), p.help);
                preset.tag = match env.preset_changes() {
                    0 => None,
                    n => Some(Span::styled(format!("  + {n} change{}", if n == 1 { "" } else { "s" }), fg(t.inline))),
                };
                out.push(Block::Item(preset));
                let mut group = None;
                for k_i in knobs(false) {
                    let k = &KNOBS[k_i];
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
            Section::Skills => self.skills(cx),
            Section::Sounds => self.sounds(cx),
            Section::Server => {
                let hs = cx.svc.state().host.clone();
                let live = cx.svc.host().map(|h| h.live()).unwrap_or(0);
                let current = bro_core::self_update::CURRENT;
                let keep = Item::new(
                    Row::Host(HostOpt::Keep),
                    "Keep sessions running",
                    switch(st.server.keep_sessions, true, t),
                    "when bro closes, and pick them up when it opens again · off: new sessions run inside bro and end with it",
                );
                let working = Item::new(
                    Row::Host(HostOpt::Working),
                    "Keep working while bro is closed",
                    switch(st.server.keep_working, st.server.keep_sessions, t),
                    "a background bro carries on with the sessions: the phone bridge, proxy, auto-resume and watchdogs keep going until you open bro again · off: they stop when bro closes (the sessions keep running)",
                );
                let sessions = crate::services::skills::plural(live, "session");
                let (v, style, help): (String, Style, String) = match (&hs.restarting, &hs.server) {
                    (Some(started), _) => (format!("{} restarting", spinner(*started)), bold(t.shine), String::new()),
                    (None, Some(s)) if s.version != current => (
                        format!("running · {sessions}"),
                        fg(t.shine),
                        format!("it runs bro {}: enter restarts it on {current} · its sessions end, and Claude and Codex sessions come back resumed", s.version),
                    ),
                    (None, Some(_)) => (format!("running · {sessions}"), fg(t.good), "enter restarts it: its sessions end, and Claude and Codex sessions come back in their panes, resuming their conversations".into()),
                    (None, None) if st.server.keep_sessions => ("not running".into(), muted(t), "enter starts it · it also starts with your next session".into()),
                    (None, None) => ("not running".into(), muted(t), "enter starts it".into()),
                };
                let mut server = Item::new(Row::Host(HostOpt::Restart), "Session server", shown(v, style), help);
                if let Some(s) = &hs.server {
                    let up = (crate::util::now_ms().max(0) as u64).saturating_sub(s.started_ms) / 1000;
                    let watching = match hs.peers.viewers {
                        0 => String::new(),
                        1 => " · 1 more window watching".into(),
                        n => format!(" · {n} more windows watching"),
                    };
                    server = server.note(vec![Span::styled(format!("pid {} · bro {} · up {}{watching}", s.pid, s.version, crate::watchdog::human(up)), muted(t))]);
                } else if let Some(p) = &hs.problem {
                    server = server.note(vec![Span::styled(format!("✗ {p}"), fg(t.danger))]);
                }
                vec![Block::Item(keep), Block::Item(working), Block::Item(server), Block::Item(startup_item(&hs, t))]
            }
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

    /// Push-to-talk: on, its key, how Hugh answers, his voice, the OpenAI key.
    fn voice_blocks(&self, cx: &Cx) -> Vec<Block> {
        let t = cx.theme;
        let bold = |c| fg(c).add_modifier(Modifier::BOLD);
        let st = cx.svc.settings();
        let v = &st.voice;
        let vs = cx.svc.state().voice.clone();
        let here = cx.svc.runs_voice();
        let label = bro_voice::Voice::hotkey_label(&voice_key(v));

        let help = match () {
            _ if !v.enabled => "nothing listens and the key stays an ordinary key".to_string(),
            _ if !here => voice_elsewhere(cx).to_string(),
            _ if vs.running.is_some() => format!("listening on {label} in every app"),
            _ if vs.key_found == Some(false) => "waiting for an OpenAI key (below)".into(),
            _ => "starting…".into(),
        };
        let mut on = Item::new(Row::Voice(VoiceOpt::On), "Push-to-talk", switch(v.enabled, vs.running.is_some(), t), help);
        if let Some(e) = vs.error.as_ref().filter(|_| v.enabled) {
            on = on.note(vec![Span::styled(format!("✗ {e}"), fg(t.danger))]);
        }

        // bro comes forward on a press (Windows): a modifier only once it's held, so chords like Win+E still work
        let raise = match () {
            _ if !cfg!(windows) => "",
            _ if bro_voice::Voice::hotkey_is_modifier(&voice_key(v)) => " · bro comes forward once you hold it",
            _ => " · bro comes forward the moment it goes down",
        };
        let (value, help) = match vs.capturing {
            Some(started) => (shown(format!("{} press a key…", spinner(started)), bold(t.shine)), format!("press it now, in any app · esc keeps {label}")),
            None if bro_voice::Voice::can_capture_hotkey() => (
                choice(label.clone(), fg(t.inline), t),
                format!("enter, then press the key you want{raise} · a G-key set to F13–F24 is ideal, as nothing else uses those"),
            ),
            None => (choice(label.clone(), fg(t.inline), t), format!("the key you hold to talk · ←/→ for another{raise}")),
        };
        let mut key = Item::new(Row::Voice(VoiceOpt::Key), "Voice key", value, help);
        if v.hotkey.is_some() {
            key.tag = Some(Span::styled("  changed", fg(t.shine)));
        }

        let live = live_mode(v);
        let mode = Item::new(
            Row::Voice(VoiceOpt::Mode),
            "Hugh answers",
            choice(if live { "out loud" } else { "as text" }, fg(t.inline), t),
            if live { "talk with Hugh through GPT-Live: he answers out loud and hands the work to your orchestrator" } else { "your words go to the orchestrator as text; its answers come back as toasts" },
        );
        let speaker_name = v.live_voice.clone().unwrap_or_else(|| bro_voice::DEFAULT_LIVE_VOICE.into());
        let speaker = Item::new(
            Row::Voice(VoiceOpt::Speaker),
            "Hugh's voice",
            choice(speaker_name, if live { fg(t.inline) } else { muted(t) }, t),
            if live { "the voice GPT-Live speaks with" } else { "only when Hugh answers out loud" },
        );

        let env = &v.api_key_env;
        let (value, help) = match vs.key_found {
            Some(true) => (shown("found", fg(t.good)), format!("read from {env} · enter looks again")),
            Some(false) => (shown("not set", fg(t.danger)), format!("set {env} to an OpenAI API key (your environment, or Windows' user variables), then enter looks again")),
            None if here && v.enabled => (shown("checking…", muted(t)), format!("looking for {env}")),
            None => (shown("not checked", muted(t)), format!("voice reads its OpenAI key from {env}")),
        };
        let api = Item::new(Row::Voice(VoiceOpt::ApiKey), "OpenAI key", value, help);
        vec![Block::Item(on), Block::Item(key), Block::Item(mode), Block::Item(speaker), Block::Item(api)]
    }

    /// JustTools, Quick Install, Smart Paste and JustImagine.
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
            "tick what to install: JustTools, JustPaste, JustWatch, JustImagine, Git, Node.js, Bun, Python, .NET 9 / 10 / 11, Claude Code, Codex, Pi, omp…",
        );
        let auto = Item::new(Row::Tool(ToolOpt::AutoUpdate), "Auto-update JustTools", switch(jts.auto_update, true, t), "when bro opens, looks on GitHub for a newer release and installs it");
        let mut out = vec![Block::Item(tools), Block::Item(quick), Block::Item(auto)];
        if bro_core::justpaste::supported() {
            out.push(Block::Item(self.smart_paste(&jt, cx)));
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

    /// The Skills view, then bro's own skill, JustTools', JustWatch's and JustImagine's, and keeping the watched repos in
    /// step.
    fn skills(&self, cx: &Cx) -> Vec<Block> {
        use crate::services::skills::plural;
        let t = cx.theme;
        let bold = |c| fg(c).add_modifier(Modifier::BOLD);
        let note = |text: String, style: Style| vec![Span::styled(text, style)];
        let st = cx.svc.settings();
        let (jt, ji, sk) = {
            let s = cx.svc.state();
            (s.justtools.clone(), s.justimagine.clone(), s.skills.clone())
        };

        let watched = st.skills.repos.len();
        let value = match sk.inventory.as_ref() {
            Some(inv) => format!("{} ›", plural(inv.everywhere().count(), "skill")),
            None if watched > 0 => format!("{} watched ›", plural(watched, "repo")),
            None => "open ›".into(),
        };
        let mut out = vec![Block::Item(Item::new(
            Row::Skill(SkillOpt::Manage),
            "Manage skills",
            shown(value, fg(t.inline)),
            "every skill your agents have and where each came from, by source and by project · watch a repo for more (Theo's tstack is a click away) · find more on Skill Issue",
        ))];
        out.push(Block::Sub("bundled".into()));
        out.push(Block::Item(Item::new(Row::Skill(SkillOpt::BroOpen), "bro-open", switch(st.skills.bro_open, true, t), Bundled::Open.about())));
        let value = match &jt.job {
            Some(job) if job.kind == JobKind::Skill => shown(format!("{} adding", spinner(job.started)), bold(t.shine)),
            _ => switch(st.justtools.skill, true, t),
        };
        let help = if jt.own_skill { "~/.claude has its own copy, which bro leaves alone" } else { Bundled::JustTools.about() };
        out.push(Block::Item(Item::new(Row::Tool(ToolOpt::Skill), "JustTools skill", value, help)));
        if bro_core::justwatch::supported() {
            out.push(Block::Item(Item::new(Row::Tool(ToolOpt::WatchSkill), "JustWatch skill", switch(st.justwatch.skill, true, t), Bundled::JustWatch.about())));
        }
        let what = Bundled::JustImagine.about();
        let (value, help) = match sk.imagine {
            Some(true) => (shown("installed", fg(t.good)), format!("{what} · enter writes them again")),
            Some(false) => (shown("not installed", muted(t)), format!("{what} · enter writes them into every agent home")),
            None => (shown("checking…", muted(t)), what.to_string()),
        };
        let mut imagine = Item::new(Row::Skill(SkillOpt::Imagine), "JustImagine skills", value, help);
        if ji.checked && ji.installed.is_none() {
            imagine = imagine.note(note("they work through JustImagine, which isn't installed yet (Integrations)".into(), muted(t)));
        }
        out.push(Block::Item(imagine));
        out.push(Block::Sub("watched repos".into()));
        out.push(Block::Item(Item::new(
            Row::Skill(SkillOpt::AutoSync),
            "Keep them in sync",
            switch(st.skills.auto_sync, true, t),
            "pulls each repo when bro opens and every 6 hours; a skill that leaves its repo leaves your homes too",
        )));
        out
    }

    /// Sounds: enabled or disabled, the packs, and PeonPing, who provides them.
    fn sounds(&self, cx: &Cx) -> Vec<Block> {
        let t = cx.theme;
        let on = cx.svc.settings().peon.on;
        let value = if on { vec![Span::styled("● enabled", fg(t.good).add_modifier(Modifier::BOLD))] } else { vec![Span::styled("○ disabled", muted(t))] };
        let enabled = Item::new(Row::Tool(ToolOpt::SoundsOn), "Sounds", value, "plays a voice line when a session starts, finishes, fails or needs you · new sessions pick it up");
        let packs = Item::new(Row::Tool(ToolOpt::Sounds), "Sound packs", shown("browse ›", fg(t.inline)), "volume, which moments speak, and 370+ packs to search, preview and install");
        let site = Item::new(Row::Tool(ToolOpt::PeonPing), "Provided by PeonPing", shown("peonping.com ›", fg(t.inline)), format!("the voice lines and packs come from PeonPing · enter opens {PEONPING_URL}"));
        vec![Block::Item(enabled), Block::Item(packs), Block::Item(site)]
    }

    /// JustPaste's Smart Paste: on / off, what it does, and that copied files stay Windows' own paste.
    fn smart_paste(&self, jt: &crate::services::justtools::JustToolsState, cx: &Cx) -> Item {
        let t = cx.theme;
        let what = "Ctrl+V in File Explorer saves a copied image as a PNG, downloads copied links and clones copied repos into that folder";
        let (value, help) = match (jt.paste_busy, &jt.paste, &jt.installed) {
            (Some(started), ..) => (shown(format!("{} switching", spinner(started)), fg(t.shine).add_modifier(Modifier::BOLD)), what.to_string()),
            _ if jt.checking || !jt.checked => (shown("checking…", muted(t)), what.to_string()),
            (_, Some(p), _) => (switch(p.running, true, t), format!("{what} · {}", bro_core::justpaste::file_note(p))),
            (_, None, None) => (shown("needs JustTools", muted(t)), format!("{what} · enter opens Quick Install")),
            (_, None, Some(_)) if jt.update().is_some() => (shown("update JustTools", fg(t.shine)), format!("{what} · this JustTools has no JustPaste yet · enter updates it")),
            (_, None, Some(_)) => (shown("not in this JustTools", muted(t)), what.to_string()),
        };
        let mut item = Item::new(Row::Tool(ToolOpt::SmartPaste), "Smart Paste in Explorer", value, help);
        match jt.paste {
            Some(p) if p.running && p.paused => item = item.note(vec![Span::styled("paused from its tray icon · Ctrl+V is all Windows until you resume it there".to_string(), muted(t))]),
            Some(p) if p.running && !p.at_sign_in => item = item.note(vec![Span::styled("won't start at sign-in · enter twice turns that back on".to_string(), muted(t))]),
            _ => {}
        }
        item
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
            Section::Voice => {
                let vs = cx.svc.state().voice.clone();
                match (&vs.capturing, &vs.running) {
                    (Some(started), _) => Span::styled(spinner(*started), fg(t.shine)),
                    (None, Some(key)) => Span::styled(bro_voice::Voice::hotkey_label(key), fg(t.inline)),
                    (None, None) if st.voice.enabled => Span::styled(bro_voice::Voice::hotkey_label(&voice_key(&st.voice)), muted(t)),
                    (None, None) => Span::styled("off", muted(t)),
                }
            }
            Section::Caveman => match st.caveman.active() {
                Some(l) => Span::styled(l.id(), fg(t.shine)),
                None => Span::styled("off", muted(t)),
            },
            Section::Watchdog => Span::styled(format!("{} idle", crate::watchdog::human(st.watchdog.idle_secs)), muted(t)),
            Section::Claude => {
                let label = claude_env::preset(&st.claude_env.preset).label;
                match st.claude_env.preset_changes() {
                    0 => Span::styled(label, muted(t)),
                    n => Span::styled(format!("{label} +{n}"), fg(t.inline)),
                }
            }
            Section::Mod => match st.claude_mod.summary() {
                "on" => Span::styled("on", fg(t.good)),
                other => Span::styled(other, muted(t)),
            },
            Section::Integrations => {
                let (jt, ji) = (cx.svc.state().justtools.clone(), cx.svc.state().justimagine.clone());
                match &jt.job {
                    Some(job) => Span::styled(spinner(job.started), fg(t.shine)),
                    None if self.imagine_update.is_some() => Span::styled("updating", fg(t.shine)),
                    None if jt.update().is_some() || ji.update().is_some() => Span::styled("update", fg(t.shine).add_modifier(Modifier::BOLD)),
                    None => Span::raw(""),
                }
            }
            Section::Skills => {
                let sk = cx.svc.state().skills.clone();
                let watched: Vec<_> = st.skills.repos.iter().filter_map(|r| sk.repos.get(r)).collect();
                match watched.iter().find_map(|r| r.syncing) {
                    Some(started) => Span::styled(spinner(started), fg(t.shine)),
                    None if watched.iter().any(|r| r.error.is_some()) => Span::styled("✗", fg(t.danger)),
                    // every skill installed, once the Skills view has looked; else what the repos brought
                    None => match sk.inventory.as_ref() {
                        Some(inv) => Span::styled(crate::services::skills::plural(inv.everywhere().count(), "skill"), muted(t)),
                        None if st.skills.repos.is_empty() => Span::raw(""),
                        None => Span::styled(crate::services::skills::plural(st.skills.repos.len(), "repo"), muted(t)),
                    },
                }
            }
            Section::Sounds => match st.peon.on {
                true => Span::styled("on", fg(t.good)),
                false => Span::styled("off", muted(t)),
            },
            Section::Server => {
                let hs = cx.svc.state().host.clone();
                match (&hs.restarting, cx.svc.host()) {
                    (Some(started), _) => Span::styled(spinner(*started), fg(t.shine)),
                    (None, Some(h)) => Span::styled(format!("{} running", h.live()), muted(t)),
                    (None, None) if hs.problem.is_some() => Span::styled("✗", fg(t.danger)),
                    (None, None) => Span::styled("off", muted(t)),
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
        note_settings(&cx.svc.settings());
        if !self.tools_checked {
            self.tools_checked = true;
            cx.svc.check_justtools();
            cx.svc.check_justimagine(true);
        }
        if !self.skills_loaded {
            self.skills_loaded = true;
            cx.svc.load_skill_repos();
        }
        self.skills_busy = cx.svc.state().skills.busy();
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
        self.tools_busy = { let jt = &cx.svc.state().justtools; jt.job.is_some() || jt.paste_busy.is_some() };
        let u = cx.svc.state().updates.clone();
        self.update_busy = u.job.is_some() || u.checking;
        self.voice_busy = cx.svc.state().voice.capturing.is_some();

        let row = self.row();
        let mut hints = vec![("j/k", "move"), ("tab", "next section"), ("enter/→", "change"), ("←", "back")];
        if row == Row::Voice(VoiceOpt::Key) && bro_voice::Voice::can_capture_hotkey() {
            hints.truncate(2);
            hints.extend([("enter", "press the key"), ("←/→", "pick one")]);
        }
        match row {
            r if team_knob(r) => hints.push(("r", "bro's default")),
            Row::Preset | Row::Knob(_) => hints.push(("r", "reset to preset")),
            Row::Watch(_) | Row::Voice(VoiceOpt::Key | VoiceOpt::Mode | VoiceOpt::Speaker) => hints.push(("r", "bro's default")),
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
        if self.tools_busy || self.update_busy || self.voice_busy || self.skills_busy {
            return Some(Duration::from_millis(100));
        }
        self.imagine_update.as_ref().map(|_| IMAGINE_RECHECK)
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        note_settings(&cx.svc.settings());
        // "press the key you want" swallows keys before any window sees them: one that arrives here got past
        // it, so it's the answer (or esc, no change); the rest wait with it
        if cx.svc.state().voice.capturing.is_some() {
            match key.code {
                KeyCode::Esc => cx.svc.voice_key_pressed_here(None),
                code => {
                    if let Some(name) = window_hotkey(code) {
                        cx.svc.voice_key_pressed_here(Some(name));
                    }
                }
            }
            return true;
        }
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
            KeyCode::Enter | KeyCode::Char(' ') => self.press(cx),
            KeyCode::Char('l') | KeyCode::Right => self.step(1, cx),
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
                        self.press(cx);
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
    fn keys_reaching_the_window_name_voice_keys() {
        for (code, label) in [(KeyCode::F(13), "F13"), (KeyCode::F(24), "F24"), (KeyCode::F(5), "F5"), (KeyCode::Insert, "Insert")] {
            let name = window_hotkey(code).expect("a voice key");
            assert!(bro_voice::Voice::parse_hotkey(&name).is_some(), "{name}");
            assert_eq!(bro_voice::Voice::hotkey_label(&name), label);
        }
        assert_eq!(window_hotkey(KeyCode::Char('a')), None);
        assert_eq!(window_hotkey(KeyCode::Enter), None);
    }

    #[test]
    fn preview_lists_vars_and_settings() {
        let mut env = ClaudeEnvSettings::default();
        assert_eq!(
            env_parts(&env),
            ["CLAUDE_CODE_NO_FLICKER=1", "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1", r#"--settings {"viewMode":"focus","spinnerTipsEnabled":false}"#, "hooks on SendMessage and ListAgents: bro hook peers project"]
        );
        env.use_preset("presenter");
        let p = env_parts(&env);
        assert!(p.iter().any(|v| v == "IS_DEMO=1") && p.iter().any(|v| v == "CLAUDE_CODE_HIDE_CWD=1"), "{p:?}");
        let peers = claude_env::knob(bro_core::peers::KNOB).unwrap();
        env.set_choice(peers, peers.options.iter().position(|o| o.id == "default").unwrap());
        assert!(!env_parts(&env).iter().any(|v| v.starts_with("hooks")), "Claude's own reach: no hooks");
    }

    #[test]
    fn rows_run_in_section_order() {
        let r = rows();
        assert_eq!(r[0], Row::Bro(BroOpt::Theme));
        let n = bro_opts().len();
        assert_eq!(r[n], Row::Host(HostOpt::Keep), "the session server follows Sessions");
        assert_eq!(r[n + 1], Row::Host(HostOpt::Working));
        assert_eq!(r[n + 2], Row::Host(HostOpt::Restart));
        assert_eq!(r[n + 3], Row::Host(HostOpt::Startup));
        let n = n + 4;
        assert_eq!(r[n], Row::Voice(VoiceOpt::On));
        let n = n + VOICE_OPTS.len();
        assert_eq!(r[n], Row::Cave(CaveOpt::Mode));
        assert_eq!(r[n + 2], Row::Watch(WatchOpt::Prompt));
        let teams = knobs(true).count();
        assert!(teams >= 1 && r[n + 2 + WATCH_OPTS.len()] == Row::Knob(0), "agent teams lead the Claude settings");
        assert_eq!(r[n + 2 + WATCH_OPTS.len() + teams], Row::Preset);
        let mods = n + 3 + WATCH_OPTS.len() + KNOBS.len();
        assert_eq!(r[mods], Row::Mod(ModOpt::On), "the mod's own section follows Claude Code's");
        let tools = mods + MOD_OPTS.len();
        assert_eq!(r[tools], Row::Tool(ToolOpt::Install));
        assert_eq!(r[tools + 1], Row::Tool(ToolOpt::QuickInstall));
        let (paste, watch) = (usize::from(bro_core::justpaste::supported()), usize::from(bro_core::justwatch::supported()));
        assert_eq!(r[tools + 3 + paste], Row::Tool(ToolOpt::Imagine));
        // the skills: the Skills view, bro's, JustTools', JustWatch's, JustImagine's, keeping the repos in sync
        let skills = tools + 4 + paste;
        assert_eq!(r[skills], Row::Skill(SkillOpt::Manage));
        assert_eq!(r[skills + 1], Row::Skill(SkillOpt::BroOpen));
        assert_eq!(r[skills + 2], Row::Tool(ToolOpt::Skill));
        assert_eq!(r[skills + 3 + watch], Row::Skill(SkillOpt::Imagine));
        assert_eq!(r[skills + 4 + watch], Row::Skill(SkillOpt::AutoSync));
        let sounds = skills + 5 + watch;
        assert_eq!(r[sounds], Row::Tool(ToolOpt::SoundsOn));
        assert_eq!(r[sounds + 1], Row::Tool(ToolOpt::Sounds));
        assert_eq!(r[sounds + 2], Row::Tool(ToolOpt::PeonPing));
        assert_eq!(r[sounds + 3], Row::Update(UpdOpt::Version));
        assert_eq!(r.len(), sounds + 5);
        // each section's rows sit together, in the order the sections are listed
        let order: Vec<usize> = r.iter().map(|row| SECTIONS.iter().position(|s| *s == Section::of(*row)).unwrap()).collect();
        assert!(order.is_sorted(), "{order:?}");
        assert!((0..SECTIONS.len()).all(|i| order.contains(&i)), "every section has a row");
    }

    #[test]
    fn the_tint_row_shows_only_for_a_see_through_window() {
        use bro_core::config::{Material, Settings};
        crate::fonts::TEST_APP_WINDOW.with(|w| w.set(true));
        let has_tint = |m: Material| {
            note_settings(&Settings { material: m, ..Default::default() });
            rows().contains(&Row::Bro(BroOpt::Tint))
        };
        assert!(!has_tint(Material::Solid), "a solid window has nothing to tint");
        assert!(has_tint(Material::Acrylic) && has_tint(Material::Glass));
        let r = rows();
        let window = r.iter().position(|x| *x == Row::Bro(BroOpt::Material)).unwrap();
        assert_eq!(r[window + 1], Row::Bro(BroOpt::Tint), "right under Window");
        crate::fonts::TEST_APP_WINDOW.with(|w| w.set(false));
        note_settings(&Settings::default());
    }

    #[test]
    fn agent_teams_sit_above_the_preset_and_list_running_teams() {
        let mut v = SettingsView::default();
        v.show_agent_teams();
        assert_eq!(v.row(), Row::Knob(KNOBS.iter().position(|k| k.id == claude_env::AGENT_TEAMS).unwrap()));
        assert!(team_knob(v.row()));
        assert!(!team_knob(Row::Preset));
        let t = crate::theme::get("graphite");
        let text = |notes: Vec<Vec<Span<'static>>>| notes.iter().map(|l| l.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>();
        assert!(text(team_notes(false, &[], &t))[0].starts_with("off"));
        assert!(text(team_notes(true, &[], &t))[0].contains(crate::team_board::EXAMPLE));
        let team = std::sync::Arc::new(crate::team_board::tests::team());
        let live = crate::services::LiveTeam { lead: "Refactor auth".into(), project: "bro-cli-v2".into(), seq: 3, team };
        let lines = text(team_notes(true, &[live], &t));
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("Refactor auth") && lines[1].contains("2 teammates, 1 working") && lines[1].contains("1 of 4 tasks done"), "{lines:?}");
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
