//! The proxy view in the desktop app: where the proxy listens, request compression with its switch, the
//! routes launches registered, the Claude pool's accounts, and the live request log (newest first, scrolled
//! by the view's own `scroll` like the terminal UI's). Narrow panes fold each row onto two lines.

use super::{ProxyView, hms, saved, tokens, upstream_label};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Tokens, Ui, brand, radius, size, weight, widgets};
use crate::pane::Cx;
use crate::services::{Avail, ProxyInfo};
use crate::views::usage::gui::{card, chip_fill, edge_fades, hints, icon_text_button, icon_text_button_width, status};
use bro_core::config::ProxySettings;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use bro_proxy::{PoolAccountStatus, ProxyEvent, Route, Upstream};
use crossterm::event::KeyModifiers;

/// The view's background (wheel, focus).
const BODY: usize = 0;
/// The compression switch.
const TRIM: usize = 1;
/// Back to the newest request.
const NEWEST: usize = 2;

const PAD: f32 = 28.0;
const MAX_W: f32 = 1240.0;
/// Narrower than this, rows take two lines.
const NARROW: f32 = 820.0;
/// Narrower than this, the live log's rows take two lines: the table needs this much room for every column the
/// terminal UI shows (in, out, cache, saved), and dropping some would hide them.
const LOG_NARROW: f32 = 980.0;

/// Everything but the log, copied out of the shared state (no lock is held while drawing).
struct Snap {
    status: Result<ProxyInfo, String>,
    /// the first six (as the terminal UI lists them)
    routes: Vec<Route>,
    routes_all: usize,
    pool: Vec<PoolAccountStatus>,
    events: usize,
    settings: ProxySettings,
    /// (requests compression ran on, estimated tokens it saved) in the recent log
    trimmed: (usize, u64),
}

fn snapshot(cx: &Cx) -> Snap {
    let st = cx.svc.state();
    let p = &st.proxy;
    let mut trimmed = (0usize, 0u64);
    for e in p.events.iter().filter(|e| e.trim_tokens_before.is_some()) {
        trimmed.0 += 1;
        trimmed.1 += e.trim_saved();
    }
    Snap {
        status: match &p.status {
            Avail::Ready(i) => Ok(i.clone()),
            other => Err(other.why()),
        },
        routes: p.routes.iter().take(6).cloned().collect(),
        routes_all: p.routes.len(),
        pool: p.pool.iter().take(6).cloned().collect(),
        events: p.events.len(),
        settings: st.settings.proxy.clone(),
        trimmed,
    }
}

/// The log rows `from..from + n`, newest first.
fn log_rows(cx: &Cx, from: usize, n: usize) -> Vec<ProxyEvent> {
    cx.svc.state().proxy.events.iter().rev().skip(from).take(n).cloned().collect()
}

/// A status code's colour, and whether it's loud (429 and server errors).
fn status_color(code: u16, t: &Tokens) -> (Color, bool) {
    match code {
        200..=299 => (t.good, false),
        429 => (t.inline, true),
        400..=499 => (t.inline, false),
        _ => (t.danger, true),
    }
}

fn latency_color(ms: u64, t: &Tokens) -> Color {
    if ms < 2_000 {
        t.good
    } else if ms < 8_000 {
        t.inline
    } else {
        t.danger
    }
}

/// The note column: the error, the pool account, or "stream".
fn note(e: &ProxyEvent) -> String {
    e.error.clone().or_else(|| e.account.as_ref().map(|a| format!("acct {a}"))).unwrap_or_else(|| if e.stream { "stream".into() } else { String::new() })
}

