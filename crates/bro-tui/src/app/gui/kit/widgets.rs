//! Reusable pieces in the Windows 11 (WinUI 3) manner: buttons, icon buttons, chips, keycaps and key tips,
//! switches, segmented choices, meters, text fields, scrollbars, cards, tooltips. Controls have 4px corners and a
//! hairline edge; floating things (cards, dialogs, menus) 8px corners and a soft shadow. Each draws itself,
//! registers its hit and returns what the caller needs to lay out the next thing.

use super::{Cursor, Style, Ui, radius, size, weight};
use crate::app::gui::actions::Action;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};

/// How loud a button is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// filled with the accent
    Primary,
    /// a standard button: a quiet fill with an edge
    Secondary,
    /// no fill until hovered (a "subtle" button)
    Ghost,
    /// filled red: destructive
    Danger,
}

/// The box of a standard control: fill, hairline stroke and the faint edge that lifts it off the page (a darker
/// bottom on light themes, a lighter top on dark ones).
pub fn control(ui: &mut Ui, r: Rect, fill: Color) {
    let t = ui.t;
    ui.panel(r, radius::CONTROL, fill, t.stroke);
    let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
    if t.light {
        ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, Color::rgba(0.0, 0.0, 0.0, 0.1));
    } else {
        ui.hline(inner.x, r.y, inner.w, Color::rgba(1.0, 1.0, 1.0, 0.03));
    }
}

/// A text button. Returns true while hovered.
pub fn button(ui: &mut Ui, r: Rect, label: &str, kind: Kind, action: Action) -> bool {
    let hot = ui.hover_amount(("button", label, r.x as i32, r.y as i32), r);
    let down = ui.pressing && hot > 0.5;
    let t = ui.t;
    let ink = match kind {
        Kind::Primary => {
            let fill = if down { t.accent.fade(0.8) } else { t.accent.fade(1.0 - 0.1 * hot) };
            ui.round(r, radius::CONTROL, fill);
            let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
            ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, Color::rgba(0.0, 0.0, 0.0, 0.18));
            if down { t.on_accent.fade(0.8) } else { t.on_accent }
        }
        Kind::Danger => {
            let fill = if down { t.danger.fade(0.8) } else { t.danger.fade(1.0 - 0.1 * hot) };
            ui.round(r, radius::CONTROL, fill);
            t.ink_on(t.danger)
        }
        Kind::Secondary => {
            let fill = if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) };
            control(ui, r, fill);
            if down { t.muted } else { t.text }
        }
        Kind::Ghost => {
            if hot > 0.0 || down {
                ui.round(r, radius::CONTROL, if down { t.text.with_alpha(0.04) } else { t.text.with_alpha(0.06 * hot) });
            }
            if down { t.muted } else { t.text }
        }
    };
    ui.label_in(r, label, Style::new(size::BODY, weight::MEDIUM, ink), Align::Center);
    ui.hit(r, action);
    hot > 0.5
}

/// Width a [`button`] with `label` wants.
pub fn button_width(ui: &mut Ui, label: &str) -> f32 {
    ui.measure(label, Style::new(size::BODY, weight::MEDIUM, ui.t.text)) + 28.0
}

/// A square icon button (no fill until hovered). `color` = the icon's resting colour.
pub fn icon_button(ui: &mut Ui, r: Rect, glyph: &str, color: Color, action: Action) -> bool {
    let hot = ui.hover_amount(("icon", glyph, r.x as i32, r.y as i32), r);
    if hot > 0.0 {
        let down = ui.pressing && hot > 0.5;
        ui.round(r, radius::CONTROL, ui.t.text.with_alpha(if down { 0.04 } else { 0.07 * hot }));
    }
    ui.icon(r, glyph, size::TITLE, color.mix(ui.t.text, 0.5 * hot));
    ui.hit(r, action);
    hot > 0.5
}

/// A small tag with text (status, counts): 4px corners. Returns its rect.
pub fn pill(ui: &mut Ui, at: Point, h: f32, text: &str, fg: Color, bg: Color) -> Rect {
    let st = Style::new(size::CAPTION, weight::SEMIBOLD, fg);
    let b = ui.line(text, st, None);
    let r = Rect::new(at.x, at.y, b.w + 12.0, h);
    ui.round(r, radius::CONTROL, bg);
    ui.draw(&b, Point::new(r.x + 6.0, r.y + (h - b.h) / 2.0));
    r
}

/// Width a [`pill`] with `text` takes.
pub fn pill_width(ui: &mut Ui, text: &str) -> f32 {
    ui.measure(text, Style::new(size::CAPTION, weight::SEMIBOLD, ui.t.text)) + 12.0
}

