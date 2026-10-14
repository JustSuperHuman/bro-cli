//! Settings view: bro's own everyday options (theme, icons, auto-resume, auto-approve, recaps, compression), caveman
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
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
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
    Icons,
    AutoResume,
    AutoApprove,
    Recaps,
    Compression,
}

const BRO_OPTS: [BroOpt; 8] = [BroOpt::Theme, BroOpt::Font, BroOpt::Rain, BroOpt::Icons, BroOpt::AutoResume, BroOpt::AutoApprove, BroOpt::Recaps, BroOpt::Compression];

/// bro's options this window can use: the font only where the terminal can switch it.
fn bro_opts() -> Vec<BroOpt> {
    BRO_OPTS.into_iter().filter(|o| *o != BroOpt::Font || crate::fonts::available()).collect()
}

const RAIN: [RainMode; 3] = [RainMode::Off, RainMode::Everywhere, RainMode::Working];

impl BroOpt {
    fn label(self) -> &'static str {
        match self {
            BroOpt::Theme => "Theme",
            BroOpt::Font => "Font",
            BroOpt::Rain => "Digital rain",
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
            BroOpt::Icons => "turn off if icons show as boxes",
            BroOpt::AutoResume => "types `continue` when an agent stops on an API error or a limit resets",
            BroOpt::AutoApprove => "answers Yes to Claude's \"Do you want to proceed?\" (bypass mode's rm / path safety checks)",
            BroOpt::Recaps => "asks Claude for a /recap of a pane left waiting unseen (a few cents each)",
            BroOpt::Compression => "routes new Claude sessions through bro-proxy to trim requests",
        }
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
            BroOpt::Theme => cx.act(Action::Run(Act::Themes)),
            BroOpt::Icons => cx.act(Action::Run(Act::ToggleIcons)),
            BroOpt::AutoResume => cx.act(Action::Run(Act::ToggleAutoResume)),
            BroOpt::Compression => cx.act(Action::Run(Act::ToggleTrim)),
            BroOpt::AutoApprove => {
                let mut s = cx.svc.settings();
                s.auto_approve = !s.auto_approve;
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

/// A selectable row: bro's options, caveman mode, JustTools, then the Claude preset, then each [`KNOBS`] switch.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Bro(BroOpt),
    Cave(CaveOpt),
    Tool(ToolOpt),
    Update(UpdOpt),
    Preset,
    Knob(usize),
}

fn rows() -> Vec<Row> {
    bro_opts()
        .into_iter()
        .map(Row::Bro)
        .chain([Row::Cave(CaveOpt::Mode), Row::Cave(CaveOpt::Subagents), Row::Tool(ToolOpt::Install), Row::Tool(ToolOpt::QuickInstall), Row::Tool(ToolOpt::Skill), Row::Tool(ToolOpt::AutoUpdate)])
        .chain(bro_core::justwatch::supported().then_some(Row::Tool(ToolOpt::WatchSkill)))
        .chain([Row::Tool(ToolOpt::Imagine), Row::Update(UpdOpt::Version), Row::Update(UpdOpt::Check), Row::Preset])
        .chain((0..KNOBS.len()).map(Row::Knob))
        .collect()
}

/// The settings pane.
#[derive(Default)]
pub struct SettingsView {
    sel: usize,
    scroll: usize,
    /// (screen row, selectable row) from the last render, for clicks
    hits: Vec<(u16, usize)>,
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
            Row::Preset => s.claude_env.overrides.clear(),
            Row::Knob(k) => {
                s.claude_env.overrides.remove(KNOBS[k].id);
            }
        }
        cx.svc.save_settings(s);
    }
}

/// "K=V  K=V" for the next direct launch.
fn env_preview(env: &ClaudeEnvSettings) -> String {
    let vars = env.vars(false);
    if vars.is_empty() {
        return "nothing — Claude Code runs with its own defaults".into();
    }
    vars.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("  ")
}

