//! Stats view (the ⓘ on the sidebar's "usage left"): token totals per login, in two tabs — v1's
//! `bro tokens` (lifetime + last 30 days, each login's share, a 30-day sparkline) and `bro profiles`
//! (rolling 24h / 7d / 30d, and what the last 30 days were made of). The logs are read on a background
//! thread (`bro_core::usage::tokens`), cached per file, so only the first open is slow.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::pane::{Cx, Pane, Waker};
use crate::theme::{Theme, mix};
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::usage::tokens::{self, ClaudeCounter, LogTotals, Progress, Tokens};
use chrono::{Duration as Days, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Days in the tokens tab's window (the default range on Claude's /stats screen).
const WINDOW_DAYS: i64 = 30;
const DAY: i64 = 24 * 3600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Tokens,
    Profiles,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::Tokens, Tab::Profiles];
    fn label(self) -> &'static str {
        match self {
            Tab::Tokens => "tokens",
            Tab::Profiles => "profiles",
        }
    }
}

/// One login's figures.
struct Login {
    harness: Harness,
    name: String,
    /// Claude's own counter (Claude logins only)
    counter: Option<ClaudeCounter>,
    logs: LogTotals,
}

struct Report {
    logins: Vec<Login>,
    /// unix seconds the scan finished
    at: i64,
    took: Duration,
}

/// A scan in flight (or done).
struct Job {
    progress: Progress,
    started: Instant,
    report: Mutex<Option<Report>>,
}

/// The stats pane.
pub struct StatsView {
    tab: Tab,
    job: Option<Arc<Job>>,
    /// last finished report, kept on screen while a rescan runs
    shown: Option<Arc<Job>>,
    scroll: usize,
    hits: Vec<(Rect, Tab)>,
}

impl StatsView {
    pub fn new() -> StatsView {
        StatsView { tab: Tab::Tokens, job: None, shown: None, scroll: 0, hits: vec![] }
    }

    fn running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.report.lock().is_none())
    }

    /// Read every login's logs on a background thread; wakes the pane when done.
    fn start(&mut self, cx: &Cx) {
        let Some(profiles) = cx.svc.state().profiles.ready().cloned() else { return };
        // one row per login directory (v1 `listUsageProfiles`)
        let mut seen = std::collections::HashSet::new();
        let profiles: Vec<_> = profiles.into_iter().filter(|p| seen.insert((p.is_claude(), bro_core::util::path_key(&p.dir)))).collect();
        let job = Arc::new(Job { progress: Progress::default(), started: Instant::now(), report: Mutex::new(None) });
        self.job = Some(job.clone());
        let waker: Waker = cx.waker();
        std::thread::spawn(move || {
            let logins: Vec<tokens::Login> = profiles.iter().map(|p| tokens::Login { claude: p.is_claude(), dir: p.dir.clone() }).collect();
            let logs = tokens::scan(&logins, &job.progress);
            let logins = profiles
                .iter()
                .zip(logs)
                .map(|(p, logs)| Login {
                    harness: if p.is_claude() { Harness::Claude } else { Harness::Codex },
                    name: p.name.clone(),
                    counter: if p.is_claude() { tokens::claude_counter(&p.dir) } else { None },
                    logs,
                })
                // pool dirs that never ran anything: nothing to show
                .filter(|l: &Login| l.counter.is_some() || l.logs.files > 0)
                .collect();
            *job.report.lock() = Some(Report { logins, at: crate::util::now_secs(), took: job.started.elapsed() });
            waker.wake();
        });
    }

    /// First read: a centred gauge of the bytes still to read.
    fn draw_loading(&self, f: &mut Frame, body: Rect, cx: &Cx) {
        let t = cx.theme;
        let w = body.width.saturating_sub(8).min(52);
        if w < 12 || body.height < 4 {
            return;
        }
        let x = body.x + (body.width - w) / 2;
        let y = body.y + body.height.saturating_sub(4) / 3;
        let at = |dy: u16| Rect { x, y: y + dy, width: w, height: 1 };
        let Some(j) = self.job.as_ref() else {
            ui::line(f, at(0), vec![Span::styled(format!("{} waiting for your logins…", ui::icon("stats")), muted(t))]);
            return;
        };
        let (done, total) = (j.progress.done.load(Ordering::Relaxed), j.progress.total.load(Ordering::Relaxed));
        ui::line(f, at(0), vec![Span::styled(format!("{} ", ui::icon("stats")), ui::accent(t)), Span::styled("tallying every token", ui::bold())]);
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        ui::line(f, at(2), share_bar(frac, w as usize, t.accent, t));
        let left = if total > 0 { format!("{} of {}", bytes(done), bytes(total)) } else { "finding logs…".into() };
        ui::line_lr(f, at(3), vec![Span::styled(left, muted(t))], vec![Span::styled("only new logs next time", muted(t))]);
    }

    fn switch(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0;
    }
}

