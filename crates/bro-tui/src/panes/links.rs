//! Link targets for the displayed terminal cells: OSC 8 labels, ordinary web URLs, and paths to files that exist
//! (`src/app.rs:42`, `C:\repo\x.ts(10,4)`, `` `README.md` ``). File links are `file://` URIs with the location in
//! the fragment (`#L42C3`); clicking one opens bro's preview instead of handing it to the OS.

use bro_bridge::file_links::FileLinkMatch;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type Links = HashMap<(u16, u16), Arc<str>>;

/// `file:///C:/repo/src/app.rs#L42C3`
pub fn file_uri(path: &Path, line: Option<u32>, column: Option<u32>) -> Option<String> {
    let mut url = url::Url::from_file_path(path).ok()?;
    if let Some(line) = line {
        url.set_fragment(Some(&match column {
            Some(column) => format!("L{line}C{column}"),
            None => format!("L{line}"),
        }));
    }
    Some(url.into())
}

/// A `file:` link's path and `#L12` / `#L12C3` location (other fragments are ignored).
pub fn parse_file_uri(uri: &str) -> Option<(PathBuf, Option<u32>, Option<u32>)> {
    let url = url::Url::parse(uri).ok().filter(|u| u.scheme() == "file")?;
    let path = url.to_file_path().ok()?;
    let (mut line, mut column) = (None, None);
    if let Some(rest) = url.fragment().and_then(|f| f.strip_prefix('L')) {
        let (l, c) = rest.split_once('C').map_or((rest, None), |(l, c)| (l, Some(c)));
        line = l.parse().ok().filter(|n| *n > 0);
        column = line.and(c.and_then(|c| c.parse().ok()).filter(|n| *n > 0));
    }
    Some((path, line, column))
}

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
#[cfg(test)]
pub fn detect(screen: &vt100::Screen) -> Links {
    detect_with(screen, &mut |_| None)
}

/// [`detect`] plus file paths: `files` gets every path-like candidate and returns the link target for the
/// ones known to be files (it must answer from a cache, never the disk — this runs on every render).
/// OSC 8 links and URLs win over a candidate that overlaps them.
pub fn detect_with(screen: &vt100::Screen, files: &mut dyn FnMut(&FileLinkMatch) -> Option<Arc<str>>) -> Links {
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
            for found in bro_bridge::file_links::find_file_links(&text) {
                let covers: Vec<&(usize, usize, u16, u16, u16)> = cells.iter().filter(|(a, b, ..)| *a < found.end && *b > found.start).collect();
                let taken = covers.iter().any(|&&(_, _, r, c, width)| (c..(c + width).min(cols)).any(|column| links.contains_key(&(r, column))));
                if taken {
                    continue;
                }
                let Some(uri) = files(&found) else { continue };
                for &&(_, _, r, c, width) in &covers {
                    for column in c..(c + width).min(cols) {
                        links.insert((r, column), uri.clone());
                    }
                }
            }
            text.clear();
            cells.clear();
        }
    }
    links
}

pub fn web_urls(text: &str) -> Vec<(usize, usize, String)> {
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

/// A local file the system would run rather than show if it "opened" it: programs, installers, scripts and
/// shortcuts. bro never opens these from a link, a preview or the media panel.
pub fn runs_when_opened(path: &Path) -> bool {
    #[cfg(feature = "gui")]
    {
        bro_media::is_executable(path)
    }
    #[cfg(not(feature = "gui"))]
    {
        const PROGRAMS: &[&str] = &[
            "exe", "com", "scr", "pif", "cpl", "msc", "msi", "msp", "msix", "msixbundle", "appx", "appxbundle", "appinstaller", "application",
            "appref-ms", "gadget", "bat", "cmd", "ps1", "psm1", "psd1", "vb", "vbe", "vbs", "js", "jse", "ws", "wsf", "wsh", "hta", "sct", "lnk",
            "url", "website", "inf", "reg", "scf", "jar", "jnlp", "py", "pyw", "pyz", "pl", "rb", "sh", "command", "app", "pkg", "dmg",
        ];
        let name = path.file_name().map(|n| n.to_string_lossy().trim_end_matches(['.', ' ']).to_ascii_lowercase()).unwrap_or_default();
        name.rsplit_once('.').is_some_and(|(_, ext)| PROGRAMS.contains(&ext))
    }
}

/// Show `path` in the file manager.
pub fn reveal(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Some(dir) = path.parent() {
                let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
            }
        }
    });
}

