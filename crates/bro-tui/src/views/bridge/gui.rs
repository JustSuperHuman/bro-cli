//! The bridge view in the desktop app: pairing for the Just Terminal phone app and web client — whether it
//! runs (with a switch), the pairing QR drawn as real modules, the URLs with their tokens masked until you
//! reveal them, and a copy button per URL. Keys (t, r, j/k, c) reach [`super::BridgeView`] as before.

use super::{BridgeView, mask, show_url};
use crate::alerts::Kind;
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::pane::Cx;
use crate::services::Avail;
use crate::views::usage::gui::{card, hints, icon_text_button, icon_text_button_width, status};
use bro_bridge::BridgeStatus;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::KeyModifiers;

/// The view's background (wheel, focus).
const BODY: usize = 0;
/// Turn the bridge on or off.
const POWER: usize = 1;
/// Reveal or hide the token.
const REVEAL: usize = 2;
/// `URL + i`: pick URL `i`.
const URL: usize = 100;
/// `COPY + i`: copy URL `i`.
const COPY: usize = 200;

const PAD: f32 = 28.0;
const MAX_W: f32 = 980.0;
/// The QR's side, logical pixels (rounded down to whole device pixels per module).
const QR_SIDE: f32 = 220.0;

/// What one frame shows, copied out of the shared state.
struct Snap {
    status: Avail<BridgeStatus>,
    qr: Option<Vec<String>>,
    enabled: bool,
}

fn snapshot(cx: &Cx) -> Snap {
    let st = cx.svc.state();
    let b = &st.bridge;
    Snap { status: b.status.clone(), qr: b.qr.clone(), enabled: b.enabled }
}

/// "this machine" (the first), "tailscale" (100.x addresses) or "lan".
fn kind(i: usize, url: &str) -> &'static str {
    if i == 0 {
        "this machine"
    } else if url.contains("://100.") {
        "tailscale"
    } else {
        "lan"
    }
}

fn kind_icon(k: &str) -> &'static str {
    match k {
        "this machine" => "\u{F0322}",
        "tailscale" => "\u{F0582}",
        _ => "\u{F0317}",
    }
}

pub(crate) fn draw(v: &mut BridgeView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let s = snapshot(cx);
    ui.hit_with(area, Action::View(cx.id, BODY), Cursor::Default);
    let page = Rect::new(area.x + PAD, area.y, (area.w - 2.0 * PAD).clamp(0.0, MAX_W), area.h);

    // header: running or not, and the switch for it
    let head = Rect::new(page.x, area.y + 18.0, page.w, 36.0);
    let (label, loud) = if s.enabled { ("Turn off", widgets::Kind::Secondary) } else { ("Turn on", widgets::Kind::Primary) };
    let bw = icon_text_button_width(ui, label);
    let br = Rect::new(head.right() - bw, head.y + 2.0, bw, 32.0);
    icon_text_button(ui, br, "\u{F0425}", label, loud, Action::View(cx.id, POWER));
    header(ui, Rect { w: br.x - 16.0 - head.x, ..head }, &s);
    let blurb = "Pair the Just Terminal app or open a URL in a browser: your agent sessions show up there live.";
    let bb = ui.para(blurb, Style::new(size::BODY, weight::REGULAR, t.muted), page.w, Some(2));
    ui.draw(&bb, Point::new(page.x, head.bottom() + 10.0));

    let foot_y = area.bottom() - 30.0;
    hints(ui, Point::new(page.x, foot_y), &[("t", if s.enabled { "turn off" } else { "turn on" }), ("r", if v.reveal { "hide token" } else { "reveal token" }), ("j/k", "pick url"), ("c", "copy url")]);

    let top = head.bottom() + 10.0 + bb.h + 26.0;
    let body = Rect::new(page.x, top, page.w, (foot_y - 14.0 - top).max(0.0));
    let Avail::Ready(st) = &s.status else {
        off(ui, body, &s);
        return true;
    };

    // the QR on the right, the URLs and the token on the left (narrow panes: the QR under them)
    let qr = s.qr.as_deref().and_then(modules);
    let beside = body.w >= 600.0;
    let qr_w = if qr.is_some() && beside { QR_SIDE + 40.0 } else { 0.0 };
    let left = Rect::new(body.x, body.y, body.w - qr_w, body.h);
    if let Some((n, dark)) = &qr
        && qr_w > 0.0
    {
        draw_qr(ui, Point::new(body.right() - QR_SIDE - 6.0, body.y + 4.0), QR_SIDE, *n, dark);
    }
    // narrow: the label over the URL
    let folded = left.w < 640.0;
    let row_h = if folded { 54.0 } else { 46.0 };

    let mut y = left.y;
    widgets::section(ui, Rect::new(left.x, y, left.w, 20.0), "URLs");
    y += 30.0;
    v.sel = v.sel.min(st.urls.len().saturating_sub(1));
    for (i, u) in st.urls.iter().enumerate() {
        url_row(ui, Rect::new(left.x, y, left.w, row_h), i, u, &st.token, v.sel == i, v.reveal, folded, cx);
        y += row_h + 4.0;
    }
    y += 18.0;
    widgets::section(ui, Rect::new(left.x, y, left.w, 20.0), "Token");
    y += 30.0;
    // the token in a read-only field: the control fill and edge, 4px corners
    let row = Rect::new(left.x, y, left.w, 44.0);
    ui.panel(row, radius::CONTROL, t.control, t.stroke);
    ui.icon(Rect::new(row.x + 12.0, row.y, 18.0, row.h), "\u{F0306}", size::TITLE, t.faint);
    let label = if v.reveal { "Hide token" } else { "Reveal token" };
    let bw = icon_text_button_width(ui, label) - 6.0;
    let rb = Rect::new(row.right() - bw - 6.0, row.y + 6.0, bw, 32.0);
    icon_text_button(ui, rb, if v.reveal { "\u{F0209}" } else { "\u{F0208}" }, label, widgets::Kind::Ghost, Action::View(cx.id, REVEAL));
    let token = if v.reveal { st.token.clone() } else { mask(&st.token) };
    ui.label_in(Rect::new(row.x + 40.0, row.y, (rb.x - row.x - 52.0).max(1.0), row.h), &token, Style { weight: weight::SEMIBOLD, ..Style::mono(size::BODY, t.inline) }, Align::Left);
    y += 44.0;
    if let Some(e) = &st.error {
        y += 14.0;
        ui.icon(Rect::new(left.x, y, 16.0, 20.0), "\u{F0026}", size::BODY, t.danger);
        let eb = ui.para(e, Style::new(size::SMALL, weight::MEDIUM, t.danger), left.w - 24.0, Some(3));
        ui.draw(&eb, Point::new(left.x + 24.0, y + 2.0));
        y += eb.h + 4.0;
    }
    if let Some((n, dark)) = &qr
        && !beside
    {
        // under the token, as big as the room left allows
        let side = (body.bottom() - y - 28.0 - 34.0).min(QR_SIDE).min(left.w);
        if side >= 120.0 {
            draw_qr(ui, Point::new(left.x, y + 28.0), side, *n, dark);
        }
    }
    true
}

