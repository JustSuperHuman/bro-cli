//! Settings view: bro's own everyday options (theme, icons, auto-resume, auto-approve, recaps, compression), caveman
//! mode (terse replies in new Claude / Codex / Pi sessions) and the Claude Code environment bro sets on every Claude launch — a preset (Standard, Quiet, Private, Lockdown,
//! Incognito, Presenter, Marathon) plus per-switch overrides. Saved to `~/.bro/v2.toml`; the Claude Code
//! part applies to sessions launched from now on.

use crate::keymap::Act;
use crate::pane::{Action, Cx, Pane};
use crate::ui::{self, fg, muted};
use bro_core::caveman::Level;
use bro_core::claude_env::{self, ClaudeEnvSettings, KNOBS, PRESETS, Source};
use bro_core::config::RainMode;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};
use std::sync::atomic::Ordering;

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

/// A selectable row: bro's options, caveman mode, then the Claude preset, then each [`KNOBS`] switch.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Bro(BroOpt),
    Cave(CaveOpt),
    Preset,
    Knob(usize),
}

fn rows() -> Vec<Row> {
    bro_opts()
        .into_iter()
        .map(Row::Bro)
        .chain([Row::Cave(CaveOpt::Mode), Row::Cave(CaveOpt::Subagents), Row::Preset])
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
}

impl SettingsView {
    pub fn new() -> SettingsView {
        SettingsView { upstream_caveman: bro_core::caveman::upstream_installed(&bro_core::paths::claude_local_dir()), ..SettingsView::default() }
    }

    fn row(&self) -> Row {
        rows()[self.sel.min(rows().len() - 1)]
    }

    /// Step the selected row's value by `dir` (+1 / -1).
    fn step(&self, dir: isize, cx: &mut Cx) {
        let mut s = cx.svc.settings();
        let env = &mut s.claude_env;
        match self.row() {
            Row::Bro(o) => return o.change(dir, cx),
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
            Row::Bro(_) | Row::Cave(_) => return,
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
    fn rows_are_bro_then_caveman_then_preset_then_knobs() {
        let r = rows();
        assert_eq!(r[0], Row::Bro(BroOpt::Theme));
        assert_eq!(r[bro_opts().len()], Row::Cave(CaveOpt::Mode));
        assert_eq!(r[bro_opts().len() + 2], Row::Preset);
        assert_eq!(r.len(), bro_opts().len() + 3 + KNOBS.len());
    }

    #[test]
    fn caveman_choices_start_off() {
        assert_eq!(cave_choices(), vec![None, Some(Level::Caveman), Some(Level::Ultracave), Some(Level::Megacave)]);
    }
}