impl Default for StatsView {
    fn default() -> Self {
        StatsView::new()
    }
}

/// "9.62B", "412M", "38.1K", "950".
pub(crate) fn compact(n: u64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    let v = n as f64;
    for (scale, unit) in UNITS {
        if v >= scale * 0.9995 {
            let x = v / scale;
            return if x >= 99.95 { format!("{x:.0}{unit}") } else if x >= 9.995 { format!("{x:.1}{unit}") } else { format!("{x:.2}{unit}") };
        }
    }
    n.to_string()
}

/// A sparkline of `values`, scaled to their own max; zero days are a dot.
fn spark(values: &[u64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|&v| match v {
            0 => '·',
            _ if max == 0 => '·',
            v => BARS[(((v as f64 / max as f64) * 7.0).round() as usize).min(7)],
        })
        .collect()
}

/// A share bar `w` cells wide in `color`, eighth-block resolution; anything above zero keeps a sliver.
fn share_bar(frac: f64, w: usize, color: Color, t: &Theme) -> Vec<Span<'static>> {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = (frac.clamp(0.0, 1.0) * (w * 8) as f64).round() as usize;
    let mut drawn = format!("{}{}", "█".repeat(eighths / 8), PARTS[eighths % 8]);
    if frac > 0.0 && drawn.is_empty() {
        drawn = "▏".into();
    }
    let used = ui::width(&drawn);
    vec![Span::styled(drawn, fg(color)), Span::styled("░".repeat(w.saturating_sub(used)), fg(mix(t.frame, Color::Rgb(0, 0, 0), 0.2)))]
}

/// The four kinds of tokens, in the order the mix bar draws them.
fn kinds(t: &Theme) -> [(&'static str, Color); 4] {
    [("input", t.shine), ("cache write", Color::Rgb(0xA7, 0x8B, 0xFA)), ("cache read", Color::Rgb(0x60, 0xA5, 0xFA)), ("output", t.good)]
}

/// One stacked bar of input / cache write / cache read / output, `w` cells.
fn mix_bar(tk: &Tokens, w: usize, t: &Theme) -> Vec<Span<'static>> {
    let total = tk.total();
    if total == 0 || w == 0 {
        return vec![Span::styled("·".repeat(w.min(1)), muted(t))];
    }
    let parts = [tk.input, tk.cache_write, tk.cache_read, tk.output];
    // every non-zero part gets at least one cell; the biggest absorbs the rounding
    let mut cells: Vec<usize> = parts.iter().map(|&p| if p == 0 { 0 } else { ((p as f64 / total as f64) * w as f64).round().max(1.0) as usize }).collect();
    let big = (0..4).max_by_key(|&i| parts[i]).unwrap_or(0);
    let sum: usize = cells.iter().sum();
    cells[big] = (cells[big] + w).saturating_sub(sum).max(1);
    kinds(t).iter().zip(cells).filter(|(_, n)| *n > 0).map(|((_, c), n)| Span::styled("█".repeat(n), fg(*c))).collect()
}

/// Right-align `s` in `w` columns.
fn ra(s: &str, w: usize) -> String {
    format!("{}{s}", " ".repeat(w.saturating_sub(ui::width(s))))
}

/// The provider heading row: glyph + name.
fn group_head(h: Harness, t: &Theme) -> Line<'static> {
    let brand = ui::harness_color(Some(h), t);
    Line::from(vec![
        Span::styled(format!(" {} ", ui::harness_glyph(Some(h))), fg(brand)),
        Span::styled(h.label().to_string(), fg(brand).add_modifier(Modifier::BOLD)),
    ])
}

struct Cols {
    name: usize,
    num: usize,
    /// share bar / mix bar width (0 = hidden)
    bar: usize,
    /// sparkline (0 = hidden)
    spark: usize,
}