/// The status beside the title: "running on :port · N clients connected", stopped, or off.
fn header(ui: &mut Ui, head: Rect, s: &Snap) {
    let t = ui.t;
    let tb = ui.line("Bridge", Style::new(20.0, weight::SEMIBOLD, t.text), None);
    ui.draw(&tb, Point::new(head.x, head.y + (head.h - tb.h) / 2.0));
    let mut x = head.x + tb.w + 16.0;
    let mid = head.y + head.h / 2.0;
    let room = |x: f32| (head.right() - x).max(1.0);
    match &s.status {
        Avail::Ready(st) if st.running => {
            // running: a check and where, then who's connected
            let text = format!("running on :{}", st.port);
            x = status(ui, Point::new(x, mid), "\u{F05E0}", t.good, &text, Style::new(size::BODY, weight::SEMIBOLD, t.text), head.right()) + 14.0;
            let clients = format!("{} client{} connected", st.clients, if st.clients == 1 { "" } else { "s" });
            let c = if st.clients > 0 { t.shine } else { t.muted };
            ui.label_in(Rect::new(x, head.y, room(x), head.h), &clients, Style::new(size::SMALL, weight::MEDIUM, c), Align::Left);
        }
        Avail::Ready(st) => {
            let ex = status(ui, Point::new(x, mid), "\u{F0159}", t.danger, "stopped", Style::new(size::BODY, weight::SEMIBOLD, t.danger), head.right()) + 10.0;
            if let Some(e) = &st.error {
                ui.label_in(Rect::new(ex, head.y, room(ex), head.h), e, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
            }
        }
        // not running: the reason sits in the body (see `off`)
        _ => {}
    }
}

/// Not running: where the pairing would be, the reason (a spinner while it starts).
fn off(ui: &mut Ui, body: Rect, s: &Snap) {
    let t = ui.t;
    let r = Rect::new(body.x, body.y, body.w.min(560.0), 96.0);
    card(ui, r, 0.0, false);
    // the icon stands on the card (no tile behind it)
    let icon = Rect::new(r.x + 20.0, r.y + 24.0, 48.0, 48.0);
    let starting = s.enabled && matches!(s.status, Avail::Loading);
    if starting {
        ui.spinner(icon.center(), 9.0, 2.6, t.accent);
    } else {
        ui.icon(icon, "\u{F0432}", 32.0, t.faint);
    }
    let x = icon.right() + 16.0;
    let why = if s.enabled { s.status.why() } else { "off — t turns it on".into() };
    let b = ui.para(&why, Style::new(size::BODY, weight::MEDIUM, if starting { t.muted } else { t.text.mix(t.muted, 0.3) }), r.right() - x - 24.0, Some(3));
    ui.draw(&b, Point::new(x, r.y + (r.h - b.h) / 2.0));
}

#[allow(clippy::too_many_arguments)]
fn url_row(ui: &mut Ui, r: Rect, i: usize, url: &str, token: &str, on: bool, reveal: bool, folded: bool, cx: &Cx) {
    let t = ui.t;
    let hot = widgets::row(ui, r, ("bridge-url", cx.id, i), on, Action::View(cx.id, URL + i));
    let k = kind(i, url);
    // copy, while the row is hovered or picked
    let copy = Rect::new(r.right() - 38.0, r.y + (r.h - 32.0) / 2.0, 32.0, 32.0);
    let show = if on { 1.0 } else { hot };
    if show > 0.02 {
        if widgets::icon_button(ui, copy, "\u{F018F}", t.muted.fade(show), Action::View(cx.id, COPY + i)) {
            widgets::tooltip(ui, copy, "copy url");
        }
    } else {
        ui.hit(copy, Action::View(cx.id, COPY + i));
    }
    let shown = show_url(url, token, reveal);
    let c = if on { t.text } else { t.text.mix(t.muted, 0.35) };
    let url_st = Style { weight: if on { weight::SEMIBOLD } else { weight::REGULAR }, ..Style::mono(size::BODY, c) };
    let icon_c = if on { t.text } else { t.faint };
    let kind_st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    if folded {
        // the label over the URL
        let l1 = Rect::new(r.x + 12.0, r.y + 7.0, r.w - 60.0, 18.0);
        ui.icon(Rect::new(l1.x, l1.y, 16.0, l1.h), kind_icon(k), size::BODY, icon_c);
        ui.label_in(Rect::new(l1.x + 24.0, l1.y, l1.w - 24.0, l1.h), k, kind_st, Align::Left);
        ui.label_in(Rect::new(l1.x, r.y + 27.0, (copy.x - 8.0 - l1.x).max(1.0), 20.0), &shown, url_st, Align::Left);
        return;
    }
    ui.icon(Rect::new(r.x + 12.0, r.y, 18.0, r.h), kind_icon(k), size::TITLE, icon_c);
    ui.label_in(Rect::new(r.x + 40.0, r.y, 100.0, r.h), k, kind_st, Align::Left);
    let x = r.x + 148.0;
    ui.label_in(Rect::new(x, r.y, (copy.x - 10.0 - x).max(1.0), r.h), &shown, url_st, Align::Left);
}

/// The pairing QR from the half-block rows (`█` both halves light, `▀` top, `▄` bottom, ` ` none): its side in
/// modules and the dark modules, cropped to the code itself.
fn modules(rows: &[String]) -> Option<(usize, Vec<bool>)> {
    let w = rows.iter().map(|r| r.chars().count()).max()?;
    let h = rows.len() * 2;
    let mut dark = vec![false; w * h];
    for (ry, row) in rows.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let (top, bottom) = match ch {
                '█' => (true, true),
                '▀' => (true, false),
                '▄' => (false, true),
                _ => (false, false),
            };
            dark[2 * ry * w + x] = !top;
            dark[(2 * ry + 1) * w + x] = !bottom;
        }
        // short rows: the rest is quiet zone
        for x in row.chars().count()..w {
            dark[2 * ry * w + x] = false;
            dark[(2 * ry + 1) * w + x] = false;
        }
    }
    // crop to the code (the rows carry their own quiet zone; we draw ours)
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if dark[y * w + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let n = (x1 - x0 + 1).max(y1 - y0 + 1);
    let mut out = vec![false; n * n];
    for y in 0..n {
        for x in 0..n {
            let (sx, sy) = (x0 + x, y0 + y);
            out[y * n + x] = sx < w && sy < h && dark[sy * w + sx];
        }
    }
    Some((n, out))
}

