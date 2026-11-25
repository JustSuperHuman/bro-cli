//! Floating theme controls over the actual app. Inspection uses registered semantic regions, not sampled pixels.
use super::actions::Action;
use super::kit::{Style, Ui, size, weight, widgets};
use crate::app::App;
use crate::theme::{editor::Field, import};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use std::cell::RefCell;

const CLOSE: usize = 1;
const MIN: usize = 2;
const INSPECT: usize = 3;
const SAVE: usize = 4;
const NAME: usize = 5;
const COLOR: usize = 6;
const SEARCH: usize = 7;
const GUIDED: usize = 8;
const LIGHT: usize = 9;
const DARK: usize = 10;
const MOVE: usize = 11;
const RESIZE: usize = 12;
const CSS: usize = 13;
const ROLE: usize = 100;
const PICK_SV: usize = 14;
const PICK_HUE: usize = 15;
thread_local! {static REGIONS:RefCell<Vec<(Rect,&'static str)>>=const{RefCell::new(Vec::new())};}
pub fn begin_frame() {
    REGIONS.with(|r| r.borrow_mut().clear());
}
pub fn region(rect: Rect, role: &'static str) {
    if !rect.is_empty() {
        REGIONS.with(|r| r.borrow_mut().push((rect, role)));
    }
}
fn under(point: Point) -> Option<(Rect, &'static str)> {
    REGIONS.with(|r| {
        r.borrow()
            .iter()
            .filter(|(r, _)| r.contains(point))
            .min_by(|(a, _), (b, _)| (a.w * a.h).total_cmp(&(b.w * b.h)))
            .copied()
    })
}
fn family(role: &str) -> &'static str {
    import::families()
        .iter()
        .find(|(_, _, roles)| roles.contains(&role))
        .map_or("canvas", |(r, _, _)| r)
}
fn title(role: &str) -> &'static str {
    import::families()
        .iter()
        .find(|(r, _, _)| *r == role)
        .map_or("Background", |(_, label, _)| label)
}
fn group(index: usize) -> &'static str {
    match index {
        0..=7 => "Foundation",
        8..=13 => "Brand & content",
        14..=15 => "Search",
        16..=19 => "Context",
        _ => "Status",
    }
}
fn rgb(value: &str) -> Color {
    import::css(value)
        .map(|c| Color::rgba(c.r, c.g, c.b, c.a))
        .unwrap_or(Color::TRANSPARENT)
}
fn hsv(hue: f32, saturation: f32, value: f32) -> Color {
    let h = hue.rem_euclid(1.0) * 6.0;
    let c = value * saturation;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = value - c;
    Color::rgba(r + m, g + m, b + m, 1.0)
}
fn pick(e: &mut crate::theme::editor::Editor, p: Point, hue: bool) {
    let (x, y, w, h) = if hue { e.hue_bounds } else { e.sv_bounds };
    if hue {
        e.hsv.0 = ((p.x - x) / w).clamp(0.0, 0.99999);
    } else {
        e.hsv.1 = ((p.x - x) / w).clamp(0.0, 1.0);
        e.hsv.2 = (1.0 - (p.y - y) / h).clamp(0.0, 1.0);
    }
    let c = hsv(e.hsv.0, e.hsv.1, e.hsv.2);
    let mut value = format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8
    );
    if let Some(alpha) = import::css(e.color.text())
        .map(|c| c.a)
        .filter(|a| *a < 1.0)
    {
        value.push_str(&format!("{:02x}", (alpha * 255.0).round() as u8));
    }
    e.field = Field::Color;
    if !e.pending {
        e.change_color(&value);
    } else {
        e.color.set_text(&value);
    }
}