pub(crate) fn draw(v: &mut ProxyView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let s = snapshot(cx);
    ui.hit_with(area, Action::View(cx.id, BODY), Cursor::Default);
    let page = Rect::new(area.x + PAD, area.y, (area.w - 2.0 * PAD).clamp(0.0, MAX_W), area.h);
    let narrow = page.w < NARROW;

    // header: where it listens, routes and requests seen
    let head = Rect::new(page.x, area.y + 18.0, page.w, 36.0);
    header(ui, head, &s);

    let foot_y = area.bottom() - 30.0;
    hints(ui, Point::new(page.x, foot_y), &[("j/k", "scroll log"), ("g", "newest"), ("c", "compression on/off")]);

    // compression
    let mut y = head.bottom() + 20.0;
    y = compression(ui, Rect::new(page.x, y, page.w, 0.0), &s, cx) + 24.0;

    // routes
    let line_h = if narrow { 46.0 } else { 30.0 };
    widgets::section(ui, Rect::new(page.x, y, page.w, 20.0), "Routes");
    y += 28.0;
    if s.routes.is_empty() {
        let st = Style::new(size::SMALL, weight::REGULAR, t.muted);
        let b = ui.para("none yet — launching a harness on a foreign provider (e.g. claude on gpt, codex on claude) adds one", st, page.w, Some(3));
        ui.draw(&b, Point::new(page.x, y + 6.0));
        y += b.h + 12.0;
    }
    for r in &s.routes {
        route_row(ui, Rect::new(page.x, y, page.w, line_h), r, narrow);
        y += line_h;
    }

    // the claude pool
    if !s.pool.is_empty() {
        y += 18.0;
        widgets::section(ui, Rect::new(page.x, y, page.w, 20.0), "Claude pool");
        y += 28.0;
        let now = crate::util::now_ms();
        for a in &s.pool {
            pool_row(ui, Rect::new(page.x, y, page.w, line_h), a, now, narrow);
            y += line_h;
        }
    }

    // live requests
    y += 22.0;
    let log_bottom = foot_y - 18.0;
    if y + 60.0 > log_bottom {
        return true;
    }
    let sec = Rect::new(page.x, y, page.w, 20.0);
    if v.scroll > 0 {
        let label = "newest";
        let bw = icon_text_button_width(ui, label) - 6.0;
        let br = Rect::new(sec.right() - bw, sec.y - 6.0, bw, 30.0);
        icon_text_button(ui, br, "\u{F005D}", label, widgets::Kind::Ghost, Action::View(cx.id, NEWEST));
        widgets::section(ui, Rect { w: br.x - 12.0 - sec.x, ..sec }, "Live requests");
    } else {
        widgets::section(ui, sec, "Live requests");
    }
    y += 30.0;
    let fold = page.w < LOG_NARROW;
    let cols = Cols::new(page.x, page.w);
    if !fold {
        cols.head(ui, y);
        y += 26.0;
        ui.hline(page.x, y, page.w, t.hairline);
        y += 2.0;
    }
    let row_h = if fold { 50.0 } else { 32.0 };
    let view = Rect::new(page.x, y, page.w, (log_bottom - y).max(0.0));
    if s.events == 0 {
        ui.label_in(Rect::new(page.x + 6.0, y, page.w, row_h), "no requests yet", Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
        return true;
    }
    // scrolled back `scroll` rows, never past the oldest
    let room = (view.h / row_h).floor().max(1.0) as usize;
    v.scroll = v.scroll.min(s.events.saturating_sub(room));
    let off = ui.ease(("proxy-scroll", cx.id, fold), v.scroll as f32 * row_h, 140.0);
    let first = (off / row_h).floor() as usize;
    let rows = log_rows(cx, first, room + 2);
    ui.clip(view, 0.0);
    for (k, e) in rows.iter().enumerate() {
        let i = first + k;
        let r = Rect::new(page.x, view.y + i as f32 * row_h - off, page.w, row_h);
        if r.y > view.bottom() {
            break;
        }
        if fold {
            folded_row(ui, r, e, i);
        } else {
            cols.row(ui, r, e, i);
        }
    }
    ui.unclip();
    edge_fades(ui, view, s.events as f32 * row_h, off);
    let vis = ui.ease(("proxy-bar", cx.id), if ui.hovered(view) { 1.0 } else { 0.0 }, 200.0);
    widgets::scrollbar(ui, Rect::new(view.x, view.y, view.w + 14.0, view.h), s.events as f32 * row_h, view.h, off, vis);
    true
}

fn header(ui: &mut Ui, head: Rect, s: &Snap) {
    let t = ui.t;
    let tb = ui.line("Proxy", Style::new(20.0, weight::SEMIBOLD, t.text), None);
    ui.draw(&tb, Point::new(head.x, head.y + (head.h - tb.h) / 2.0));
    let x = head.x + tb.w + 16.0;
    let mid = head.y + head.h / 2.0;
    match &s.status {
        Ok(i) => {
            // listening: a check and the port, then the base URL and what went through
            let port = format!(":{}", i.port);
            let mut x = status(ui, Point::new(x, mid), "\u{F05E0}", t.good, &port, Style::new(size::BODY, weight::SEMIBOLD, t.text), head.right()) + 16.0;
            let seen = format!("{} routes · {} requests seen", s.routes_all, s.events);
            let seen_st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
            let sw = ui.measure(&seen, seen_st);
            // the URL gives way first
            let base_room = head.right() - x - sw - 16.0;
            if base_room > 60.0 {
                let base = ui.line(&i.base, Style::mono(size::SMALL, t.muted), Some(base_room));
                ui.draw(&base, Point::new(x, mid - base.h / 2.0));
                x += base.w + 16.0;
            }
            ui.label_in(Rect::new(x, head.y, (head.right() - x).max(1.0), head.h), &seen, seen_st, Align::Left);
        }
        Err(why) => {
            status(ui, Point::new(x, mid), "\u{F0028}", t.danger, why, Style::new(size::SMALL, weight::MEDIUM, t.muted), head.right());
        }
    }
}

/// The compression card (a WinUI settings card): the title with its level, what it saved, and the switch with
/// "On"/"Off" beside it, as WinUI's toggle. Returns its bottom.
fn compression(ui: &mut Ui, at: Rect, s: &Snap, cx: &Cx) -> f32 {
    let t = ui.t;
    let on = s.settings.trim;
    let x = at.x + 16.0;
    let state_st = Style::new(size::BODY, weight::REGULAR, t.text);
    let state_w = ui.measure("Off", state_st).max(ui.measure("On", state_st));
    let text_w = at.w - 32.0 - widgets::SWITCH_W - 12.0 - state_w - 24.0;
    let detail = if on {
        let routed = if s.settings.trim_route_native { "native logins launched from now on go through the proxy" } else { "proxied launches only" };
        format!("~{} tokens saved in {} requests (estimate) · {routed}", tokens(Some(s.trimmed.1)), s.trimmed.0)
    } else {
        "c turns on llmtrim (lossless; small savings on Claude Code, whose history is cached)".to_string()
    };
    let db = ui.para(&detail, Style::new(size::SMALL, weight::REGULAR, t.muted), text_w.max(40.0), Some(3));
    let r = Rect::new(at.x, at.y, at.w, 16.0 + 22.0 + 2.0 + db.h + 16.0);
    card(ui, r, 0.0, false);
    let line1 = Rect::new(x, r.y + 16.0, text_w, 22.0);
    let tb = ui.label_in(line1, "Compression", Style::new(size::TITLE, weight::SEMIBOLD, t.text), Align::Left);
    if on {
        let level = bro_proxy::TrimLevel::parse(&s.settings.trim_level).as_str();
        widgets::pill(ui, Point::new(line1.x + tb.w + 10.0, line1.y + 1.0), 20.0, level, t.inline, chip_fill(t));
    }
    ui.draw(&db, Point::new(x, line1.bottom() + 2.0));
    let sw = Point::new(r.right() - 16.0 - widgets::SWITCH_W, r.y + (r.h - widgets::SWITCH_H) / 2.0);
    widgets::switch(ui, sw, on, Action::View(cx.id, TRIM));
    ui.label_in(Rect::new(sw.x - 12.0 - state_w, r.y, state_w, r.h), if on { "On" } else { "Off" }, state_st, Align::Right);
    r.bottom()
}

/// A quiet hover behind rows you read across.
fn row_hover(ui: &mut Ui, key: impl std::hash::Hash, r: Rect) {
    let hot = ui.hover_amount(key, r);
    if hot > 0.0 {
        ui.round(r, radius::CONTROL, ui.t.hover.fade(hot));
    }
}

fn route_row(ui: &mut Ui, r: Rect, route: &Route, narrow: bool) {
    let t = ui.t;
    row_hover(ui, ("proxy-route", &route.id), r);
    let model = route.default_model.clone().unwrap_or_else(|| "client's model".into());
    let mc = if route.default_model.is_some() { t.inline } else { t.faint };
    let id_st = Style::mono(size::SMALL, t.shine);
    let label_st = Style::new(size::BODY, weight::MEDIUM, t.text);
    let up_st = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let model_st = Style::new(size::SMALL, weight::MEDIUM, mc);
    let id_w = 72.0;
    if narrow {
        // id · label … model, the upstream under the label
        let l1 = Rect::new(r.x, r.y + 4.0, r.w, 20.0);
        let l2 = Rect::new(r.x + id_w, r.y + 24.0, r.w - id_w - 8.0, 18.0);
        let mb = ui.label_in(Rect::new(r.x, l1.y, r.w - 8.0, l1.h), &model, model_st, Align::Right);
        ui.label_in(Rect::new(r.x + 6.0, l1.y, id_w - 10.0, l1.h), &route.id, id_st, Align::Left);
        ui.label_in(Rect::new(r.x + id_w, l1.y, (r.w - id_w - mb.w - 24.0).max(1.0), l1.h), &route.label, label_st, Align::Left);
        upstream_cell(ui, l2, &route.upstream, up_st);
        return;
    }
    let w = r.w;
    let model_w = (w * 0.24).clamp(120.0, 260.0);
    let rest = w - id_w - model_w - 16.0;
    let label_w = rest * 0.5;
    ui.label_in(Rect::new(r.x + 6.0, r.y, id_w - 10.0, r.h), &route.id, id_st, Align::Left);
    ui.label_in(Rect::new(r.x + id_w, r.y, label_w - 12.0, r.h), &route.label, label_st, Align::Left);
    upstream_cell(ui, Rect::new(r.x + id_w + label_w, r.y, rest - label_w - 12.0, r.h), &route.upstream, up_st);
    ui.label_in(Rect::new(r.right() - model_w - 8.0, r.y, model_w, r.h), &model, model_st, Align::Right);
}

/// The upstream's real logo, when it has one: Claude's for Claude logins and the pool, OpenAI's for the ChatGPT
/// backend, Gemini's for Google's, else the one its base URL names (OpenRouter, DeepSeek, Anthropic…).
fn upstream_mark(u: &Upstream) -> Option<brand::Mark> {
    match u {
        Upstream::ClaudePool { .. } | Upstream::ClaudeOAuth { .. } => Some(brand::Mark::Logo(brand::Logo::Claude)),
        Upstream::ChatGptCodex { .. } => Some(brand::Mark::Logo(brand::Logo::OpenAi)),
        Upstream::GeminiCodeAssist { .. } => Some(brand::Mark::Logo(brand::Logo::Gemini)),
        Upstream::OpenAiChat { base_url, .. } | Upstream::OpenAiResponses { base_url, .. } | Upstream::Anthropic { base_url, .. } => brand::provider(base_url),
    }
}

/// Where a route goes: the upstream's logo (a quiet cloud when it has none), then the upstream in words.
fn upstream_cell(ui: &mut Ui, r: Rect, u: &Upstream, st: Style) {
    let mark = Rect::new(r.x, r.y, 14.0, r.h);
    match upstream_mark(u) {
        Some(m) => ui.mark(m, mark, 14.0, None),
        None => ui.icon(mark, "\u{F0163}", size::TITLE, ui.t.faint),
    }
    ui.label_in(Rect::new(r.x + 22.0, r.y, (r.w - 22.0).max(1.0), r.h), &upstream_label(u), st, Align::Left);
}

fn pool_row(ui: &mut Ui, r: Rect, a: &PoolAccountStatus, now: i64, narrow: bool) {
    let t = ui.t;
    row_hover(ui, ("proxy-pool", &a.name), r);
    let cooling = a.rate_limited_until_ms.filter(|until| *until > now);
    let (c, state) = match (a.available, cooling) {
        (_, Some(until)) => (t.inline, format!("cooling {}", crate::util::countdown((until - now) / 1000))),
        (true, None) => (t.good, "ready".to_string()),
        (false, None) => (t.danger, "unavailable".to_string()),
    };
    let u = &a.usage;
    let figures = format!(
        "5h  {} req  ·  {} in  ·  {} out  ·  ${:.2}",
        u.window_requests,
        tokens(Some(u.window_input_tokens)),
        tokens(Some(u.window_output_tokens)),
        u.window_cost_usd
    );
    let name_st = Style::new(size::BODY, weight::SEMIBOLD, t.text);
    let state_st = Style::new(size::SMALL, weight::SEMIBOLD, c);
    let fig_st = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let l1 = if narrow { Rect::new(r.x, r.y + 4.0, r.w, 20.0) } else { r };
    // its state as an icon: a check when ready, a clock while it cools down, a cross when unavailable
    let glyph = match (a.available, cooling) {
        (_, Some(_)) => "\u{F0150}",
        (true, None) => "\u{F05E0}",
        (false, None) => "\u{F0159}",
    };
    ui.icon(Rect::new(r.x + 4.0, l1.y, 16.0, l1.h), glyph, size::TITLE, c);
    let (fx, fy, fh) = if narrow {
        let sb = ui.label_in(Rect::new(r.x, l1.y, r.w - 8.0, l1.h), &state, state_st, Align::Right);
        ui.label_in(Rect::new(r.x + 28.0, l1.y, (r.w - 28.0 - sb.w - 20.0).max(1.0), l1.h), &a.name, name_st, Align::Left);
        (r.x + 28.0, r.y + 24.0, 18.0)
    } else {
        ui.label_in(Rect::new(r.x + 28.0, r.y, 150.0, r.h), &a.name, name_st, Align::Left);
        let sw = ui.label_in(Rect::new(r.x + 184.0, r.y, 130.0, r.h), &state, state_st, Align::Left).w;
        (r.x + 184.0 + sw.max(110.0) + 20.0, r.y, r.h)
    };
    let fb = ui.label_in(Rect::new(fx, fy, (r.right() - fx).max(1.0), fh), &figures, fig_st, Align::Left);
    if let Some(e) = &u.last_error {
        let ex = fx + fb.w + 18.0;
        if ex + 60.0 < r.right() {
            ui.icon(Rect::new(ex, fy, 14.0, fh), "\u{F0026}", size::SMALL, t.danger);
            ui.label_in(Rect::new(ex + 18.0, fy, r.right() - ex - 18.0, fh), e, Style::new(size::SMALL, weight::MEDIUM, t.danger), Align::Left);
        }
    }
}

/// The status code as a small neutral chip with the code in its colour, at `at` (left, vertical middle).
/// Returns its width.
fn code_chip(ui: &mut Ui, at: Point, status: u16) -> f32 {
    let t = ui.t;
    let (sc, strong) = status_color(status, t);
    let code = status.to_string();
    let cs = Style::new(size::CAPTION, if strong { weight::BOLD } else { weight::SEMIBOLD }, sc);
    let cw = ui.measure(&code, cs) + 14.0;
    let chip = Rect::new(at.x, at.y - 10.0, cw, 20.0);
    ui.round(chip, radius::CONTROL, chip_fill(t));
    ui.label_in(chip, &code, cs, Align::Center);
    cw
}

/// "anthropic → openrouter".
fn route_spans(e: &ProxyEvent, t: &Tokens) -> Vec<(String, Style)> {
    vec![
        (e.inbound.clone(), Style::new(size::SMALL, weight::MEDIUM, t.text)),
        ("  →  ".to_string(), Style::new(size::SMALL, weight::REGULAR, t.faint)),
        (e.upstream.clone(), Style::new(size::SMALL, weight::MEDIUM, t.text)),
    ]
}

/// A log row on two lines: time, code, where it went, latency; then the model, the tokens and the note.
fn folded_row(ui: &mut Ui, r: Rect, e: &ProxyEvent, i: usize) {
    let t = ui.t;
    row_hover(ui, ("proxy-log", e.at_ms, i), r.inset(1.0));
    let l1 = Rect::new(r.x + 8.0, r.y + 6.0, r.w - 16.0, 20.0);
    let l2 = Rect::new(r.x + 8.0, r.y + 27.0, r.w - 16.0, 18.0);
    let small = |c: Color| Style::new(size::SMALL, weight::REGULAR, c);
    let tb = ui.label_in(Rect { w: 70.0, ..l1 }, &hms(e.at_ms), Style::mono(size::SMALL, t.muted), Align::Left);
    let cx = l1.x + tb.w.max(62.0) + 10.0;
    let cw = code_chip(ui, Point::new(cx, l1.y + l1.h / 2.0), e.status);
    let lat = format!("{:.1}s", e.latency_ms as f64 / 1000.0);
    let lb = ui.label_in(l1, &lat, Style::new(size::SMALL, weight::SEMIBOLD, latency_color(e.latency_ms, t)), Align::Right);
    let rx = cx + cw + 12.0;
    let rb = ui.rich(&route_spans(e, t), size::SMALL, Some((l1.right() - lb.w - 16.0 - rx).max(1.0)), false, Some(1));
    ui.draw(&rb, Point::new(rx, l1.y + (l1.h - rb.h) / 2.0));
    // the second line, under the route: model · tokens … note
    let mut spans = vec![(e.model.clone(), Style::new(size::SMALL, weight::MEDIUM, t.shine))];
    for (v, what) in [(e.input_tokens, "in"), (e.output_tokens, "out"), (e.cache_read_tokens, "cache")] {
        if v.is_some() {
            spans.push((format!("  ·  {} {what}", tokens(v)), small(t.muted)));
        }
    }
    if e.trim_tokens_before.is_some() {
        spans.push((format!("  ·  {} saved", saved(e)), small(if e.trim_saved() > 0 { t.good } else { t.muted })));
    }
    let n = note(e);
    let nb = (!n.is_empty()).then(|| ui.line(&n, small(if e.error.is_some() { t.danger } else { t.faint }), Some((l2.w * 0.45).max(40.0))));
    let note_w = nb.as_ref().map(|b| b.w + 14.0).unwrap_or(0.0);
    let mb = ui.rich(&spans, size::SMALL, Some((l2.right() - note_w - rx).max(1.0)), false, Some(1));
    ui.draw(&mb, Point::new(rx, l2.y + (l2.h - mb.h) / 2.0));
    if let Some(b) = nb {
        ui.draw(&b, Point::new(l2.right() - b.w, l2.y + (l2.h - b.h) / 2.0));
    }
}

/// The live log's columns at a width (at least [`LOG_NARROW`]: narrower logs fold each row onto two lines
/// instead of dropping columns).
struct Cols {
    x: f32,
    w: f32,
    time: f32,
    code: f32,
    route: f32,
    model: f32,
    num: f32,
    nums: Vec<&'static str>,
    latency: f32,
}

impl Cols {
    fn new(x: f32, w: f32) -> Cols {
        let nums: Vec<&str> = vec!["in", "out", "cache", "saved"];
        let (time, code, num, latency) = (80.0, 58.0, 70.0, 74.0);
        let flex = (w - time - code - num * nums.len() as f32 - latency - 24.0).max(120.0);
        // the note takes what route and model leave
        let (route, model) = (flex * 0.34, flex * 0.33);
        Cols { x, w, time, code, route, model, num, nums, latency }
    }

    /// Left edges: (code, route, model, numbers, latency, note).
    fn edges(&self) -> (f32, f32, f32, f32, f32, f32) {
        let time = self.x + 6.0;
        let code = time + self.time;
        let route = code + self.code;
        let model = route + self.route;
        let nums = model + self.model;
        let latency = nums + self.num * self.nums.len() as f32;
        (code, route, model, nums, latency, latency + self.latency + 24.0)
    }

    /// The column labels, sentence case.
    fn head(&self, ui: &mut Ui, y: f32) {
        let st = Style::new(size::SMALL, weight::SEMIBOLD, ui.t.muted);
        let (code, route, model, nums, latency, note) = self.edges();
        let cell = |x: f32, w: f32| Rect::new(x, y, w, 20.0);
        ui.label_in(cell(self.x + 6.0, self.time), "Time", st, Align::Left);
        ui.label_in(cell(code, self.code), "Code", st, Align::Left);
        ui.label_in(cell(route, self.route - 12.0), "In → upstream", st, Align::Left);
        ui.label_in(cell(model, self.model - 12.0), "Model", st, Align::Left);
        for (k, n) in self.nums.iter().enumerate() {
            let label = match *n {
                "in" => "In",
                "out" => "Out",
                "cache" => "Cache",
                _ => "Saved",
            };
            ui.label_in(cell(nums + k as f32 * self.num, self.num), label, st, Align::Right);
        }
        ui.label_in(cell(latency, self.latency), "Latency", st, Align::Right);
        ui.label_in(cell(note, (self.x + self.w - note).max(1.0)), "Note", st, Align::Left);
    }

    fn row(&self, ui: &mut Ui, r: Rect, e: &ProxyEvent, i: usize) {
        let t = ui.t;
        row_hover(ui, ("proxy-log", e.at_ms, i), r);
        let (code_x, route_x, model_x, nums_x, latency_x, note_x) = self.edges();
        let cell = |x: f32, w: f32| Rect::new(x, r.y, w, r.h);
        let small = |c: Color| Style::new(size::SMALL, weight::REGULAR, c);
        ui.label_in(cell(self.x + 6.0, self.time), &hms(e.at_ms), Style::mono(size::SMALL, t.muted), Align::Left);
        code_chip(ui, Point::new(code_x, r.y + r.h / 2.0), e.status);
        let rb = ui.rich(&route_spans(e, t), size::SMALL, Some(self.route - 12.0), false, Some(1));
        ui.draw(&rb, Point::new(route_x, r.y + (r.h - rb.h) / 2.0));
        ui.label_in(cell(model_x, self.model - 12.0), &e.model, Style::new(size::SMALL, weight::MEDIUM, t.shine), Align::Left);
        for (k, n) in self.nums.iter().enumerate() {
            let (text, c) = match *n {
                "in" => (tokens(e.input_tokens), t.text),
                "out" => (tokens(e.output_tokens), t.text),
                "cache" => (tokens(e.cache_read_tokens), t.muted),
                _ => (saved(e), if e.trim_saved() > 0 { t.good } else { t.muted }),
            };
            let c = if text == "—" { t.faint } else { c };
            ui.label_in(cell(nums_x + k as f32 * self.num, self.num), &text, small(c), Align::Right);
        }
        let lat = format!("{:.1}s", e.latency_ms as f64 / 1000.0);
        ui.label_in(cell(latency_x, self.latency), &lat, Style::new(size::SMALL, weight::SEMIBOLD, latency_color(e.latency_ms, t)), Align::Right);
        let nw = (self.x + self.w - note_x - 6.0).max(1.0);
        let nc = if e.error.is_some() { t.danger } else { t.muted };
        ui.label_in(cell(note_x, nw), &note(e), small(nc), Align::Left);
    }
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut ProxyView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    match n {
        // the terminal UI's `c`: no toast
        TRIM => {
            let on = !cx.svc.settings().proxy.trim;
            cx.svc.set_trim(on);
        }
        NEWEST => v.scroll = 0,
        _ => {}
    }
}

/// The wheel over target `n` (None: elsewhere in the view): three log rows a notch, like the terminal UI.
pub(crate) fn wheel(v: &mut ProxyView, _n: Option<usize>, down: bool, cx: &mut Cx) {
    if down {
        v.scroll += 3;
    } else {
        v.scroll = v.scroll.saturating_sub(3);
    }
    let n = cx.svc.state().proxy.events.len();
    v.scroll = v.scroll.min(n.saturating_sub(1));
}