/// A key hint like `alt+n`, drawn as keycaps: one per key, a small gap between them. Returns the width
/// ([`kbd_width`] measures it first).
pub fn kbd(ui: &mut Ui, at: Point, chord: &str) -> f32 {
    let t = ui.t;
    let st = Style::new(size::CAPTION, weight::MEDIUM, t.muted);
    let mut x = at.x;
    let keys = chord_keys(chord);
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            x += 3.0;
        }
        let b = ui.line(k, st, None);
        let w = (b.w + 10.0).max(18.0);
        let r = Rect::new(x, at.y, w, 18.0);
        keycap(ui, r, false);
        ui.draw(&b, Point::new(r.x + (w - b.w) / 2.0, r.y + (18.0 - b.h) / 2.0 - 0.5));
        x += w;
    }
    x - at.x
}

/// The width [`kbd`] takes for `chord`.
pub fn kbd_width(ui: &mut Ui, chord: &str) -> f32 {
    let st = Style::new(size::CAPTION, weight::MEDIUM, ui.t.muted);
    let mut w = 0.0;
    for (i, k) in chord_keys(chord).iter().enumerate() {
        if i > 0 {
            w += 3.0;
        }
        w += (ui.measure(k, st) + 10.0).max(18.0);
    }
    w
}

/// The keys of a chord: "alt+shift+t" → ["alt", "shift", "t"]; "ctrl+space S" → ["ctrl", "space", "S"]; a lone
/// "+" or "-" stays a key.
pub fn chord_keys(chord: &str) -> Vec<String> {
    let mut out = vec![];
    for part in chord.split(' ').filter(|p| !p.is_empty()) {
        if part.len() > 1 && part.contains('+') {
            let mut rest = part;
            while let Some(i) = rest.find('+').filter(|i| *i > 0) {
                out.push(rest[..i].to_string());
                rest = &rest[i + 1..];
            }
            if !rest.is_empty() {
                out.push(rest.to_string());
            }
        } else {
            out.push(part.to_string());
        }
    }
    out
}

/// A keycap's box: a control with a heavier bottom edge, like a key.
fn keycap(ui: &mut Ui, r: Rect, strong: bool) {
    let t = ui.t;
    let fill = if strong { t.raised } else { t.control };
    ui.panel(r, radius::CONTROL, fill, t.stroke);
    let inner = Rect::new(r.x + 2.0, r.y, (r.w - 4.0).max(0.0), r.h);
    ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, if t.light { Color::rgba(0.0, 0.0, 0.0, 0.18) } else { Color::rgba(0.0, 0.0, 0.0, 0.45) });
}

/// A key tip: the key that reaches something, shown while alt is held, centred on `at` (the caller puts it at
/// the bottom centre of the thing it opens). Drawn on its own layer, over everything. Returns its rect.
pub fn keytip(ui: &mut Ui, at: Point, keys: &str) -> Rect {
    let t = ui.t;
    let prev = ui.layer();
    let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let parts = chord_keys(keys);
    let label = parts.join(" + ");
    let b = ui.line(&label, st, None);
    let w = (b.w + 12.0).max(22.0);
    let h = 22.0;
    let r = Rect::new((at.x - w / 2.0).round(), (at.y - h / 2.0).round(), w, h);
    ui.shadow(r, radius::CONTROL, 8.0, 2.0, t.shadow.fade(0.7));
    ui.panel(r, radius::CONTROL, t.raised, t.border);
    ui.draw(&b, Point::new(r.x + (w - b.w) / 2.0, r.y + (h - b.h) / 2.0 - 0.5));
    ui.set_layer(prev);
    r
}

/// Width and height of a [`switch`].
pub const SWITCH_W: f32 = 40.0;
pub const SWITCH_H: f32 = 20.0;

/// A toggle switch (40×20, WinUI's): an outlined track when off, the accent when on. Returns its rect.
pub fn switch(ui: &mut Ui, at: Point, on: bool, action: Action) -> Rect {
    let t = ui.t;
    let r = Rect::new(at.x, at.y, SWITCH_W, SWITCH_H);
    let k = ui.ease(("switch", at.x as i32, at.y as i32), if on { 1.0 } else { 0.0 }, 140.0);
    let hot = ui.hover_amount(("switch-hover", at.x as i32, at.y as i32), r.outset(3.0));
    let off_fill = t.text.with_alpha(0.04 * hot);
    ui.round(r, r.h / 2.0, off_fill.mix(t.accent, k));
    if k < 1.0 {
        ui.outline(r, r.h / 2.0, 1.0, t.muted.fade(1.0 - k));
    }
    let knob_r = 6.0 + 1.0 * hot;
    let knob = Point::new(r.x + 10.0 + 20.0 * k, r.y + r.h / 2.0);
    ui.circle(knob, knob_r, t.muted.mix(t.on_accent, k));
    ui.hit(r.outset(3.0), action);
    r
}

