//! The file preview overlay: a file path clicked in a terminal (or one an agent opened with `bro open`), loaded
//! and highlighted on a worker thread, scrolled to its line. Big files page in as you scroll; keys open it in
//! your editor, copy its path or mention it to the agent. Files the terminal can't display open
//! directly in the system's default app.

use crate::layout::PaneId;
use crate::pane::Event;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_bridge::file_preview::{self, DEFAULT_LIMIT, Kind, MAX_LIMIT, PreviewRequest, ResolveRoots};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Span,
};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
/// Past this, the rest of a load shows unhighlighted rather than keep you waiting.
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(1500);
/// Start loading the next page this many lines before the loaded edge.
const PREFETCH: usize = 20;

/// One line of highlighted text.
pub type Styled = Vec<(Style, String)>;

/// A text file, or the loaded window of one.
pub struct Doc {
    pub path: PathBuf,
    pub relative: String,
    /// `@src/app.rs` for the agent (absolute when it isn't under the session's cwd)
    pub mention: String,
    pub language: String,
    /// first loaded line (1-based)
    pub start: usize,
    pub lines: Vec<Styled>,
    pub total: usize,
    pub bytes: u64,
}

impl Doc {
    fn end(&self) -> usize {
        self.start + self.lines.len().saturating_sub(1)
    }
}

/// Video, audio, images: handed to the system after loading.
pub struct Media {
    pub path: PathBuf,
    pub relative: String,
    pub mention: String,
    pub mime: String,
    pub bytes: u64,
}

pub enum Body {
    Loading,
    /// `path`: the file exists (binary, too big…), so the system can still open it
    Failed { message: String, path: Option<PathBuf> },
    Text(Doc),
    Media(Media),
}

pub enum Payload {
    Text(Doc),
    Media(Media),
}

/// A worker's answer (stale ones — an older request — are dropped).
pub struct Loaded {
    pub req: u64,
    pub result: Result<Payload, (String, Option<PathBuf>)>,
}

/// What the app should do after a key, click or load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Close,
    /// open it in your editor
    Editor,
    /// hand it to the OS default app
    System,
    Copy,
    /// type `@path ` into the session
    Mention,
    /// an OSC 8 `file://` link bro can't preview (outside the workspace, a folder): open it the old way
    Fallback(String),
}

pub struct FileView {
    /// the session it came from (mentions go there)
    pub pane: Option<PaneId>,
    pub roots: ResolveRoots,
    /// what to load: the clicked text at first, the resolved path once known
    pub target: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub fallback: Option<String>,
    pub body: Body,
    /// first line on screen (1-based)
    pub top: usize,
    /// columns scrolled right
    pub left: usize,
    req: u64,
    loading: bool,
    /// bring the target line into view at the next draw
    center: bool,
    rows: usize,
    /// clickable hints from the last draw
    pub hits: Vec<(Rect, Outcome)>,
    pub frame: Rect,
    theme: Theme,
    tx: Sender<Event>,
}

impl FileView {
    /// Open the overlay and start loading `target` (a path, relative to the session or absolute).
    #[allow(clippy::too_many_arguments)]
    pub fn open(pane: Option<PaneId>, roots: ResolveRoots, target: String, line: Option<u32>, column: Option<u32>, fallback: Option<String>, theme: &Theme, tx: Sender<Event>) -> FileView {
        let mut v = FileView {
            pane,
            roots,
            target,
            line,
            column,
            fallback,
            body: Body::Loading,
            top: 1,
            left: 0,
            req: 0,
            loading: false,
            center: true,
            rows: 20,
            hits: vec![],
            frame: Rect::default(),
            theme: theme.clone(),
            tx,
        };
        v.request(PreviewRequest { line: line.unwrap_or(1) as usize, start: None, limit: None });
        v
    }