/// Black modules on a white plate (4px corners, a hairline edge, no shadow) with a quiet zone, each module a
/// whole number of device pixels; "scan to pair" under it.
fn draw_qr(ui: &mut Ui, at: Point, room: f32, n: usize, dark: &[bool]) {
    let t = ui.t;
    let quiet = 3;
    let total = n + 2 * quiet;
    let module = ((room * ui.scale) / total as f32).floor().max(1.0);
    let side = module * total as f32 / ui.scale;
    // snap the corner to a device pixel so every module lands on whole pixels
    let ox = (at.x * ui.scale).round() + ((room - side) * ui.scale / 2.0).round();
    let oy = (at.y * ui.scale).round();
    let plate = Rect::new(ox / ui.scale, oy / ui.scale, side, side);
    ui.panel(plate, radius::CONTROL, Color::WHITE, t.hairline);
    let ink = Color::hex(0x0b0c0f);
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if !dark[y * n + x] {
                x += 1;
                continue;
            }
            // one quad per run of dark modules
            let start = x;
            while x < n && dark[y * n + x] {
                x += 1;
            }
            let px = ox + (quiet + start) as f32 * module;
            let py = oy + (quiet + y) as f32 * module;
            ui.fill(Rect::new(px / ui.scale, py / ui.scale, (x - start) as f32 * module / ui.scale, module / ui.scale), ink);
        }
    }
    let cap = Rect::new(at.x, plate.bottom() + 12.0, room, 18.0);
    ui.icon(Rect::new(cap.x + room / 2.0 - 46.0, cap.y, 16.0, cap.h), "\u{F011C}", size::BODY, t.faint);
    ui.label_in(Rect::new(cap.x + room / 2.0 - 26.0, cap.y, 100.0, cap.h), "scan to pair", Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Left);
}

