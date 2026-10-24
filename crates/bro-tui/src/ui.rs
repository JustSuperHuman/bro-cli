//! Shared drawing: the rounded frame with its title set into the border (oriel's look), popups, icons with
//! ASCII fallbacks, the logo, meters, harness brand colours and small text helpers.

use crate::theme::{Theme, mix};
use bro_core::Harness;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
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
    ("stats", "\u{F0128}", "▥"),
    ("info", "\u{F02FD}", "ⓘ"),
    ("cloud", "\u{F0163}", "≋"),
    ("theme", "\u{F03D8}", "&"),
    ("bell", "\u{F009A}", "!"),
    ("sound", "\u{F057E}", "♪"),
    ("split", "\u{F0E4D}", "|"),
    ("close", "\u{F0156}", "×"),
    ("tab", "\u{F04E9}", "+"),
    ("quit", "\u{F0206}", "⏻"),
    ("folder", "\u{F0256}", "▸"),
    ("file", "\u{F0214}", "▤"),
    ("account", "\u{F0004}", "@"),
    ("proxy", "\u{F04E1}", "⇄"),
    ("bridge", "\u{F05A9}", "⌁"),
    ("rocket", "\u{F0463}", "»"),
    ("help", "\u{F02D7}", "?"),
    ("key", "\u{F0306}", "⌘"),
    ("settings", "\u{F0493}", "⚙"),
    ("caveman", "⛏", "⛏"),
    ("mic", "\u{F036C}", "♪"),
    ("watchdog", "\u{F0A43}", "◉"),
    ("history", "\u{F02DA}", "↺"),
    ("branch", "\u{E725}", "⎇"),
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
    // the real logo, when the terminal can show images (see icons.rs)
    if let Some(m) = crate::icons::marker(h) {
        return m;
    }
    icon(match h {
        Some(Harness::Claude) => "claude",
        Some(Harness::Codex) => "codex",
        Some(Harness::Pi) => "pi",
        Some(Harness::Omp) => "omp",
        None => "term",
    })
}

/// Glyph and brand colour for an API provider id ("openrouter", "my-gemini"), so a row reads as its brand at a
/// glance: the provider's real logo when the terminal shows images (see icons.rs), else a one-cell symbol every
/// font has. Anthropic and OpenAI reuse the Claude / Codex marks. Unknown providers get `◇` in `fallback`.
pub fn provider_glyph(id: &str, fallback: Color, t: &Theme) -> (&'static str, Color) {
    let id = id.to_ascii_lowercase();
    let has = |k: &str| id.contains(k);
    if has("anthropic") || has("claude") {
        return (harness_glyph(Some(Harness::Claude)), harness_color(Some(Harness::Claude), t));
    }
    if has("openai") || has("chatgpt") || has("codex") {
        return (harness_glyph(Some(Harness::Codex)), harness_color(Some(Harness::Codex), t));
    }
    // (logo, colour of its text glyph)
    let (logo, color) = if has("gemini") || has("google") || has("vertex") {
        ("gemini", Color::Rgb(0x6F, 0x9B, 0xF7))
    } else if has("openrouter") {
        ("openrouter", Color::Rgb(0x8B, 0x8D, 0xF8))
    } else if has("deepseek") {
        ("deepseek", Color::Rgb(0x4D, 0x6B, 0xFE))
    } else if has("groq") {
        ("groq", Color::Rgb(0xF5, 0x50, 0x36))
    } else if has("mistral") || has("codestral") {
        ("mistral", Color::Rgb(0xFA, 0x82, 0x1F))
    } else if has("kimi") {
        ("kimi", t.fg)
    } else if has("moonshot") {
        ("moonshot", t.fg)
    } else if has("grok") {
        ("grok", t.fg)
    } else if has("xai") {
        ("xai", t.fg)
    } else if has("zhipu") || has("glm") || has("bigmodel") {
        ("zhipu", Color::Rgb(0x3B, 0x5B, 0xFD))
    } else if has("zai") || has("z.ai") {
        ("zai", t.fg)
    } else if has("meta") || has("llama") && !has("ollama") {
        ("meta", Color::Rgb(0x1D, 0x7C, 0xF2))
    } else if has("ollama") {
        ("ollama", t.fg)
    } else if has("lmstudio") || has("lm-studio") {
        ("lmstudio", t.fg)
    } else if has("qwen") || has("dashscope") || has("alibaba") {
        ("qwen", Color::Rgb(0x61, 0x5C, 0xED))
    } else if has("sakana") {
        ("sakana", Color::Rgb(0xE5, 0x48, 0x4D))
    } else {
        return ("◇", fallback);
    };
    (crate::icons::glyph(logo), color)
}