pub fn draw(app: &mut App, ui: &mut Ui, window: Rect, panes: &[super::layout::PaneBox]) {
    let Some(e) = app.theme_editor.as_mut() else {
        return;
    };
    let t = ui.t;
    // Stable section roles cover blank areas; finer controls register their own roles while drawing.
    region(window, "canvas");
    let sidebar_end = panes
        .iter()
        .map(|b| b.card.x)
        .reduce(f32::min)
        .unwrap_or(270.0);
    region(
        Rect::new(
            window.x,
            window.y,
            (sidebar_end - window.x - 10.0).max(0.0),
            window.h,
        ),
        "sidebar",
    );
    for pane in panes {
        region(pane.header, "toolbar");
        region(
            pane.content,
            if app.focus.on {
                "canvas"
            } else {
                "terminalBackground"
            },
        );
    }
    let w = e.size.0.min(window.w - 24.0).max(280.0);
    let h = if e.minimized {
        48.0
    } else {
        e.size.1.min(window.h - 72.0).max(400.0)
    };
    let (x, y) = e
        .position
        .unwrap_or((window.right() - w - 18.0, window.bottom() - h - 18.0));
    let x = x.clamp(
        window.x + 8.0,
        (window.right() - w - 8.0).max(window.x + 8.0),
    );
    let y = y.clamp(
        window.y + 40.0,
        (window.bottom() - h - 8.0).max(window.y + 40.0),
    );
    e.bounds = (x, y, w, h);
    let panel = Rect::new(x, y, w, h);
    let layer = ui.layer();
    let matched = REGIONS.with(|r| {
        r.borrow()
            .iter()
            .filter(|(_, role)| family(role) == e.role)
            .map(|(rect, _)| *rect)
            .collect::<Vec<_>>()
    });
    if e.field == Field::Color && e.advanced && !e.inspect {
        for rect in &matched {
            ui.outline(*rect, 8.0, 1.0, t.accent.with_alpha(0.38));
        }
    }
    widgets::card(ui, panel, 16.0);
    ui.hit_with(panel, Action::ThemeEditor(0), super::kit::Cursor::Default);
    ui.hit_with(
        Rect::new(x, y, w - 82.0, 46.0),
        Action::ThemeEditor(MOVE),
        super::kit::Cursor::Default,
    );
    ui.label(
        Point::new(x + 18.0, y + 14.0),
        if e.minimized {
            "Theme editor · live"
        } else {
            "Make it yours"
        },
        Style::new(16.0, weight::SEMIBOLD, t.text),
    );
    widgets::icon_button(
        ui,
        Rect::new(panel.right() - 74.0, y + 8.0, 28.0, 28.0),
        if e.minimized {
            "\u{F004B}"
        } else {
            "\u{F0374}"
        },
        t.muted,
        Action::ThemeEditor(MIN),
    );
    widgets::icon_button(
        ui,
        Rect::new(panel.right() - 38.0, y + 8.0, 28.0, 28.0),
        "\u{F0156}",
        t.muted,
        Action::ThemeEditor(CLOSE),
    );
    if e.minimized {
        return;
    }
    let inner = Rect::new(x + 18.0, y + 52.0, w - 36.0, h - 124.0);
    ui.clip(inner, 0.0);
    let mut cy = inner.y;
    ui.label(
        Point::new(inner.x, cy),
        "Theme name",
        Style::new(size::SMALL, weight::MEDIUM, t.muted),
    );
    cy += 20.0;
    widgets::field(
        ui,
        Rect::new(inner.x, cy, inner.w, 32.0),
        e.name.text(),
        (e.field == Field::Name).then_some(e.name.cursor()),
        "Give this theme a name",
        e.field == Field::Name,
        Action::ThemeEditor(NAME),
    );
    cy += 44.0;
    widgets::segmented(
        ui,
        Rect::new(inner.x, cy, (inner.w - 12.0) / 2.0, 30.0),
        &["Light", "Dark"],
        if e.light { 0 } else { 1 },
        |i| Action::ThemeEditor(if i == 0 { LIGHT } else { DARK }),
    );
    widgets::button(
        ui,
        Rect::new(
            inner.x + (inner.w + 12.0) / 2.0,
            cy,
            (inner.w - 12.0) / 2.0,
            30.0,
        ),
        if e.inspect {
            "Click an app area…"
        } else {
            "Inspect app colors"
        },
        widgets::Kind::Secondary,
        Action::ThemeEditor(INSPECT),
    );
    cy += 44.0;
    ui.label(
        Point::new(inner.x, cy),
        "Colors",
        Style::new(14.0, weight::SEMIBOLD, t.text),
    );
    let mode_label = if e.advanced {
        "Advanced · on"
    } else {
        "Advanced"
    };
    let mw = widgets::button_width(ui, mode_label);
    widgets::button(
        ui,
        Rect::new(inner.right() - mw, cy - 5.0, mw, 28.0),
        mode_label,
        widgets::Kind::Secondary,
        Action::ThemeEditor(GUIDED),
    );
    cy += 26.0;
    ui.label(
        Point::new(inner.x, cy),
        if e.advanced {
            "Linked colors stay readable together."
        } else {
            "Two colors. The rest follows."
        },
        Style::new(size::SMALL, weight::REGULAR, t.muted),
    );
    cy += 27.0;
    if e.advanced {
        widgets::field(
            ui,
            Rect::new(inner.x, cy, inner.w, 30.0),
            e.search.text(),
            (e.field == Field::Search).then_some(e.search.cursor()),
            "Find a color family…",
            e.field == Field::Search,
            Action::ThemeEditor(SEARCH),
        );
        cy += 36.0;
        let list_h = (inner.bottom() - cy - 256.0).clamp(58.0, 142.0);
        let rows = ((list_h / 34.0).floor() as usize).max(1);
        let filtered = e.filtered();
        e.scroll = e.scroll.min(filtered.len().saturating_sub(rows));
        ui.clip(Rect::new(inner.x, cy, inner.w, list_h), 0.0);
        for (n, (role, label, _)) in filtered.iter().enumerate().skip(e.scroll).take(rows) {
            let rr = Rect::new(inner.x, cy + (n - e.scroll) as f32 * 34.0, inner.w, 30.0);
            let index = import::families()
                .iter()
                .position(|(r, _, _)| r == role)
                .unwrap_or(0);
            if *role == e.role {
                ui.round(rr, 8.0, t.selected);
            }
            ui.round(
                Rect::new(rr.x + 7.0, rr.y + 5.0, 20.0, 20.0),
                6.0,
                rgb(e.value(role)),
            );
            let group_width = if rr.w >= 340.0 { 112.0 } else { 0.0 };
            ui.label_in(
                Rect::new(rr.x + 36.0, rr.y, rr.w - 44.0 - group_width, 30.0),
                label,
                Style::new(size::BODY, weight::MEDIUM, t.text),
                Align::Left,
            );
            if group_width > 0.0 {
                ui.label_in(
                    Rect::new(rr.right() - 110.0, rr.y, 100.0, 30.0),
                    group(index),
                    Style::new(10.5, weight::REGULAR, t.faint),
                    Align::Right,
                );
            }
            ui.hit(rr, Action::ThemeEditor(ROLE + index));
        }
        ui.unclip();
        cy += list_h + 10.0;
    } else {
        for (index, (role, label)) in [("canvas", "Background"), ("accent", "Accent")]
            .into_iter()
            .enumerate()
        {
            let rr = Rect::new(inner.x, cy, inner.w, 37.0);
            if e.role == role {
                ui.round(rr, 8.0, t.selected);
            }
            ui.round(
                Rect::new(rr.x + 8.0, rr.y + 7.0, 23.0, 23.0),
                7.0,
                rgb(e.value(role)),
            );
            ui.label(
                Point::new(rr.x + 42.0, rr.y + 10.0),
                label,
                Style::new(size::BODY, weight::MEDIUM, t.text),
            );
            let value = import::display_hex(e.value(role)).unwrap_or_default();
            ui.label_in(
                Rect::new(rr.right() - 120.0, rr.y, 108.0, 37.0),
                &value,
                Style::mono(size::SMALL, t.muted),
                Align::Right,
            );
            let family_index = import::families()
                .iter()
                .position(|(r, _, _)| *r == role)
                .unwrap_or(index);
            ui.hit(rr, Action::ThemeEditor(ROLE + family_index));
            cy += 41.0;
        }
        cy += 6.0;
    }
    ui.label(
        Point::new(inner.x, cy),
        title(e.role),
        Style::new(size::SMALL, weight::MEDIUM, t.text),
    );
    if e.advanced {
        ui.label_in(
            Rect::new(inner.right() - 120.0, cy - 4.0, 120.0, 24.0),
            &format!(
                "{} visible {}",
                matched.len(),
                if matched.len() == 1 { "use" } else { "uses" }
            ),
            Style::new(11.0, weight::REGULAR, t.muted),
            Align::Right,
        );
    }
    cy += 20.0;
    widgets::field(
        ui,
        Rect::new(inner.x, cy, inner.w, 32.0),
        e.color.text(),
        (e.field == Field::Color).then_some(e.color.cursor()),
        "#rrggbb or a literal CSS color",
        e.field == Field::Color,
        Action::ThemeEditor(COLOR),
    );
    cy += 42.0;
    let sv = Rect::new(inner.x, cy, inner.w, if e.advanced { 90.0 } else { 122.0 });
    e.sv_bounds = (sv.x, sv.y, sv.w, sv.h);
    ui.scene.quad(
        bro_gfx::Quad::new(
            ui.px(sv),
            bro_gfx::Fill::Horizontal(Color::WHITE, hsv(e.hsv.0, 1.0, 1.0)),
        )
        .radius(8.0 * ui.scale),
    );
    ui.scene.quad(
        bro_gfx::Quad::new(
            ui.px(sv),
            bro_gfx::Fill::Vertical(Color::TRANSPARENT, Color::BLACK),
        )
        .radius(8.0 * ui.scale),
    );
    let marker = Rect::new(
        sv.x + e.hsv.1 * sv.w - 5.0,
        sv.y + (1.0 - e.hsv.2) * sv.h - 5.0,
        10.0,
        10.0,
    );
    ui.outline(marker, 5.0, 2.0, Color::WHITE);
    ui.hit(sv, Action::ThemeEditor(PICK_SV));
    cy = sv.bottom() + 10.0;
    let hue = Rect::new(inner.x, cy, inner.w, 14.0);
    e.hue_bounds = (hue.x, hue.y, hue.w, hue.h);
    ui.clip(hue, 7.0);
    for i in 0..6 {
        let rect = Rect::new(
            hue.x + i as f32 * hue.w / 6.0,
            hue.y,
            hue.w / 6.0 + 1.0,
            hue.h,
        );
        ui.scene.quad(bro_gfx::Quad::new(
            ui.px(rect),
            bro_gfx::Fill::Horizontal(
                hsv(i as f32 / 6.0, 1.0, 1.0),
                hsv((i + 1) as f32 / 6.0, 1.0, 1.0),
            ),
        ));
    }
    ui.unclip();
    ui.outline(
        Rect::new(hue.x + e.hsv.0 * hue.w - 4.0, hue.y - 2.0, 8.0, hue.h + 4.0),
        4.0,
        2.0,
        Color::WHITE,
    );
    ui.hit(hue, Action::ThemeEditor(PICK_HUE));
    cy = hue.bottom() + 16.0;
    let css_label = if e.field == Field::Css {
        "Custom CSS · export only"
    } else if !e.css.text().is_empty() {
        "Custom CSS included · edit for T3 Code"
    } else {
        "Custom CSS for T3 Code…"
    };
    if cy + 32.0 < inner.bottom() {
        widgets::button(
            ui,
            Rect::new(inner.x, cy, inner.w, 28.0),
            css_label,
            widgets::Kind::Secondary,
            Action::ThemeEditor(CSS),
        );
        cy += 36.0;
        if e.field == Field::Css {
            let r = Rect::new(inner.x, cy, inner.w, (inner.bottom() - cy).max(24.0));
            ui.panel(r, 8.0, t.sunken, t.stroke);
            let text = ui.para(e.css.text(), Style::mono(11.0, t.text), r.w - 16.0, Some(4));
            ui.draw(&text, Point::new(r.x + 8.0, r.y + 5.0));
        }
    }
    ui.unclip();
    let footer_y = panel.bottom() - 68.0;
    ui.hline(x + 18.0, footer_y - 8.0, w - 36.0, t.hairline);
    let note = if e.saving {
        "Saving a separate copy…"
    } else if e.pending {
        "Updating live preview…"
    } else if !e.valid_name() {
        "Use a name of 1–48 characters."
    } else if let Some(error) = e.error.as_ref().or(e.save_error.as_ref()) {
        error.as_str()
    } else {
        "Live preview · Cancel restores your theme"
    };
    let note_box = ui.line(
        note,
        Style::new(
            11.0,
            weight::REGULAR,
            if e.error.is_some() || e.save_error.is_some() {
                t.danger
            } else {
                t.muted
            },
        ),
        Some(inner.w),
    );
    ui.draw(&note_box, Point::new(inner.x, footer_y));
    let note_area = Rect::new(inner.x, footer_y, inner.w, 19.0);
    if ui.hovered(note_area) && (e.error.is_some() || e.save_error.is_some()) {
        widgets::tooltip(ui, note_area, note);
    }
    let save = Rect::new(panel.right() - 150.0, footer_y + 23.0, 132.0, 32.0);
    if e.can_save() {
        widgets::button(
            ui,
            save,
            "Save theme",
            widgets::Kind::Primary,
            Action::ThemeEditor(SAVE),
        );
    } else {
        ui.panel(save, 8.0, t.control, t.hairline);
        ui.label_in(
            save,
            "Save theme",
            Style::new(size::BODY, weight::MEDIUM, t.faint),
            Align::Center,
        );
    }
    widgets::button(
        ui,
        Rect::new(inner.x, save.y, 92.0, 32.0),
        "Cancel",
        widgets::Kind::Secondary,
        Action::ThemeEditor(CLOSE),
    );
    ui.hit_with(
        Rect::new(panel.right() - 13.0, panel.bottom() - 13.0, 13.0, 13.0),
        Action::ThemeEditor(RESIZE),
        super::kit::Cursor::ResizeH,
    );
    if e.inspect {
        if let Some(pointer) = ui.pointer.filter(|p| !panel.contains(*p)) {
            if let Some((r, role)) = under(pointer) {
                ui.outline(r, 8.0, 2.0, t.accent);
                let label = format!("{} · click to edit", title(family(role)));
                let at = Rect::new(
                    (pointer.x + 14.0).min(window.right() - 250.0),
                    (pointer.y + 14.0).min(window.bottom() - 42.0),
                    238.0,
                    32.0,
                );
                ui.panel(at, 8.0, t.raised, t.border);
                ui.label_in(
                    at,
                    &label,
                    Style::new(size::SMALL, weight::MEDIUM, t.text),
                    Align::Center,
                );
            }
        }
    }
    ui.set_layer(layer);
}

