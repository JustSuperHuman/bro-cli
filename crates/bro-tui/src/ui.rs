//! Shared drawing: the rounded frame with its title set into the border (oriel's look), popups, icons with
//! ASCII fallbacks, the logo, meters, harness brand colours and small text helpers.

use crate::theme::{Theme, mix};
use bro_core::Harness;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use std::sync::atomic::{AtomicBool, Ordering};

/// false = plain-text fallbacks for terminals without a Nerd Font.
pub static NERD: AtomicBool = AtomicBool::new(true);

// (name, nerd font glyph, fallback). Material Design codepoints from nerd-fonts glyphnames.json.
const ICONS: &[(&str, &str, &str)] = &[
    ("window", "\u{F05AF}", "#"),
    ("term", "\u{F018D}", "❯"),
    ("claude", "\u{F0674}", "✻"),
    ("codex", "\u{F0169}", "◎"),
    ("pi", "π", "π"),
    ("omp", "\u{F06A9}", "◆"),
    ("search", "\u{F0349}", "›"),
    ("gauge", "\u{F029A}", "◔"),
    ("cloud", "\u{F0163}", "≋"),
    ("theme", "\u{F03D8}", "&"),
    ("bell", "\u{F009A}", "!"),
    ("split", "\u{F0E4D}", "|"),
    ("close", "\u{F0156}", "×"),
    ("tab", "\u{F04E9}", "+"),
    ("quit", "\u{F0206}", "⏻"),
    ("folder", "\u{F0256}", "▸"),
    ("account", "\u{F0004}", "@"),
    ("proxy", "\u{F04E1}", "⇄"),
    ("bridge", "\u{F05A9}", "⌁"),
    ("rocket", "\u{F0463}", "»"),
    ("help", "\u{F02D7}", "?"),
    ("key", "\u{F0306}", "⌘"),
    ("history", "\u{F02DA}", "↺"),
];

/// The icon for `name` (Nerd Font glyph or its fallback, per [`NERD`]).
pub fn icon(name: &str) -> &'static str {
    let nerd = NERD.load(Ordering::Relaxed);
    ICONS.iter().find(|i| i.0 == name).map(|i| if nerd { i.1 } else { i.2 }).unwrap_or("")
}

/// Icon + space, or nothing.
pub fn lead(name: &str) -> String {
    let g = icon(name);
    if g.is_empty() { String::new() } else { format!("{g} ") }
}

/// Brand colour for a harness (shells get the muted colour).
pub fn harness_color(h: Option<Harness>, t: &Theme) -> Color {
    match h {
        Some(Harness::Claude) => Color::Rgb(0xD9, 0x77, 0x57),
        Some(Harness::Codex) => Color::Rgb(0x2E, 0xC4, 0x9A),
        Some(Harness::Pi) => Color::Rgb(0xA7, 0x8B, 0xFA),
        Some(Harness::Omp) => Color::Rgb(0x60, 0xA5, 0xFA),
        None => t.muted,
    }
}

/// Glyph for a harness (a shell prompt for plain shells).
pub fn harness_glyph(h: Option<Harness>) -> &'static str {
    icon(match h {
        Some(Harness::Claude) => "claude",
        Some(Harness::Codex) => "codex",
        Some(Harness::Pi) => "pi",
        Some(Harness::Omp) => "omp",
        None => "term",
    })
}