/// The frame: rounded hairline border, bold title set into the top edge, an optional status on the right of the
/// top edge and an optional subtitle bottom-right. Returns the inner rect.
pub fn frame_ex(f: &mut Frame, area: Rect, title: Line<'_>, status: Option<Line<'_>>, subtitle: Option<&str>, focused: bool, t: &Theme) -> Rect {
    let border = if focused { t.accent } else { t.frame };
    let set = if focused { t.skin.focus } else { t.skin.border };
    let title = crate::skin::title(title, border, t);
    let mut block = Block::default().borders(Borders::ALL).border_set(set).border_style(Style::default().fg(border)).title(title);
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

/// Project-colored session chrome. The active pane has a heavy outline and a full-width
/// title band; terminal output keeps its own colors and every original row/column.
///
/// `standout` is the orchestrator's frame: a double border that keeps most of its color even when another
/// session has focus. `status_up` puts the status at the right of the title band (leaving room for the close
/// button) instead of on the bottom edge, which shared borders hand to the session below.
#[allow(clippy::too_many_arguments)]
pub fn session_frame_as(f: &mut Frame, area: Rect, title: Line<'_>, status: Option<Line<'_>>, subtitle: Option<&str>, project: &str, minimize: Option<bool>, minimize_hot: bool, color: Color, focused: bool, standout: bool, status_up: bool, t: &Theme) -> Rect {
    let border = match (focused, standout) {
        (true, _) => color,
        (false, true) => mix(color, t.frame, 0.8),
        (false, false) => mix(color, t.frame, 0.45),
    };
    let set = match (standout, focused) {
        (true, _) => ratatui::symbols::border::DOUBLE,
        (false, true) => t.skin.session_focus,
        (false, false) => t.skin.border,
    };
    let block = Block::default().borders(Borders::ALL)
        .border_set(set)
        .border_style(fg(border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if area.width < 3 || area.height < 2 {
        return inner;
    }
    let head = Rect { x: area.x + 1, y: area.y, width: area.width - 2, height: 1 };
    let style = band_style(color, focused, t);
    f.render_widget(Block::default().style(style), head);
    let marker = if focused { t.skin.marker } else { " " };
    // the title brings its own leading space
    let lead = if project.is_empty() { marker.to_string() } else { format!("{marker} {} ·", fit(project, (head.width as usize / 3).max(6))) };
    let mut spans = vec![];
    if let Some(minimized) = minimize {
        let button = if minimized { " + " } else { " − " };
        let button_style = if minimize_hot { style.add_modifier(Modifier::REVERSED) } else { style };
        spans.push(Span::styled(button, button_style));
    }
    spans.push(Span::styled(lead, style));
    spans.extend(title.spans.into_iter().map(|s| Span::styled(s.content, style)));
    if status_up {
        // on the band: the focused one's ink, else each part's own color; three cells kept for the close button
        let right: Vec<Span> = status.map(|s| s.spans).unwrap_or_default().into_iter().map(|s| Span::styled(s.content, if focused { style } else { s.style.bg(crate::theme::project_tint(color, t)) })).collect();
        line_lr(f, Rect { width: head.width.saturating_sub(3), ..head }, spans, right);
        if let Some(s) = subtitle {
            let foot = Rect { x: area.x + 1, y: area.bottom() - 1, width: area.width.saturating_sub(7), height: 1 };
            line_lr(f, foot, vec![], vec![Span::styled(format!(" {s} "), fg(color))]);
        }
        return inner;
    }
    line(f, head, spans);
    // Status stays in its semantic color on the bottom edge. Reserve the close button on the right.
    let foot = Rect { x: area.x + 1, y: area.bottom() - 1, width: area.width.saturating_sub(7), height: 1 };
    let right = subtitle.map(|s| vec![Span::styled(format!(" {s} "), fg(color))]).unwrap_or_default();
    line_lr(f, foot, status.map(|s| s.spans).unwrap_or_default(), right);
    inner
}

/// A session's title band: the project color with dark ink when focused, else the color on its tint.
pub fn band_style(color: Color, focused: bool, t: &Theme) -> Style {
    if focused { fg(ink_on(color, t)).bg(color).add_modifier(Modifier::BOLD) } else { fg(color).bg(crate::theme::project_tint(color, t)) }
}

/// Text that reads on a solid `color`: whichever of dark ink or white contrasts more (WCAG), so a yellow project
/// in a light theme gets dark text. A named terminal colour can't be measured: dark themes' bands take the dark
/// ink, light ones white.
pub fn ink_on(color: Color, t: &Theme) -> Color {
    let Color::Rgb(r, g, b) = color else { return if t.is_light() { Color::Rgb(255, 255, 255) } else { t.deep() } };
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let lum = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    // white and near-black contrast equally at ~0.19
    if lum > 0.19 { t.deep() } else { Color::Rgb(255, 255, 255) }
}

/// Where tiles share a border line, turn the corners they drew over each other into joins (┬ ┴ ├ ┤ ┼). Only cells
/// on two or more of `rects`' edges that still hold a line glyph change, so titles on a shared edge stay.
pub fn join_borders(buf: &mut ratatui::buffer::Buffer, rects: &[Rect]) {
    const UP: u8 = 1;
    const DOWN: u8 = 2;
    const LEFT: u8 = 4;
    const RIGHT: u8 = 8;
    let mut arms: std::collections::HashMap<(u16, u16), (u8, u8)> = std::collections::HashMap::new();
    for r in rects.iter().filter(|r| r.width >= 2 && r.height >= 2) {
        let (x1, y1) = (r.right() - 1, r.bottom() - 1);
        let mut add = |x: u16, y: u16, a: u8| {
            let e = arms.entry((x, y)).or_default();
            e.0 |= a;
        };
        for x in r.x..=x1 {
            let a = if x > r.x { LEFT } else { 0 } | if x < x1 { RIGHT } else { 0 };
            add(x, r.y, a);
            add(x, y1, a);
        }
        for y in r.y..=y1 {
            let a = if y > r.y { UP } else { 0 } | if y < y1 { DOWN } else { 0 };
            add(r.x, y, a);
            add(x1, y, a);
        }
        // how many tiles' edges pass through each cell
        let mut seen = std::collections::HashSet::new();
        for x in r.x..=x1 {
            seen.insert((x, r.y));
            seen.insert((x, y1));
        }
        for y in r.y..=y1 {
            seen.insert((r.x, y));
            seen.insert((x1, y));
        }
        for cell in seen {
            arms.entry(cell).or_default().1 += 1;
        }
    }
    let area = buf.area;
    for ((x, y), (a, n)) in arms {
        if n < 2 || !area.contains(Position { x, y }) {
            continue;
        }
        let Some(cell) = buf.cell_mut(Position { x, y }) else { continue };
        if !"─│╭╮╰╯┌┐└┘━┃┏┓┗┛═║╔╗╚╝├┤┬┴┼".contains(cell.symbol()) || cell.symbol().is_empty() || cell.symbol() == " " {
            continue;
        }
        let glyph = match a {
            x if x == UP | DOWN | LEFT | RIGHT => "┼",
            x if x == UP | DOWN | RIGHT => "├",
            x if x == UP | DOWN | LEFT => "┤",
            x if x == LEFT | RIGHT | DOWN => "┬",
            x if x == LEFT | RIGHT | UP => "┴",
            x if x == UP | DOWN => "│",
            x if x == LEFT | RIGHT => "─",
            _ => continue,
        };
        cell.set_symbol(glyph);
    }
}

/// The frame around a group of tiled sessions: a rule in the project color along the top with what they share
/// and how many there are set into it. The ring's other sides stay blank, a gutter rather than a second border
/// beside each session's own. The group holding the focused session is drawn at full strength, the others
/// softened so the eye finds the active one.
pub fn group_frame(f: &mut Frame, area: Rect, project: &str, sessions: usize, color: Color, focused: bool, t: &Theme) {
    let border = if focused { color } else { mix(color, t.frame, 0.5) };
    if area.height == 0 {
        return;
    }
    let horizontal = t.skin.border.horizontal_top;
    line(f, Rect { height: 1, ..area }, vec![Span::styled(horizontal.repeat(area.width as usize), fg(border))]);
    if area.width < 8 {
        return;
    }
    let name = if project.is_empty() { "other" } else { project };
    let count = format!("{sessions} session{}", if sessions == 1 { "" } else { "s" });
    let room = area.width.saturating_sub(6) as usize;
    let mut spans = vec![Span::styled(format!(" ● {} ", fit(name, room.saturating_sub(count.len() + 3).max(4))), fg(border).add_modifier(Modifier::BOLD))];
    if room > name.chars().count() + count.len() + 8 {
        spans.push(Span::styled(format!("· {count} "), fg(if focused { mix(color, t.fg, 0.35) } else { t.muted })));
    }
    line(f, Rect { x: area.x + 2, y: area.y, width: area.width - 4, height: 1 }, spans);
}

/// A centered floating box (palette, launcher, dialogs). Clears what's under it and returns the inner rect.
pub fn popup(f: &mut Frame, screen: Rect, w: u16, h: u16, title: &str, t: &Theme) -> Rect {
    let r = centered(screen, w, h);
    clear(f, r, t);
    frame(f, r, title, None, true, t)
}

/// Blank `r` for an overlay: the terminal's own background, or a painted theme's.
pub fn clear(f: &mut Frame, r: Rect, t: &Theme) {
    f.render_widget(Clear, r);
    if t.painted() {
        f.render_widget(Block::default().style(Style::default().bg(t.bg).fg(t.fg)), r);
    }
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
