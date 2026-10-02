//! Link targets for the displayed terminal cells: OSC 8 labels and ordinary web URLs.

use std::collections::HashMap;
use std::sync::Arc;

pub type Links = HashMap<(u16, u16), Arc<str>>;

/// Only ordinary browser, mail and file links are handed to the OS, never shell commands.
pub fn target(raw: &str) -> Option<String> {
    if raw.len() > 8192 || raw.chars().any(char::is_control) {
        return None;
    }
    let url = url::Url::parse(raw).ok()?;
    let allowed = match url.scheme() {
        "http" | "https" => url.host_str().is_some(),
        "mailto" => !url.path().is_empty(),
        "file" => url.to_file_path().is_ok(),
        _ => false,
    };
    allowed.then(|| url.into())
}

/// Inspect only the visible screen. Wrapped rows form one logical line; hard newlines do not.
pub fn detect(screen: &vt100::Screen) -> Links {
    let mut links = Links::new();
    let mut explicit: HashMap<&str, Option<Arc<str>>> = HashMap::new();
    let (rows, cols) = screen.size();
    let mut text = String::new();
    let mut cells = vec![];
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else { continue };
            if let Some(uri) = cell.hyperlink()
                && let Some(uri) = explicit.entry(uri).or_insert_with(|| target(uri).map(Arc::from)) {
                    links.insert((row, col), uri.clone());
                }
            if cell.is_wide_continuation() {
                continue;
            }
            let start = text.len();
            text.push_str(if cell.contents().is_empty() { " " } else { cell.contents() });
            cells.push((start, text.len(), row, col, if cell.is_wide() { 2 } else { 1 }));
        }
        if !screen.row_wrapped(row) || row + 1 == rows {
            for (start, end, uri) in web_urls(&text) {
                let uri: Arc<str> = Arc::from(uri);
                for &(a, b, r, c, width) in &cells {
                    if a < end && b > start {
                        for column in c..(c + width).min(cols) {
                            // The explicit target wins when a hyperlink's label looks like a URL.
                            links.entry((r, column)).or_insert_with(|| uri.clone());
                        }
                    }
                }
            }
            text.clear();
            cells.clear();
        }
    }
    links
}