/// The frame: rounded hairline border, bold title set into the top edge, an optional status on the right of the
/// top edge and an optional subtitle bottom-right. Returns the inner rect.
pub fn frame_ex(f: &mut Frame, area: Rect, title: Line<'_>, status: Option<Line<'_>>, subtitle: Option<&str>, focused: bool, t: &Theme) -> Rect {
    let border = if focused { t.accent } else { t.frame };
    let mut block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border)).title(title);
    if let Some(s) = status {
        block = block.title(s.right_aligned());
    }
    if let Some(s) = subtitle {
        block = block.title_bottom(Line::from(Span::styled(format!(" {s} "), Style::default().fg(t.muted))).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// [`frame_ex`] with a plain string title.
pub fn frame(f: &mut Frame, area: Rect, title: &str, subtitle: Option<&str>, focused: bool, t: &Theme) -> Rect {
    let style = Style::default().fg(if focused { t.accent } else { t.muted }).add_modifier(Modifier::BOLD);
    let title = if title.is_empty() { Line::default() } else { Line::from(Span::styled(format!(" {title} "), style)) };
    frame_ex(f, area, title, None, subtitle, focused, t)
}

/// A centered floating box (palette, launcher, dialogs). Clears what's under it and returns the inner rect.
pub fn popup(f: &mut Frame, screen: Rect, w: u16, h: u16, title: &str, t: &Theme) -> Rect {
    let r = centered(screen, w, h);
    f.render_widget(Clear, r);
    frame(f, r, title, None, true, t)
}

/// The rect of a `w`×`h` box centered horizontally, a third of the way down.
pub fn centered(screen: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(screen.width.saturating_sub(2));
    let h = h.min(screen.height.saturating_sub(2));
    Rect { x: screen.x + (screen.width - w) / 2, y: screen.y + (screen.height.saturating_sub(h)) / 3, width: w, height: h }
}

pub fn muted(t: &Theme) -> Style {
    Style::default().fg(t.muted)
}
pub fn accent(t: &Theme) -> Style {
    Style::default().fg(t.accent)
}
pub fn bold_accent(t: &Theme) -> Style {
    Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
}
pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}
pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Display width of a string.
pub fn width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Truncate to `w` display columns with an ellipsis.
pub fn fit(s: &str, w: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// `s` truncated/padded to exactly `w` columns.
pub fn pad(s: &str, w: usize) -> String {
    let s = fit(s, w);
    let n = width(&s);
    format!("{s}{}", " ".repeat(w.saturating_sub(n)))
}

/// Draw one line of spans into a 1-row rect.
pub fn line(f: &mut Frame, r: Rect, spans: Vec<Span<'_>>) {
    if r.height == 0 || r.width == 0 {
        return;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), Rect { height: 1, ..r });
}

/// Left spans plus right-aligned spans in one row (right side wins when it's tight).
pub fn line_lr(f: &mut Frame, r: Rect, left: Vec<Span<'_>>, right: Vec<Span<'_>>) {
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    let lw = (r.width as usize).saturating_sub(rw);
    let mut spans = vec![];
    let mut used = 0;
    for s in left {
        let room = lw.saturating_sub(used);
        if room == 0 {
            break;
        }
        let text = fit(&s.content, room);
        used += width(&text);
        spans.push(Span::styled(text, s.style));
    }
    spans.push(Span::raw(" ".repeat(lw.saturating_sub(used))));
    spans.extend(right);
    line(f, r, spans);
}

/// A thin horizontal rule in the frame colour, with an optional label: "── usage ─────".
pub fn rule(f: &mut Frame, r: Rect, label: &str, t: &Theme) {
    let w = r.width as usize;
    if label.is_empty() {
        line(f, r, vec![Span::styled("─".repeat(w), fg(t.frame))]);
        return;
    }
    let head = format!("── {label} ");
    let rest = w.saturating_sub(width(&head));
    line(f, r, vec![Span::styled("── ", fg(t.frame)), Span::styled(format!("{label} "), muted(t).add_modifier(Modifier::BOLD)), Span::styled("─".repeat(rest), fg(t.frame))]);
}

/// Colour for a usage percentage: calm, warm, hot.
pub fn pct_color(pct: f32, t: &Theme) -> Color {
    if pct >= 85.0 {
        t.danger
    } else if pct >= 60.0 {
        t.inline
    } else {
        t.good
    }
}

/// A smooth bar using eighth-blocks, `w` columns wide.
pub fn bar(pct: f32, w: usize, t: &Theme) -> Vec<Span<'static>> {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = ((pct.clamp(0.0, 100.0) / 100.0) * (w * 8) as f32).round() as usize;
    let full = eighths / 8;
    let part = PARTS[eighths % 8];
    let used = full + usize::from(!part.is_empty());
    let c = pct_color(pct, t);
    vec![
        Span::styled(format!("{}{part}", "█".repeat(full)), fg(c)),
        Span::styled("░".repeat(w.saturating_sub(used)), fg(mix(t.frame, Color::Rgb(0, 0, 0), 0.2))),
    ]
}