fn cols(width: usize, nums: usize, spark: bool) -> Cols {
    let name = 16;
    let num = 10;
    let rest = width.saturating_sub(3 + name + nums * num + 2);
    let spark_w = if spark && rest >= 26 + WINDOW_DAYS as usize + 2 { WINDOW_DAYS as usize } else { 0 };
    let bar = rest.saturating_sub(if spark_w > 0 { spark_w + 3 } else { 0 }).saturating_sub(5);
    let bar = if bar >= 6 { bar.min(28) } else { 0 };
    Cols { name, num, bar, spark: spark_w }
}

impl Report {
    fn of(&self, h: Harness) -> impl Iterator<Item = &Login> {
        self.logins.iter().filter(move |l| l.harness == h)
    }
    fn harnesses(&self) -> Vec<Harness> {
        [Harness::Claude, Harness::Codex].into_iter().filter(|h| self.of(*h).next().is_some()).collect()
    }
}

/// A lifetime figure: the count, and whether it's only a floor (summed from the logs still on disk).
type Lifetime = (u64, bool);

/// (lifetime, window, daily over the window) for a login — Claude's own counter when it has one, else its logs.
fn token_figures(l: &Login, days: &[NaiveDate]) -> (Lifetime, u64, Vec<u64>) {
    let (from, to) = (days[0], *days.last().unwrap());
    match &l.counter {
        Some(c) => {
            // the counter is only as fresh as `lastComputedDate`: the days after it come from the transcripts
            let logs = l.logs.daily();
            let after = |d: &NaiveDate| c.through.is_some_and(|t| *d > t);
            let daily: Vec<u64> = days.iter().map(|d| if after(d) { logs.get(d) } else { c.daily.get(d) }.copied().unwrap_or(0)).collect();
            let since: u64 = logs.iter().filter(|(d, _)| after(d)).map(|(_, n)| n).sum();
            ((c.lifetime + since, false), daily.iter().sum(), daily)
        }
        None => {
            let daily = l.logs.daily();
            let window = daily.range(from..=to).map(|(_, n)| n).sum();
            ((l.logs.all().total(), true), window, days.iter().map(|d| daily.get(d).copied().unwrap_or(0)).collect())
        }
    }
}

/// "9.62B", or "≥1.63B" when it's a floor.
fn lifetime_text((n, floor): Lifetime) -> String {
    format!("{}{}", if floor { "≥" } else { "" }, compact(n))
}