fn heading(text: &str, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    vec![Span::styled(format!("──── {text} "), muted(t).add_modifier(Modifier::BOLD))]
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
        let env = cx.svc.settings().claude_env;
        let reset = matches!(self.row(), Row::Preset | Row::Knob(_));
        let mut hints = vec![("j/k", "move"), ("enter/→", "change"), ("←", "back")];
        if reset {
            hints.push(("r", "reset to preset"));
        }
        let area = ui::hint_line(f, area, &hints, t);
        let all = rows();
        let idx = |r: Row| all.iter().position(|x| *x == r).unwrap_or(0);
        let mark = |i: usize| if self.sel == i { "▌" } else { " " };
        let name = |i: usize, label: &str| Span::styled(format!("{} {:<26}", mark(i), label), if self.sel == i { ui::bold() } else { Style::default() });
        // (selectable row, spans); None = heading or text.
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![];
        lines.push((None, vec![Span::styled(" SETTINGS ", ui::bold_accent(t)), Span::styled("  saved to ~/.bro/v2.toml", muted(t))]));
        lines.push((None, vec![]));
        lines.push((None, heading("bro", t)));
        for o in bro_opts() {
            let i = idx(Row::Bro(o));
            let v = o.value(cx);
            let style = if v == "off" { muted(t) } else { fg(t.inline) };
            // the font row says what the current pick is like
            let help = match cx.svc.settings().font.as_deref().and_then(crate::fonts::get) {
                Some(f) if o == BroOpt::Font => format!("{} · ←/→ to try another", f.blurb),
                _ => o.help().to_string(),
            };
            lines.push((Some(i), vec![name(i, o.label()), Span::styled(ui::pad(&v, 16), style), Span::styled(format!(" {help}"), muted(t))]));
        }

        lines.push((None, vec![]));
        lines.push((None, heading("caveman · terse replies, every fact kept · new Claude, Codex and Pi sessions", t)));
        let cave = cx.svc.settings().caveman;
        let i = idx(Row::Cave(CaveOpt::Mode));
        let (v, style, help) = match cave.active() {
            Some(l) => (format!("‹ {} ›", l.id()), fg(t.shine).add_modifier(Modifier::BOLD), l.help()),
            None => ("off".to_string(), muted(t), "agents write normally · rules from github.com/JuliusBrussee/caveman"),
        };
        lines.push((Some(i), vec![name(i, "Caveman mode"), Span::styled(ui::pad(&v, 16), style), Span::styled(format!(" {help}"), muted(t))]));
        let i = idx(Row::Cave(CaveOpt::Subagents));
        let sub_on = cave.subagents && cave.on;
        lines.push((Some(i), vec![
            name(i, "Claude subagents too"),
            Span::styled(ui::pad(if cave.subagents { "on" } else { "off" }, 16), if sub_on { fg(t.inline) } else { muted(t) }),
            Span::styled(" their reports back to the main agent get shorter too", muted(t)),
        ]));
        if self.upstream_caveman {
            lines.push((None, vec![Span::styled(
                format!("  {:<26}the Caveman plugin is installed in ~/.claude too: bro keeps it quiet while this is on", ""),
                muted(t),
            )]));
        }

        lines.push((None, vec![]));
        lines.push((None, heading("integrations · tools your agents can call", t)));
        let jt = cx.svc.state().justtools.clone();
        let jts = cx.svc.settings().justtools;
        self.tools_busy = jt.job.is_some();
        let note = |text: String, style: Style| (None, vec![Span::styled(format!("  {:<26}{text}", ""), style)]);
        let i = idx(Row::Tool(ToolOpt::Install));
        let installing = jt.job.as_ref().filter(|j| j.kind != JobKind::Skill);
        let version = jt.installed.as_ref().map(|f| f.version.clone().unwrap_or_else(|| "installed".into()));
        let (v, style, help): (String, Style, &str) = match (installing, &version, jt.update()) {
            (Some(job), _, _) => (format!("{} {}", spinner(job.started), if job.kind == JobKind::Update { "updating" } else { "installing" }), fg(t.shine).add_modifier(Modifier::BOLD), ""),
            _ if jt.checking || !jt.checked => ("checking…".into(), muted(t), ""),
            (_, None, _) => ("not installed".into(), muted(t), "enter opens Quick Install · image, video, PDF and port commands in one binary"),
            (_, Some(v), Some(new)) => (format!("{v} → {new}"), fg(t.shine).add_modifier(Modifier::BOLD), "a newer release is out · enter updates"),
            (_, Some(v), None) if jt.offline => (v.clone(), fg(t.inline), "couldn't reach GitHub to look for a newer release · enter tries again"),
            (_, Some(v), None) => (v.clone(), fg(t.inline), "the newest release · enter checks again"),
        };
        lines.push((Some(i), vec![name(i, "JustTools"), Span::styled(ui::pad(&v, 16), style), Span::styled(format!(" {help}"), muted(t))]));
        if let Some(job) = installing {
            // one segment per step, then where it is and how long it has taken
            let at = Step::ALL.iter().position(|s| *s == job.step).unwrap_or(0) + 1;
            lines.push((None, vec![
                Span::raw(format!("  {:<26}", "")),
                Span::styled("━━━━".repeat(at), fg(t.accent)),
                Span::styled("━━━━".repeat(Step::ALL.len() - at), fg(t.frame)),
                Span::styled(format!("  {at} of {} · {} · {}s", Step::ALL.len(), step_label(job.step), job.started.elapsed().as_secs()), muted(t)),
            ]));
        } else if let Some(e) = &jt.error {
            let retry = if version.is_none() || jt.update().is_some() { " · enter tries again" } else { "" };
            lines.push(note(format!("✗ {e}{retry}"), fg(t.danger)));
        } else if let Some(done) = &jt.done {
            lines.push(note(format!("✓ {done}"), fg(t.good)));
        } else if let Some(found) = jt.installed.as_ref().filter(|f| !f.on_path()) {
            lines.push(note(format!("in {} · restart bro so new sessions find it on PATH", found.bin.display()), muted(t)));
        } else if jt.checked && version.is_none() {
            lines.push(note("github.com/JustGains/JustTools · the download is checked against its SHA-256 before it installs".into(), muted(t)));
        }
        let i = idx(Row::Tool(ToolOpt::QuickInstall));
        lines.push((Some(i), vec![
            name(i, "Quick install"),
            Span::styled(ui::pad("open ›", 16), fg(t.inline)),
            Span::styled(" tick what to install: JustTools, JustWatch, JustImagine, Git, Node.js, Bun, Python, .NET 9 / 10 / 11, Claude Code, Codex, Pi, omp…", muted(t)),
        ]));
        let i = idx(Row::Tool(ToolOpt::Skill));
        let (v, style) = match &jt.job {
            Some(job) if job.kind == JobKind::Skill => (format!("{} adding", spinner(job.started)), fg(t.shine).add_modifier(Modifier::BOLD)),
            _ if jts.skill => ("on".into(), fg(t.inline)),
            _ => ("off".into(), muted(t)),
        };
        let help = if jt.own_skill { "~/.claude has its own copy, which bro leaves alone" } else { "tells new Claude and Codex sessions when to use the just* commands" };
        lines.push((Some(i), vec![name(i, "JustTools skill"), Span::styled(ui::pad(&v, 16), style), Span::styled(format!(" {help}"), muted(t))]));
        let i = idx(Row::Tool(ToolOpt::AutoUpdate));
        lines.push((Some(i), vec![
            name(i, "Auto-update JustTools"),
            Span::styled(ui::pad(if jts.auto_update { "on" } else { "off" }, 16), if jts.auto_update { fg(t.inline) } else { muted(t) }),
            Span::styled(" when bro opens, looks on GitHub for a newer release and installs it", muted(t)),
        ]));
        if bro_core::justwatch::supported() {
            let i = idx(Row::Tool(ToolOpt::WatchSkill));
            let on = cx.svc.settings().justwatch.skill;
            lines.push((Some(i), vec![
                name(i, "JustWatch skill"),
                Span::styled(ui::pad(if on { "on" } else { "off" }, 16), if on { fg(t.inline) } else { muted(t) }),
                Span::styled(" lets new Claude and Codex sessions check, restart and tail your dev servers through JustWatch", muted(t)),
            ]));
        }

        let ji = cx.svc.state().justimagine.clone();
        let i = idx(Row::Tool(ToolOpt::Imagine));
        let (v, style, help): (String, Style, String) = match (&ji.installed, ji.update()) {
            _ if self.imagine_update.is_some() => ("updating in a tab".into(), fg(t.shine).add_modifier(Modifier::BOLD), "pulling from GitHub, then npm install and a service restart".into()),
            _ if ji.checking || !ji.checked => ("checking…".into(), muted(t), String::new()),
            (None, _) => ("not installed".into(), muted(t), "enter opens Quick Install · image, video, song and voice studio that runs in the background".into()),
            (Some(f), Some(n)) => (format!("+{}", commits(n)), fg(t.shine).add_modifier(Modifier::BOLD), format!("{} is behind GitHub · enter pulls them and restarts the service", f.describe())),
            (Some(f), None) if f.dir.is_none() => (f.describe(), fg(t.inline), "runs from a copy bro doesn't manage, so bro leaves its updates to you".into()),
            (Some(f), None) if ji.error.is_some() => (f.describe(), fg(t.inline), format!("couldn't compare with GitHub: {} · enter tries again", ji.error.as_deref().unwrap_or_default())),
            (Some(f), None) if f.url.is_some() => (f.describe(), fg(t.inline), "up to date with GitHub · enter opens it in your browser, signed in".into()),
            (Some(f), None) => (f.describe(), fg(t.inline), "up to date with GitHub · enter checks again".into()),
        };
        lines.push((Some(i), vec![name(i, "JustImagine"), Span::styled(ui::pad(&v, 16), style), Span::styled(format!(" {help}"), muted(t))]));
        match ji.installed.as_ref() {
            Some(f) if f.url.is_some() => lines.push(note(format!("open at {} · starts at login through {}", f.url.as_deref().unwrap_or_default(), bro_core::justimagine::mechanism()), muted(t))),
            Some(_) => lines.push(note("its service isn't answering · `bro install justimagine --force` reinstalls and starts it".into(), fg(t.danger))),
            None => {}
        }

        lines.push((None, vec![]));
        lines.push((None, heading("updates · new bro releases from GitHub", t)));
        let u = cx.svc.state().updates.clone();
        self.update_busy = u.job.is_some() || u.checking;
        let current = bro_core::self_update::CURRENT;
        let i = idx(Row::Update(UpdOpt::Version));
        let bold = |c| fg(c).add_modifier(Modifier::BOLD);
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
        lines.push((Some(i), vec![name(i, "bro version"), Span::styled(ui::pad(&v, 16), style), Span::styled(format!(" {help}"), muted(t))]));
        if let Some(job) = &u.job {
            let at = bro_core::self_update::Step::ALL.iter().position(|s| *s == job.step).unwrap_or(0) + 1;
            let all = bro_core::self_update::Step::ALL.len();
            lines.push((None, vec![
                Span::raw(format!("  {:<26}", "")),
                Span::styled("━━━━".repeat(at), fg(t.accent)),
                Span::styled("━━━━".repeat(all - at), fg(t.frame)),
                Span::styled(format!("  {at} of {all} · {} · {}s", job.step.label(), job.started.elapsed().as_secs()), muted(t)),
            ]));
        } else if let Some(e) = &u.error {
            lines.push(note(format!("✗ {e} · enter tries again"), fg(t.danger)));
        }
        let i = idx(Row::Update(UpdOpt::Check));
        let check = cx.svc.settings().updates.check;
        lines.push((Some(i), vec![
            name(i, "Check for updates"),
            Span::styled(ui::pad(if check { "on" } else { "off" }, 16), if check { fg(t.inline) } else { muted(t) }),
            Span::styled(" when bro opens and every 12 hours; a new release is announced once, nothing installs on its own", muted(t)),
        ]));

        lines.push((None, vec![]));
        lines.push((None, heading("Claude Code · applies to sessions launched from now on", t)));
        let p = claude_env::preset(&env.preset);
        let i = idx(Row::Preset);
        lines.push((Some(i), vec![
            name(i, "Preset"),
            Span::styled(format!("‹ {} ›", p.label), fg(t.shine).add_modifier(Modifier::BOLD)),
            Span::styled(
                match env.overrides.len() {
                    0 => String::new(),
                    n => format!("  + {n} change{}", if n == 1 { "" } else { "s" }),
                },
                fg(t.inline),
            ),
        ]));
        lines.push((None, vec![Span::styled(format!("  {:<26}{}", "", p.help), muted(t))]));
        let mut group = None;
        for (k_i, k) in KNOBS.iter().enumerate() {
            if group != Some(k.group) {
                group = Some(k.group);
                lines.push((None, vec![]));
                lines.push((None, heading(k.group.label(), t)));
            }
            let (o, src) = env.choice(k);
            let style = match src {
                Source::Override => fg(t.shine).add_modifier(Modifier::BOLD),
                Source::Preset => fg(t.inline),
                Source::Default => muted(t),
            };
            let tag = if matches!(src, Source::Override) { "*" } else { " " };
            let i = idx(Row::Knob(k_i));
            lines.push((Some(i), vec![
                name(i, k.label),
                Span::styled(format!("{}{tag}", ui::pad(k.options[o].label, 15)), style),
                Span::styled(format!(" {}", k.help), muted(t)),
            ]));
        }
        lines.push((None, vec![]));
        lines.push((None, heading("next Claude launch sets", t)));
        lines.push((None, vec![Span::styled(format!("  {}", env_preview(&env)), fg(t.inline))]));
        lines.push((None, vec![Span::styled("  * changed from the preset · more variables: [claude_env.extra] in ~/.bro/v2.toml", muted(t))]));

        // Keep the selection on screen.
        let room = area.height as usize;
        let at = lines.iter().position(|(s, _)| *s == Some(self.sel)).unwrap_or(0);
        if at < self.scroll {
            self.scroll = at.saturating_sub(2);
        } else if at >= self.scroll + room {
            self.scroll = at + 1 - room;
        }
        // the last switch brings the launch preview under it into view
        if self.sel + 1 == all.len() {
            self.scroll = lines.len().saturating_sub(room);
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        self.hits.clear();
        for (n, (sel, spans)) in lines.into_iter().skip(self.scroll).take(room).enumerate() {
            let r = Rect { y: area.y + n as u16, height: 1, ..area };
            let selected = sel == Some(self.sel);
            if let Some(s) = sel {
                self.hits.push((r.y, s));
            }
            let spans = if selected { spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(t.frame))).collect() } else { spans };
            ui::line(f, r, spans);
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
    fn preview_lists_vars_or_says_nothing() {
        let mut env = ClaudeEnvSettings::default();
        assert!(env_preview(&env).starts_with("nothing"));
        env.use_preset("presenter");
        let p = env_preview(&env);
        assert!(p.contains("IS_DEMO=1") && p.contains("CLAUDE_CODE_HIDE_CWD=1"), "{p}");
    }

    #[test]
    fn rows_are_bro_then_caveman_then_justtools_then_preset_then_knobs() {
        let r = rows();
        assert_eq!(r[0], Row::Bro(BroOpt::Theme));
        assert_eq!(r[bro_opts().len()], Row::Cave(CaveOpt::Mode));
        assert_eq!(r[bro_opts().len() + 2], Row::Tool(ToolOpt::Install));
        assert_eq!(r[bro_opts().len() + 3], Row::Tool(ToolOpt::QuickInstall));
        let watch = usize::from(bro_core::justwatch::supported());
        assert_eq!(r[bro_opts().len() + 6 + watch], Row::Tool(ToolOpt::Imagine));
        assert_eq!(r[bro_opts().len() + 7 + watch], Row::Update(UpdOpt::Version));
        assert_eq!(r[bro_opts().len() + 9 + watch], Row::Preset);
        assert_eq!(r.len(), bro_opts().len() + 10 + watch + KNOBS.len());
    }

    #[test]
    fn caveman_choices_start_off() {
        assert_eq!(cave_choices(), vec![None, Some(Level::Caveman), Some(Level::Ultracave), Some(Level::Megacave)]);
    }
}
