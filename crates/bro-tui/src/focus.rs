//! Focus mode (the desktop app): an agent session shown as its conversation instead of its terminal. What you
//! said, what it said and what it did, in big clean type, with a box to reply in. The harness keeps running
//! underneath: a reply is typed into it, and while it shows a choice (a permission prompt, a question) the keys
//! go to the choice, which focus mode draws as buttons ([`choice`]).
//!
//! This is the state the keys change (`app::focus` routes them here); `app/gui/focus` draws it. The terminal UI
//! never turns it on.

use crate::layout::PaneId;
use crate::textarea::TextArea;
use bro_core::catalogue::ModelChoice;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A message you sent stays on screen as sent until the transcript has it, at most this long (a message the
/// agent queued behind a long turn lands in the transcript only when it's read).
pub const SENT_KEEP: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub struct Focus {
    pub quips: crate::quips::Library,
    /// focus mode is on (the window's switch, remembered in its prefs)
    pub on: bool,
    /// sessions showing their terminal while focus mode is on
    pub peek: HashSet<PaneId>,
    /// what you're writing to each session
    pub drafts: HashMap<PaneId, Draft>,
    /// lists read off the UI thread: the models a login can switch to, a folder's commands
    pub lists: Arc<parking_lot::Mutex<Lists>>,
}

/// What [`Focus::lists`] holds, by what it's for (a Codex home, an agent and folder).
#[derive(Default)]
pub struct Lists {
    pub models: HashMap<PathBuf, Arc<Vec<ModelChoice>>>,
    /// a provider's models as switch choices, with how many its catalogue listed then
    pub providers: HashMap<String, (usize, Arc<Vec<ModelChoice>>)>,
    pub commands: HashMap<String, (Instant, Arc<Vec<Command>>)>,
    /// being read now
    pub asking: HashSet<String>,
}

/// A conversation scroll the keys asked for (the window owns the scroll position).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// The reply box of one session, and what you sent from it.
#[derive(Default)]
pub struct Draft {
    pub terminal_quips: crate::quips::Terminal,
    pub quips: crate::quips::Picker,
    pub area: TextArea,
    /// the box's width in cells as last drawn (up and down move by the rows you see)
    pub cols: usize,
    /// messages on their way: shown at once, until the transcript has them
    pub sent: Vec<Sent>,
    /// pictures going with the next message (pasted, dropped): thumbnails over the text, each held in the text by
    /// its token, in the order they go
    pub attachments: Vec<Attachment>,
    /// the list open over the box: the models or the thinking levels (the commands open as you type `/`)
    pub menu: Option<Menu>,
    /// the command on the commands list the keys are on
    pub command: usize,
    /// the commands list was closed (esc) on this text: it opens again when the text changes
    pub dismissed: Option<String>,
    /// a model or thinking change you picked: shown at once, typed into the session when it's free
    pub change: Option<Change>,
    /// a scroll asked for with the keys, for the window to do
    pub scroll: Option<Scroll>,
    /// walking back through what you sent (up on an empty box): how far back
    pub recall: Option<usize>,
}

/// A picture going with the next message, and its token: an invisible character holding its place in the text,
/// where it was pasted. Deleting the token doesn't take the picture off (it goes back at the end); its popover does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub path: PathBuf,
    pub token: char,
}

/// The tokens: variation selectors 17-256, which take no room and are never typed.
const TOKENS: std::ops::RangeInclusive<u32> = 0xE0100..=0xE01EF;

/// A picture's token.
pub fn is_token(c: char) -> bool {
    TOKENS.contains(&(c as u32))
}

/// `text` without its tokens: what the box shows.
pub fn visible(text: &str) -> String {
    text.chars().filter(|c| !is_token(*c)).collect()
}

/// A message in pieces, in order: its text, and each picture where its token was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Picture(PathBuf),
}

