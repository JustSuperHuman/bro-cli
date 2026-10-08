//! File path candidates in terminal or chat text: Rust port of Just Terminal's
//! `src/lib/fileLinks.ts::findFileLinks` (same rules, byte offsets instead of
//! UTF-16 ones). Candidates are only text matches; whether a file exists is the
//! resolver's job ([`crate::file_preview::resolve_path`]).
//!
//! Recognised: Markdown link targets, quoted/backticked paths (spaces kept),
//! bare paths with a separator, bare `name.ext`, `file://` URLs, and the
//! location suffixes `:L`, `:L:C`, `#L12`, `#L12C3`, `(L,C)`. Callers join
//! soft-wrapped terminal rows into one line first; a hard newline ends a path.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLinkMatch {
    /// Byte range of the clickable label in the input text.
    pub start: usize,
    pub end: usize,
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The text shown for the link (a Markdown link's label, else the match).
    pub label: String,
}

static MARKDOWN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\((<[^>\n]+>|[^)\n]+)\)").unwrap());
// `([`"'])([^`"'\n]+)\1` without a backreference: one alternative per quote.
static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"`([^`"'\n]+)`|"([^`"'\n]+)"|'([^`"'\n]+)'"#).unwrap());
static BARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?:https?://[^\s<>`"']+)|(?:file://[^\s<>`"']+)|"#,
        r"(?:(?:(?:[a-zA-Z]:[\\/]|\.{1,2}[/\\]|/)?[\p{L}\p{N}_@~+.\[\]-]+(?:[/\\][\p{L}\p{N}_@~+.\[\]-]+)+(?:\.[0-9A-Za-z_-]+)?",
        r"|[\p{L}\p{N}_@+~-]+(?:\.[0-9A-Za-z_-]+)+)",
        r"(?::[0-9]+(?::[0-9]+)?|#L[0-9]+(?:C[0-9]+)?|\([0-9]+,[0-9]+\))?)",
    ))
    .unwrap()
});
static LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?::([0-9]+)(?::([0-9]+))?|#L([0-9]+)(?:C([0-9]+))?|\(([0-9]+),([0-9]+)\))$")
        .unwrap()
});
static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*:").unwrap());
static DRIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z]:[\\/]").unwrap());
static DOTTED_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[\p{L}\p{N}_@+~-]+\.)+[\p{L}\p{N}_-]+$").unwrap());
static SPECIAL_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:Dockerfile|Makefile|LICENSE|\.gitignore|\.env)$").unwrap()
});
static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[0-9]+\.)+[0-9]+$").unwrap());
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^/\\\s]+@[^/\\\s]+\.[^/\\\s]+$").unwrap());

fn number(text: Option<regex::Match<'_>>) -> Option<u32> {
    let text = text?.as_str();
    // Absurdly long numbers saturate rather than disappear, like JS `Number`.
    Some(
        text.parse::<u64>()
            .map_or(u32::MAX, |n| n.min(u64::from(u32::MAX)) as u32)
            .max(1),
    )
}

/// Splits a trailing `:L`, `:L:C`, `#L12`, `#L12C3` or `(L,C)` off a path.
/// Drive letters (`C:\x`) are not locations.
pub fn split_location(raw: &str) -> (String, Option<u32>, Option<u32>) {
    let Some(captures) = LOCATION.captures(raw) else {
        return (raw.to_string(), None, None);
    };
    let whole = captures.get(0).expect("match");
    let line = number(captures.get(1).or(captures.get(3)).or(captures.get(5)));
    let column = number(captures.get(2).or(captures.get(4)).or(captures.get(6)));
    (raw[..whole.start()].to_string(), line, column)
}

/// `decodeURIComponent`: `None` on malformed escapes or invalid UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `file://[localhost]/path` to a plain path (`/C:/x` loses its leading slash).
pub(crate) fn file_url_path(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    if url
        .host_str()
        .is_some_and(|host| !host.is_empty() && host != "localhost")
    {
        return None;
    }
    let mut value = percent_decode(url.path())?;
    if let Some(fragment) = url.fragment() {
        value.push('#');
        value.push_str(&percent_decode(fragment)?);
    }
    let bytes = value.as_bytes();
    if bytes.len() >= 4
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && bytes[3] == b'/'
    {
        value.remove(0);
    }
    Some(value)
}