pub fn click(app: &mut App, n: usize, p: Point) {
    if n == CLOSE {
        app.cancel_theme_editor();
        return;
    }
    let Some(e) = &mut app.theme_editor else {
        return;
    };
    if e.saving {
        return;
    }
    match n {
        MIN => {
            e.minimized = !e.minimized;
            e.field = Field::None;
        }
        INSPECT => {
            e.inspect = !e.inspect;
            e.field = Field::None;
        }
        SAVE => e.save(),
        NAME => {
            e.field = Field::Name;
            e.name.select_all();
        }
        COLOR => {
            e.select(e.role);
        }
        SEARCH => {
            e.field = Field::Search;
        }
        GUIDED => e.guided(),
        LIGHT => e.mode(true),
        DARK => e.mode(false),
        MOVE | RESIZE => {
            let (x, y, w, h) = e.bounds;
            e.drag = Some((
                p.x,
                p.y,
                if n == MOVE { x } else { w },
                if n == MOVE { y } else { h },
                n == RESIZE,
            ));
        }
        CSS => {
            e.field = if e.field == Field::Css {
                Field::None
            } else {
                Field::Css
            };
        }
        n if (ROLE..ROLE + import::families().len()).contains(&n) => {
            e.select(import::families()[n - ROLE].0)
        }
        PICK_SV | PICK_HUE => {
            e.picker_drag = Some(n == PICK_HUE);
            pick(e, p, n == PICK_HUE);
        }
        _ => e.field = Field::None,
    }
}
pub fn drag(app: &mut App, p: Point) -> bool {
    let Some(e) = &mut app.theme_editor else {
        return false;
    };
    if let Some(hue) = e.picker_drag {
        pick(e, p, hue);
        return true;
    }
    let Some((px, py, x, y, resize)) = e.drag else {
        return false;
    };
    if resize {
        e.size = (
            (x + p.x - px).clamp(340.0, 650.0),
            (y + p.y - py).clamp(620.0, 1000.0),
        );
    } else {
        e.position = Some((x + p.x - px, y + p.y - py));
    }
    true
}
pub fn release(app: &mut App, p: Point) -> bool {
    let Some(e) = &mut app.theme_editor else {
        return false;
    };
    if let Some(hue) = e.picker_drag.take() {
        pick(e, p, hue);
        let value = e.color.text().to_string();
        e.change_color(&value);
        return true;
    }
    e.drag.take().is_some()
}
pub fn inspect_click(app: &mut App, p: Point) -> bool {
    let Some(e) = &mut app.theme_editor else {
        return false;
    };
    if !e.inspect {
        return false;
    }
    if e.pending {
        return true;
    }
    if let Some((_, role)) = under(p) {
        e.search.set_text("");
        e.advanced = true;
        e.file.as_object_mut().unwrap().remove("managed");
        e.minimized = false;
        e.select(family(role));
        let at = e
            .filtered()
            .iter()
            .position(|(r, _, _)| *r == e.role)
            .unwrap_or(0);
        e.scroll = at;
        e.inspect = false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hsv_picker_spans_primary_colors_and_grayscale() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Color::rgb8(255, 0, 0));
        assert_eq!(hsv(1.0 / 3.0, 1.0, 1.0), Color::rgb8(0, 255, 0));
        assert_eq!(hsv(2.0 / 3.0, 1.0, 1.0), Color::rgb8(0, 0, 255));
        assert_eq!(hsv(0.7, 0.0, 0.5), Color::rgba(0.5, 0.5, 0.5, 1.0));
    }
    #[test]
    fn inspector_selects_the_smallest_semantic_region_and_linked_family() {
        begin_frame();
        region(Rect::new(0.0, 0.0, 800.0, 600.0), "canvas");
        region(Rect::new(100.0, 100.0, 200.0, 80.0), "messageForeground");
        let (_, role) = under(Point::new(150.0, 120.0)).unwrap();
        assert_eq!(role, "messageForeground");
        assert_eq!(family(role), "messageSurface");
        assert_eq!(family("terminalCursor"), "accent");
        assert_eq!(under(Point::new(700.0, 400.0)).unwrap().1, "canvas");
    }
}