/// A list open over the reply box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Model,
    Thinking,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub kind: MenuKind,
    /// the row the keys are on
    pub sel: usize,
    /// what's typed while it's open: it narrows the list (a provider can have hundreds of models)
    pub query: String,
}

/// A model or thinking change picked in the footer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// what the transcript said when it was picked: the change shows until the transcript says something else
    pub was: (Option<String>, Option<String>),
    /// typed into the session yet (Codex takes it only between turns), and how far the conversation was then
    pub sent: Option<usize>,
}

/// A command the box offers as you type `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// without the slash
    pub name: String,
    /// what follows it ("[model]", "<path>")
    pub hint: Option<String>,
    pub detail: Option<String>,
}

impl Command {
    /// It needs something after it ("<path>"): choosing it waits for that instead of running.
    pub fn needs_more(&self) -> bool {
        self.hint.as_deref().is_some_and(|h| h.starts_with('<'))
    }
}

/// The commands matching `query` (what's typed after `/`), best first: the name itself, names starting with it,
/// names with a word starting with it (`review` finds `security-review`), then names holding it. All of them,
/// by name, for nothing typed yet.
pub fn matching(all: &[Command], query: &str) -> Vec<Command> {
    let q = query.to_ascii_lowercase();
    let mut scored: Vec<(u8, &Command)> = all
        .iter()
        .filter_map(|c| {
            let n = c.name.to_ascii_lowercase();
            let rank = if q.is_empty() || n == q {
                0
            } else if n.starts_with(&q) {
                1
            } else if n.split(['-', ':', '_']).any(|w| w.starts_with(&q)) {
                2
            } else if n.contains(&q) {
                3
            } else {
                return None;
            };
            Some((rank, c))
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| if q.is_empty() { std::cmp::Ordering::Equal } else { a.1.name.len().cmp(&b.1.name.len()) }).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.into_iter().map(|(_, c)| c.clone()).collect()
}

/// What's typed after `/` while the box holds only the start of a command (no space yet): the commands list's
/// query.
pub fn command_query(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    (!rest.contains(char::is_whitespace)).then_some(rest)
}

/// A message sent from focus mode.
#[derive(Clone, Debug)]
pub struct Sent {
    pub text: String,
    pub at: Instant,
    /// the conversation's items when it was sent (it lands after them)
    pub after: usize,
    /// sent while the agent was busy: it waits for the turn to end
    pub queued: bool,
    /// the pictures that went with it
    pub pictures: Vec<PathBuf>,
}

fn capital(w: &str) -> String {
    let mut c = w.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// A model as people say it: "claude-opus-5-5" → "Opus 5.5", "claude-sonnet-4-5-20250929" → "Sonnet 4.5",
/// "gpt-6.1-sol" → "GPT-6.1 Sol".
pub fn model_name(raw: &str) -> String {
    let m = raw.trim().rsplit('/').next().unwrap_or(raw);
    let m = m.strip_prefix("claude-").unwrap_or(m);
    let parts: Vec<&str> = m.split(['-', '_']).filter(|p| !p.is_empty()).collect();
    if parts.first().is_some_and(|p| p.eq_ignore_ascii_case("gpt")) {
        let mut out = format!("GPT-{}", parts.get(1).copied().unwrap_or_default());
        for w in parts.iter().skip(2) {
            out.push(' ');
            out.push_str(&capital(w));
        }
        return out.trim_end_matches('-').to_string();
    }
    let (mut name, mut version, mut words) = (String::new(), Vec::<&str>::new(), Vec::<String>::new());
    for p in parts {
        if p.chars().all(|c| c.is_ascii_digit()) {
            // a release date isn't part of the name
            if p.len() <= 2 {
                version.push(p);
            }
        } else if name.is_empty() {
            name = capital(p);
        } else {
            words.push(capital(p));
        }
    }
    let mut out = name;
    if !version.is_empty() {
        out.push(' ');
        out.push_str(&version.join("."));
    }
    for w in words {
        out.push(' ');
        out.push_str(&w);
    }
    out
}

/// The pictures a paste is made of, when it's nothing else: one or more absolute paths to pictures (quoted when
/// they hold spaces, as a copied file or a drag pastes them). None for anything with other words in it.
pub fn only_pictures(text: &str) -> Option<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = vec![];
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (word, tail) = match rest.strip_prefix('"').and_then(|r| r.find('"').map(|end| (&r[..end], &r[end + 1..]))) {
            Some(quoted) => quoted,
            None => rest.split_at(rest.find(char::is_whitespace).unwrap_or(rest.len())),
        };
        let path = PathBuf::from(word.trim_start_matches("file://"));
        if !path.is_absolute() || !is_picture(&path) {
            return None;
        }
        out.push(path);
        rest = tail.trim_start();
    }
    (!out.is_empty()).then_some(out)
}

