//! The stats view in the desktop app: the two tabs of token totals per login — lifetime and the last 30 days
//! with each login's share and a 30-day sparkline, and the rolling 24h / 7d / 30d with what the 30 days were
//! made of — over the same background scan as the terminal UI (started on first draw, kept on screen while a
//! rescan runs).

use super::{DAY, Lifetime, Report, StatsView, Tab, WINDOW_DAYS, bytes, compact, lifetime_text, token_figures};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Tokens, Ui, brand, size, weight, widgets};
use crate::pane::Cx;
use crate::views::usage::gui::{brand_color, edge_fades, gauge, gauge_fit, hints};
use bro_core::Harness;
use bro_core::usage::tokens::{self, Tokens as Counts};
use bro_gfx::text::Align;
use bro_gfx::{Color, Corners, Point, Rect};
use chrono::{Duration as Days, NaiveDate};
use crossterm::event::KeyModifiers;
use std::sync::atomic::Ordering;

/// The view's background (wheel, focus).
const BODY: usize = 0;
const RESCAN: usize = 1;
/// `TAB + i`: tab chip `i` of [`Tab::ALL`].
const TAB: usize = 10;

const PAD: f32 = 28.0;
const MAX_W: f32 = 880.0;
/// Pixels per step of the view's scroll (j/k move one, the wheel two).
const STEP: f32 = 36.0;
const ROW: f32 = 38.0;
const GROUP: f32 = 40.0;
const NUM: f32 = 104.0;

/// The tokens tab's figures, copied out of the report.
struct TokensTab {
    first: NaiveDate,
    today: NaiveDate,
    lifetime: Lifetime,
    window: u64,
    shares: Vec<(Harness, f64)>,
    groups: Vec<(Harness, Vec<TokRow>, Option<TokRow>)>,
}

struct TokRow {
    name: String,
    lifetime: Lifetime,
    window: u64,
    daily: Option<Vec<u64>>,
}

/// The profiles tab's figures.
struct ProfilesTab {
    totals: [Counts; 3],
    groups: Vec<(Harness, Vec<(String, [Counts; 3])>)>,
}

enum Body {
    /// no report yet: waiting for profiles, or the first read's progress
    Loading,
    NoLogins,
    Tokens(TokensTab),
    Profiles(ProfilesTab),
}

fn tokens_tab(r: &Report) -> TokensTab {
    let today = tokens::local_date(r.at);
    let days: Vec<NaiveDate> = (0..WINDOW_DAYS).rev().map(|i| today - Days::days(i)).collect();
    let figures: Vec<_> = r.logins.iter().map(|l| token_figures(l, &days)).collect();
    let window: u64 = figures.iter().map(|f| f.1).sum();
    let lifetime: Lifetime = (figures.iter().map(|f| f.0.0).sum(), figures.iter().any(|f| f.0.1));
    let mut shares = vec![];
    let mut groups = vec![];
    for h in r.harnesses() {
        let members: Vec<_> = r.logins.iter().zip(&figures).filter(|(l, _)| l.harness == h).collect();
        let n: u64 = members.iter().map(|(_, f)| f.1).sum();
        if window > 0 {
            shares.push((h, n as f64 * 100.0 / window as f64));
        }
        let rows = members.iter().map(|(l, (lt, w, d))| TokRow { name: l.name.clone(), lifetime: *lt, window: *w, daily: Some(d.clone()) }).collect();
        // a single-login provider is its own subtotal
        let sub = (members.len() > 1).then(|| TokRow {
            name: "subtotal".into(),
            lifetime: (members.iter().map(|(_, f)| f.0.0).sum(), members.iter().any(|(_, f)| f.0.1)),
            window: n,
            daily: None,
        });
        groups.push((h, rows, sub));
    }
    TokensTab { first: days[0], today, lifetime, window, shares, groups }
}

fn profiles_tab(r: &Report) -> ProfilesTab {
    let windows = [r.at - DAY, r.at - 7 * DAY, r.at - 30 * DAY];
    let figures: Vec<[Counts; 3]> = r.logins.iter().map(|l| windows.map(|from| l.logs.since(from))).collect();
    let mut totals = [Counts::default(); 3];
    for f in &figures {
        for (t, x) in totals.iter_mut().zip(f) {
            t.add(x);
        }
    }
    let groups = r.harnesses().into_iter().map(|h| (h, r.logins.iter().zip(&figures).filter(|(l, _)| l.harness == h).map(|(l, f)| (l.name.clone(), *f)).collect())).collect();
    ProfilesTab { totals, groups }
}