/// Keyboard hint spans: "key what · key what".
pub fn hints(pairs: &[(&str, &str)], t: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![];
    for (i, (k, what)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", muted(t)));
        }
        spans.push(Span::styled(k.to_string(), Style::default().fg(t.shine).add_modifier(Modifier::BOLD)));
        if !what.is_empty() {
            spans.push(Span::styled(format!(" {what}"), muted(t)));
        }
    }
    spans
}

/// Draw hints on the last row of `area`; returns the rect above.
pub fn hint_line(f: &mut Frame, area: Rect, pairs: &[(&str, &str)], t: &Theme) -> Rect {
    if area.height < 2 {
        return area;
    }
    let mut spans = vec![Span::raw(" ")];
    spans.extend(hints(pairs, t));
    line(f, Rect { y: area.bottom() - 1, height: 1, ..area }, spans);
    Rect { height: area.height - 1, ..area }
}

// ------------------------------------------------------------------ logo
const LOGO: [&str; 6] = [
    "██████╗ ██████╗  ██████╗ ",
    "██╔══██╗██╔══██╗██╔═══██╗",
    "██████╔╝██████╔╝██║   ██║",
    "██╔══██╗██╔══██╗██║   ██║",
    "██████╔╝██║  ██║╚██████╔╝",
    "╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ",
];

/// Draw the big logo centered in `area`; returns the rows used. Rainbow on animated themes, else an
/// accent→shine gradient.
pub fn logo(f: &mut Frame, area: Rect, t: &Theme, time: f64) -> u16 {
    let w = LOGO[0].chars().count() as u16;
    if area.width < w + 2 || area.height < LOGO.len() as u16 {
        line(f, area, vec![Span::styled("bro", bold_accent(t))]);
        return 1;
    }
    let x = area.x + (area.width - w) / 2;
    for (r, row) in LOGO.iter().enumerate() {
        let spans: Vec<Span> = row
            .chars()
            .enumerate()
            .map(|(i, c)| {
                let col = if t.animated {
                    crate::theme::rainbow_at((i as f64 + r as f64 * 0.8) * 0.018 - time * 0.06, 0.5)
                } else {
                    mix(t.accent, t.shine, (i as f32 / w as f32) * 0.8)
                };
                let st = if c == '█' { fg(col) } else { fg(mix(col, Color::Rgb(40, 40, 50), 0.45)) };
                Span::styled(c.to_string(), st)
            })
            .collect();
        line(f, Rect { x, y: area.y + r as u16, width: w, height: 1 }, spans);
    }
    LOGO.len() as u16
}

/// Title text in the accent colour (the rainbow is kept for the welcome logo only).
pub fn title_spans(text: &str, t: &Theme, _time: f64) -> Vec<Span<'static>> {
    vec![Span::styled(text.to_string(), bold_accent(t))]
}

/// Colour for a "% left" figure: red at 20 or below, amber at 50 or below, else green (v1 `leftFigure`).
pub fn left_color(left: f64, t: &Theme) -> Color {
    // a fixed amber: some palettes use the same pink for "inline" and "danger"
    const AMBER: Color = Color::Rgb(0xff, 0xb8, 0x6c);
    if left <= 20.0 {
        t.danger
    } else if left <= 50.0 {
        AMBER
    } else {
        t.good
    }
}

/// A fuel-gauge meter of what's left (`n` segments), coloured like [`left_color`].
pub fn left_meter(left: f64, n: usize, t: &Theme) -> Vec<Span<'static>> {
    let filled = ((left.clamp(0.0, 100.0) / 100.0) * n as f64).round() as usize;
    vec![Span::styled("▰".repeat(filled), fg(left_color(left, t))), Span::styled("▱".repeat(n - filled.min(n)), fg(t.frame))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_and_pad() {
        assert_eq!(fit("hello", 10), "hello");
        assert_eq!(fit("hello world", 6), "hello…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(width(&pad("abcdefgh", 4)), 4);
        assert!(LOGO.iter().all(|r| r.chars().count() == LOGO[0].chars().count()));
    }
}
