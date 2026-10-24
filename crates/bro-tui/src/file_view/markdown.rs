//! Markdown in the file preview, rendered line for line: each source line stays one line (so line numbers,
//! the target line and paging still hold) with its marks turned into styles. Headings lose their `#`s, bullets
//! become `•`, emphasis, code and links lose their marks, tables and rules are drawn with box lines. Code in
//! fences keeps the highlighter's colours. The switch between this and the source is remembered in
//! `~/.bro/v2-file-view.json`.

use super::Styled;
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use std::sync::atomic::{AtomicU8, Ordering};

/// 0 = not read yet, 1 = rendered, 2 = source
static PREF: AtomicU8 = AtomicU8::new(0);

fn pref_path() -> std::path::PathBuf {
    crate::util::bro_dir().join("v2-file-view.json")
}

/// Show Markdown rendered (the default) or as source, as last chosen.
pub fn rendered_pref() -> bool {
    match PREF.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let rendered = if cfg!(test) {
                true
            } else {
                std::fs::read_to_string(pref_path())
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v["markdown_rendered"].as_bool())
                    .unwrap_or(true)
            };
            PREF.store(if rendered { 1 } else { 2 }, Ordering::Relaxed);
            rendered
        }
    }
}

/// Remember the choice (written off the UI thread).
pub fn save_rendered_pref(rendered: bool) {
    PREF.store(if rendered { 1 } else { 2 }, Ordering::Relaxed);
    if cfg!(test) {
        return;
    }
    std::thread::spawn(move || {
        let body = serde_json::json!({ "markdown_rendered": rendered }).to_string();
        let _ = crate::util::atomic_write(&pref_path(), body.as_bytes());
    });
}

/// Render `raw` (the loaded window's lines, cleaned) using `highlighted` (the same lines highlighted) for code.
pub fn render(raw: &[String], highlighted: &[Styled], t: &Theme) -> Vec<Styled> {
    let text = Style::default().fg(t.fg);
    let faint = Style::default().fg(t.frame);
    let mut fence: Option<String> = None;
    let mut out = Vec::with_capacity(raw.len());
    for (i, line) in raw.iter().enumerate() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        // fenced code: the fences become a thin rule naming the language, the code keeps its colours
        let mark = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        if let Some(m) = mark
            && fence.as_deref().is_none_or(|open| open == m && trimmed.trim_end() == m)
        {
            let lang = trimmed[m.len()..].trim();
            let closing = fence.is_some();
            fence = if closing { None } else { Some(m.to_string()) };
            let label = if closing || lang.is_empty() { "─".repeat(40) } else { format!("── {lang} {}", "─".repeat(36usize.saturating_sub(lang.len()))) };
            out.push(vec![(faint, format!("{indent}{label}"))]);
            continue;
        }
        if fence.is_some() {
            out.push(highlighted.get(i).cloned().unwrap_or_else(|| vec![(text, line.clone())]));
            continue;
        }
        out.push(block(indent, trimmed, t));
    }
    out
}

/// One line outside code fences.
fn block(indent: &str, trimmed: &str, t: &Theme) -> Styled {
    let text = Style::default().fg(t.fg);
    let faint = Style::default().fg(t.frame);
    let accent = Style::default().fg(t.accent);
    // headings
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && (trimmed[hashes..].starts_with(' ') || trimmed[hashes..].is_empty()) {
        let style = match hashes {
            1 => accent.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            2 => accent.add_modifier(Modifier::BOLD),
            _ => text.add_modifier(Modifier::BOLD),
        };
        let mut s = inline(trimmed[hashes..].trim().trim_end_matches('#').trim_end(), style, t);
        s.insert(0, (style, indent.to_string()));
        return s;
    }
    // rules
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() >= 3 && ["-", "*", "_"].iter().any(|c| compact.chars().all(|x| x.to_string() == *c)) {
        return vec![(faint, "─".repeat(60))];
    }
    // quotes
    if let Some(rest) = trimmed.strip_prefix('>') {
        let mut s = vec![(accent, format!("{indent}│ "))];
        s.extend(inline(rest.strip_prefix(' ').unwrap_or(rest), Style::default().fg(t.muted).add_modifier(Modifier::ITALIC), t));
        return s;
    }
    // tables
    if trimmed.starts_with('|') {
        if trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
            let rule: String = trimmed.trim_end().chars().map(|c| if c == '|' { '┼' } else { '─' }).collect();
            return vec![(faint, format!("{indent}{rule}"))];
        }
        let mut s = vec![(text, indent.to_string())];
        for (n, cell) in trimmed.trim_end().split('|').enumerate() {
            if n > 0 {
                s.push((faint, "│".into()));
            }
            s.extend(inline(cell, text, t));
        }
        return s;
    }
    // lists: tasks, bullets, numbers
    for (mark, glyph) in [("- [ ] ", "☐ "), ("- [x] ", "☑ "), ("- [X] ", "☑ "), ("* [ ] ", "☐ "), ("* [x] ", "☑ ")] {
        if let Some(rest) = trimmed.strip_prefix(mark) {
            let mut s = vec![(text, indent.to_string()), (accent, glyph.to_string())];
            s.extend(inline(rest, text, t));
            return s;
        }
    }
    for mark in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(mark) {
            let mut s = vec![(text, indent.to_string()), (accent, "• ".to_string())];
            s.extend(inline(rest, text, t));
            return s;
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") ")) {
        let mut s = vec![(text, indent.to_string()), (accent, trimmed[..digits + 2].to_string())];
        s.extend(inline(&trimmed[digits + 2..], text, t));
        return s;
    }
    let mut s = vec![(text, indent.to_string())];
    s.extend(inline(trimmed, text, t));
    s
}

