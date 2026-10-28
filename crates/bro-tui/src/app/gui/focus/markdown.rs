//! The Markdown agents write, as blocks focus mode lays out in reflowing type: headings, paragraphs, list items
//! (bullets, numbers, task boxes), quotes, fenced code, rules and tables; inside them bold, italic, code,
//! strikethrough and links. Unclosed marks stay as written. Not CommonMark: what agents actually write.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Item { depth: usize, mark: Mark, spans: Vec<Span> },
    Quote(Vec<Span>),
    Code { lang: String, lines: Vec<String> },
    Rule,
    /// rows of cells; the first row is the header when `header`
    Table { rows: Vec<Vec<Vec<Span>>>, header: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    Bullet,
    Number(String),
    Check(bool),
}

/// A run of text with one style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
}

impl Span {
    fn plain(text: &str) -> Span {
        Span { text: text.to_string(), ..Span::default() }
    }
}

/// The text of `spans`, marks gone.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

fn fence(line: &str) -> Option<(&'static str, &str)> {
    let t = line.trim_start();
    ["```", "~~~"].into_iter().find(|m| t.starts_with(m)).map(|m| (m, t[m.len()..].trim()))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start();
    let n = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &t[n..];
    (rest.is_empty() || rest.starts_with(' ')).then(|| (n as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

/// A list item: (indent, mark, text).
fn item(line: &str) -> Option<(usize, Mark, &str)> {
    let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
    let t = line.trim_start();
    let (mark, rest) = if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")).or_else(|| t.strip_prefix("• ")) {
        (Mark::Bullet, r)
    } else {
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 3 {
            return None;
        }
        let after = &t[digits.len()..];
        let r = after.strip_prefix(". ").or_else(|| after.strip_prefix(") "))?;
        (Mark::Number(digits), r)
    };
    let rest = rest.trim_start();
    if mark == Mark::Bullet {
        for (box_, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(r) = rest.strip_prefix(box_) {
                return Some((indent, Mark::Check(done), r));
            }
        }
    }
    Some((indent, mark, rest))
}

fn table_row(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.starts_with('|') {
        return None;
    }
    let inner = t.trim_start_matches('|').trim_end_matches('|');
    Some(inner.split('|').map(|c| c.trim().to_string()).collect())
}

fn is_table_rule(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
}

/// Parse `text` into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Block> = vec![];
    let mut para: Vec<String> = vec![];
    // list item indents in force, to tell their depth
    let mut indents: Vec<usize> = vec![];
    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(inline(&para.join(" "))));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((m, lang)) = fence(line) {
            flush(&mut para, &mut out);
            let mut code = vec![];
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(m) {
                code.push(lines[i].trim_end().to_string());
                i += 1;
            }
            out.push(Block::Code { lang: lang.to_string(), lines: code });
            i += 1;
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }
        if let Some((level, title)) = heading(line) {
            flush(&mut para, &mut out);
            indents.clear();
            out.push(Block::Heading(level, inline(title)));
            i += 1;
            continue;
        }
        if is_rule(line) && para.is_empty() {
            indents.clear();
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if let Some(first) = table_row(line)
            && let Some(second) = lines.get(i + 1).and_then(|l| table_row(l))
            && is_table_rule(&second)
        {
            flush(&mut para, &mut out);
            let mut rows = vec![first.iter().map(|c| inline(c)).collect::<Vec<_>>()];
            i += 2;
            while let Some(r) = lines.get(i).and_then(|l| table_row(l)) {
                rows.push(r.iter().map(|c| inline(c)).collect());
                i += 1;
            }
            out.push(Block::Table { rows, header: true });
            continue;
        }
        if let Some((indent, mark, rest)) = item(line) {
            flush(&mut para, &mut out);
            while indents.last().is_some_and(|&d| d > indent) {
                indents.pop();
            }
            if indents.last() != Some(&indent) {
                indents.push(indent);
            }
            let depth = indents.len() - 1;
            // lines indented under it carry on its text
            let mut text = rest.to_string();
            while let Some(next) = lines.get(i + 1)
                && !next.trim().is_empty()
                && item(next).is_none()
                && fence(next).is_none()
                && next.starts_with([' ', '\t'])
            {
                text.push(' ');
                text.push_str(next.trim());
                i += 1;
            }
            out.push(Block::Item { depth, mark, spans: inline(&text) });
            i += 1;
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix('>') {
            flush(&mut para, &mut out);
            let mut text = vec![rest.trim().to_string()];
            while let Some(next) = lines.get(i + 1).and_then(|l| l.trim_start().strip_prefix('>')) {
                text.push(next.trim().to_string());
                i += 1;
            }
            out.push(Block::Quote(inline(&text.join(" "))));
            i += 1;
            continue;
        }
        indents.clear();
        para.push(line.trim().to_string());
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

/// Where the closing `mark` after byte `from` is (None: the mark is literal).
fn closes(text: &str, from: usize, mark: &str) -> Option<usize> {
    text[from..].find(mark).map(|j| from + j).filter(|&j| j > from)
}

fn url_end(text: &str, from: usize) -> usize {
    let rest = &text[from..];
    let mut end = rest.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`')).unwrap_or(rest.len());
    // closing punctuation isn't part of the address
    while end > 0 && rest[..end].ends_with(['.', ',', ';', ':', '!', '?', ')', '\'']) {
        if rest[..end].ends_with(')') && rest[..end].matches('(').count() >= rest[..end].matches(')').count() {
            break;
        }
        end -= 1;
    }
    from + end
}

/// Inline marks: `code`, **bold**, *italic* / _italic_, ~~strike~~, [links](url), bare https:// addresses.
pub fn inline(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = vec![];
    let mut cur = Span::default();
    let (mut bold, mut italic, mut strike) = (false, false, false);
    let mut i = 0;
    let push = |out: &mut Vec<Span>, cur: &mut Span| {
        if !cur.text.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    let style = |bold, italic, strike| Span { bold, italic, strike, ..Span::default() };
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or(' ');
        // escapes
        if c == '\\' && let Some(n) = rest[1..].chars().next() && n.is_ascii_punctuation() {
            cur.text.push(n);
            i += 1 + n.len_utf8();
            continue;
        }
        if c == '`' {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            let mark = &rest[..ticks];
            if let Some(j) = closes(text, i + ticks, mark) {
                push(&mut out, &mut cur);
                let code = text[i + ticks..j].trim();
                out.push(Span { text: code.to_string(), code: true, bold, italic, strike, link: None });
                cur = style(bold, italic, strike);
                i = j + ticks;
                continue;
            }
        }
        if rest.starts_with("**") || rest.starts_with("__") {
            let mark = &rest[..2];
            if bold || closes(text, i + 2, mark).is_some() {
                push(&mut out, &mut cur);
                bold = !bold;
                cur = style(bold, italic, strike);
                i += 2;
                continue;
            }
        }
        if rest.starts_with("~~") && (strike || closes(text, i + 2, "~~").is_some()) {
            push(&mut out, &mut cur);
            strike = !strike;
            cur = style(bold, italic, strike);
            i += 2;
            continue;
        }
        // *italic*, and _italic_ only at word edges (snake_case stays as it is)
        if c == '*' || c == '_' {
            let before = text[..i].chars().next_back();
            let after = rest[1..].chars().next();
            let opens = !italic && after.is_some_and(|a| !a.is_whitespace()) && (c == '*' || before.is_none_or(|b| !b.is_alphanumeric())) && closes(text, i + 1, &c.to_string()).is_some();
            let shuts = italic && before.is_some_and(|b| !b.is_whitespace()) && (c == '*' || after.is_none_or(|a| !a.is_alphanumeric()));
            if opens || shuts {
                push(&mut out, &mut cur);
                italic = !italic;
                cur = style(bold, italic, strike);
                i += 1;
                continue;
            }
        }
        // [label](url)
        if c == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close + 2..].find(')')
            && !rest[..close].contains('\n')
        {
            let label = &rest[1..close];
            let url = rest[close + 2..close + 2 + end].trim();
            push(&mut out, &mut cur);
            for mut s in inline(label) {
                s.bold |= bold;
                s.italic |= italic;
                s.link = Some(url.to_string());
                out.push(s);
            }
            cur = style(bold, italic, strike);
            i += close + 2 + end + 1;
            continue;
        }
        // <https://…> and bare addresses
        if (rest.starts_with("https://") || rest.starts_with("http://")) && text[..i].chars().next_back().is_none_or(|b| !b.is_alphanumeric()) {
            let end = url_end(text, i);
            if end > i + 8 {
                push(&mut out, &mut cur);
                let url = &text[i..end];
                out.push(Span { text: url.to_string(), link: Some(url.to_string()), bold, italic, strike, code: false });
                cur = style(bold, italic, strike);
                i = end;
                continue;
            }
        }
        if c == '<' && let Some(end) = rest.find('>') && (rest[1..end].starts_with("http://") || rest[1..end].starts_with("https://")) {
            push(&mut out, &mut cur);
            let url = &rest[1..end];
            out.push(Span { text: url.to_string(), link: Some(url.to_string()), bold, italic, strike, code: false });
            cur = style(bold, italic, strike);
            i += end + 1;
            continue;
        }
        cur.text.push(c);
        i += c.len_utf8();
    }
    push(&mut out, &mut cur);
    if out.is_empty() {
        out.push(Span::plain(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(spans: &[Span]) -> Vec<(&str, bool, bool, bool)> {
        spans.iter().map(|s| (s.text.as_str(), s.bold, s.italic, s.code)).collect()
    }

    #[test]
    fn blocks() {
        let md = "# What changed\n\nThe daemon owns the work,\nand the apps are views.\n\n- one\n- two\n  carried on\n  - nested\n1. first\n\n```rust\nfn a() {}\n```\n> a quote\n---\n| a | b |\n|---|:-:|\n| 1 | 2 |\n- [x] done";
        let b = parse(md);
        assert!(matches!(&b[0], Block::Heading(1, s) if plain(s) == "What changed"));
        assert!(matches!(&b[1], Block::Para(s) if plain(s) == "The daemon owns the work, and the apps are views."));
        assert!(matches!(&b[2], Block::Item { depth: 0, mark: Mark::Bullet, spans } if plain(spans) == "one"));
        assert!(matches!(&b[3], Block::Item { depth: 0, spans, .. } if plain(spans) == "two carried on"));
        assert!(matches!(&b[4], Block::Item { depth: 1, spans, .. } if plain(spans) == "nested"));
        assert!(matches!(&b[5], Block::Item { mark: Mark::Number(n), .. } if n == "1"));
        assert!(matches!(&b[6], Block::Code { lang, lines } if lang == "rust" && lines == &["fn a() {}".to_string()]));
        assert!(matches!(&b[7], Block::Quote(s) if plain(s) == "a quote"));
        assert_eq!(b[8], Block::Rule);
        assert!(matches!(&b[9], Block::Table { rows, header: true } if rows.len() == 2 && plain(&rows[1][1]) == "2"));
        assert!(matches!(&b[10], Block::Item { mark: Mark::Check(true), spans, .. } if plain(spans) == "done"));
    }

    #[test]
    fn inline_marks() {
        assert_eq!(texts(&inline("a **bold** and *it* `code` end")), [("a ", false, false, false), ("bold", true, false, false), (" and ", false, false, false), ("it", false, true, false), (" ", false, false, false), ("code", false, false, true), (" end", false, false, false)]);
        assert_eq!(plain(&inline("snake_case_name stays")), "snake_case_name stays");
        assert_eq!(plain(&inline("2 * 3 = 6 and a ** b")), "2 * 3 = 6 and a ** b", "unclosed marks are literal");
        let l = inline("see [the docs](https://docs.rs/x) or https://example.com/a.");
        assert_eq!(l[1].link.as_deref(), Some("https://docs.rs/x"));
        assert_eq!(l[1].text, "the docs");
        assert_eq!(l[3].text, "https://example.com/a");
        assert_eq!(l[4].text, ".", "the full stop isn't part of the address");
        assert_eq!(plain(&inline("\\*not italic\\*")), "*not italic*");
    }
}
