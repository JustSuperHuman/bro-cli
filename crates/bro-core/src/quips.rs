//! Local text expanders. Only Quips used in a message travel to the agent.

use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Quip {
    pub trigger: String,
    pub name: String,
    pub instructions: String,
}

impl Quip {
    pub fn token(&self) -> String { format!(":{}", self.trigger) }
}

pub struct Library {
    pub items: Vec<Quip>,
    pub error: Option<String>,
    path: Option<PathBuf>,
}

impl Default for Library {
    fn default() -> Self { Self { items: builtins(), error: None, path: None } }
}

fn builtins() -> Vec<Quip> {
    [
        ("full-send", "Full-Send", "Do not ask questions. Make reasonable assumptions and take this task as far as you can. Finish the work, verify the result, and fix issues you find. For a pull request, follow its checks through and merge when ready. If required permissions or access block you, complete everything else and report the blocker."),
        ("full-stop", "Full-Stop", "Do not ask questions. Make reasonable assumptions, finish the work, and verify the result. Prepare everything for my review, then stop before the final action. Do not merge, enable auto-merge, publish, deploy, or send anything on my behalf. Leave any pull request open and ready for me to merge. Explain what is ready and what remains."),
        ("plain-english", "Plain English", "Use everyday language. Explain unfamiliar terms and include a concrete example when it helps. Assume I am new to the topic without talking down to me."),
        ("keep-it-short", "Keep It Short", "Give me the useful answer first. Keep it brief and specific. Leave out introductions, repeated points, and closing summaries."),
        ("step-by-step", "Step by Step", "Break this into clear, numbered steps in the order I should follow them. Tell me what to do and what to expect at each step."),
        ("make-a-plan", "Make a Plan", "Give me a practical plan before doing the work. Include the main steps, assumptions, and any decisions I need to make. Do not make changes or take external actions yet."),
        ("second-look", "Second Look", "Review your answer or work before finishing. Check for mistakes, missing details, and weak assumptions. Fix what you can and tell me about any remaining uncertainty."),
        ("check-the-facts", "Check the Facts", "Verify factual claims with reliable sources when tools are available. Link the sources you actually checked, distinguish facts from assumptions, and say clearly what you could not verify. Never invent sources."),
    ].into_iter().map(|(trigger, name, instructions)| Quip { trigger: trigger.into(), name: name.into(), instructions: instructions.into() }).collect()
}

pub fn summary(trigger: &str) -> &'static str {
    match trigger {
        "full-send" => "Finish it, check it, and take it all the way.",
        "full-stop" => "Get it ready. Leave the final action to you.",
        "plain-english" => "An explanation anyone can follow.",
        "keep-it-short" => "The useful answer, with fewer words.",
        "step-by-step" => "Clear steps you can follow in order.",
        "make-a-plan" => "See the approach before any work begins.",
        "second-look" => "Check the work for mistakes and loose ends.",
        "check-the-facts" => "Verify claims and show the sources.",
        _ => "Your own instructions, ready when you need them.",
    }
}

impl Library {
    pub fn load(path: PathBuf) -> Self {
        let mut lib = Self { path: Some(path.clone()), ..Self::default() };
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Vec<Quip>>(&bytes) {
                Ok(items) => {
                    let mut seen = std::collections::HashSet::new();
                    if items.iter().any(|q| validate(q).is_err() || !seen.insert(q.trigger.clone())) {
                        lib.error = Some("Your Quips file has an invalid or duplicate shortcut. Fix it before saving.".into());
                    } else { lib.items = items; }
                }
                Err(e) => lib.error = Some(format!("Couldn't read your Quips: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => lib.error = Some(format!("Couldn't read your Quips: {e}")),
        }
        lib
    }

    /// Persist first, so an unsuccessful save never changes the active library.
    pub fn save_items(&mut self, items: Vec<Quip>) -> Result<(), String> {
        if let Some(e) = &self.error { return Err(e.clone()); }
        let mut seen = std::collections::HashSet::new();
        for q in &items {
            validate(q)?;
            if !seen.insert(&q.trigger) { return Err("That shortcut is already in use.".into()); }
        }
        if let Some(path) = &self.path {
            crate::util::write_json_pretty(path, &items).map_err(|e| format!("Couldn't save your Quips: {e}"))?;
        }
        self.items = items;
        Ok(())
    }

    pub fn matching(&self, query: &str) -> Vec<usize> {
        let q = query.to_lowercase();
        self.items.iter().enumerate().filter(|(_, item)| item.trigger.contains(&q) || item.name.to_lowercase().contains(&q)).map(|(i, _)| i).collect()
    }

    pub fn matches<'a>(&'a self, text: &str) -> Vec<(Range<usize>, &'a Quip)> {
        tokens(text).into_iter().filter_map(|r| self.items.iter().find(|q| q.trigger.eq_ignore_ascii_case(&text[r.start + 1..r.end])).map(|q| (r, q))).collect()
    }

    pub fn expand(&self, text: &str) -> Result<String, String> {
        let found = self.matches(text);
        let has = |name| found.iter().any(|(_, q)| q.trigger == name);
        if has("full-send") && has("full-stop") {
            return Err("Choose Full-Send or Full-Stop for this message. They have different stopping points.".into());
        }
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        let mut seen = std::collections::HashSet::new();
        for (range, q) in found {
            out.push_str(&text[at..range.start]);
            let body = if seen.insert(&q.trigger) { q.instructions.as_str() } else { "" };
            // Keep the original token, including its case, for transcript display and recall.
            out.push_str(&format!("<bro-quip name=\"{}\">{}</bro-quip>", &text[range.start + 1..range.end], escape(body)));
            at = range.end;
        }
        out.push_str(&text[at..]);
        Ok(out)
    }
}

pub fn path() -> PathBuf { crate::paths::bro_dir().join("quips.json") }

/// A terminal keeps selected definitions locally until the next message is sent.
#[derive(Default)]
pub struct Terminal {
    pub query: Option<String>,
    pub selected: usize,
    pub pending: Vec<Quip>,
    pub before: Option<char>,
    pub command: Option<bool>,
    pub code_ticks: usize,
    pub tick_run: usize,
}

impl Terminal {
    pub fn finish_ticks(&mut self) {
        if self.tick_run > 0 {
            if self.code_ticks == 0 { self.code_ticks = self.tick_run; }
            else if self.code_ticks == self.tick_run { self.code_ticks = 0; }
            self.tick_run = 0;
        }
    }