/// v1 `bro tokens`: lifetime and the last 30 days per login, with each login's share of the window.
fn tokens_lines(r: &Report, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let today = tokens::local_date(r.at);
    let days: Vec<NaiveDate> = (0..WINDOW_DAYS).rev().map(|i| today - Days::days(i)).collect();
    let c = cols(width, 2, true);
    let figures: Vec<_> = r.logins.iter().map(|l| token_figures(l, &days)).collect();
    let window_total: u64 = figures.iter().map(|f| f.1).sum();
    let lifetime_total: Lifetime = (figures.iter().map(|f| f.0.0).sum(), figures.iter().any(|f| f.0.1));

    let mut out = vec![];
    // headline: the two totals and the provider split
    let mut head = vec![
        Span::raw("  "),
        Span::styled(lifetime_text(lifetime_total), ui::bold_accent(t)),
        Span::styled(" lifetime   ", muted(t)),
        Span::styled(compact(window_total), ui::bold().fg(t.shine)),
        Span::styled(format!(" last {WINDOW_DAYS} days"), muted(t)),
    ];
    for h in r.harnesses() {
        let n: u64 = r.logins.iter().zip(&figures).filter(|(l, _)| l.harness == h).map(|(_, f)| f.1).sum();
        if window_total > 0 {
            head.push(Span::styled(format!("    {} ", ui::harness_glyph(Some(h))), fg(ui::harness_color(Some(h), t))));
            head.push(Span::styled(format!("{:.0}%", n as f64 * 100.0 / window_total as f64), ui::bold()));
        }
    }
    out.push(Line::from(head));
    out.push(Line::default());

    let mut header = vec![Span::styled(format!("   {}", ui::pad("login", c.name)), muted(t)), Span::styled(ra("lifetime", c.num), muted(t)), Span::styled(ra(&format!("last {WINDOW_DAYS}d"), c.num), muted(t))];
    if c.bar > 0 {
        header.push(Span::styled(format!("   {}", ui::pad("share", c.bar + 5)), muted(t)));
    }
    if c.spark > 0 {
        // the sparkline's own axis: first day … today
        let (a, b) = (days[0].format("%b %-d").to_string(), today.format("%b %-d").to_string());
        let gap = c.spark.saturating_sub(ui::width(&a) + ui::width(&b) + 2);
        header.push(Span::styled(format!("  {a} {} {b}", "╌".repeat(gap)), muted(t)));
    }
    out.push(Line::from(header));

    let row = |name: Span<'static>, lifetime: Lifetime, window: u64, daily: Option<&[u64]>, color: Color, strong: bool| -> Line<'static> {
        let num_style = if strong { ui::bold() } else { Style::default().fg(t.fg) };
        let life_style = if lifetime.1 { num_style.fg(mix(t.fg, t.muted, 0.5)) } else { num_style };
        let mut s = vec![Span::raw("   "), name, Span::styled(ra(&lifetime_text(lifetime), c.num), life_style), Span::styled(ra(&compact(window), c.num), num_style.fg(t.shine))];
        if c.bar > 0 {
            let frac = if window_total > 0 { window as f64 / window_total as f64 } else { 0.0 };
            s.push(Span::raw("   "));
            s.extend(share_bar(frac, c.bar, color, t));
            s.push(Span::styled(format!("{:>4.0}%", frac * 100.0), muted(t)));
        }
        if let Some(d) = daily.filter(|_| c.spark > 0) {
            s.push(Span::styled(format!("  {}", spark(d)), fg(mix(color, t.muted, 0.25))));
        }
        Line::from(s)
    };
    for h in r.harnesses() {
        let brand = ui::harness_color(Some(h), t);
        out.push(Line::default());
        out.push(group_head(h, t));
        let members: Vec<_> = r.logins.iter().zip(&figures).filter(|(l, _)| l.harness == h).collect();
        for (l, (lifetime, window, daily)) in &members {
            out.push(row(Span::styled(ui::pad(&l.name, c.name), Style::default().fg(t.fg)), *lifetime, *window, Some(daily), brand, false));
        }
        // a single-login provider is its own subtotal
        if members.len() > 1 {
            let lifetime = (members.iter().map(|(_, f)| f.0.0).sum(), members.iter().any(|(_, f)| f.0.1));
            let window = members.iter().map(|(_, f)| f.1).sum();
            out.push(row(Span::styled(ui::pad("subtotal", c.name), muted(t).add_modifier(Modifier::ITALIC)), lifetime, window, None, brand, false));
        }
    }
    out.push(Line::from(Span::styled(format!("   {}", "─".repeat(c.name + 2 * c.num + if c.bar > 0 { c.bar + 8 } else { 0 })), fg(t.frame))));
    out.push(row(Span::styled(ui::pad("total", c.name), ui::bold_accent(t)), lifetime_total, window_total, None, t.accent, true));
    if lifetime_total.1 {
        out.push(Line::default());
        out.push(Line::from(vec![Span::styled("   ≥ ", ui::bold()), Span::styled("at least — counted from the logs still on disk", muted(t))]));
    }
    out
}

