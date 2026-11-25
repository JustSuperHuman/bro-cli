//! The Slop Collector's section (beta): the switch and its two choices, what it finds in your sessions right now
//! and what it has cleaned up, all from the session server's last report (see [`bro_core::slop`]). Plain words
//! first; what a thing is exactly (a headless Chrome, Remotion's compositor) comes after, for whoever wants it. The
//! page leads with what's in the way when it can't work yet (an older session server, sessions running inside bro).

use super::{Block, InfoRow, Item, Row, Stat, choice, shown, switch};
use crate::alerts::Kind;
use crate::keymap::Act;
use crate::pane::{Action, Cx};
use crate::ui::{fg, muted};
use bro_core::slop::{Cleanup, Leftover, Report, SlopSettings, TempFiles, WAIT_CHOICES, and_list, bytes};
use ratatui::style::Modifier;
use ratatui::text::Span;

/// The section's rows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum SlopOpt {
    On,
    /// how long a leftover sits unused before it's cleaned up, or "when I say"
    Wait,
    /// what renders and tests leave, or anything idle
    Scope,
    /// clean up everything listed now
    Now,
    /// the session server predates the collector: restart it
    Restart,
}

pub(super) const SLOP_OPTS: [SlopOpt; 5] = [SlopOpt::On, SlopOpt::Restart, SlopOpt::Wait, SlopOpt::Scope, SlopOpt::Now];

/// Where the collector stands.
enum Stage {
    Off,
    /// on, on a platform it can't look on yet
    Unsupported,
    /// on, but new sessions run inside bro, out of its reach
    Inside,
    /// on, with no session server running
    NoServer,
    /// on, with a session server from before it
    OldServer(String),
    Ready(Box<Report>),
}

fn stage(cx: &Cx) -> Stage {
    let st = cx.svc.settings();
    if !st.slop.on {
        return Stage::Off;
    }
    let host = cx.svc.state().host.clone();
    match (host.slop, host.server) {
        (Some(r), _) => Stage::Ready(Box::new(r)),
        _ if !bro_core::slop::supported() => Stage::Unsupported,
        _ if !st.server.keep_sessions => Stage::Inside,
        (None, None) => Stage::NoServer,
        (None, Some(s)) => Stage::OldServer(s.version),
    }
}

/// Whether `o` is shown: the choices once the collector works, the restart only while it's in the way.
pub(super) fn visible(o: SlopOpt, cx: &Cx) -> bool {
    match o {
        SlopOpt::On => true,
        SlopOpt::Restart => matches!(stage(cx), Stage::OldServer(_)),
        SlopOpt::Wait | SlopOpt::Scope | SlopOpt::Now => matches!(stage(cx), Stage::Ready(_)),
    }
}

/// Clean up after each wait in turn, then "when I say" (cleaning up on its own off).
fn step_wait(s: &mut SlopSettings, dir: isize) {
    let last = WAIT_CHOICES.len() as isize;
    let at = match s.collect {
        false => last,
        true => WAIT_CHOICES.iter().position(|&w| w >= s.wait_mins).unwrap_or(WAIT_CHOICES.len() - 1) as isize,
    };
    match (at + dir.signum()).clamp(0, last) {
        n if n == last => s.collect = false,
        n => {
            s.collect = true;
            s.wait_mins = WAIT_CHOICES[n as usize];
        }
    }
}

/// "10 min", "1 h".
fn minutes(m: u64) -> String {
    match m {
        m if m >= 60 && m % 60 == 0 => format!("{} h", m / 60),
        m => format!("{m} min"),
    }
}

/// Enter (or ←/→) on `o`.
pub(super) fn step(o: SlopOpt, dir: isize, cx: &mut Cx) {
    let mut s = cx.svc.settings();
    let slop = &mut s.slop;
    match o {
        SlopOpt::On => slop.on = !slop.on,
        SlopOpt::Scope => slop.anything_idle = !slop.anything_idle,
        SlopOpt::Wait => step_wait(slop, dir),
        SlopOpt::Restart => return cx.act(Action::Run(Act::RestartServer)),
        SlopOpt::Now => {
            let anything = cx.svc.state().host.slop.as_ref().is_some_and(Report::anything);
            match () {
                _ if !anything => cx.toast(Kind::Info, "Nothing to clean up: everything in your sessions is accounted for"),
                _ if cx.svc.is_demo() => cx.toast(Kind::Info, "The demo doesn't close anything"),
                _ => cx.svc.slop_look(true),
            }
            return;
        }
    }
    cx.svc.save_settings(s);
    // the server reads the settings itself: have it look now rather than at its next check
    cx.svc.slop_settings_changed();
}