    fn request(&mut self, r: PreviewRequest) {
        self.req = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        self.loading = true;
        let (req, roots, target, theme, tx) = (self.req, self.roots.clone(), self.target.clone(), self.theme.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = crate::services::guard("file_view::load", || read(&roots, &target, r, &theme)).unwrap_or_else(|e| Err((e, None)));
            let _ = tx.send(Event::FileView(Box::new(Loaded { req, result })));
        });
    }

    /// A worker finished. Files without a terminal preview go to the system viewer.
    pub fn apply(&mut self, loaded: Loaded) -> Outcome {
        if loaded.req != self.req {
            return Outcome::None;
        }
        self.loading = false;
        match loaded.result {
            Ok(Payload::Text(doc)) => {
                self.target = doc.path.to_string_lossy().into_owned();
                self.body = Body::Text(doc);
                self.top = self.top.min(self.max_top());
            }
            Ok(Payload::Media(m)) => {
                self.target = m.path.to_string_lossy().into_owned();
                self.body = Body::Media(m);
                return Outcome::System;
            }
            // a page that didn't load: keep what's shown
            Err(_) if matches!(self.body, Body::Text(_)) => {}
            Err((message, path)) => {
                if path.is_none()
                    && message != file_preview::NOT_FOUND
                    && let Some(uri) = self.fallback.clone()
                {
                    return Outcome::Fallback(uri);
                }
                let open_in_system = path.is_some();
                self.body = Body::Failed { message, path };
                if open_in_system {
                    return Outcome::System;
                }
            }
        }
        Outcome::None
    }

    /// The file being shown, once it's known.
    pub fn path(&self) -> Option<&Path> {
        match &self.body {
            Body::Text(d) => Some(&d.path),
            Body::Media(m) => Some(&m.path),
            Body::Failed { path, .. } => path.as_deref(),
            Body::Loading => None,
        }
    }

    pub fn mention(&self) -> Option<&str> {
        match &self.body {
            Body::Text(d) => Some(&d.mention),
            Body::Media(m) => Some(&m.mention),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    fn total(&self) -> usize {
        match &self.body {
            Body::Text(d) => d.total.max(d.end()),
            _ => 1,
        }
    }

    fn max_top(&self) -> usize {
        self.total().saturating_sub(self.rows).saturating_add(1).max(1)
    }

    /// Scroll by `delta` lines, loading the next or earlier page when you near the loaded edge.
    pub fn scroll(&mut self, delta: isize) {
        self.top = (self.top as isize + delta).clamp(1, self.max_top() as isize) as usize;
        self.page();
    }

    fn page(&mut self) {
        let Body::Text(d) = &self.body else { return };
        if self.loading {
            return;
        }
        let (start, end, total) = (d.start, d.end(), d.total);
        let line = self.line.unwrap_or(1) as usize;
        if start > 1 && self.top < start + PREFETCH {
            let new_start = start.saturating_sub(DEFAULT_LIMIT).max(1);
            let new_end = end.min(new_start + MAX_LIMIT - 1);
            self.request(PreviewRequest { line, start: Some(new_start), limit: Some(new_end + 1 - new_start) });
        } else if end < total && self.top + self.rows + PREFETCH > end {
            let want_end = total.min(end + DEFAULT_LIMIT);
            let new_start = if want_end + 1 - start > MAX_LIMIT { want_end + 1 - MAX_LIMIT } else { start };
            self.request(PreviewRequest { line, start: Some(new_start), limit: Some(want_end + 1 - new_start) });
        }
    }

    fn center_on_target(&mut self) {
        let line = self.line.unwrap_or(1) as usize;
        self.top = line.saturating_sub(self.rows / 3).max(1).min(self.max_top());
        if let Some(c) = self.column {
            let c = c as usize;
            // keep the column on screen when the line is long
            self.left = if c > 60 { c.saturating_sub(20) } else { 0 };
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let page = self.rows.saturating_sub(1).max(1) as isize;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let media = matches!(self.body, Body::Media(_));
        let openable = self.path().is_some();
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::Char('d') if ctrl => self.scroll(page / 2),
            KeyCode::Char('u') if ctrl => self.scroll(-page / 2),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(page),
            KeyCode::Char('f') if ctrl => self.scroll(page),
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll(-page),
            KeyCode::Home | KeyCode::Char('g') => self.scroll(-(self.top as isize)),
            KeyCode::End | KeyCode::Char('G') => self.scroll(self.total() as isize),
            KeyCode::Left | KeyCode::Char('h') => self.left = self.left.saturating_sub(8),
            KeyCode::Right | KeyCode::Char('l') => self.left += 8,
            KeyCode::Char('0') => self.left = 0,
            KeyCode::Char('e') if media => return Outcome::System,
            KeyCode::Char('e') if openable => return Outcome::Editor,
            KeyCode::Enter if matches!(self.body, Body::Text(_)) => return Outcome::Editor,
            KeyCode::Enter | KeyCode::Char('o') if openable => return Outcome::System,
            KeyCode::Char('c') | KeyCode::Char('y') if openable => return Outcome::Copy,
            KeyCode::Char('@') | KeyCode::Char('i') if self.mention().is_some() => return Outcome::Mention,
            _ => {}
        }
        Outcome::None
    }

    pub fn mouse(&mut self, m: MouseEvent) -> Outcome {
        let pos = Position { x: m.column, y: m.row };
        match m.kind {
            MouseEventKind::ScrollDown => self.scroll(3),
            MouseEventKind::ScrollUp => self.scroll(-3),
            MouseEventKind::ScrollRight => self.left += 8,
            MouseEventKind::ScrollLeft => self.left = self.left.saturating_sub(8),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((_, o)) = self.hits.iter().find(|(r, _)| r.contains(pos)) {
                    return o.clone();
                }
                if !self.frame.contains(pos) {
                    return Outcome::Close;
                }
            }
            _ => {}
        }
        Outcome::None
    }

    fn title(&self) -> String {
        let name = match &self.body {
            Body::Text(d) => d.relative.clone(),
            Body::Media(m) => m.relative.clone(),
            _ => self.target.clone(),
        };
        let name = if name.is_empty() { self.target.clone() } else { name };
        match (self.line, &self.body) {
            (Some(l), Body::Text(_) | Body::Loading | Body::Failed { .. }) => format!("{name}:{l}"),
            _ => name,
        }
    }
}