    pub fn observe_text(&mut self, text: &str) {
        for c in text.chars() {
            if c == '`' { self.tick_run += 1; } else { self.finish_ticks(); }
        }
    }
}

pub fn expand_selected(items: &[Quip]) -> Result<String, String> {
    let text = items.iter().map(Quip::token).collect::<Vec<_>>().join(" ");
    Library { items: items.to_vec(), error: None, path: None }.expand(&text)
}

fn validate(q: &Quip) -> Result<(), String> {
    if q.trigger.is_empty() || q.trigger.len() > 48 || !q.trigger.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') || q.trigger.starts_with('-') || q.trigger.ends_with('-') {
        return Err("Use lowercase letters, numbers, and hyphens for the shortcut, like my-quip.".into());
    }
    if q.name.trim().is_empty() || q.instructions.trim().is_empty() {
        return Err("Give your Quip a name and some instructions.".into());
    }
    Ok(())
}

fn escape(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

/// Restore the short form even after the Quip has been edited or removed locally.
pub fn collapse(text: &str) -> String {
    static TAG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    TAG.get_or_init(|| regex::Regex::new(r#"(?s)<bro-quip name="([a-zA-Z0-9-]+)">.*?</bro-quip>"#).unwrap()).replace_all(text, ":$1").into_owned()
}

/// Explicit shortcuts only, outside code, escaped text, URLs, and path components.
fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut out = vec![];
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut code = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let n = bytes[i..].iter().take_while(|b| **b == b'`').count();
            if code == 0 { code = n; } else if code == n { code = 0; }
            i += n;
            continue;
        }
        if code == 0 && bytes[i] == b':' && (i == 0 || text[..i].chars().next_back().is_some_and(|c| c.is_whitespace() || "([{\"'".contains(c))) {
            let end = i + 1 + bytes[i + 1..].iter().take_while(|b| b.is_ascii_alphanumeric() || **b == b'-').count();
            if end > i + 1 && (end == bytes.len() || !text[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || "_/:\\-".contains(c))) {
                out.push(i..end);
            }
            i = end;
        } else { i += 1; }
    }
    out
}

/// The partial token under the caret. Completing it replaces just this range.
pub fn query(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    if !text.is_char_boundary(cursor) { return None; }
    let head = &text[..cursor];
    let start = head.rfind(':')?;
    if start > 0 && !head[..start].chars().next_back().is_some_and(|c| c.is_whitespace() || "([{\"'".contains(c)) { return None; }
    if !head[start + 1..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') { return None; }
    // The same code exclusion as expansion, including a bare colon.
    let probe = format!("{}x{}", head, &text[cursor..]);
    if !tokens(&probe).iter().any(|r| r.start == start) { return None; }
    let end = cursor + text[cursor..].bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'-').count();
    Some((start..end, head[start + 1..].to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_only_explicit_tokens_and_preserves_the_short_transcript() {
        let lib = Library::default();
        let source = "Café :Full-Send, then :plain-english. :Full-Send";
        let wire = lib.expand(source).unwrap();
        assert!(wire.contains("Do not ask questions."));
        assert!(!wire.contains("Break this into clear"), "unused Quips never travel to the model");
        assert_eq!(wire.matches("Do not ask questions.").count(), 1);
        assert_eq!(collapse(&wire), source);
        for source in ["`:full-send`", "```\n:full-send\n```", "https://x/:full-send", "C:\\:full-send", "\\:full-send", ":full-send-more", ":full-send_foo", ":full-sendé", "ordinary full-send", ":unknown"] {
            assert_eq!(lib.expand(source).unwrap(), source);
        }
        assert!(lib.expand(":full-send :full-stop").is_err());
    }

    #[test]
    fn completion_uses_the_caret_and_obeys_code_boundaries() {
        assert_eq!(query("Help :pl please", 8), Some((5..8, "pl".into())));
        assert_eq!(query("é :", 4), Some((3..4, "".into())));
        assert_eq!(query("`:full", 6), None);
        assert_eq!(query("https://x", 6), None);
        assert_eq!(query("(:full-send)", 6), Some((1..11, "full".into())));
    }

    #[test]
    fn custom_quips_persist_and_bad_files_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quips.json");
        let mut lib = Library::load(path.clone());
        let mut items = lib.items.clone();
        items.push(Quip { trigger: "my-voice".into(), name: "My Voice".into(), instructions: "Use <simple> words & examples.".into() });
        lib.save_items(items.clone()).unwrap();
        assert_eq!(Library::load(path.clone()).items, items);
        let wire = lib.expand(":my-voice").unwrap();
        assert!(wire.contains("&lt;simple&gt; words &amp;"));
        assert_eq!(collapse(&wire), ":my-voice");
        items.push(items[0].clone());
        assert!(lib.save_items(items).is_err());
        std::fs::write(&path, "broken").unwrap();
        let mut broken = Library::load(path.clone());
        assert!(broken.save_items(vec![]).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
    }
}