/// A picture, by its name (nothing is read).
pub fn is_picture(path: &Path) -> bool {
    const EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "heif", "tif", "tiff", "avif", "svg"];
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// Slash commands that open the harness's own screens (pickers, settings, sign-in) rather than talk: sent from
/// focus mode, the session shows its terminal for them.
const SCREENS: &[&str] = &[
    "add-dir", "agents", "apps", "approvals", "bashes", "bug", "config", "context", "cost", "diff", "doctor", "effort", "experimental", "export", "feedback", "help", "hooks", "ide", "import", "install-github-app", "keymap", "login", "logout",
    "mcp", "memories", "memory", "mention", "model", "output-style", "permissions", "plugin", "plugins", "privacy-settings", "ps", "release-notes", "resume", "rewind", "sandbox", "settings", "skills", "stats", "status", "statusline",
    "tasks", "terminal-setup", "theme", "title", "todos", "upgrade", "usage", "vim",
];

/// Commands that open a screen on their own but act at once when given what they'd ask for (`/model opus`).
const DIRECT_WITH_ARGS: &[&str] = &["add-dir", "cd", "config", "effort", "export", "mcp", "model", "output-style", "rename"];

/// `text` is a slash command that opens one of the harness's own screens.
pub fn opens_screen(text: &str) -> bool {
    let Some(rest) = text.trim().strip_prefix('/') else { return false };
    let mut words = rest.split_whitespace();
    let name = words.next().unwrap_or_default().to_ascii_lowercase();
    if words.next().is_some() && DIRECT_WITH_ARGS.contains(&name.as_str()) {
        return false;
    }
    SCREENS.contains(&name.as_str())
}

impl Focus {
    pub fn draft(&mut self, id: PaneId) -> &mut Draft {
        self.drafts.entry(id).or_default()
    }

    /// Show `id`'s terminal instead of its conversation (or back).
    pub fn toggle_peek(&mut self, id: PaneId) {
        if !self.peek.remove(&id) {
            self.peek.insert(id);
        }
    }

    /// Forget sessions that closed.
    pub fn retain(&mut self, live: impl Fn(PaneId) -> bool) {
        self.peek.retain(|id| live(*id));
        self.drafts.retain(|id, _| live(*id));
    }
}

/// Whether what the transcript says you said (`said`) is the message you sent (`sent`), whitespace aside: the
/// same words, or holding them (messages queued behind a turn can arrive as one).
pub fn holds(said: &str, sent: &str) -> bool {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (said, sent) = (flat(said), flat(sent));
    !sent.is_empty() && (said == sent || said.contains(&sent))
}

impl Draft {
    /// Drop the sent messages the transcript now has (`landed`) and those that waited too long.
    pub fn reconcile(&mut self, landed: impl Fn(&Sent) -> bool) {
        let now = Instant::now();
        self.sent.retain(|s| now.saturating_duration_since(s.at) < SENT_KEEP && !landed(s));
    }

    /// Nothing written in the box (pictures may still be waiting in it).
    pub fn blank(&self) -> bool {
        visible(self.area.text()).trim().is_empty()
    }