// ------------------------------------------------------------------ loading (worker thread)

/// Resolve and read `target` (blocking): text highlighted with the theme's colours, media described.
pub fn read(roots: &ResolveRoots, target: &str, r: PreviewRequest, theme: &Theme) -> Result<Payload, (String, Option<PathBuf>)> {
    let resolved = file_preview::resolve_path(roots, target).map_err(|e| (e, None))?;
    if resolved.kind != Kind::File {
        return Err(("This is a folder, not a file.".into(), None));
    }
    let path = resolved.path;
    let mention = mention_for(&path, roots);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if let Some(mime) = file_preview::media_mime(&ext).or_else(|| file_preview::image_mime(&ext)) {
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        return Ok(Payload::Media(Media { relative: resolved.relative, mention, mime: mime.into(), bytes, path }));
    }
    let v = file_preview::read_preview(roots, &path.to_string_lossy(), r).map_err(|e| (e, Some(path.clone())))?;
    if v["kind"] != "text" {
        return Err(("This file can't be shown as text.".into(), Some(path)));
    }
    let content = v["content"].as_str().unwrap_or_default();
    let number = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    let language = v["language"].as_str().unwrap_or("plaintext").to_string();
    let lines = highlight(content, &path, &language, theme);
    let start = number("startLine").max(1);
    let total = number("totalLines").max(start + lines.len().saturating_sub(1));
    Ok(Payload::Text(Doc { relative: resolved.relative, mention, language, start, lines, total, bytes: v["bytes"].as_u64().unwrap_or(0), path }))
}