/// A row of choices with one selected (a segmented control). `act(i)` gives each choice's action. Returns the
/// rect.
pub fn segmented(ui: &mut Ui, r: Rect, options: &[&str], selected: usize, act: impl Fn(usize) -> Action) -> Rect {
    let t = ui.t;
    // on light themes the track is a faint grey, so the chosen (white) segment stands off it
    control(ui, r, if t.light { t.text.with_alpha(0.045) } else { t.control });
    let n = options.len().max(1) as f32;
    let w = (r.w - 4.0) / n;
    for (i, o) in options.iter().enumerate() {
        let c = Rect::new(r.x + 2.0 + i as f32 * w, r.y + 2.0, w, r.h - 4.0);
        let sel = i == selected;
        if sel {
            ui.panel(c, radius::CONTROL - 1.0, if t.light { Color::WHITE } else { t.text.with_alpha(0.09) }, t.stroke);
        } else if ui.hovered(c) {
            ui.round(c, radius::CONTROL - 1.0, t.text.with_alpha(0.04));
        }
        let ink = if sel { t.text } else { t.muted };
        ui.label_in(c, o, Style::new(size::SMALL, if sel { weight::SEMIBOLD } else { weight::MEDIUM }, ink), Align::Center);
        ui.hit(c, act(i));
    }
    r
}

/// A thin progress bar (scans, loads): a hairline track and a 3px bar.
pub fn meter(ui: &mut Ui, r: Rect, fraction: f32, color: Color) {
    let h = r.h.min(3.0);
    let y = r.y + (r.h - h) / 2.0;
    ui.round(Rect::new(r.x, y + (h - 1.0) / 2.0, r.w, 1.0), 0.5, ui.t.text.with_alpha(0.2));
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        ui.round(Rect::new(r.x, y, (r.w * f).max(h), h), h / 2.0, color);
    }
}

/// How much is left (or used) as `n` little segments, like the terminal UI's ▰▰▰▱▱: filled ones in `fill`,
/// the rest faint. `gap` between segments.
pub fn segments(ui: &mut Ui, r: Rect, fraction: f32, n: usize, fill: Color) {
    let n = n.max(1);
    let gap = if r.w / n as f32 > 6.0 { 2.0 } else { 1.0 };
    let w = ((r.w - gap * (n - 1) as f32) / n as f32).max(1.0);
    let filled = ((fraction.clamp(0.0, 1.0) * n as f32).round() as usize).min(n);
    let empty = ui.t.text.with_alpha(if ui.t.light { 0.12 } else { 0.14 });
    let rad = (r.h / 2.0).min(1.5);
    for i in 0..n {
        let s = Rect::new(r.x + i as f32 * (w + gap), r.y, w, r.h);
        ui.round(s, rad, if i < filled { fill } else { empty });
    }
}

/// How many segments fit in `w` px at about `per` px each (at least 5).
pub fn segment_count(w: f32, per: f32) -> usize {
    ((w / per.max(1.0)).floor() as usize).max(5)
}

/// A text field (single line): the text, a caret at byte `cursor` when focused, the placeholder when empty.
/// Clicking it runs `action`.
pub fn field(ui: &mut Ui, r: Rect, value: &str, cursor: Option<usize>, placeholder: &str, focused: bool, action: Action) {
    let t = ui.t;
    if focused {
        ui.panel(r, radius::CONTROL, t.field_focus, t.accent.with_alpha(0.8));
    } else {
        let hot = ui.hover_amount(("field", r.x as i32, r.y as i32), r);
        ui.panel(r, radius::CONTROL, t.control.mix(t.control_hover, hot), t.stroke);
        let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
        ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, t.muted.with_alpha(0.45));
    }
    let inner = Rect::new(r.x + 10.0, r.y, r.w - 20.0, r.h);
    let st = Style::new(size::BODY, weight::REGULAR, t.text);
    ui.clip(inner, 0.0);
    if value.is_empty() {
        ui.label_in(inner, placeholder, st.color(t.faint), Align::Left);
    } else {
        let b = ui.line(value, st, None);
        // keep the caret in view
        let caret_x = cursor.map(|c| b.layout.caret_x(c.min(value.len())) / ui.scale).unwrap_or(b.w);
        let shift = (caret_x - inner.w + 4.0).max(0.0);
        ui.draw(&b, Point::new(inner.x - shift, r.y + (r.h - b.h) / 2.0));
        if focused && cursor.is_some() {
            draw_caret(ui, Point::new(inner.x - shift + caret_x, r.y + (r.h - 16.0) / 2.0), 16.0);
        }
    }
    if value.is_empty() && focused {
        draw_caret(ui, Point::new(inner.x, r.y + (r.h - 16.0) / 2.0), 16.0);
    }
    ui.unclip();
    ui.hit_with(r, action, Cursor::Text);
}