/// r: cleaning up goes back to bro's (on its own, after 10 min).
pub(super) fn reset(o: SlopOpt, s: &mut SlopSettings) -> bool {
    match o {
        SlopOpt::Wait => {
            let d = SlopSettings::default();
            s.wait_mins = d.wait_mins;
            s.collect = d.collect;
            true
        }
        _ => false,
    }
}

/// Cleaning up isn't bro's.
pub(super) fn changed(o: SlopOpt, s: &SlopSettings) -> bool {
    let d = SlopSettings::default();
    o == SlopOpt::Wait && (s.wait_mins != d.wait_mins || s.collect != d.collect)
}

use bro_core::slop::duration as span;

pub(crate) fn ago(at_ms: u64) -> String {
    let now = crate::util::now_ms().max(0) as u64;
    match now.saturating_sub(at_ms) / 1000 {
        s if s < 45 => "just now".into(),
        s => format!("{} ago", span(s)),
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Where a leftover is: its project in its session, or the session.
pub(crate) fn place(project: &Option<String>, session: &str) -> String {
    match project {
        Some(p) if !p.eq_ignore_ascii_case(session) => format!("{p} in {session}"),
        Some(p) => p.clone(),
        None => format!("in {session}"),
    }
}

/// Why a leftover is one: "unused for 6 min".
pub(crate) fn why(l: &Leftover) -> String {
    format!("unused for {}", span(l.waited_secs))
}

/// When a leftover goes: "closes in 5 min", "waits for you".
pub(crate) fn due(l: &Leftover) -> String {
    match l.due_secs {
        None => "waits for you".into(),
        Some(0) => "closes at the next check".into(),
        Some(d) => format!("closes in {}", span(d)),
    }
}

/// What the temp folders are: "547 folders from Remotion and Puppeteer".
pub(crate) fn temp_detail(t: &TempFiles) -> String {
    match t.tools.as_slice() {
        [] => plural(t.folders as u64, "folder", "folders"),
        tools => format!("{} from {}", plural(t.folders as u64, "folder", "folders"), and_list(tools)),
    }
}

/// When the temp folders go.
pub(crate) fn temp_due(t: &TempFiles, collect: bool) -> &'static str {
    match () {
        _ if t.clearing => "clearing now",
        _ if collect => "clears at the next check",
        _ => "waits for you",
    }
}

/// A cleanup's line under its name: where, why, and what it was exactly.
pub(crate) fn cleanup_detail(c: &Cleanup) -> String {
    if c.temp_only() {
        let folders = plural(c.folders as u64, "temp folder", "temp folders");
        return match (c.what.is_empty(), c.by_hand) {
            (true, false) => folders,
            (true, true) => format!("{folders} · cleared by you"),
            (false, false) => format!("{folders} from {}", c.what),
            (false, true) => format!("{folders} from {} · cleared by you", c.what),
        };
    }
    let why = if c.by_hand { "closed by you".to_string() } else { format!("unused for {}", span(c.waited_secs)) };
    format!("{} · {why} · {}, {}", place(&c.project, &c.session_label), c.what, plural(c.processes as u64, "process", "processes"))
}

/// What a toast says when the collector cleaned something up.
pub(crate) fn toast(c: &Cleanup) -> String {
    if c.temp_only() {
        return format!("Slop Collector cleared {} of temp files ({})", bytes(c.disk), plural(c.folders as u64, "folder", "folders"));
    }
    let at = c.project.clone().unwrap_or_else(|| c.session_label.clone());
    let back = format!("{} of memory back", bytes(c.memory));
    match c.by_hand {
        true => format!("Slop Collector closed {} in {at} · {back}", c.name()),
        false => format!("Slop Collector closed {} in {at}, unused for {} · {back}", c.name(), span(c.waited_secs)),
    }
}

/// ...when it cleaned up several at once.
pub(crate) fn toast_many(many: &[Cleanup]) -> String {
    let memory: u64 = many.iter().map(|c| c.memory).sum();
    let disk: u64 = many.iter().map(|c| c.disk).sum();
    let back: Vec<String> = [(memory, "of memory"), (disk, "on disk")].into_iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{} {w}", bytes(n))).collect();
    format!("Slop Collector cleaned up {} · {} back", plural(many.len() as u64, "leftover", "leftovers"), back.join(" and "))
}

/// Beside the section's name: off, how much is left, or all clear.
pub(super) fn summary(cx: &Cx) -> Span<'static> {
    let t = cx.theme;
    match stage(cx) {
        Stage::Off => Span::styled("off", muted(t)),
        Stage::Ready(r) if r.anything() => {
            let n = r.leftovers.len() + usize::from(r.temp.folders > 0);
            Span::styled(format!("{n} left"), fg(t.shine).add_modifier(Modifier::BOLD))
        }
        Stage::Ready(_) => Span::styled("all clear", fg(t.good)),
        Stage::OldServer(_) => Span::styled("restart", fg(t.shine)),
        _ => Span::styled("waiting", muted(t)),
    }
}