/// The four kinds of tokens, in the order the mix bar draws them (the terminal UI's colours).
fn kinds(t: &Tokens) -> [(&'static str, Color); 4] {
    [("input", t.shine), ("cache write", Color::hex(0xA78BFA)), ("cache read", Color::hex(0x60A5FA)), ("output", t.good)]
}

/// The scan's state for the status line: (scanning, text).
fn status(v: &StatsView) -> (bool, String) {
    match (&v.job, v.running()) {
        (Some(j), true) => {
            let (done, total) = (j.progress.done.load(Ordering::Relaxed), j.progress.total.load(Ordering::Relaxed));
            (true, if total > 0 { format!("reading logs {} / {}", bytes(done), bytes(total)) } else { "reading logs…".into() })
        }
        _ => match v.shown.as_ref().and_then(|j| j.report.lock().as_ref().map(|r| (r.at, r.took))) {
            Some((at, took)) => {
                let ago = (crate::util::now_secs() - at).max(0) as u64;
                let when = if ago < 60 { "just now".to_string() } else { format!("{} ago", crate::util::short_dur(ago)) };
                (false, format!("logs read {when} in {:.1}s", took.as_secs_f64()))
            }
            None => (false, String::new()),
        },
    }
}

pub(crate) fn draw(v: &mut StatsView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    // the terminal UI's first-render work: start reading the logs once the profiles are in, pick up a finished read
    if v.job.is_none() {
        v.start(cx);
    }
    if let Some(j) = &v.job
        && j.report.lock().is_some()
    {
        v.shown = Some(j.clone());
    }
    let t = ui.t;
    ui.hit_with(area, Action::View(cx.id, BODY), Cursor::Default);
    let page = Rect::new(area.x + PAD, area.y, (area.w - 2.0 * PAD).clamp(0.0, MAX_W), area.h);

    // header: the tabs, the scan's status and a rescan button
    let head = Rect::new(page.x, area.y + 18.0, page.w, 36.0);
    let sel = Tab::ALL.iter().position(|x| *x == v.tab).unwrap_or(0);
    let labels: Vec<&str> = Tab::ALL.iter().map(|x| x.label()).collect();
    widgets::segmented(ui, Rect::new(head.x, head.y + 2.0, 220.0, 32.0), &labels, sel, |i| Action::View(cx.id, TAB + i));
    let (scanning, text) = status(v);
    let rescan = Rect::new(head.right() - 32.0, head.y + 2.0, 32.0, 32.0);
    let dim = if scanning { t.faint.fade(0.6) } else { t.muted };
    if widgets::icon_button(ui, rescan, "\u{F0450}", dim, Action::View(cx.id, RESCAN)) && !scanning {
        widgets::tooltip(ui, rescan, "rescan");
    }
    // the status sits left of the button, never over the tabs
    let st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let room_x = head.x + 220.0 + 16.0 + if scanning { 18.0 } else { 0.0 };
    let tx = (rescan.x - 8.0 - ui.measure(&text, st)).max(room_x);
    if rescan.x - 8.0 - tx > 24.0 {
        ui.label_in(Rect::new(tx, head.y, rescan.x - 8.0 - tx, head.h), &text, st, Align::Left);
        if scanning {
            ui.spinner(Point::new(tx - 14.0, head.y + head.h / 2.0), 4.6, 1.6, t.accent);
        }
    }

    let foot_y = area.bottom() - 30.0;
    hints(ui, Point::new(page.x, foot_y), &[("tab", "switch"), ("r", "rescan"), ("1/2", "tokens / profiles"), ("j/k", "scroll")]);
    let body = Rect::new(page.x, head.bottom() + 26.0, page.w, (foot_y - 14.0 - head.bottom() - 26.0).max(0.0));

    let data = match v.shown.as_ref().map(|j| j.report.lock()) {
        Some(guard) => match guard.as_ref() {
            Some(r) if r.logins.is_empty() => Body::NoLogins,
            Some(r) => match v.tab {
                Tab::Tokens => Body::Tokens(tokens_tab(r)),
                Tab::Profiles => Body::Profiles(profiles_tab(r)),
            },
            None => Body::Loading,
        },
        None => Body::Loading,
    };
    match data {
        Body::Loading => loading(v, ui, body),
        Body::NoLogins => {
            ui.icon(Rect::new(body.x, body.y, 18.0, 24.0), "\u{F0004}", size::TITLE, t.faint);
            ui.label_in(Rect::new(body.x + 28.0, body.y, body.w - 28.0, 24.0), "no Claude or Codex logins yet — alt+o to add one", Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Left);
        }
        Body::Tokens(d) => scrolled(v, ui, body, cx, |ui, top, w, draw| draw_tokens(ui, &d, body.x, top, w, draw)),
        Body::Profiles(d) => scrolled(v, ui, body, cx, |ui, top, w, draw| draw_profiles(ui, &d, body.x, top, w, draw)),
    }
    true
}

/// Lay the content out once to measure it (`draw` false), clamp the view's scroll to it, then draw it scrolled.
fn scrolled(v: &mut StatsView, ui: &mut Ui, body: Rect, cx: &Cx, f: impl Fn(&mut Ui, f32, f32, bool) -> f32) {
    let content = f(ui, 0.0, body.w, false);
    let max_off = (content - body.h).max(0.0);
    let max_steps = (max_off / STEP).ceil() as usize;
    v.scroll = v.scroll.min(max_steps);
    let off = ui.ease(("stats-scroll", cx.id, v.tab == Tab::Tokens), (v.scroll as f32 * STEP).min(max_off), 160.0);
    let view = Rect::new(body.x - 12.0, body.y - 6.0, body.w + 24.0, body.h + 6.0);
    ui.clip(view, 0.0);
    f(ui, body.y - off, body.w, true);
    ui.unclip();
    edge_fades(ui, view, content + 6.0, off);
    let vis = ui.ease(("stats-bar", cx.id), if ui.hovered(body) { 1.0 } else { 0.0 }, 200.0);
    widgets::scrollbar(ui, Rect::new(body.x, body.y, body.w + 16.0, body.h), content, body.h, off, vis);
}

/// The first read: a gauge of the bytes still to read (or the wait for the profiles).
fn loading(v: &StatsView, ui: &mut Ui, body: Rect) {
    let t = ui.t;
    let w = (body.w - 64.0).clamp(160.0, 420.0);
    let x = body.x + (body.w - w) / 2.0;
    let y = body.y + (body.h - 110.0).max(0.0) / 3.0;
    // the icon stands on the page (no tile behind it)
    let icon = Rect::new(x - 2.0, y, 26.0, 36.0);
    let Some(j) = v.job.as_ref() else {
        ui.icon(icon, "\u{F0128}", 22.0, t.faint);
        ui.label_in(Rect::new(icon.right() + 10.0, y, w - 36.0, 36.0), "waiting for your logins…", Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Left);
        return;
    };
    let (done, total) = (j.progress.done.load(Ordering::Relaxed), j.progress.total.load(Ordering::Relaxed));
    ui.icon(icon, "\u{F0128}", 22.0, t.accent);
    ui.label_in(Rect::new(icon.right() + 10.0, y, w - 36.0, 36.0), "tallying every token", Style::new(size::HEADING, weight::SEMIBOLD, t.text), Align::Left);
    let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
    widgets::meter(ui, Rect::new(x, y + 56.0, w, 6.0), frac as f32, t.accent);
    let left = if total > 0 { format!("{} of {}", bytes(done), bytes(total)) } else { "finding logs…".into() };
    let row = Rect::new(x, y + 72.0, w, 18.0);
    ui.label_in(row, &left, Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Left);
    ui.label_in(row, "only new logs next time", Style::new(size::SMALL, weight::REGULAR, t.faint), Align::Right);
    // the gauge moves with the read
    ui.ticking = true;
}

/// One big figure of a headline: the value, its caption (after a logo, for a provider's share), and whether a
/// hairline sets it apart from the ones before it.
struct Figure {
    value: String,
    caption: Vec<(String, Style)>,
    mark: Option<brand::Mark>,
    apart: bool,
}

/// A row of big figures with their captions under them, smaller when they don't fit in `w`.
fn headline(ui: &mut Ui, at: Point, w: f32, figures: &[Figure], draw: bool) {
    let t = ui.t;
    // lays the row out at a size (drawing it when asked); returns its width
    let lay = |ui: &mut Ui, big: f32, gap: f32, draw: bool| -> f32 {
        let mut x = at.x;
        for (i, f) in figures.iter().enumerate() {
            if i > 0 {
                x += if f.apart { gap * 2.0 } else { gap };
            }
            if f.apart && i > 0 && draw {
                ui.vline(x - gap, at.y + 6.0, big * 1.6, t.hairline);
            }
            let vb = ui.line(&f.value, Style::new(big, weight::SEMIBOLD, t.text), None);
            let cb = ui.rich(&f.caption, size::SMALL, None, false, Some(1));
            let mark_w = if f.mark.is_some() { 18.0 } else { 0.0 };
            if draw {
                ui.draw(&vb, Point::new(x - 1.0, at.y));
                let cy = at.y + vb.h + 2.0;
                if let Some(m) = f.mark {
                    ui.mark(m, Rect::new(x, cy, 13.0, cb.h), 13.0, None);
                }
                ui.draw(&cb, Point::new(x + mark_w, cy));
            }
            x += vb.w.max(mark_w + cb.w);
        }
        x - at.x
    };
    if draw {
        let (big, gap) = if lay(ui, 34.0, 44.0, false) <= w { (34.0, 44.0) } else { (24.0, 28.0) };
        lay(ui, big, gap, true);
    }
}

/// "last 30d" → "Last 30d": column labels are sentence case, never caps.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Column labels in sentence case, the first left-aligned, the rest right-aligned at their right edges.
fn col_head(ui: &mut Ui, y: f32, cols: &[(Rect, &str, Align)], draw: bool) {
    if !draw {
        return;
    }
    let st = Style::new(size::SMALL, weight::SEMIBOLD, ui.t.muted);
    for (r, text, align) in cols {
        ui.label_in(Rect::new(r.x, y, r.w, 20.0), &sentence(text), st, *align);
    }
}

/// A provider's heading: its logo and name.
fn group_head(ui: &mut Ui, x: f32, y: f32, h: Harness, draw: bool) {
    if !draw {
        return;
    }
    let t = ui.t;
    let r = Rect::new(x, y + GROUP - 28.0, 22.0, 22.0);
    ui.mark(brand::harness(h), r, 16.0, None);
    ui.label_in(Rect::new(r.right() + 10.0, r.y, 200.0, r.h), h.label(), Style::new(size::BODY, weight::SEMIBOLD, t.text), Align::Left);
}

/// The tokens tab; returns its height.
fn draw_tokens(ui: &mut Ui, d: &TokensTab, x: f32, top: f32, w: f32, draw: bool) -> f32 {
    let t = ui.t;
    let mut y = top;
    // headline: the two totals and the provider split
    let caption = |s: String| vec![(s, Style::new(size::SMALL, weight::MEDIUM, t.muted))];
    let mut figures = vec![
        Figure { value: lifetime_text(d.lifetime), caption: caption("lifetime".into()), mark: None, apart: false },
        Figure { value: compact(d.window), caption: caption(format!("last {WINDOW_DAYS} days")), mark: None, apart: false },
    ];
    for (k, (h, pct)) in d.shares.iter().enumerate() {
        figures.push(Figure { value: format!("{pct:.0}%"), caption: caption(h.label().to_string()), mark: Some(brand::harness(*h)), apart: k == 0 });
    }
    headline(ui, Point::new(x, y), w, &figures, draw);
    y += 98.0;

    // columns: login · lifetime · last 30d · share · the 30 days (the sparkline goes first, then the share)
    let num = if w < 640.0 { 84.0 } else { NUM };
    let rest = w - 150.0 - 2.0 * num;
    let days = WINDOW_DAYS as f32;
    let spark_w = if rest >= 24.0 + 150.0 + 28.0 + days * 4.0 { (rest - 24.0 - 188.0 - 28.0).clamp(days * 4.0, days * 6.0) } else { 0.0 };
    let share_w = (rest - 24.0 - if spark_w > 0.0 { 28.0 + spark_w } else { 0.0 }).min(188.0);
    let show_spark = spark_w > 0.0;
    let show_share = share_w >= 110.0;
    let name_w = w - 2.0 * num - if show_share { 24.0 + share_w } else { 0.0 } - if show_spark { 28.0 + spark_w } else { 0.0 };
    let c_name = Rect::new(x, 0.0, name_w, 0.0);
    let c_life = Rect::new(c_name.right(), 0.0, num, 0.0);
    let c_win = Rect::new(c_life.right(), 0.0, num, 0.0);
    let c_share = Rect::new(c_win.right() + 24.0, 0.0, share_w, 0.0);
    let c_spark = Rect::new(c_share.right() + 28.0, 0.0, spark_w, 0.0);
    let mut cols = vec![(c_name, "login", Align::Left), (c_life, "lifetime", Align::Right), (c_win, "last 30d", Align::Right)];
    if show_share {
        cols.push((c_share, "share", Align::Left));
    }
    col_head(ui, y, &cols, draw);
    if show_spark && draw {
        // the sparkline's own axis: first day … today
        let st = Style::new(size::SMALL, weight::SEMIBOLD, t.muted);
        let a = ui.label_in(Rect::new(c_spark.x, y, c_spark.w, 20.0), &d.first.format("%b %-d").to_string(), st, Align::Left);
        let b = ui.label_in(Rect::new(c_spark.x, y, c_spark.w, 20.0), &d.today.format("%b %-d").to_string(), st, Align::Right);
        let (lx, rx) = (c_spark.x + a.w + 8.0, c_spark.right() - b.w - 8.0);
        if rx > lx {
            ui.hline(lx, y + 10.0, rx - lx, t.hairline);
        }
    }
    y += 26.0;
    if draw {
        ui.hline(x, y, w, t.hairline);
    }
    y += 2.0;

    let row = |ui: &mut Ui, y: f32, r: &TokRow, color: Color, kind: RowKind| {
        if !draw {
            return;
        }
        let t = ui.t;
        let (name_st, num_w) = match kind {
            RowKind::Login => (Style::new(size::BODY, weight::MEDIUM, t.text), weight::MEDIUM),
            RowKind::Subtotal => (Style { italic: true, ..Style::new(size::BODY, weight::REGULAR, t.muted) }, weight::MEDIUM),
            RowKind::Total => (Style::new(size::BODY, weight::BOLD, t.text), weight::BOLD),
        };
        let indent = if kind == RowKind::Total { 0.0 } else { 32.0 };
        ui.label_in(Rect::new(c_name.x + indent, y, c_name.w - indent - 8.0, ROW), &r.name, name_st, Align::Left);
        // a floor (counted from the logs still on disk) reads quieter
        let life_c = if r.lifetime.1 { t.text.mix(t.muted, 0.5) } else { t.text };
        ui.label_in(Rect::new(c_life.x, y, c_life.w, ROW), &lifetime_text(r.lifetime), Style::new(size::BODY, num_w, life_c), Align::Right);
        ui.label_in(Rect::new(c_win.x, y, c_win.w, ROW), &compact(r.window), Style::new(size::BODY, num_w, t.shine), Align::Right);
        if show_share {
            // the login's share of the tokens, as the usage gauges: little segments
            let frac = if d.window > 0 { r.window as f64 / d.window as f64 } else { 0.0 };
            let bar = gauge_fit(ui, Rect::new(c_share.x, y + ROW / 2.0 - 3.0, c_share.w - 52.0, 6.0));
            gauge(ui, bar, frac as f32, color);
            ui.label_in(Rect::new(c_share.right() - 52.0, y, 52.0, ROW), &format!("{:.0}%", frac * 100.0), Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Right);
        }
        if show_spark && let Some(daily) = &r.daily {
            spark(ui, Rect::new(c_spark.x, y + 7.0, c_spark.w, ROW - 13.0), daily, color.mix(t.muted, 0.25));
        }
    };
    for (h, rows, sub) in &d.groups {
        group_head(ui, x, y, *h, draw);
        y += GROUP;
        for r in rows {
            row(ui, y, r, brand_color(*h, t), RowKind::Login);
            y += ROW;
        }
        if let Some(s) = sub {
            row(ui, y, s, brand_color(*h, t), RowKind::Subtotal);
            y += ROW;
        }
    }
    y += 8.0;
    if draw {
        ui.hline(x, y, w, t.border);
    }
    y += 4.0;
    row(ui, y, &TokRow { name: "total".into(), lifetime: d.lifetime, window: d.window, daily: None }, t.accent, RowKind::Total);
    y += ROW + 10.0;
    if d.lifetime.1 {
        y += 8.0;
        if draw {
            let spans = vec![("≥  ".to_string(), Style::new(size::SMALL, weight::BOLD, t.text)), ("at least — counted from the logs still on disk".to_string(), Style::new(size::SMALL, weight::REGULAR, t.muted))];
            let b = ui.rich(&spans, size::SMALL, Some(w), false, Some(1));
            ui.draw(&b, Point::new(x, y));
        }
        y += 20.0;
    }
    y + 8.0 - top
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Login,
    Subtotal,
    Total,
}

/// A sparkline of thin bars (softly squared, like the gauges' segments) scaled to their own max; zero days are a
/// dot.
fn spark(ui: &mut Ui, r: Rect, values: &[u64], c: Color) {
    let t = ui.t;
    let max = values.iter().copied().max().unwrap_or(0);
    let n = values.len().max(1) as f32;
    let pitch = r.w / n;
    let bw = (pitch - 2.0).clamp(2.0, 6.0);
    for (i, &v) in values.iter().enumerate() {
        let cx = r.x + pitch * i as f32 + pitch / 2.0;
        if v == 0 || max == 0 {
            ui.circle(Point::new(cx, r.bottom() - 1.0), 1.1, t.faint.fade(0.7));
            continue;
        }
        let h = (2.0 + (v as f32 / max as f32) * (r.h - 2.0)).min(r.h);
        // today's bar in full colour
        let fill = if i + 1 == values.len() { c.mix(t.text, 0.15) } else { c };
        ui.round(Rect::new(cx - bw / 2.0, r.bottom() - h, bw, h), (bw / 2.0).min(1.5), fill);
    }
}

/// The profiles tab; returns its height.
fn draw_profiles(ui: &mut Ui, d: &ProfilesTab, x: f32, top: f32, w: f32, draw: bool) -> f32 {
    let t = ui.t;
    let mut y = top;
    let figures: Vec<Figure> = ["last 24h", "last 7d", "last 30d"]
        .iter()
        .enumerate()
        .map(|(i, label)| Figure { value: compact(d.totals[i].total()), caption: vec![(label.to_string(), Style::new(size::SMALL, weight::MEDIUM, t.muted))], mark: None, apart: false })
        .collect();
    headline(ui, Point::new(x, y), w, &figures, draw);
    y += 98.0;

    let num = if w < 640.0 { 84.0 } else { NUM };
    let mix_w = (w - 130.0 - 3.0 * num - 28.0).min(280.0);
    let show_mix = mix_w >= 80.0;
    let name_w = w - 3.0 * num - if show_mix { 28.0 + mix_w } else { 0.0 };
    let c_name = Rect::new(x, 0.0, name_w, 0.0);
    let c_num = |i: usize| Rect::new(x + name_w + i as f32 * num, 0.0, num, 0.0);
    let c_mix = Rect::new(c_num(2).right() + 28.0, 0.0, mix_w, 0.0);
    let mut cols = vec![(c_name, "login", Align::Left), (c_num(0), "24h", Align::Right), (c_num(1), "7d", Align::Right), (c_num(2), "30d", Align::Right)];
    if show_mix {
        cols.push((c_mix, "30d mix", Align::Left));
    }
    col_head(ui, y, &cols, draw);
    y += 26.0;
    if draw {
        ui.hline(x, y, w, t.hairline);
    }
    y += 2.0;

    let row = |ui: &mut Ui, y: f32, name: &str, f: &[Counts; 3], strong: bool| {
        if !draw {
            return;
        }
        let t = ui.t;
        let wt = if strong { weight::BOLD } else { weight::MEDIUM };
        let indent = if strong { 0.0 } else { 32.0 };
        ui.label_in(Rect::new(c_name.x + indent, y, c_name.w - indent - 8.0, ROW), name, Style::new(size::BODY, wt, t.text), Align::Left);
        for (i, k) in f.iter().enumerate() {
            let (text, c) = match k.total() {
                0 => ("·".to_string(), t.faint),
                n => (compact(n), if i == 0 { t.shine } else { t.text }),
            };
            ui.label_in(Rect::new(c_num(i).x, y, num, ROW), &text, Style::new(size::BODY, wt, c), Align::Right);
        }
        if show_mix {
            mix_bar(ui, Rect::new(c_mix.x, y + ROW / 2.0 - 4.0, c_mix.w, 8.0), &f[2]);
        }
    };
    for (h, members) in &d.groups {
        group_head(ui, x, y, *h, draw);
        y += GROUP;
        for (name, f) in members {
            row(ui, y, name, f, false);
            y += ROW;
        }
    }
    y += 8.0;
    if draw {
        ui.hline(x, y, w, t.border);
    }
    y += 4.0;
    row(ui, y, "total", &d.totals, true);
    y += ROW + 18.0;

    // the legend, and what the 30 days were made of
    if show_mix {
        let mut lx = x;
        for (name, c) in kinds(t) {
            if draw {
                ui.round(Rect::new(lx, y + 4.0, 10.0, 10.0), 2.0, c);
            }
            let b = ui.line(name, Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
            if draw {
                ui.draw(&b, Point::new(lx + 16.0, y + 9.0 - b.h / 2.0));
            }
            lx += 16.0 + b.w + 22.0;
        }
        y += 28.0;
    }
    let mix = &d.totals[2];
    if mix.total() > 0 {
        let pct = |n: u64| n as f64 * 100.0 / mix.total() as f64;
        let spans = vec![
            (format!("{:.0}%", pct(mix.cache_read)), Style::new(size::SMALL, weight::BOLD, kinds(t)[2].1)),
            (" of the last 30 days was re-read from cache  ·  ".to_string(), Style::new(size::SMALL, weight::REGULAR, t.muted)),
            (format!("{:.1}%", pct(mix.output)), Style::new(size::SMALL, weight::BOLD, t.good)),
            (" was written by the model".to_string(), Style::new(size::SMALL, weight::REGULAR, t.muted)),
        ];
        let b = ui.rich(&spans, size::SMALL, Some(w), true, Some(2));
        if draw {
            ui.draw(&b, Point::new(x, y));
        }
        y += b.h;
    }
    y + 12.0 - top
}

/// One stacked bar of input / cache write / cache read / output (2px corners at its ends); every part above
/// zero keeps a sliver.
fn mix_bar(ui: &mut Ui, r: Rect, tk: &Counts) {
    let t = ui.t;
    let total = tk.total();
    let rad = (r.h / 2.0).min(2.0);
    ui.round(r, rad, t.text.with_alpha(0.08));
    if total == 0 {
        return;
    }
    let parts = [tk.input, tk.cache_write, tk.cache_read, tk.output];
    let gap = 2.0;
    let shown: Vec<(f32, Color)> = parts.iter().zip(kinds(t)).filter(|(p, _)| **p > 0).map(|(p, (_, c))| (*p as f32 / total as f32, c)).collect();
    let room = r.w - gap * (shown.len().saturating_sub(1)) as f32;
    let min = 3.0;
    // slivers first, the rest shared by size
    let small: f32 = shown.iter().map(|(f, _)| if f * room < min { min } else { 0.0 }).sum();
    let big: f32 = shown.iter().filter(|(f, _)| f * room >= min).map(|(f, _)| *f).sum();
    let mut x = r.x;
    let n = shown.len();
    for (i, (f, c)) in shown.into_iter().enumerate() {
        let w = if f * room < min { min } else { f / big.max(1e-6) * (room - small) };
        let w = if i + 1 == n { r.right() - x } else { w };
        let corners = match (i == 0, i + 1 == n) {
            (true, true) => Corners::all(rad),
            (true, false) => Corners::left(rad),
            (false, true) => Corners::right(rad),
            (false, false) => Corners::ZERO,
        };
        ui.round_corners(Rect::new(x, r.y, w.max(1.0), r.h), corners, c);
        x += w + gap;
    }
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut StatsView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    match n {
        RESCAN if !v.running() => v.start(cx),
        n if (TAB..TAB + Tab::ALL.len()).contains(&n) => v.switch(Tab::ALL[n - TAB]),
        _ => {}
    }
}

/// The wheel over target `n` (None: elsewhere in the view): two steps a notch; drawing clamps it.
pub(crate) fn wheel(v: &mut StatsView, _n: Option<usize>, down: bool, _cx: &mut Cx) {
    if down {
        v.scroll += 2;
    } else {
        v.scroll = v.scroll.saturating_sub(2);
    }
}