/// Called on a worker after a deliberate click. The target is passed as data, never to a shell. A local
/// program (see [`runs_when_opened`]) is refused: opening one would run it.
pub fn open(raw: &str) -> Result<(), String> {
    let uri = target(raw).ok_or_else(|| "unsupported link".to_string())?;
    if let Some((path, _, _)) = parse_file_uri(&uri)
        && runs_when_opened(&path)
    {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        return Err(format!("bro doesn't run programs from a link or a preview · {name}"));
    }
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
    fn programs_never_open_from_a_link() {
        for p in ["C:/x/setup.exe", "C:/x/run.bat", "C:/x/tool.ps1", "C:/x/go.cmd", "C:/x/a.vbs", "C:/x/x.lnk", "C:/x/x.msi"] {
            assert!(runs_when_opened(Path::new(p)), "{p}");
            let uri = file_uri(Path::new(p), None, None).unwrap();
            assert!(open(&uri).unwrap_err().contains("doesn't run programs"), "{p}");
        }
        for p in ["C:/x/notes.md", "C:/x/photo.png", "C:/x/clip.mp4", "C:/x/report.pdf"] {
            assert!(!runs_when_opened(Path::new(p)), "{p}");
        }
    }

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

    /// Every candidate is a file at `/repo/<path>` (the cache's job is tested in file_cache).
    fn all_files(m: &FileLinkMatch) -> Option<Arc<str>> {
        file_uri(&std::env::temp_dir().join(&m.path), m.line, m.column).map(Arc::from)
    }

    #[test]
    fn file_paths_follow_wrapping_wide_cells_and_locations() {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process("界 crates/bro-tui/src/main.rs:42:7 x\r\nC:\\repo\\a.ts(10,4)".as_bytes());
        let links = detect_with(p.screen(), &mut all_files);
        assert!(!links.contains_key(&(0, 0)) && !links.contains_key(&(0, 2)));
        let first = &links[&(0, 3)];
        let (path, line, column) = parse_file_uri(first).unwrap();
        assert!(path.ends_with("main.rs"));
        assert_eq!((line, column), (Some(42), Some(7)));
        assert_eq!(&links[&(1, 0)], first, "a wrapped path stays one link");
        // 17 characters fit on the first row after "界 "; the other 14 wrap
        let wrapped_end = (1, "crates/bro-tui/src/main.rs:42:7".len() as u16 - 17 - 1);
        assert_eq!(&links[&wrapped_end], first);
        let windows = links.iter().find(|((r, _), _)| *r == 2).map(|(_, uri)| parse_file_uri(uri).unwrap()).expect("next line");
        assert_eq!((windows.1, windows.2), (Some(10), Some(4)), "(L,C) locations");
    }

    #[test]
    fn urls_and_osc_links_win_over_paths_and_unknown_paths_do_not_link() {
        let mut p = vt100::Parser::new(3, 80, 0);
        p.process(b"https://example.com/src/app.rs \x1b]8;;https://docs.example/\x07docs/readme.md\x1b]8;;\x07 src/x.rs");
        let links = detect_with(p.screen(), &mut all_files);
        assert_eq!(&*links[&(0, 22)], "https://example.com/src/app.rs");
        assert_eq!(&*links[&(0, 32)], "https://docs.example/");
        assert!(links[&(0, 47)].starts_with("file:"));
        assert!(detect_with(p.screen(), &mut |_| None).values().all(|uri| !uri.starts_with("file:")), "only confirmed files link");
        assert!(detect(p.screen()).values().all(|uri| !uri.starts_with("file:")));
    }

    #[test]
    fn file_uris_round_trip_locations() {
        let path = std::env::temp_dir().join("dir with space").join("a.rs");
        let uri = file_uri(&path, Some(3), Some(9)).unwrap();
        assert!(uri.ends_with("#L3C9") && uri.contains("%20"), "{uri}");
        assert_eq!(parse_file_uri(&uri), Some((path.clone(), Some(3), Some(9))));
        assert_eq!(parse_file_uri(&file_uri(&path, None, None).unwrap()), Some((path.clone(), None, None)));
        let other = format!("{}#section", file_uri(&path, None, None).unwrap());
        assert_eq!(parse_file_uri(&other), Some((path, None, None)));
        assert!(parse_file_uri("https://example.com/#L3").is_none());
    }
}