/// The section's blocks.
pub(super) fn blocks(cx: &Cx) -> Vec<Block> {
    let t = cx.theme;
    let s = cx.svc.settings().slop;
    let stage = stage(cx);
    let ready = matches!(stage, Stage::Ready(_));
    let help = match &stage {
        Stage::Off => "closes programs your sessions left running, and clears the temp files they leave".to_string(),
        Stage::Ready(r) if r.looked_ms == 0 => "taking its first look at your sessions".to_string(),
        Stage::Ready(r) => format!("checks {} every 30 seconds · last check {}", plural(r.sessions as u64, "session", "sessions"), ago(r.looked_ms)),
        _ => "on, and waiting until it can see your sessions".to_string(),
    };
    let mut on = Item::new(Row::Slop(SlopOpt::On), "Slop Collector", switch(s.on, ready || !s.on, t), help);
    on.tag = Some(Span::styled("  beta", fg(t.shine)));
    let mut out = vec![Block::Item(on)];
    match stage {
        Stage::Off => {
            let ledger = cx.svc.state().host.slop.as_ref().map(|r| r.ledger.clone()).unwrap_or_default();
            let mut tiles = vec![];
            if ledger.memory > 0 {
                tiles.push(Stat { value: Span::styled(bytes(ledger.memory), fg(t.good).add_modifier(Modifier::BOLD)), label: "memory freed while it was on".into() });
            }
            if ledger.disk > 0 {
                tiles.push(Stat { value: Span::styled(bytes(ledger.disk), fg(t.good).add_modifier(Modifier::BOLD)), label: "temp files cleared".into() });
            }
            if !tiles.is_empty() {
                out.push(Block::Stats(tiles));
            }
            out.extend(primer(t));
        }
        Stage::Unsupported => out.push(Block::Text(vec![Span::styled("Slop Collector runs on Windows for now. macOS and Linux are next.", muted(t))])),
        Stage::Inside => out.push(Block::Text(vec![Span::styled(
            "Your sessions run inside bro right now, where Slop Collector can't see them. Turn on Keep sessions running under Session server, and the sessions you start after that are covered.",
            fg(t.shine),
        )])),
        Stage::NoServer => out.push(Block::Text(vec![Span::styled("Slop Collector runs in the session server, which isn't running. It starts with your next session.", muted(t))])),
        Stage::OldServer(version) => out.push(Block::Item(Item::new(
            Row::Slop(SlopOpt::Restart),
            "Session server",
            shown(format!("bro {version}"), fg(t.shine).add_modifier(Modifier::BOLD)),
            "it's older than Slop Collector, so restart it to start cleaning up · its sessions end, and Claude and Codex sessions come back where they were",
        ))),
        Stage::Ready(r) => out.extend(ready_blocks(&s, &r, cx)),
    }
    out
}

