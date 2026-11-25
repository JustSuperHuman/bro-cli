use super::*;
use crate::app::gui::actions::Action as GuiAction;
use crate::app::gui::kit::{Style, Ui, radius, size, weight, widgets};
use crate::views::profiles::gui::{focus_ring, page, page_card};
use bro_gfx::{Point, Rect};

const BG: usize = 100;
const OPEN_BROWSER: usize = 99;

pub(crate) fn draw(v: &mut AutomationView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    v.poll(cx);
    let t = ui.t;
    let col = page(area, 740.0);
    ui.hit(area, GuiAction::View(cx.id, BG));
    ui.clip(area, 0.0);
    let mut y = col.y - v.scroll;
    let mut selected = None;
    let title = ui.para("Browser and computer", Style::new(24.0, weight::SEMIBOLD, t.text), col.w, None);
    ui.draw(&title, Point::new(col.x, y));
    y += title.h + 8.0;
    let sub = ui.para(
        "Set up once. Use the same tools in Claude, Codex, Pi and omp.",
        Style::new(size::BODY, weight::REGULAR, t.muted),
        col.w,
        None,
    );
    ui.draw(&sub, Point::new(col.x, y));
    y += sub.h + 24.0;
    let open = Rect::new(col.x, y, widgets::button_width(ui, "Open browser"), 32.0);
    widgets::button(ui, open, "Open browser", widgets::Kind::Secondary, GuiAction::View(cx.id, OPEN_BROWSER));
    y += 48.0;
    for i in 0..2 {
        let x = col.x + 20.0;
        let w = (col.w - 40.0).max(80.0);
        let heading = if i == 0 { "JustBrowse" } else { "CUA" };
        let detail = if i == 0 {
            "Your signed-in browser, with separate tabs for each agent."
        } else {
            "Desktop apps, controlled through Cua Driver."
        };
        let description = ui.para(detail, Style::new(size::BODY, weight::REGULAR, t.muted), w, None);
        let instruction = if i == 0 {
            "After installing, open chrome://extensions or edge://extensions. Turn on Developer mode, load the extension folder, then click Connect in the extension."
        } else if cfg!(target_os = "macos") {
            "Grant Accessibility and Screen Recording to CuaDriver. The setup guide walks you through the permissions."
        } else if cfg!(target_os = "linux") {
            "Use a graphical X11 or XWayland session with AT-SPI 2. The setup guide covers desktop requirements."
        } else {
            "Keep your Windows desktop signed in. CUA can work with apps running at its own privilege level."
        };
        let guide = ui.para(instruction, Style::new(size::SMALL, weight::REGULAR, t.muted), w, None);
        let checking = v.checks[i].is_some();
        let note = v.notice[i].as_ref().map(|r| {
            let (text, color) = match r {
                Ok(s) => (s, t.good),
                Err(s) => (s, t.danger),
            };
            ui.para(text, Style::new(size::SMALL, weight::MEDIUM, color), w, None)
        });
        let install = if v.installed(i) { "Update" } else { "Install" };
        let buttons = if i == 0 {
            [
                (install, B_INSTALL),
                ("Extension folder", B_FOLDER),
                (if checking { "Checking…" } else { "Check connection" }, B_CHECK),
            ]
        } else {
            [
                (install, C_INSTALL),
                ("Setup guide", C_GUIDE),
                (if checking { "Checking…" } else { "Check readiness" }, C_CHECK),
            ]
        };
        let widths: Vec<f32> = buttons.iter().map(|(label, _)| widgets::button_width(ui, label)).collect();
        let mut bx = 0.0;
        let mut by = 0.0;
        let mut positions = vec![];
        for width in &widths {
            let width = width.min(w);
            if bx > 0.0 && bx + width > w {
                bx = 0.0;
                by += 40.0;
            }
            positions.push((bx, by, width));
            bx += width + 8.0;
        }
        let h = 20.0
            + 60.0
            + description.h
            + 14.0
            + guide.h
            + 18.0
            + 32.0
            + 16.0
            + 32.0
            + by
            + note.as_ref().map_or(0.0, |n| n.h + 12.0)
            + 20.0;
        let card = Rect::new(col.x, y, col.w, h);
        page_card(ui, card);
        ui.icon(
            Rect::new(x, y + 20.0, 28.0, 28.0),
            if i == 0 { "\u{F059F}" } else { "\u{F0379}" },
            26.0,
            t.text,
        );
        ui.label(Point::new(x + 40.0, y + 18.0), heading, Style::new(18.0, weight::SEMIBOLD, t.text));
        ui.label(
            Point::new(x + 40.0, y + 43.0),
            v.status(i, cx),
            Style::new(size::SMALL, weight::MEDIUM, t.muted),
        );
        let mut cy = y + 20.0 + 60.0;
        ui.draw(&description, Point::new(x, cy));
        cy += description.h + 14.0;
        ui.draw(&guide, Point::new(x, cy));
        cy += guide.h + 18.0;
        let enabled = v.enabled(i, cx);
        let n = if i == 0 { B_ENABLE } else { C_ENABLE };
        let toggle = Rect::new(x, cy, w, 32.0);
        ui.label(
            Point::new(x, cy + 6.0),
            "Use in new sessions",
            Style::new(size::BODY, weight::MEDIUM, t.text),
        );
        let switch = widgets::switch(
            ui,
            Point::new(x + w - widgets::SWITCH_W, cy + 6.0),
            enabled,
            GuiAction::View(cx.id, n),
        );
        if v.sel == n {
            selected = Some(switch);
        }
        if cx.focused && v.sel == n {
            focus_ring(ui, switch);
        }
        ui.hit(toggle, GuiAction::View(cx.id, n));
        cy += 32.0 + 16.0;
        for (((label, n), (dx, dy, width)), index) in buttons.iter().zip(positions).zip(0..) {
            let rect = Rect::new(x + dx, cy + dy, width, 32.0);
            if v.sel == *n {
                selected = Some(rect);
            }
            let available = v.available(*n);
            if available {
                widgets::button(
                    ui,
                    rect,
                    label,
                    if index == 0 && !v.installed(i) {
                        widgets::Kind::Primary
                    } else {
                        widgets::Kind::Secondary
                    },
                    GuiAction::View(cx.id, *n),
                );
            } else {
                ui.panel(rect, radius::CONTROL, t.control.fade(0.5), t.hairline);
                ui.label_in(
                    rect,
                    label,
                    Style::new(size::BODY, weight::MEDIUM, t.faint),
                    bro_gfx::text::Align::Center,
                );
            }
            if cx.focused && v.sel == *n {
                focus_ring(ui, rect);
            }
        }
        if let Some(note) = &note {
            ui.draw(note, Point::new(x, cy + by + 32.0 + 12.0));
        }
        y = card.bottom() + 16.0;
    }
    let policy = ui.para("Bro supplies these tools to new and resumed sessions. Installing either replaces Claude and Codex's built-in browser and computer tools. Existing sessions keep their current tools until resumed.", Style::new(size::SMALL, weight::REGULAR, t.muted), col.w, None);
    ui.draw(&policy, Point::new(col.x, y + 4.0));
    y += policy.h + 24.0;
    ui.unclip();
    let max_scroll = (y + v.scroll - area.bottom() + 16.0).max(0.0);
    if v.reveal {
        if let Some(rect) = selected {
            if rect.y < area.y + 12.0 {
                v.scroll = (v.scroll - (area.y + 12.0 - rect.y)).max(0.0);
            }
            if rect.bottom() > area.bottom() - 12.0 {
                v.scroll += rect.bottom() - area.bottom() + 12.0;
            }
        }
        v.reveal = false;
        ui.anim.moving = true;
    }
    v.scroll = v.scroll.min(max_scroll);
    widgets::scrollbar(ui, area, y + v.scroll - area.y, area.h, v.scroll, 1.0);
    true
}

pub(crate) fn click(v: &mut AutomationView, n: usize, cx: &mut Cx) {
    if n == OPEN_BROWSER { cx.svc.open_browser_page(); return; }
    if n < COUNT {
        v.activate(n, cx);
    }
}

pub(crate) fn wheel(v: &mut AutomationView, down: bool) {
    v.scroll = (v.scroll + if down { 64.0 } else { -64.0 }).max(0.0);
}
