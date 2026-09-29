//! Headless testing (from z4-oriel): render into ratatui's `TestBackend`, dump plain text, and save coloured
//! HTML snapshots to `crates/bro-tui/snapshots/` (open them in a browser, or screenshot them for docs).
//!
//! Tests never touch the real `~/.bro`: [`isolate`] points `BRO_DIR` (and the pool/codex dirs) at a temp dir.

#![cfg(test)]

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use std::sync::OnceLock;

/// Point bro's data dirs at a per-process temp dir (idempotent).
pub fn isolate() {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("bro-tui-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        // SAFETY: set once, before any test reads these (OnceLock), and only in tests
        unsafe {
            std::env::set_var("BRO_DIR", d.join(".bro"));
            std::env::set_var("CLAUDE_POOL_DIR", d.join(".claude-max-pool"));
            std::env::set_var("BRO_CODEX_PROFILES_DIR", d.join("codex-profiles"));
        }
        d
    });
}

/// Where snapshots go.
pub fn snapshot_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots")
}

/// Render `draw` at w×h; returns the text dump and saves `snapshots/<name>.html`.
pub fn snapshot(name: &str, w: u16, h: u16, draw: impl FnOnce(&mut ratatui::Frame)) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    term.draw(draw).expect("draw");
    let buf = term.backend().buffer().clone();
    save_html(&buf, &snapshot_dir().join(format!("{name}.html")));
    dump(&buf)
}

/// Plain text, one line per row (trailing spaces trimmed).
pub fn dump(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Write a rendered buffer as a coloured HTML page (terminal black, Cascadia-ish mono).
pub fn save_html(buf: &Buffer, path: &std::path::Path) {
    use ratatui::style::{Color, Modifier};
    fn css(c: Color, fallback: &str) -> String {
        const ANSI: [&str; 16] = ["#1e1e1e", "#e05a5a", "#6fbf73", "#e6b673", "#4aa8d4", "#bd93f9", "#56c8d8", "#cfcfcf", "#6e6e6e", "#ff7b72", "#a6e3a1", "#ffd580", "#7fd0ff", "#d6acff", "#8be9fd", "#ffffff"];
        match c {
            Color::Reset => fallback.into(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Indexed(i) if i < 16 => ANSI[i as usize].into(),
            Color::Indexed(i) if i >= 232 => {
                let v = 8 + (i - 232) * 10;
                format!("#{v:02x}{v:02x}{v:02x}")
            }
            Color::Indexed(i) => {
                let i = i - 16;
                let f = |x: u8| if x == 0 { 0 } else { 55 + x * 40 };
                format!("#{:02x}{:02x}{:02x}", f(i / 36), f((i / 6) % 6), f(i % 6))
            }
            Color::Black => ANSI[0].into(),
            Color::Red => ANSI[1].into(),
            Color::Green => ANSI[2].into(),
            Color::Yellow => ANSI[3].into(),
            Color::Blue => ANSI[4].into(),
            Color::Magenta => ANSI[5].into(),
            Color::Cyan => ANSI[6].into(),
            Color::Gray => ANSI[7].into(),
            Color::DarkGray => ANSI[8].into(),
            Color::LightRed => ANSI[9].into(),
            Color::LightGreen => ANSI[10].into(),
            Color::LightYellow => ANSI[11].into(),
            Color::LightBlue => ANSI[12].into(),
            Color::LightMagenta => ANSI[13].into(),
            Color::LightCyan => ANSI[14].into(),
            Color::White => ANSI[15].into(),
        }
    }
    let mut h = String::from(
        "<!doctype html><meta charset=utf-8><title>bro snapshot</title><style>body{margin:0;background:#0c0c0c}pre{margin:0;padding:10px;font:15px/1.22 'CaskaydiaCove Nerd Font Mono','Cascadia Mono NF','Cascadia Mono',Consolas,monospace;color:#d8d8d8}span{white-space:pre}</style><pre>",
    );
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let (mut fg, mut bg) = (css(c.fg, "#d8d8d8"), css(c.bg, "transparent"));
            if c.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
                if fg == "transparent" {
                    fg = "#0c0c0c".into();
                }
            }
            let mut st = String::new();
            if c.modifier.contains(Modifier::BOLD) {
                st.push_str("font-weight:bold;");
            }
            if c.modifier.contains(Modifier::DIM) {
                // fade the text only (opacity would fade a painted background too)
                fg = format!("color-mix(in srgb, {fg} 60%, transparent)");
            }
            if c.modifier.contains(Modifier::ITALIC) {
                st.push_str("font-style:italic;");
            }
            if c.modifier.contains(Modifier::UNDERLINED) {
                st.push_str("text-decoration:underline;");
            }
            let sym = c.symbol().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            h.push_str(&format!("<span style=\"color:{fg};background:{bg};{st}\">{sym}</span>"));
        }
        h.push('\n');
    }
    h.push_str("</pre>");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, h);
}