    /// A picture for the next message, its token where the caret is.
    pub fn attach(&mut self, path: PathBuf) {
        if self.attachments.iter().any(|a| a.path == path) {
            return;
        }
        let token = TOKENS.clone().filter_map(char::from_u32).find(|t| !self.attachments.iter().any(|a| a.token == *t) && !self.area.text().contains(*t));
        let Some(token) = token else { return };
        self.area.insert(token.encode_utf8(&mut [0; 4]));
        self.attachments.push(Attachment { path, token });
        self.keep_tokens();
    }

    /// Take a picture off the next message (its popover's Remove), and its token with it.
    pub fn detach(&mut self, n: usize) {
        if n < self.attachments.len() {
            self.attachments.remove(n);
            self.keep_tokens();
        }
    }

    /// Every picture's token in the text once, after an edit: one that was deleted goes back at the end (the
    /// picture stays), a stray one goes. The pictures follow their tokens' order.
    pub fn keep_tokens(&mut self) {
        let text = self.area.text();
        let cur = self.area.cursor();
        let mut out = String::with_capacity(text.len() + 4 * self.attachments.len());
        let mut seen: Vec<char> = vec![];
        let mut at = cur;
        for (i, c) in text.char_indices() {
            if is_token(c) && (!self.attachments.iter().any(|a| a.token == c) || seen.contains(&c)) {
                if i < cur {
                    at -= c.len_utf8();
                }
                continue;
            }
            if is_token(c) {
                seen.push(c);
            }
            out.push(c);
        }
        for a in &self.attachments {
            if !seen.contains(&a.token) {
                out.push(a.token);
                seen.push(a.token);
            }
        }
        self.attachments.sort_by_key(|a| seen.iter().position(|t| *t == a.token));
        if out != text {
            self.area.set_text_at(&out, at);
        }
    }

    /// The message in pieces: its text split where the pictures' tokens are, with a space either side of each
    /// picture, the ends trimmed.
    #[cfg(test)]
    pub fn parts(&self) -> Vec<Part> {
        self.parts_with_text(self.area.text())
    }

    /// Split expanded text with the same picture anchors, without changing the editable draft.
    pub fn parts_with_text(&self, source: &str) -> Vec<Part> {
        let mut parts: Vec<Part> = vec![];
        let mut text = String::new();
        for c in source.chars() {
            match self.attachments.iter().find(|a| a.token == c) {
                Some(a) => {
                    if !text.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut text)));
                    }
                    parts.push(Part::Picture(a.path.clone()));
                }
                None if is_token(c) => {}
                None => text.push(c),
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        let n = parts.len();
        for i in 0..n {
            let (before, after) = (i > 0 && matches!(parts[i - 1], Part::Picture(_)), i + 1 < n && matches!(parts[i + 1], Part::Picture(_)));
            if let Part::Text(t) = &mut parts[i] {
                let mut s = if i == 0 { t.trim_start().to_string() } else { t.clone() };
                if i + 1 == n {
                    s = s.trim_end().to_string();
                }
                if before && !s.starts_with(char::is_whitespace) {
                    s.insert(0, ' ');
                }
                if after && !s.ends_with(char::is_whitespace) {
                    s.push(' ');
                }
                *t = s;
            }
        }
        parts.retain(|p| !matches!(p, Part::Text(t) if t.trim().is_empty() && t.len() <= 1) || n == 1);
        parts
    }
}

// ---------------------------------------------------------------------------------------------------------
// The choice on screen
// ---------------------------------------------------------------------------------------------------------

/// A choice the harness shows and waits on: a permission prompt, a question with options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// "Do you want to proceed?"
    pub question: String,
    /// the lines just above the question (what it's about: "Bash command", "npm publish"), top first
    pub context: Vec<String>,
    pub options: Vec<Opt>,
    /// the keys it says it takes ("Enter to select · Tab/Arrow keys to navigate · Esc to cancel")
    pub hint: Option<String>,
}