/// The live page: the two choices and the button, the numbers, what's left and what's been cleaned up.
fn ready_blocks(s: &SlopSettings, r: &Report, cx: &Cx) -> Vec<Block> {
    let t = cx.theme;
    let item = |o: SlopOpt, label: &str, value, help: String| Block::Item(Item::new(Row::Slop(o), label, value, help));
    let bold = |c| fg(c).add_modifier(Modifier::BOLD);
    let left = r.leftovers.len() as u64;
    let mut out = vec![
        item(
            SlopOpt::Wait,
            "Clean up",
            choice(if s.collect { format!("after {}", minutes(s.wait_mins)) } else { "when I say".into() }, fg(t.inline), t),
            if s.collect { "once a leftover has sat unused this long · temp files go as soon as nothing holds them".into() } else { "lists what it finds and waits for you".into() },
        ),
        item(
            SlopOpt::Scope,
            "What to close",
            choice(if s.anything_idle { "anything idle" } else { "renders & tests" }, fg(t.inline), t),
            if s.anything_idle {
                "also scripts and watchers left running on their own, once they go idle · windows and servers stay".into()
            } else {
                "what renders and tests leave running, like the headless browsers Remotion, Playwright and Puppeteer start".into()
            },
        ),
    ];

    // Clean up now: the sizes at the right, the counts under
    let mut sizes = vec![];
    let mut counts = vec![];
    if left > 0 {
        sizes.push(bytes(r.leftover_memory()));
        counts.push(format!("closes {}", plural(left, "program", "programs")));
    }
    if r.temp.folders > 0 {
        sizes.push(format!("{} on disk", bytes(r.temp.bytes)));
        counts.push(format!("clears {}", plural(r.temp.folders as u64, "temp folder", "temp folders")));
    }
    let (value, help) = match counts.as_slice() {
        [] => (shown("all clear", fg(t.good)), "nothing has been left behind".to_string()),
        _ => (shown(sizes.join(" · "), bold(t.shine)), format!("{} right away", and_list(&counts))),
    };
    out.push(item(SlopOpt::Now, "Clean up now", value, help));

    // the numbers, once there are some: a fresh start says "all clear" above and nothing more
    if left > 0 || r.ledger.memory > 0 || r.ledger.disk > 0 {
        let mut tiles = vec![Stat {
            value: Span::styled(if left > 0 { bytes(r.leftover_memory()) } else { "none".into() }, bold(if left > 0 { t.shine } else { t.good })),
            label: if left > 0 { format!("memory held by {}", plural(left, "leftover", "leftovers")) } else { "left running".into() },
        }];
        if r.ledger.memory > 0 {
            tiles.push(Stat { value: Span::styled(bytes(r.ledger.memory), bold(t.good)), label: format!("memory freed since {}", since(r.ledger.since_ms)) });
        }
        if r.ledger.disk > 0 {
            tiles.push(Stat { value: Span::styled(bytes(r.ledger.disk), bold(t.good)), label: "temp files cleared".into() });
        }
        out.push(Block::Stats(tiles));
    }

    if r.anything() {
        out.push(Block::Sub("left behind".into()));
        let mut rows: Vec<InfoRow> = r
            .leftovers
            .iter()
            .map(|l| InfoRow {
                title: l.name().to_string(),
                detail: format!("{} · {} · {}, {}", place(&l.project, &l.session_label), why(l), l.what, plural(l.processes as u64, "process", "processes")),
                value: Span::styled(bytes(l.memory), bold(t.fg)),
                note: Span::styled(due(l), if l.due_secs.is_some() { fg(t.shine) } else { muted(t) }),
            })
            .collect();
        if r.temp.folders > 0 {
            rows.push(InfoRow {
                title: "Temp files".into(),
                detail: format!("{} · throwaway browser profiles and render folders", temp_detail(&r.temp)),
                value: Span::styled(bytes(r.temp.bytes), bold(t.fg)),
                note: Span::styled(temp_due(&r.temp, r.collect), if r.collect || r.temp.clearing { fg(t.shine) } else { muted(t) }),
            });
        }
        out.push(Block::Rows(rows));
    }
    if r.others.groups > 0 {
        let mut text = format!(
            "Also running on their own: {} ({}, {}). They may still be working, so they're left to you.",
            and_list(&r.others.names),
            plural(r.others.processes as u64, "process", "processes"),
            bytes(r.others.memory)
        );
        if !s.anything_idle {
            text.push_str(" Set What to close to anything idle to have them closed once they go idle.");
        }
        out.push(Block::Text(vec![]));
        out.push(Block::Text(vec![Span::styled(text, muted(t))]));
    }
    out.push(Block::Sub("recently cleaned up".into()));
    if r.ledger.recent.is_empty() {
        out.push(Block::Text(vec![Span::styled("Nothing yet. When a render or test leaves something behind, it shows up here, with a note when it's gone.", muted(t))]));
    } else {
        out.push(Block::Rows(
            r.ledger
                .recent
                .iter()
                .take(8)
                .map(|c| InfoRow {
                    title: c.name().to_string(),
                    detail: cleanup_detail(c),
                    value: Span::styled(bytes(if c.temp_only() { c.disk } else { c.memory }), bold(t.good)),
                    note: Span::styled(ago(c.at_ms), muted(t)),
                })
                .collect(),
        ));
    }
    out.push(Block::Text(vec![]));
    out.push(Block::Text(vec![Span::styled("Never closed: anything with a window, a server you're using, or anything your sessions are still running.", muted(t))]));
    out
}