/// v1 `bro profiles`: rolling 24h / 7d / 30d per login, and what the 30 days were made of.
fn profiles_lines(r: &Report, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let c = cols(width, 3, false);
    let windows = [r.at - DAY, r.at - 7 * DAY, r.at - 30 * DAY];
    let figures: Vec<[Tokens; 3]> = r.logins.iter().map(|l| windows.map(|from| l.logs.since(from))).collect();
    let mut totals = [Tokens::default(); 3];
    for f in &figures {
        for (t, x) in totals.iter_mut().zip(f) {
            t.add(x);
        }
    }

    let mut out = vec![];
    let mut head = vec![Span::raw("  ")];
    for (i, label) in ["last 24h", "last 7d", "last 30d"].iter().enumerate() {
        if i > 0 {
            head.push(Span::raw("    "));
        }
        head.push(Span::styled(compact(totals[i].total()), if i == 0 { ui::bold_accent(t) } else { ui::bold().fg(t.shine) }));
        head.push(Span::styled(format!(" {label}"), muted(t)));
    }
    out.push(Line::from(head));
    out.push(Line::default());

    let mut header = vec![Span::styled(format!("   {}", ui::pad("login", c.name)), muted(t))];
    for l in ["24h", "7d", "30d"] {
        header.push(Span::styled(ra(l, c.num), muted(t)));
    }
    if c.bar > 0 {
        header.push(Span::styled(format!("   {}", ui::pad("30d mix", c.bar)), muted(t)));
    }
    out.push(Line::from(header));

    let row = |name: Span<'static>, f: &[Tokens; 3], strong: bool| -> Line<'static> {
        let base = if strong { ui::bold() } else { Style::default().fg(t.fg) };
        let mut s = vec![Span::raw("   "), name];
        for (i, x) in f.iter().enumerate() {
            let text = if x.total() == 0 { "·".to_string() } else { compact(x.total()) };
            let style = if x.total() == 0 { muted(t) } else if i == 0 { base.fg(t.shine) } else { base };
            s.push(Span::styled(ra(&text, c.num), style));
        }
        if c.bar > 0 {
            s.push(Span::raw("   "));
            s.extend(mix_bar(&f[2], c.bar, t));
        }
        Line::from(s)
    };
    for h in r.harnesses() {
        out.push(Line::default());
        out.push(group_head(h, t));
        for (l, f) in r.logins.iter().zip(&figures).filter(|(l, _)| l.harness == h) {
            out.push(row(Span::styled(ui::pad(&l.name, c.name), Style::default().fg(t.fg)), f, false));
        }
    }
    out.push(Line::from(Span::styled(format!("   {}", "─".repeat(c.name + 3 * c.num + if c.bar > 0 { c.bar + 3 } else { 0 })), fg(t.frame))));
    out.push(row(Span::styled(ui::pad("total", c.name), ui::bold_accent(t)), &totals, true));

    out.push(Line::default());
    if c.bar > 0 {
        let mut legend = vec![Span::raw("  ")];
        for (name, color) in kinds(t) {
            legend.push(Span::styled("■ ", fg(color)));
            legend.push(Span::styled(format!("{name}   "), muted(t)));
        }
        out.push(Line::from(legend));
    }
    let mix = &totals[2];
    if mix.total() > 0 {
        let pct = |n: u64| n as f64 * 100.0 / mix.total() as f64;
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{:.0}%", pct(mix.cache_read)), ui::bold().fg(kinds(t)[2].1)),
            Span::styled(" of the last 30 days was re-read from cache  ·  ", muted(t)),
            Span::styled(format!("{:.1}%", pct(mix.output)), ui::bold().fg(t.good)),
            Span::styled(" was written by the model", muted(t)),
        ]));
    }
    out
}

impl Pane for StatsView {
    fn title(&self) -> String {
        "stats".into()
    }
    fn icon(&self) -> &'static str {
        "stats"
    }
    fn view(&self) -> Option<&'static str> {
        Some("stats")
    }
    fn tick_every(&self) -> Option<Duration> {
        // the progress line moves while a scan runs
        self.running().then_some(Duration::from_millis(250))
    }
    fn poll(&mut self, _cx: &mut Cx) {
        if let Some(j) = &self.job
            && j.report.lock().is_some()
        {
            self.shown = Some(j.clone());
        }
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        if self.job.is_none() {
            self.start(cx);
        }
        self.poll(cx);
        self.hits.clear();
        let area = ui::hint_line(f, area, &[("tab", "switch"), ("r", "rescan"), ("1/2", "tokens / profiles"), ("j/k", "scroll")], t);
        if area.height < 3 {
            return;
        }

        // tab strip + status
        let mut x = area.x + 1;
        let mut left = vec![Span::raw(" ")];
        for tab in Tab::ALL {
            let label = format!(" {} ", tab.label());
            let w = ui::width(&label) as u16;
            let style = if tab == self.tab { Style::default().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD) } else { muted(t) };
            left.push(Span::styled(label, style));
            left.push(Span::raw(" "));
            self.hits.push((Rect { x, y: area.y, width: w, height: 1 }, tab));
            x += w + 1;
        }
        let status = match (&self.job, self.running()) {
            (Some(j), true) => {
                let (done, total) = (j.progress.done.load(Ordering::Relaxed), j.progress.total.load(Ordering::Relaxed));
                let spin = ["◐", "◓", "◑", "◒"][(j.started.elapsed().as_millis() / 200 % 4) as usize];
                if total > 0 {
                    format!("{spin} reading logs {} / {} ", bytes(done), bytes(total))
                } else {
                    format!("{spin} reading logs… ")
                }
            }
            _ => match self.shown.as_ref().and_then(|j| j.report.lock().as_ref().map(|r| (r.at, r.took))) {
                Some((at, took)) => {
                    let ago = (crate::util::now_secs() - at).max(0) as u64;
                    let when = if ago < 60 { "just now".to_string() } else { format!("{} ago", crate::util::short_dur(ago)) };
                    format!("logs read {when} in {:.1}s ", took.as_secs_f64())
                }
                None => String::new(),
            },
        };
        ui::line_lr(f, Rect { height: 1, ..area }, left, vec![Span::styled(status, muted(t))]);

        let body = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
        let lines = match self.shown.as_ref().map(|j| j.report.lock()) {
            Some(guard) => match guard.as_ref() {
                Some(r) if r.logins.is_empty() => vec![Line::from(Span::styled("  no Claude or Codex logins yet — alt+o to add one", muted(t)))],
                Some(r) => match self.tab {
                    Tab::Tokens => tokens_lines(r, body.width as usize, t),
                    Tab::Profiles => profiles_lines(r, body.width as usize, t),
                },
                None => vec![],
            },
            None => {
                self.draw_loading(f, body, cx);
                return;
            }
        };
        self.scroll = self.scroll.min(lines.len().saturating_sub(body.height as usize));
        for (i, l) in lines.into_iter().skip(self.scroll).take(body.height as usize).enumerate() {
            f.render_widget(l, Rect { y: body.y + i as u16, height: 1, ..body });
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                self.switch(if self.tab == Tab::Tokens { Tab::Profiles } else { Tab::Tokens })
            }
            KeyCode::Char('1') | KeyCode::Char('t') => self.switch(Tab::Tokens),
            KeyCode::Char('2') | KeyCode::Char('p') => self.switch(Tab::Profiles),
            KeyCode::Char('r') if !self.running() => self.start(cx),
            KeyCode::Char('r') => {}
            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, _cx: &mut Cx) {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = Position { x: ev.column, y: ev.row };
                if let Some((_, tab)) = self.hits.iter().find(|(r, _)| r.contains(pos)) {
                    let tab = *tab;
                    self.switch(tab);
                }
            }
            MouseEventKind::ScrollDown => self.scroll += 1,
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
    }
}