/// One option of a [`Choice`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    /// the number that picks it
    pub key: String,
    pub label: String,
    /// the line(s) under it
    pub detail: Option<String>,
    /// the harness's cursor is on it
    pub focused: bool,
}

/// What a row says once the frame around it is gone: box-drawing edges off both ends, trailing space off.
fn unframed(row: &str) -> String {
    const FRAME: &[char] = &['│', '┃', '║', '╭', '╰', '╮', '╯', '▌', '▐'];
    let t = row.trim_end();
    let t = t.trim_end_matches(|c: char| FRAME.contains(&c) || c.is_whitespace());
    let t = t.strip_prefix(|c: char| FRAME.contains(&c)).unwrap_or(t);
    t.to_string()
}

fn indent(row: &str) -> usize {
    row.chars().take_while(|c| c.is_whitespace()).count()
}

/// The option on a row: (its number, where the number starts, its label, whether the cursor is on it).
fn option(row: &str) -> Option<(String, usize, String, bool)> {
    let key = crate::panes::term::prompt_option(row)?;
    let col = row.find(&*key)?;
    let before = &row[..col];
    let focused = before.chars().any(|c| matches!(c, '❯' | '›' | '>' | '●'));
    let label = row[col + key.len() + 1..].trim().to_string();
    Some((key, row[..col].chars().count(), label, focused))
}

/// A line of key hints under a choice.
fn is_hint(row: &str) -> bool {
    let l = row.to_lowercase();
    ["enter to", "esc to", "tab to", "arrow keys", "to navigate", "to cancel", "to select", "to confirm"].iter().any(|m| l.contains(m))
}

/// A rule or a box's edge: where the context above a question stops.
fn is_rule(row: &str) -> bool {
    let t = row.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '─' | '━' | '═' | '-' | '╌' | '┄' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'))
}