/// `@src/app.rs` when it's under the session's cwd, else the absolute path; quoted when it has spaces.
fn mention_for(path: &Path, roots: &ResolveRoots) -> String {
    let cwd = roots.cwd.canonicalize().map(crate::util::strip_verbatim).unwrap_or_else(|_| roots.cwd.clone());
    let text = match path.strip_prefix(&cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => path.to_string_lossy().into_owned(),
    };
    if text.contains(char::is_whitespace) { format!("@\"{text}\"") } else { format!("@{text}") }
}

/// Tabs to spaces, and no control characters reaching the screen.
fn clean(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for c in line.trim_end_matches('\r').chars() {
        match c {
            '\t' => {
                let n = 4 - col % 4;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            c if c.is_control() => {
                out.push('·');
                col += 1;
            }
            c => {
                out.push(c);
                col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            }
        }
    }
    out
}

static SYNTAXES: LazyLock<syntect::parsing::SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// The theme's colours by slot. Highlighting runs with a synthetic syntect theme whose colours are slot numbers,
/// mapped back here — so code matches bro's theme, including colours that aren't RGB.
struct Palette([Color; 7]);
const FG: usize = 0;
const MUTED: usize = 1;
const GOOD: usize = 2;
const INLINE: usize = 3;
const ACCENT: usize = 4;
const SHINE: usize = 5;
const DANGER: usize = 6;

impl Palette {
    fn new(t: &Theme) -> Palette {
        Palette([t.fg, t.muted, t.good, t.inline, t.accent, t.shine, t.danger])
    }

    fn code(slot: usize) -> syntect::highlighting::Color {
        syntect::highlighting::Color { r: slot as u8, g: 0x5a, b: 0xa5, a: 0xff }
    }

    fn style(&self, s: syntect::highlighting::Style) -> Style {
        use syntect::highlighting::FontStyle;
        let c = s.foreground;
        let slot = if c.g == 0x5a && c.b == 0xa5 && (c.r as usize) < self.0.len() { c.r as usize } else { FG };
        let mut st = Style::default().fg(self.0[slot]);
        if s.font_style.contains(FontStyle::BOLD) {
            st = st.add_modifier(Modifier::BOLD);
        }
        if s.font_style.contains(FontStyle::ITALIC) {
            st = st.add_modifier(Modifier::ITALIC);
        }
        if s.font_style.contains(FontStyle::UNDERLINE) {
            st = st.add_modifier(Modifier::UNDERLINED);
        }
        st
    }

    fn syntect_theme() -> syntect::highlighting::Theme {
        use syntect::highlighting::{FontStyle, ScopeSelectors, StyleModifier, Theme as SyntectTheme, ThemeItem, ThemeSettings};
        let item = |scope: &str, slot: usize, font: Option<FontStyle>| ThemeItem {
            scope: scope.parse::<ScopeSelectors>().unwrap_or_default(),
            style: StyleModifier { foreground: Some(Self::code(slot)), background: None, font_style: font },
        };
        SyntectTheme {
            settings: ThemeSettings { foreground: Some(Self::code(FG)), ..Default::default() },
            scopes: vec![
                item("comment, punctuation.definition.comment", MUTED, Some(FontStyle::ITALIC)),
                item("string, punctuation.definition.string, constant.character.escape", GOOD, None),
                item("constant.numeric, constant.language, constant.character, constant.other, support.constant", INLINE, None),
                item("keyword, storage, keyword.operator.word, keyword.control", ACCENT, None),
                item("keyword.operator, punctuation.separator, punctuation.terminator", FG, None),
                item("entity.name.function, support.function, meta.function-call.generic, variable.function", SHINE, None),
                item("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, support.type, support.class, entity.other.inherited-class, storage.type.rust", INLINE, None),
                item("entity.name.tag", ACCENT, None),
                item("entity.other.attribute-name, meta.attribute, support.type.property-name", SHINE, None),
                item("variable.parameter, variable.other.member", FG, None),
                item("entity.name.namespace, entity.name.module", SHINE, None),
                item("markup.heading, markup.heading entity.name, punctuation.definition.heading", ACCENT, Some(FontStyle::BOLD)),
                item("markup.bold", FG, Some(FontStyle::BOLD)),
                item("markup.italic", FG, Some(FontStyle::ITALIC)),
                item("markup.raw, markup.inline.raw, markup.fenced_code", INLINE, None),
                item("markup.underline.link, string.other.link, meta.link", SHINE, Some(FontStyle::UNDERLINE)),
                item("markup.list punctuation, punctuation.definition.list_item, markup.quote", MUTED, None),
                item("markup.inserted, meta.diff.header.to-file", GOOD, None),
                item("markup.deleted, meta.diff.header.from-file", DANGER, None),
                item("markup.changed, meta.diff.range", INLINE, None),
                item("invalid", DANGER, None),
            ],
            ..Default::default()
        }
    }
}

/// Highlight `content` (the loaded window) for `path`, within [`HIGHLIGHT_BUDGET`].
fn highlight(content: &str, path: &Path, language: &str, t: &Theme) -> Vec<Styled> {
    let palette = Palette::new(t);
    let plain = |line: String| vec![(Style::default().fg(t.fg), line)];
    let lines: Vec<String> = content.split('\n').map(clean).collect();
    let ss = &*SYNTAXES;
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let name = path.file_name().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let token = match language {
        "shell" => "sh",
        "powershell" => "ps1",
        "csharp" => "cs",
        "cpp" => "cpp",
        "plaintext" => "",
        other => other,
    };
    let syntax = ss
        .find_syntax_by_extension(&ext)
        .or_else(|| ss.find_syntax_by_extension(&name))
        .or_else(|| (!token.is_empty()).then(|| ss.find_syntax_by_token(token)).flatten())
        .or_else(|| lines.first().and_then(|l| ss.find_syntax_by_first_line(l)))
        .filter(|s| s.name != "Plain Text");
    let Some(syntax) = syntax else { return lines.into_iter().map(plain).collect() };
    let theme = Palette::syntect_theme();
    let mut h = syntect::easy::HighlightLines::new(syntax, &theme);
    let started = Instant::now();
    let mut out = Vec::with_capacity(lines.len());
    let mut over = false;
    for line in lines {
        over = over || started.elapsed() > HIGHLIGHT_BUDGET;
        if over {
            out.push(plain(line));
            continue;
        }
        let with_newline = format!("{line}\n");
        match h.highlight_line(&with_newline, ss) {
            Ok(ranges) => out.push(
                ranges
                    .into_iter()
                    .filter_map(|(st, text)| {
                        let text = text.trim_end_matches('\n');
                        (!text.is_empty()).then(|| (palette.style(st), text.to_string()))
                    })
                    .collect(),
            ),
            Err(_) => {
                over = true;
                out.push(plain(line));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ drawing

fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// `width` columns of `line` starting `left` columns in.
fn slice(line: &Styled, left: usize, width: usize) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthChar;
    let mut out = vec![];
    let (mut col, mut used) = (0usize, 0usize);
    for (style, text) in line {
        let mut part = String::new();
        for c in text.chars() {
            let w = c.width().unwrap_or(0);
            if col < left {
                col += w;
                continue;
            }
            if used + w > width {
                break;
            }
            part.push(c);
            used += w;
            col += w;
        }
        if !part.is_empty() {
            out.push(Span::styled(part, *style));
        }
        if used >= width {
            break;
        }
    }
    out
}

pub fn draw(f: &mut Frame, area: Rect, v: &mut FileView, hover: Position, t: &Theme) {
    v.hits.clear();
    let w = area.width.saturating_sub(4).min(200);
    let h = area.height.saturating_sub(2);
    v.frame = ui::centered(area, w, h);
    let title = format!("{}{}", ui::lead("file"), ui::fit(&v.title(), w.saturating_sub(10) as usize));
    let inner = ui::popup(f, area, w, h, &title, t);
    if inner.height < 4 || inner.width < 16 {
        return;
    }
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let header = Rect { height: 1, ..inner };
    let body = Rect { y: inner.y + 1, height: inner.height - 2, ..inner };
    let hint_row = Rect { y: inner.bottom() - 1, height: 1, ..inner };
    v.rows = body.height as usize;
    if v.center && matches!(v.body, Body::Text(_)) {
        v.center = false;
        v.center_on_target();
        v.page();
    }

    // header: what it is, where you are
    let (left, right) = match &v.body {
        Body::Text(d) => {
            let lang = if d.language == "plaintext" { "text".to_string() } else { d.language.clone() };
            let left = format!("{lang} · {} line{} · {}", d.total, if d.total == 1 { "" } else { "s" }, size_text(d.bytes));
            let last = (v.top + v.rows).saturating_sub(1).min(d.total.max(1));
            let mut right = format!("{}–{} of {}", v.top, last, d.total);
            if v.left > 0 {
                right = format!("→{} · {right}", v.left);
            }
            if v.loading {
                right = format!("loading… · {right}");
            }
            (left, right)
        }
        Body::Media(m) => (format!("{} · {}", m.mime, size_text(m.bytes)), String::new()),
        Body::Failed { .. } => (String::new(), String::new()),
        Body::Loading => ("loading…".into(), String::new()),
    };
    let path_text = v.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    ui::line_lr(f, header, vec![Span::styled(left, muted(t)), Span::styled(format!("   {path_text}"), fg(t.frame))], vec![Span::styled(right, muted(t))]);

    match &v.body {
        Body::Loading => ui::line(f, Rect { y: body.y + 1, height: 1, ..body }, vec![Span::styled("  loading…", muted(t))]),
        Body::Failed { message, path } => {
            ui::line(f, Rect { y: body.y + 1, height: 1, ..body }, vec![Span::styled(format!("  {message}"), fg(t.danger))]);
            if path.is_some() {
                ui::line(f, Rect { y: body.y + 3, height: 1, ..body }, vec![Span::styled("  ⏎ opens it with the system's default app", muted(t))]);
            }
        }
        Body::Media(m) => {
            let kind = m.mime.split('/').next().unwrap_or_default();
            let (glyph, what, how) = match kind {
                "video" => ("▶", "video", "⏎ plays it in your default player · it streams on the phone and web too"),
                "audio" => ("♪", "audio", "⏎ plays it in your default player · it streams on the phone and web too"),
                _ => ("▣", "image", "⏎ opens it in your default viewer · the phone and web show it inline"),
            };
            let name = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mid = body.y + body.height / 3;
            ui::line(f, Rect { y: mid, height: 1, ..body }, vec![Span::styled(format!("  {glyph}  "), ui::bold_accent(t)), Span::styled(name, ui::bold())]);
            ui::line(f, Rect { y: mid + 1, height: 1, ..body }, vec![Span::styled(format!("     {what} · {} · {}", m.mime, size_text(m.bytes)), muted(t))]);
            ui::line(f, Rect { y: mid + 3, height: 1, ..body }, vec![Span::styled(format!("     {how}"), muted(t))]);
        }
        Body::Text(d) => {
            let total = d.total.max(d.end());
            let gutter = total.to_string().len().max(3);
            let text_w = (body.width as usize).saturating_sub(gutter + 2);
            let target = v.line.map(|l| l as usize);
            for row in 0..body.height {
                let n = v.top + row as usize;
                if n > total {
                    break;
                }
                let y = body.y + row;
                let r = Rect { y, height: 1, ..body };
                let here = target == Some(n);
                let num = Span::styled(format!("{n:>gutter$} "), if here { ui::bold_accent(t) } else { fg(t.frame) });
                let bar = Span::styled(if here { "▌" } else { " " }, ui::accent(t));
                if n < d.start || n > d.end() {
                    ui::line(f, r, vec![num, bar, Span::styled("…", muted(t))]);
                    continue;
                }
                let mut spans = vec![num, bar];
                spans.extend(slice(&d.lines[n - d.start], v.left, text_w));
                ui::line(f, r, spans);
                if here {
                    let tint = crate::theme::project_tint(t.accent, t);
                    f.buffer_mut().set_style(Rect { x: r.x + gutter as u16 + 2, width: r.width.saturating_sub(gutter as u16 + 2), ..r }, Style::default().bg(tint));
                }
            }
        }
    }

    // hints, clickable
    let mut pairs: Vec<(&str, &str, Outcome)> = vec![];
    match &v.body {
        Body::Text(_) => pairs.extend([("e", "editor", Outcome::Editor), ("o", "system app", Outcome::System), ("c", "copy path", Outcome::Copy), ("@", "mention", Outcome::Mention), ("↑↓ pgup pgdn", "scroll", Outcome::None)]),
        Body::Media(_) => pairs.extend([("⏎", "open", Outcome::System), ("c", "copy path", Outcome::Copy), ("@", "mention", Outcome::Mention)]),
        Body::Failed { path: Some(_), .. } => pairs.extend([("⏎", "system app", Outcome::System), ("c", "copy path", Outcome::Copy)]),
        _ => {}
    }
    pairs.push(("esc", "close", Outcome::Close));
    let mut x = hint_row.x + 1;
    for (i, (k, what, o)) in pairs.into_iter().enumerate() {
        let sep = if i > 0 { " · " } else { "" };
        let label_w = (ui::width(k) + 1 + ui::width(what)) as u16;
        let start = x + ui::width(sep) as u16;
        if start + label_w > hint_row.right() {
            break;
        }
        let r = Rect { x: start, y: hint_row.y, width: label_w, height: 1 };
        let hot = o != Outcome::None && r.contains(hover);
        let under = if hot { Modifier::UNDERLINED } else { Modifier::empty() };
        ui::line(f, Rect { x, width: hint_row.right() - x, ..hint_row }, vec![
            Span::styled(sep, muted(t)),
            Span::styled(k.to_string(), Style::default().fg(t.shine).add_modifier(Modifier::BOLD | under)),
            Span::styled(format!(" {what}"), muted(t).add_modifier(under)),
        ]);
        if o != Outcome::None {
            v.hits.push((r, o));
        }
        x = start + label_w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        crate::theme::get("graphite")
    }

    fn roots(dir: &Path) -> ResolveRoots {
        ResolveRoots { cwd: dir.to_path_buf(), project: None }
    }

    #[test]
    fn reads_highlights_and_describes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.rs"), "// hi\nfn main() {\n\tlet s = \"x\";\x1b[31m\n}\n").unwrap();
        std::fs::write(dir.path().join("clip one.mp4"), [0u8; 2048]).unwrap();
        let Ok(Payload::Text(d)) = read(&roots(dir.path()), "src/app.rs", PreviewRequest { line: 2, ..Default::default() }, &theme()) else { panic!("text") };
        assert_eq!((d.start, d.total, d.language.as_str(), d.relative.as_str(), d.mention.as_str()), (1, 4, "rust", "src/app.rs", "@src/app.rs"));
        let text = |l: &Styled| l.iter().map(|(_, s)| s.as_str()).collect::<String>();
        assert_eq!(text(&d.lines[2]), "    let s = \"x\";·[31m", "tabs expand, escapes never reach the screen");
        assert!(d.lines[0][0].0.add_modifier.contains(Modifier::ITALIC), "comments are highlighted");
        let colors: std::collections::HashSet<_> = d.lines.iter().flatten().map(|(s, _)| s.fg).collect();
        assert!(colors.len() >= 3, "keywords, strings and comments get distinct colours: {colors:?}");
        let Ok(Payload::Media(m)) = read(&roots(dir.path()), "clip one.mp4", PreviewRequest::default(), &theme()) else { panic!("media") };
        assert_eq!((m.mime.as_str(), m.bytes, m.mention.as_str()), ("video/mp4", 2048, "@\"clip one.mp4\""));
        assert!(matches!(read(&roots(dir.path()), "src", PreviewRequest::default(), &theme()), Err((_, None))));
        assert!(matches!(read(&roots(dir.path()), "nope.rs", PreviewRequest::default(), &theme()), Err((ref m, None)) if m == file_preview::NOT_FOUND));
    }

    #[test]
    fn opens_at_the_line_and_pages_through_big_files() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=2000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), body).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut v = FileView::open(None, roots(dir.path()), "big.txt".into(), Some(1500), None, None, &theme(), tx);
        let next = |v: &mut FileView| {
            let Ok(Event::FileView(l)) = rx.recv_timeout(Duration::from_secs(10)) else { panic!("loaded") };
            assert_eq!(v.apply(*l), Outcome::None);
        };
        next(&mut v);
        let Body::Text(d) = &v.body else { panic!("text") };
        assert!(d.start <= 1500 && d.end() >= 1500 && d.total == 2000);
        let mut out = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        out.draw(|f| draw(f, f.area(), &mut v, Position::default(), &theme())).unwrap();
        assert!(v.top <= 1500 && 1500 < v.top + v.rows, "the target line is on screen");
        let screen: String = out.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("line 1500"));
        // scrolling to the top loads earlier pages until line 1 is in
        for _ in 0..20 {
            v.key(KeyEvent::from(KeyCode::Char('g')));
            if !v.is_loading() {
                break;
            }
            next(&mut v);
        }
        let Body::Text(d) = &v.body else { panic!("text") };
        assert_eq!((v.top, d.start), (1, 1));
        assert!(d.lines.len() <= MAX_LIMIT);
        // and the end
        for _ in 0..20 {
            v.key(KeyEvent::from(KeyCode::Char('G')));
            if !v.is_loading() {
                break;
            }
            next(&mut v);
        }
        let Body::Text(d) = &v.body else { panic!("text") };
        assert_eq!(d.end(), 2000);
        assert_eq!(v.top, v.max_top());
        assert_eq!(v.key(KeyEvent::from(KeyCode::Char('e'))), Outcome::Editor);
        assert_eq!(v.key(KeyEvent::from(KeyCode::Char('@'))), Outcome::Mention);
        assert_eq!(v.key(KeyEvent::from(KeyCode::Esc)), Outcome::Close);
    }

    #[test]
    fn links_bro_cant_preview_go_back_to_the_system() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("notes.md"), "x").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let uri = crate::panes::links::file_uri(&outside.path().join("notes.md"), None, None).unwrap();
        let target = outside.path().join("notes.md").to_string_lossy().into_owned();
        let mut v = FileView::open(None, roots(dir.path()), target, None, None, Some(uri.clone()), &theme(), tx);
        let Ok(Event::FileView(l)) = rx.recv_timeout(Duration::from_secs(10)) else { panic!("loaded") };
        assert_eq!(v.apply(*l), Outcome::Fallback(uri));
    }

    #[test]
    fn slicing_respects_wide_characters() {
        let line: Styled = vec![(Style::default(), "ab界cd".into())];
        let text = |spans: Vec<Span>| spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(slice(&line, 0, 3)), "ab");
        assert_eq!(text(slice(&line, 2, 3)), "界c");
        assert_eq!(text(slice(&line, 4, 10)), "cd");
    }
}