/// A blinking text caret.
pub fn draw_caret(ui: &mut Ui, at: Point, h: f32) {
    ui.ticking = true;
    let on = (ui.time * 1.8).fract() < 0.6;
    if on {
        ui.top_fill(Rect::new(at.x, at.y, 1.5, h), 0.75, ui.t.text);
    }
}

/// A slim scrollbar for a view of `view` pixels over `content` pixels scrolled `offset` down, drawn at the
/// right edge of `r` while `visible` (0..1 fades it).
pub fn scrollbar(ui: &mut Ui, r: Rect, content: f32, view: f32, offset: f32, visible: f32) {
    if content <= view + 0.5 || visible <= 0.0 {
        return;
    }
    let track = Rect::new(r.right() - 5.0, r.y + 3.0, 2.0, r.h - 6.0);
    let h = (track.h * view / content).max(24.0).min(track.h);
    let y = track.y + (track.h - h) * (offset / (content - view)).clamp(0.0, 1.0);
    ui.top_fill(Rect::new(track.x, y, track.w, h), 1.0, ui.t.text.with_alpha(0.4 * visible));
}

/// A group heading: semibold text, as WinUI titles a group of settings (no caps, no rule).
pub fn section(ui: &mut Ui, r: Rect, text: &str) {
    let st = Style::new(size::SMALL, weight::SEMIBOLD, ui.t.text.mix(ui.t.muted, 0.35));
    let b = ui.line(text, st, Some(r.w));
    ui.draw(&b, Point::new(r.x, r.y + (r.h - b.h) / 2.0));
}

/// A floating card (menus, dialogs, flyouts): soft shadow, fill, hairline stroke. Swallows clicks. `radius` is
/// capped at the overlay radius (8).
pub fn card(ui: &mut Ui, r: Rect, radius: f32) {
    let rad = radius.min(super::radius::OVERLAY);
    ui.shadow(r, rad, 32.0, 8.0, ui.t.shadow.fade(0.75));
    ui.shadow(r, rad, 4.0, 1.0, ui.t.shadow.fade(0.35));
    ui.panel(r, rad, ui.t.raised, ui.t.border);
    ui.block(r);
}

/// Dim everything under `r` (modal dialogs); clicks on it run `action`.
pub fn scrim(ui: &mut Ui, r: Rect, amount: f32, action: Action) {
    ui.fill(r, ui.t.scrim.fade(amount));
    ui.hit_with(r, action, Cursor::Default);
}

/// A selectable list row background: selected, hovered or plain (4px corners, no side marker). Returns the
/// hover amount.
pub fn row(ui: &mut Ui, r: Rect, key: impl std::hash::Hash, selected: bool, action: Action) -> f32 {
    let hot = ui.hover_amount(("row", key), r);
    if selected {
        ui.round(r, radius::CONTROL, ui.t.selected);
    } else if hot > 0.0 {
        ui.round(r, radius::CONTROL, ui.t.hover.fade(hot));
    }
    ui.hit(r, action);
    hot
}

/// A tooltip above (or below, near the top) `anchor`, on its own layer.
pub fn tooltip(ui: &mut Ui, anchor: Rect, text: &str) {
    let screen = ui.screen;
    let prev = ui.layer();
    let st = Style::new(size::SMALL, weight::REGULAR, ui.t.text);
    let b = ui.para(text, st, 320.0, Some(6));
    let w = b.w + 16.0;
    let h = b.h + 10.0;
    let mut x = anchor.x + (anchor.w - w) / 2.0;
    x = x.clamp(screen.x + 6.0, screen.right() - w - 6.0);
    let mut y = anchor.y - h - 6.0;
    if y < screen.y + 4.0 {
        y = anchor.bottom() + 6.0;
    }
    let r = Rect::new(x, y, w, h);
    ui.shadow(r, radius::CONTROL, 12.0, 4.0, ui.t.shadow.fade(0.8));
    ui.panel(r, radius::CONTROL, ui.t.raised, ui.t.border);
    ui.draw(&b, Point::new(r.x + 8.0, r.y + 5.0));
    ui.set_layer(prev);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_split_into_keys() {
        assert_eq!(chord_keys("alt+shift+t"), ["alt", "shift", "t"]);
        assert_eq!(chord_keys("ctrl+space S"), ["ctrl", "space", "S"]);
        assert_eq!(chord_keys("alt++"), ["alt", "+"]);
        assert_eq!(chord_keys("+"), ["+"]);
        assert_eq!(chord_keys("f1"), ["f1"]);
        assert_eq!(segment_count(100.0, 8.0), 12);
        assert_eq!(segment_count(10.0, 8.0), 5);
    }
}