fn web_urls(text: &str) -> Vec<(usize, usize, String)> {
    let mut found = vec![];
    let mut after = 0;
    for (start, _) in text.char_indices() {
        if start < after {
            continue;
        }
        let rest = &text[start..];
        let www = rest.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("www."));
        if !www && !["https://", "http://"].iter().any(|prefix| rest.get(..prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(prefix))) {
            continue;
        }
        if text[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | '@')) {
            continue;
        }
        let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`')).unwrap_or(rest.len());
        let mut candidate = &rest[..end];
        loop {
            let Some(last) = candidate.chars().next_back() else { break };
            let unmatched = match last {
                ')' => Some(('(', ')')),
                ']' => Some(('[', ']')),
                '}' => Some(('{', '}')),
                _ => None,
            }.is_some_and(|(open, close)| candidate.matches(close).count() > candidate.matches(open).count());
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') || unmatched {
                candidate = &candidate[..candidate.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        let normalized = if www { format!("https://{candidate}") } else { candidate.to_string() };
        if let Some(uri) = target(&normalized) {
            after = start + candidate.len();
            found.push((start, after, uri));
        }
    }
    found
}

/// Called on a worker after a deliberate click. The target is passed as data, never to a shell.
pub fn open(raw: &str) -> Result<(), String> {
    let uri = target(raw).ok_or_else(|| "unsupported link".to_string())?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let wide: Vec<u16> = uri.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        // SAFETY: both strings are NUL-terminated and live for the duration of the call.
        let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) } as isize;
        if result <= 32 { Err(format!("couldn't open link (Windows error {result})")) } else { Ok(()) }
    }
    #[cfg(not(windows))]
    {
        let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let status = std::process::Command::new(program).arg(uri).status().map_err(|e| format!("couldn't open link: {e}"))?;
        if status.success() { Ok(()) } else { Err("couldn't open link in the default application".into()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_links_trim_prose_punctuation_but_keep_balanced_url_syntax() {
        let urls = web_urls("See (https://example.com/a_(b)?q=one&two=2#here), www.example.org. and http://localhost:3000/path.");
        assert_eq!(urls.iter().map(|(_, _, url)| url.as_str()).collect::<Vec<_>>(), [
            "https://example.com/a_(b)?q=one&two=2#here", "https://www.example.org/", "http://localhost:3000/path",
        ]);
        assert!(target("javascript:alert(1)").is_none());
        assert!(target("data:text/html,hi").is_none());
        assert!(target("https://example.com/\ncommand").is_none());
        assert!(target("file:///C:/project/README.md").is_some());
        assert_eq!(target("https://example.com/?q=hello&next=world").unwrap(), "https://example.com/?q=hello&next=world");
    }

    #[test]
    fn visible_links_follow_wrapping_and_unicode_cell_coordinates() {
        let mut p = vt100::Parser::new(6, 24, 100);
        p.process("界 https://example.com/a/very/long/path\r\nnext".as_bytes());
        let links = detect(p.screen());
        assert!(!links.contains_key(&(0, 0)));
        assert!(!links.contains_key(&(0, 1)));
        assert_eq!(&*links[&(0, 3)], "https://example.com/a/very/long/path");
        assert_eq!(&*links[&(1, 4)], "https://example.com/a/very/long/path");
        assert!(!links.contains_key(&(2, 0)), "hard line breaks end URLs");
    }

    #[test]
    fn osc_links_survive_chunking_styling_and_wide_labels() {
        let mut p = vt100::Parser::new(6, 24, 100);
        let output = "\x1b]8;id=docs;https://example.com/a;b?x=1&y=2\x1b\\界\x1b[0m docs\x1b]8;;\x07 plain";
        for byte in output.as_bytes() {
            p.process(&[*byte]);
        }
        let links = detect(p.screen());
        for col in 0..7 {
            assert_eq!(&*links[&(0, col)], "https://example.com/a;b?x=1&y=2");
        }
        assert!(!links.contains_key(&(0, 7)), "closing OSC ends the link");
        p.process(b"\rX\x1b[K");
        assert!(detect(p.screen()).is_empty(), "overwriting and erasing remove stale links");
    }

    #[test]
    fn link_labels_keep_their_targets_in_scrollback_and_alternate_screens() {
        let mut p = vt100::Parser::new(3, 30, 100);
        p.process(b"\x1b]8;;https://example.com/first\x07docs\x1b]8;;\x07\r\n\x1b]8;;https://example.com/second\x07docs\x1b]8;;\x07");
        assert_eq!(&*detect(p.screen())[&(0, 0)], "https://example.com/first");
        assert_eq!(&*detect(p.screen())[&(1, 0)], "https://example.com/second");
        p.process(b"\x1b[?1049hother screen");
        assert!(detect(p.screen()).is_empty());
        p.process(b"\x1b[?1049l\r\nline 3\r\nline 4\r\nline 5");
        assert!(detect(p.screen()).is_empty());
        p.screen_mut().set_scrollback(2);
        assert_eq!(&*detect(p.screen())[&(0, 0)], "https://example.com/first");
        p.screen_mut().set_size(4, 40);
        assert!(detect(p.screen()).values().any(|uri| uri.as_ref() == "https://example.com/first"));
    }

    #[test]
    fn explicit_target_wins_over_a_url_shaped_label_and_moves_with_cells() {
        let mut p = vt100::Parser::new(3, 40, 0);
        p.process(b"\x1b]8;;https://actual.example/\x07https://label.example/\x1b]8;;\x07");
        assert_eq!(&*detect(p.screen())[&(0, 0)], "https://actual.example/");
        p.process(b"\r\x1b[2@");
        let links = detect(p.screen());
        assert!(!links.contains_key(&(0, 0)));
        assert_eq!(&*links[&(0, 2)], "https://actual.example/");
        p.process(b"\x1bc");
        assert!(detect(p.screen()).is_empty());
    }
}