/// "1.2 GB", "340 MB".
fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 { format!("{:.1} GB", n / 1e9) } else { format!("{:.0} MB", n / 1e6) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(38_140), "38.1K");
        assert_eq!(compact(999_999), "1.00M");
        assert_eq!(compact(412_000_000), "412M");
        assert_eq!(compact(9_620_345_123), "9.62B");
        assert_eq!(compact(12_000_000_000_000), "12.0T");
    }

    #[test]
    fn sparkline_scales_to_its_max() {
        assert_eq!(spark(&[0, 1, 2, 4]), "·▃▅█");
        assert_eq!(spark(&[0, 0]), "··");
    }

    #[test]
    fn claude_counter_is_topped_up_from_transcripts_after_its_last_day() {
        let now = crate::util::now_secs();
        let today = tokens::local_date(now);
        let days: Vec<NaiveDate> = (0..WINDOW_DAYS).rev().map(|i| today - Days::days(i)).collect();
        let yesterday = today - Days::days(1);
        let mut logs = LogTotals::default();
        // yesterday is in both (the counter wins), today only in the logs
        logs.hours.insert(now - DAY, Tokens { input: 999, ..Default::default() });
        logs.hours.insert(now - now.rem_euclid(3600), Tokens { output: 40, ..Default::default() });
        let counter = ClaudeCounter { lifetime: 1_000, daily: [(yesterday, 7)].into_iter().collect(), sessions: None, through: Some(yesterday) };
        let l = Login { harness: Harness::Claude, name: "local".into(), counter: Some(counter), logs: logs.clone() };
        let (lifetime, window, daily) = token_figures(&l, &days);
        if tokens::local_date(now - DAY) == yesterday {
            assert_eq!(lifetime, (1_040, false));
            assert_eq!(window, 47);
            assert_eq!(&daily[daily.len() - 2..], &[7, 40]);
        }
        // no counter: the logs, as a floor
        let l = Login { harness: Harness::Claude, name: "x".into(), counter: None, logs };
        assert_eq!(token_figures(&l, &days).0, (1_039, true));
    }

    #[test]
    fn mix_bar_fills_its_width() {
        let t = crate::theme::get("graphite");
        let tk = Tokens { input: 1, output: 30, cache_read: 900, cache_write: 69 };
        let w: usize = mix_bar(&tk, 20, &t).iter().map(|s| ui::width(&s.content)).sum();
        assert_eq!(w, 20);
    }
}