/// Copy URL `i` (the terminal UI's `c`).
fn copy(v: &mut BridgeView, i: usize, cx: &mut Cx) {
    let url = match &cx.svc.state().bridge.status {
        Avail::Ready(s) => s.urls.get(i).cloned(),
        _ => None,
    };
    if let Some(u) = url {
        v.sel = i;
        crate::clip::copy(&u);
        cx.toast(Kind::Info, "copied the pairing URL (it includes the token)");
    }
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut BridgeView, n: usize, _p: Point, _mods: KeyModifiers, count: u32, cx: &mut Cx) {
    match n {
        POWER => {
            // the terminal UI's `t`
            let on = !cx.svc.state().bridge.enabled;
            cx.svc.set_bridge_enabled(on);
            cx.toast(Kind::Bridge, if on { "starting the bridge…" } else { "bridge off" });
        }
        REVEAL => v.reveal = !v.reveal,
        n if (COPY..COPY + 100).contains(&n) => copy(v, n - COPY, cx),
        // a double click copies the URL too
        n if (URL..URL + 100).contains(&n) => {
            v.sel = n - URL;
            if count >= 2 {
                copy(v, n - URL, cx);
            }
        }
        _ => {}
    }
}

/// The wheel over target `n` (None: elsewhere in the view): nothing scrolls here.
pub(crate) fn wheel(_v: &mut BridgeView, _n: Option<usize>, _down: bool, _cx: &mut Cx) {}

#[cfg(test)]
mod tests {
    use super::modules;

    /// The bridge's half-block rendering of a code (`bro_bridge`'s `net::render_qr`): light modules are the drawn
    /// blocks, a quiet zone of two, and an odd last row paired with quiet zone.
    fn render(dark: &[Vec<bool>]) -> Vec<String> {
        const QUIET: usize = 2;
        let width = dark.len();
        let size = width + 2 * QUIET;
        let light = |x: usize, y: usize| x < QUIET || y < QUIET || x >= QUIET + width || y >= QUIET + width || !dark[y - QUIET][x - QUIET];
        (0..size)
            .step_by(2)
            .map(|y| {
                (0..size)
                    .map(|x| match (light(x, y), y + 1 >= size || light(x, y + 1)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn pairing_qr_rows_decode_to_the_same_modules() {
        // a version-1 sized code (odd, so the last half-block row pairs with the quiet zone): finders in three
        // corners, noise elsewhere
        const N: usize = 21;
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut m = vec![vec![false; N]; N];
        for row in m.iter_mut() {
            for cell in row.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *cell = seed % 3 == 0;
            }
        }
        for (oy, ox) in [(0, 0), (0, N - 7), (N - 7, 0)] {
            for y in 0..7 {
                for x in 0..7 {
                    let ring = y == 0 || y == 6 || x == 0 || x == 6;
                    let core = (2..=4).contains(&y) && (2..=4).contains(&x);
                    m[oy + y][ox + x] = ring || core;
                }
            }
        }
        let (n, dark) = modules(&render(&m)).expect("a code");
        assert_eq!(n, N);
        for (y, row) in m.iter().enumerate() {
            for (x, want) in row.iter().enumerate() {
                assert_eq!(dark[y * N + x], *want, "module ({x}, {y})");
            }
        }
        // the demo's pairing QR decodes too (25 modules)
        assert_eq!(modules(&crate::services::demo::fake_qr()).map(|(n, _)| n), Some(25));
    }
}