/// "Oct 31", from Unix ms, in your time zone.
fn since(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms as i64).single().map_or_else(String::new, |d| d.format("%b %-d").to_string())
}

/// What it cleans up and what it leaves alone, while it's off.
fn primer(t: &crate::theme::Theme) -> Vec<Block> {
    let row = |title: &str, detail: &str, value: Span<'static>| InfoRow { title: title.into(), detail: detail.into(), value, note: Span::raw("") };
    vec![
        Block::Sub("what it cleans up".into()),
        Block::Rows(vec![
            row(
                "Programs left running",
                "when a render or test gets cut short, the browser it was using can keep running out of sight · usually a headless browser from Remotion, Playwright or Puppeteer",
                Span::styled("closed", fg(t.shine)),
            ),
            row("Temp files", "the throwaway browser profiles and render folders those runs leave in your temp folder", Span::styled("cleared", fg(t.shine))),
        ]),
        Block::Sub("what it leaves alone".into()),
        Block::Rows(vec![row(
            "Anything you're using",
            "windows on screen, servers you're connected to, and whatever your sessions are still running",
            Span::styled("kept", fg(t.good)),
        )]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_dates_read_like_a_person_wrote_them() {
        assert_eq!(span(40), "40 s");
        assert_eq!(span(600), "10 min");
        assert_eq!(span(4800), "1 h 20 min");
        assert_eq!(span(7200), "2 h");
        assert_eq!(span(3 * 86_400), "3 days");
        assert_eq!((minutes(10), minutes(60)), ("10 min".to_string(), "1 h".to_string()));
        // noon UTC: Oct 31 in any time zone a person lives in
        assert_eq!(since(1_793_448_000_000), "Oct 31");
        assert_eq!(place(&Some("film".into()), "Night Before"), "film in Night Before");
        assert_eq!(place(&None, "Night Before"), "in Night Before");
    }

    #[test]
    fn clean_up_steps_through_the_waits_then_when_i_say() {
        let mut s = SlopSettings::default();
        step_wait(&mut s, 1);
        assert_eq!((s.collect, s.wait_mins), (true, 15));
        for _ in 0..5 {
            step_wait(&mut s, 1);
        }
        assert!(!s.collect, "past an hour: only when you say");
        assert!(changed(SlopOpt::Wait, &s));
        step_wait(&mut s, -1);
        assert_eq!((s.collect, s.wait_mins), (true, 60));
        assert!(reset(SlopOpt::Wait, &mut s));
        assert_eq!(s, SlopSettings::default());
        assert!(!changed(SlopOpt::Wait, &s));
    }

    #[test]
    fn toasts_say_what_closed_where_and_what_came_back() {
        let c = Cleanup {
            title: "Chrome from Remotion".into(),
            what: "Remotion's headless Chrome".into(),
            project: Some("film".into()),
            session_label: "Night Before".into(),
            processes: 21,
            memory: 7_194_000_000,
            waited_secs: 600,
            ..Cleanup::default()
        };
        assert_eq!(toast(&c), "Slop Collector closed Chrome from Remotion in film, unused for 10 min · 6.7 GB of memory back");
        let by_hand = Cleanup { by_hand: true, ..c.clone() };
        assert_eq!(toast(&by_hand), "Slop Collector closed Chrome from Remotion in film · 6.7 GB of memory back");
        // a cleanup from before titles: its exact name
        let old = Cleanup { title: String::new(), ..c.clone() };
        assert!(toast(&old).starts_with("Slop Collector closed Remotion's headless Chrome in film"));
        let temp = Cleanup { title: "Temp files".into(), what: "Puppeteer and Remotion".into(), folders: 547, disk: 15_700_000_000, ..Cleanup::default() };
        assert_eq!(toast(&temp), "Slop Collector cleared 14.6 GB of temp files (547 folders)");
        assert_eq!(cleanup_detail(&temp), "547 temp folders from Puppeteer and Remotion");
        assert_eq!(toast_many(&[c, temp]), "Slop Collector cleaned up 2 leftovers · 6.7 GB of memory and 14.6 GB on disk back");
    }
}