/// The choice at the bottom of a screen (its rows, top first): numbered options from 1 under a question,
/// anything indented under an option its detail. None when there isn't one (two options at least).
pub fn choice(rows: &[String]) -> Option<Choice> {
    let lines: Vec<String> = rows.iter().map(|r| unframed(r)).collect();
    let end = lines.iter().rposition(|l| !l.trim().is_empty())? + 1;
    let start = end.saturating_sub(48);
    let opts: Vec<usize> = (start..end).filter(|&i| option(&lines[i]).is_some()).collect();
    let &last = opts.last()?;
    // the options must be the last thing on screen: below them only their details, blanks and key hints
    let tail_ok = (last + 1..end).all(|i| {
        let l = &lines[i];
        l.trim().is_empty() || is_hint(l) || is_rule(l) || indent(l) > option(&lines[last]).map_or(0, |o| o.1) + 1
    });
    if !tail_ok {
        return None;
    }
    // walk up while the rows between options are details (indented under the option above) or blank
    let mut group = vec![last];
    let mut at = last;
    while let Some(&prev) = opts.iter().rev().find(|&&j| j < at) {
        let col = option(&lines[prev]).map_or(0, |o| o.1);
        if (prev + 1..at).all(|k| lines[k].trim().is_empty() || indent(&lines[k]) > col + 1) {
            group.push(prev);
            at = prev;
        } else {
            break;
        }
    }
    group.reverse();
    let mut options: Vec<Opt> = vec![];
    for (n, &i) in group.iter().enumerate() {
        let (key, col, label, focused) = option(&lines[i])?;
        let until = group.get(n + 1).copied().unwrap_or(end);
        let detail: Vec<&str> = (i + 1..until).map(|k| lines[k].as_str()).take_while(|l| !l.trim().is_empty()).filter(|l| indent(l) > col + 1 && !is_hint(l)).map(str::trim).collect();
        options.push(Opt { key, label, detail: (!detail.is_empty()).then(|| detail.join(" ")), focused });
    }
    // numbered from 1, in order: a numbered list in the agent's reply isn't a choice
    let numbered = options.iter().enumerate().all(|(i, o)| o.key == (i + 1).to_string());
    if options.len() < 2 || !numbered {
        return None;
    }
    let mut q = group[0];
    while q > 0 && lines[q - 1].trim().is_empty() {
        q -= 1;
    }
    let question = if q > 0 { lines[q - 1].trim().to_string() } else { String::new() };
    let mut context = vec![];
    let mut k = q.saturating_sub(1);
    while k > 0 && context.len() < 6 {
        k -= 1;
        let l = &lines[k];
        if is_rule(l) {
            break;
        }
        if !l.trim().is_empty() {
            context.push(l.trim().to_string());
        }
    }
    context.reverse();
    let hint = (last + 1..end).map(|i| lines[i].trim()).find(|l| is_hint(l)).map(str::to_string);
    Some(Choice { question, context, options, hint })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_permission_prompt_is_a_choice() {
        let screen = rows("● Running the tests\n\n╭──────────────────────────────╮\n│ Bash command                 │\n│                              │\n│   npm publish                │\n│   Publish the package        │\n│                              │\n│ Do you want to proceed?      │\n│ ❯ 1. Yes                     │\n│   2. Yes, and don't ask again│\n│   3. No, and tell Claude what to do differently (esc) │\n╰──────────────────────────────╯\n");
        let c = choice(&screen).expect("a choice");
        assert_eq!(c.question, "Do you want to proceed?");
        assert_eq!(c.context, ["Bash command", "npm publish", "Publish the package"]);
        let got: Vec<(&str, &str, bool)> = c.options.iter().map(|o| (o.key.as_str(), o.label.as_str(), o.focused)).collect();
        assert_eq!(got, [("1", "Yes", true), ("2", "Yes, and don't ask again", false), ("3", "No, and tell Claude what to do differently (esc)", false)]);
    }

    #[test]
    fn a_question_keeps_its_option_details_and_hint() {
        let screen = rows(" ☐ When  ✔ Submit\n\nWhen does the flicker happen?\n\n❯ 1. Idle, just looking\n     Happens on its own\n  2. While things update\n     When Claude is working\n  3. Type something.\n\nEnter to select · Tab/Arrow keys to navigate · Esc to cancel\n");
        let c = choice(&screen).expect("a choice");
        assert_eq!(c.question, "When does the flicker happen?");
        assert_eq!(c.options.len(), 3);
        assert_eq!(c.options[0].detail.as_deref(), Some("Happens on its own"));
        assert_eq!(c.options[1].detail.as_deref(), Some("When Claude is working"));
        assert_eq!(c.options[2].detail, None);
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("Esc to cancel")));
        assert_eq!(c.context, ["☐ When  ✔ Submit"]);
    }

    #[test]
    fn a_sent_message_is_found_in_what_the_transcript_says() {
        assert!(holds("ship  it\nnow", "ship it now"));
        assert!(holds("first thing\n\nship it", "ship it"), "queued messages can arrive as one");
        assert!(!holds("ship it later", "ship it now"));
        assert!(!holds("anything", " "));
    }

    #[test]
    fn commands_with_screens_are_told_from_the_rest() {
        assert!(opens_screen("/model"));
        assert!(opens_screen("  /config  "));
        assert!(opens_screen("/resume 0b6e"));
        assert!(!opens_screen("/model claude-sonnet-5-5"), "given the model, it switches without its picker");
        assert!(!opens_screen("/effort high"));
        assert!(!opens_screen("/compact keep the plan"));
        assert!(!opens_screen("/review"));
        assert!(!opens_screen("model the data"));
    }

    fn draft(text: &str) -> Draft {
        let mut d = Draft::default();
        d.area.set_text(text);
        d
    }

    #[test]
    fn a_picture_holds_its_place_with_a_token_that_takes_no_room() {
        let token = char::from_u32(*TOKENS.start()).unwrap();
        assert_eq!(unicode_width::UnicodeWidthChar::width(token).unwrap_or(0), 0, "the box wraps as if it weren't there");
        let mut d = draft("compare ");
        d.attach(PathBuf::from("/a.png"));
        d.area.insert(" with ");
        d.attach(PathBuf::from("/b b.png"));
        d.area.insert(" please");
        assert_eq!(visible(d.area.text()), "compare  with  please");
        assert_eq!(d.parts(), [Part::Text("compare ".into()), Part::Picture("/a.png".into()), Part::Text(" with ".into()), Part::Picture("/b b.png".into()), Part::Text(" please".into())]);
        assert!(!d.blank());
        // the whole text deleted: the pictures stay, their tokens back at the end
        d.area.set_text("");
        d.keep_tokens();
        assert!(d.blank());
        assert_eq!(d.parts(), [Part::Picture("/a.png".into()), Part::Picture("/b b.png".into())]);
        // a token deleted on its own goes to the end, and the pictures go in that order
        let mut d = draft("look ");
        d.attach(PathBuf::from("/a.png"));
        d.area.insert(" and ");
        d.attach(PathBuf::from("/b.png"));
        let text: String = d.area.text().chars().filter(|c| *c != d.attachments[0].token).collect();
        d.area.set_text(&text);
        d.keep_tokens();
        assert_eq!(d.attachments.iter().map(|a| a.path.to_string_lossy().into_owned()).collect::<Vec<_>>(), ["/b.png", "/a.png"]);
        assert_eq!(d.parts(), [Part::Text("look  and ".into()), Part::Picture("/b.png".into()), Part::Picture("/a.png".into())], "the spaces you typed around it stay");
        // only its popover takes a picture off
        d.detach(0);
        assert_eq!(d.parts(), [Part::Text("look  and ".into()), Part::Picture("/a.png".into())]);
        assert_eq!(d.area.text().chars().filter(|c| is_token(*c)).count(), 1);
    }

    #[test]
    fn a_pasted_path_to_a_picture_is_the_picture() {
        let root = if cfg!(windows) { r"C:\Users\me" } else { "/home/me" };
        let one = format!("{root}{}paste-1.png", std::path::MAIN_SEPARATOR);
        assert_eq!(only_pictures(&one), Some(vec![PathBuf::from(&one)]));
        let spaced = format!("\"{root} shots{}a b.PNG\" {one}", std::path::MAIN_SEPARATOR);
        assert_eq!(only_pictures(&spaced).map(|p| p.len()), Some(2));
        assert_eq!(only_pictures(&format!("look at {one}")), None, "words around it: it's text");
        assert_eq!(only_pictures("paste-1.png"), None, "a name alone could be anywhere");
        assert_eq!(only_pictures(&format!("{root}{}notes.txt", std::path::MAIN_SEPARATOR)), None);
    }

    #[test]
    fn the_commands_list_finds_what_you_start_typing() {
        let all: Vec<Command> = ["model", "mcp", "memory", "security-review", "review", "compact"].iter().map(|n| Command { name: n.to_string(), hint: None, detail: None }).collect();
        let names = |q: &str| matching(&all, q).into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("m"), ["mcp", "model", "memory", "compact"]);
        assert_eq!(names("review"), ["review", "security-review"]);
        assert_eq!(names("").len(), 6);
        assert!(names("zzz").is_empty());
        assert_eq!(command_query("/mo"), Some("mo"));
        assert_eq!(command_query("/model opus"), None, "once it has arguments the list goes");
        assert_eq!(command_query("hi /mo"), None);
    }

    #[test]
    fn numbered_lists_in_replies_are_not_choices() {
        assert_eq!(choice(&rows("Plan:\n1. Read the code\n2. Fix it\n\n> ")), None, "the input box is below the list");
        assert_eq!(choice(&rows("Steps\n  2. second\n  3. third\n")), None, "a choice starts at 1");
        assert_eq!(choice(&rows("Do you want to?\n❯ 1. Yes\n")), None, "one option isn't a choice");
    }
}