/// Emphasis, code spans, links and images inside a line, their marks dropped.
fn inline(src: &str, base: Style, t: &Theme) -> Styled {
    let mut out: Styled = vec![];
    let mut plain = String::new();
    let flush = |out: &mut Styled, plain: &mut String, style: Style| {
        if !plain.is_empty() {
            out.push((style, std::mem::take(plain)));
        }
    };
    let mut rest = src;
    while let Some(c) = rest.chars().next() {
        // `code`
        if c == '`' {
            let ticks = rest.chars().take_while(|&c| c == '`').count();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                flush(&mut out, &mut plain, base);
                out.push((Style::default().fg(t.inline), rest[ticks..ticks + end].to_string()));
                rest = &rest[ticks + end + ticks..];
                continue;
            }
        }
        // **strong**, __strong__, ~~strike~~, *em*, _em_
        let pairs: [(&str, Modifier); 5] = [("**", Modifier::BOLD), ("__", Modifier::BOLD), ("~~", Modifier::CROSSED_OUT), ("*", Modifier::ITALIC), ("_", Modifier::ITALIC)];
        if let Some((mark, m)) = pairs.iter().find(|(mark, _)| rest.starts_with(mark)) {
            let after = &rest[mark.len()..];
            // `_` inside words (snake_case) stays as it is
            let word_before = plain.chars().last().is_some_and(char::is_alphanumeric);
            let opens = after.chars().next().is_some_and(|c| !c.is_whitespace()) && !(mark.starts_with('_') && word_before);
            if opens && let Some(end) = after.find(mark) && end > 0 {
                flush(&mut out, &mut plain, base);
                out.extend(inline(&after[..end], base.add_modifier(*m), t));
                rest = &after[end + mark.len()..];
                continue;
            }
        }
        // [text](url) and ![alt](url)
        let image = rest.starts_with("![");
        if (c == '[' || image)
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
        {
            let label = &rest[if image { 2 } else { 1 }..close];
            flush(&mut out, &mut plain, base);
            let link = base.fg(t.accent).add_modifier(Modifier::UNDERLINED);
            if image {
                out.push((link, format!("▣ {}", if label.is_empty() { "image" } else { label })));
            } else {
                out.extend(inline(label, link, t));
            }
            rest = &rest[close + end + 1..];
            continue;
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut out, &mut plain, base);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &Styled) -> String {
        s.iter().map(|(_, t)| t.as_str()).collect()
    }

    #[test]
    fn renders_line_for_line() {
        let t = crate::theme::get("graphite");
        let src = "# Title\n\nSome **bold** and `code`, a [link](http://x) and snake_case.\n- item\n- [x] done\n1. first\n> quote\n---\n| a | b |\n|---|---|\n```rust\nlet x = 1;\n```";
        let raw: Vec<String> = src.split('\n').map(str::to_string).collect();
        let hl: Vec<Styled> = raw.iter().map(|l| vec![(Style::default(), l.clone())]).collect();
        let out = render(&raw, &hl, &t);
        assert_eq!(out.len(), raw.len(), "one line per source line");
        let got: Vec<String> = out.iter().map(text).collect();
        assert_eq!(got[0], "Title");
        assert_eq!(got[2], "Some bold and code, a link and snake_case.");
        assert!(out[2].iter().any(|(s, t)| t == "bold" && s.add_modifier.contains(Modifier::BOLD)));
        assert_eq!(got[3], "• item");
        assert_eq!(got[4], "☑ done");
        assert_eq!(got[5], "1. first");
        assert_eq!(got[6], "│ quote");
        assert!(got[7].starts_with('─'));
        assert_eq!(got[8], "│ a │ b │");
        assert!(got[10].starts_with("── rust"));
        assert_eq!(got[11], "let x = 1;");
        assert!(got[12].starts_with('─'));
    }
}