struct Link {
    path: String,
    line: Option<u32>,
    column: Option<u32>,
}

fn parse(raw: &str) -> Option<Link> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix('<').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('>').unwrap_or(trimmed);
    let mut value = trimmed.to_string();
    if value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        value = file_url_path(&value)?;
    }
    if value.chars().any(|c| c.is_control()) || value.contains("://") || value.starts_with("\\\\") {
        return None;
    }
    let (value, line, column) = split_location(&value);
    if SCHEME.is_match(&value) && !DRIVE.is_match(&value) {
        return None;
    }
    if value.is_empty()
        || !(value.contains(['/', '\\'])
            || DOTTED_NAME.is_match(&value)
            || SPECIAL_NAME.is_match(&value))
    {
        return None;
    }
    if VERSION.is_match(&value) || EMAIL.is_match(&value) {
        return None;
    }
    Some(Link {
        path: value,
        line,
        column,
    })
}

/// Every file path candidate in `text`, ordered by position, non-overlapping.
pub fn find_file_links(text: &str) -> Vec<FileLinkMatch> {
    let mut result: Vec<FileLinkMatch> = Vec::new();
    let mut occupied: Vec<(usize, usize)> = Vec::new();
    let add = |result: &mut Vec<FileLinkMatch>,
               occupied: &mut Vec<(usize, usize)>,
               start: usize,
               end: usize,
               raw: &str,
               label: &str| {
        if occupied.iter().any(|&(a, b)| start < b && end > a) {
            return;
        }
        if let Some(link) = parse(raw) {
            result.push(FileLinkMatch {
                start,
                end,
                path: link.path,
                line: link.line,
                column: link.column,
                label: label.to_string(),
            });
            occupied.push((start, end));
        }
    };
    // Codex Markdown destinations and quoted paths preserve spaces.
    for captures in MARKDOWN.captures_iter(text) {
        let whole = captures.get(0).expect("match");
        add(
            &mut result,
            &mut occupied,
            whole.start(),
            whole.end(),
            &captures[2],
            &captures[1],
        );
        occupied.push((whole.start(), whole.end()));
    }
    for captures in QUOTED.captures_iter(text) {
        let whole = captures.get(0).expect("match");
        let inner = captures
            .get(1)
            .or(captures.get(2))
            .or(captures.get(3))
            .expect("quoted body");
        add(
            &mut result,
            &mut occupied,
            whole.start() + 1,
            whole.end() - 1,
            inner.as_str(),
            inner.as_str(),
        );
    }
    for found in BARE.find_iter(text) {
        let start = found.start();
        let raw = found.as_str().trim_end_matches(['.', ',', ';', '!', '?']);
        // Do not turn pieces of URLs, email addresses or escape codes into files.
        let after_word = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '/' | '\\'));
        if raw
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("http:"))
            || raw
                .get(..6)
                .is_some_and(|p| p.eq_ignore_ascii_case("https:"))
            || after_word
        {
            continue;
        }
        add(
            &mut result,
            &mut occupied,
            start,
            start + raw.len(),
            raw,
            raw,
        );
    }
    result.sort_by_key(|link| link.start);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(text: &str) -> Option<(String, Option<u32>, Option<u32>)> {
        find_file_links(text)
            .into_iter()
            .next()
            .map(|link| (link.path, link.line, link.column))
    }

    #[test]
    fn detects_the_same_paths_as_the_mobile_client() {
        let cases: &[(&str, &str, Option<u32>, Option<u32>)] = &[
            ("Updated src/app.ts:42:3.", "src/app.ts", Some(42), Some(3)),
            (
                "F:\\terminal\\src\\app.ts:17",
                "F:\\terminal\\src\\app.ts",
                Some(17),
                None,
            ),
            (
                "See /Users/james/project/file.ts#L25C4",
                "/Users/james/project/file.ts",
                Some(25),
                Some(4),
            ),
            (
                "`F:\\My Project\\read me.md:7`",
                "F:\\My Project\\read me.md",
                Some(7),
                None,
            ),
            (
                "[app.ts](F:/terminal/src/app.ts:9)",
                "F:/terminal/src/app.ts",
                Some(9),
                None,
            ),
            (
                "[My Report](<F:/My Project/report.md:12>)",
                "F:/My Project/report.md",
                Some(12),
                None,
            ),
            (
                "file:///F:/My%20Project/main.ts#L18",
                "F:/My Project/main.ts",
                Some(18),
                None,
            ),
            ("README.md", "README.md", None, None),
            ("app.ts:42", "app.ts", Some(42), None),
            ("app/[id]/page.tsx:18", "app/[id]/page.tsx", Some(18), None),
            ("`Dockerfile`", "Dockerfile", None, None),
            ("./src/app.ts(12,5)", "./src/app.ts", Some(12), Some(5)),
            ("😀 see src/日本.ts", "src/日本.ts", None, None),
            (
                "error at crates/bro-tui/src/main.rs#L12C3, see",
                "crates/bro-tui/src/main.rs",
                Some(12),
                Some(3),
            ),
            (
                "C:\\repo\\x.ts(10,4): error TS2322",
                "C:\\repo\\x.ts",
                Some(10),
                Some(4),
            ),
            (
                "look at \"docs/file links.md\"",
                "docs/file links.md",
                None,
                None,
            ),
        ];
        for &(text, path, line, column) in cases {
            assert_eq!(
                first(text),
                Some((path.to_string(), line, column)),
                "{text}"
            );
        }
    }

    #[test]
    fn urls_emails_and_versions_are_not_files() {
        for text in [
            "https://example.com/src/app.ts:9",
            "[site](https://example.com/file.ts)",
            "hello@example.com",
            "1.2.3",
            "mailto:me@example.com",
            "version 10.0.26300",
        ] {
            assert_eq!(find_file_links(text), vec![], "{text}");
        }
    }

    #[test]
    fn trailing_punctuation_offsets_and_labels() {
        let text = "Edit src/a.rs, then (see b/c.rs). Done!";
        let links = find_file_links(text);
        assert_eq!(
            links.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(),
            ["src/a.rs", "b/c.rs"]
        );
        for link in &links {
            assert_eq!(&text[link.start..link.end], link.label);
        }
        let markdown = "see [the app](src/app.rs#L3) now";
        let link = &find_file_links(markdown)[0];
        assert_eq!(
            (link.label.as_str(), link.path.as_str(), link.line),
            ("the app", "src/app.rs", Some(3))
        );
        assert_eq!(&markdown[link.start..link.end], "[the app](src/app.rs#L3)");
    }

    #[test]
    fn hard_newlines_end_a_path_and_unicode_offsets_are_bytes() {
        let links = find_file_links("界 src/a\n/b.rs");
        assert_eq!(
            links.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(),
            ["src/a"],
            "never joined across the newline"
        );
        let text = "界界 src/lib.rs:5";
        let link = &find_file_links(text)[0];
        assert_eq!(&text[link.start..link.end], "src/lib.rs:5");
    }

    #[test]
    fn split_location_handles_every_suffix_but_not_drive_letters() {
        assert_eq!(
            split_location("a.rs:4:2"),
            ("a.rs".into(), Some(4), Some(2))
        );
        assert_eq!(split_location("a.rs#L9"), ("a.rs".into(), Some(9), None));
        assert_eq!(
            split_location("a.rs(3,1)"),
            ("a.rs".into(), Some(3), Some(1))
        );
        assert_eq!(
            split_location("C:\\x\\a.rs"),
            ("C:\\x\\a.rs".into(), None, None)
        );
        assert_eq!(split_location("a.rs:0"), ("a.rs".into(), Some(1), None));
        assert_eq!(
            split_location("a.rs:99999999999"),
            ("a.rs".into(), Some(u32::MAX), None)
        );
    }
}
