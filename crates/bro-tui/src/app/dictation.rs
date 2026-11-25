use super::App;
use crate::{
    alerts::Kind,
    pane::{Activity, Event, PaneId},
    textarea::TextArea,
};
use bro_core::config::{DictationSettings, DictationShortcut};
use bro_voice::{Dictation, VoiceConfig, VoiceEvent};
use std::{
    ops::Range,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct ComposerDictation {
    generation: u64,
    take: Option<Take>,
    pressed: Option<(Instant, DictationShortcut)>,
}

struct Take {
    pane: PaneId,
    terminal: bool,
    draft: String,
    selection: Range<usize>,
    settings: DictationSettings,
    key_env: String,
    language: Option<String>,
    recorder: Dictation,
    phase: Phase,
    send: bool,
    raw: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Phase {
    Recording(f32),
    Transcribing,
    Cleaning,
}

enum TakeResult {
    Capture(VoiceEvent),
    Cleaned(Option<String>),
}

pub(crate) struct Update(TakeResult);

impl ComposerDictation {
    pub(crate) fn active(&self) -> bool {
        self.take.is_some()
    }

    pub(crate) fn phase(&self, pane: PaneId) -> Option<Phase> {
        self.take
            .as_ref()
            .filter(|take| take.pane == pane)
            .map(|take| take.phase)
    }

    pub(crate) fn cancel(&mut self, pane: PaneId) {
        if self.phase(pane).is_some() {
            self.cancel_all();
        }
    }

    pub(crate) fn cancel_all(&mut self) {
        self.take = None;
        self.pressed = None;
    }

    pub(crate) fn finish(&mut self, pane: PaneId, send: bool) {
        let Some(take) = self.take.as_mut().filter(|take| take.pane == pane) else {
            return;
        };
        take.send |= send;
        if matches!(take.phase, Phase::Recording(_)) {
            take.phase = Phase::Transcribing;
            self.pressed = None;
            take.recorder.finish();
        }
    }

    pub(crate) fn release_shortcut(&mut self) -> bool {
        let Some((pressed, mode)) = self.pressed.take() else {
            return false;
        };
        if mode == DictationShortcut::Hold
            || mode == DictationShortcut::Auto && pressed.elapsed() >= Duration::from_millis(300)
        {
            if let Some(pane) = self.take.as_ref().map(|take| take.pane) {
                self.finish(pane, false);
            }
        }
        true
    }
}

impl App {
    pub(crate) fn can_dictate(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| {
            p.as_term_ref().is_some_and(|term| {
                term.meta.harness.is_some() && term.queued.is_none() && term.spawn_error().is_none()
            }) && self.focus_choice(pane).is_none()
                && (self.focus_view(pane) || p.activity() != Some(Activity::Blocked))
        })
    }

    pub(crate) fn toggle_dictation(&mut self, pane: PaneId, by_shortcut: bool) {
        if let Some(take) = &self.dictation.take {
            self.dictation.finish(take.pane, false);
            return;
        }
        if !self.can_dictate(pane) {
            return;
        }
        let voice = self.svc.settings().voice;
        if !voice.dictation.enabled {
            #[cfg(feature = "gui")]
            self.show_dictation_settings();
            return;
        }
        if self.svc.is_demo() {
            return self.toast(Kind::Info, "Dictation is available in a live session.");
        }

        let terminal = !self.focus_view(pane);
        let (draft, selection) = if terminal {
            (String::new(), 0..0)
        } else {
            let d = self.focus.draft(pane);
            d.menu = None;
            d.quips.open = false;
            d.quips.dismissed = Some((d.area.text().to_owned(), d.area.cursor()));
            let cursor = d.area.cursor();
            let selection = d.area.selection().unwrap_or(cursor..cursor);
            (d.area.text().to_owned(), selection)
        };
        let mut keywords = voice
            .dictation
            .dictionary
            .keys()
            .filter(|word| !word.trim().is_empty())
            .take(100)
            .cloned()
            .collect::<Vec<_>>();
        if !voice.dictation.correction_cue.trim().is_empty() {
            keywords.insert(0, voice.dictation.correction_cue.trim().to_owned());
        }
        if let Some(term) = self.panes.get(&pane).and_then(|p| p.as_term_ref())
            && let Some(project) = term.meta.project.root.file_name().and_then(|n| n.to_str())
            && !keywords
                .iter()
                .any(|word| word.eq_ignore_ascii_case(project))
        {
            keywords.insert(0, project.to_owned());
        }
        keywords.truncate(100);
        let config = VoiceConfig {
            api_key_env: voice.api_key_env.clone(),
            model: voice
                .model
                .unwrap_or_else(|| bro_voice::DEFAULT_MODEL.into()),
            language: voice.language.clone(),
            keywords,
            min_ms: 200,
            max_secs: 300,
            ..VoiceConfig::default()
        };
        self.dictation.generation += 1;
        let id = self.dictation.generation;
        let tx = self.tx.clone();
        let recorder = match Dictation::start(
            config,
            Box::new(move |event| {
                let _ = tx.send(Event::Dictation(id, Update(TakeResult::Capture(event))));
            }),
        ) {
            Ok(recorder) => recorder,
            Err(e) => return self.toast(Kind::Error, format!("Couldn’t start dictation. {e}")),
        };
        self.dictation.pressed = by_shortcut.then(|| (Instant::now(), voice.dictation.shortcut));
        self.dictation.take = Some(Take {
            pane,
            terminal,
            draft,
            selection,
            recorder,
            phase: Phase::Recording(0.0),
            settings: voice.dictation,
            language: voice.language,
            key_env: voice.api_key_env,
            send: false,
            raw: None,
        });
    }

    pub(crate) fn dictation_event(&mut self, id: u64, Update(result): Update) {
        if id != self.dictation.generation {
            return;
        }
        let Some(take) = &mut self.dictation.take else {
            return;
        };
        match result {
            TakeResult::Capture(VoiceEvent::Level(level)) => {
                if matches!(take.phase, Phase::Recording(_)) {
                    take.phase = Phase::Recording(level);
                }
            }
            TakeResult::Capture(VoiceEvent::Transcribing) => {
                take.phase = Phase::Transcribing;
                self.dictation.pressed = None;
            }
            TakeResult::Capture(VoiceEvent::Transcript(text)) => {
                let raw = normalize(&text, &take.settings, take.language.as_deref());
                if raw.is_empty() {
                    self.dictation.cancel_all();
                    return self.toast(Kind::Info, "No speech detected.");
                }
                if take.settings.cleanup {
                    let [before, selected, after] = take.context();
                    let tx = self.tx.clone();
                    let key = take.key_env.clone();
                    let model = take
                        .settings
                        .cleanup_model
                        .clone()
                        .unwrap_or_else(|| "gpt-4.1".into());
                    let cue = take.settings.correction_cue.clone();
                    let text = raw.clone();
                    let spawned = std::thread::Builder::new()
                        .name("bro-dictation-cleanup".into())
                        .spawn(move || {
                            let edited = bro_voice::clean_transcript(
                                &key,
                                &model,
                                &text,
                                [&before, &selected, &after],
                                &cue,
                            );
                            let _ =
                                tx.send(Event::Dictation(id, Update(TakeResult::Cleaned(edited))));
                        });
                    if spawned.is_ok() {
                        take.phase = Phase::Cleaning;
                        // Capture has ended and the raw transcript is all that Skip needs.
                        take.raw = Some(raw);
                        return;
                    }
                }
                self.insert_dictation(raw);
            }
            TakeResult::Cleaned(text) => {
                if let Some(raw) = take.raw.take() {
                    let edited = text.map(|text| apply_dictionary(&text, &take.settings));
                    self.insert_dictation(edited.unwrap_or(raw));
                }
            }
            TakeResult::Capture(VoiceEvent::Cancelled) => {
                self.dictation.cancel_all();
                self.toast(Kind::Info, "No speech detected.");
            }
            TakeResult::Capture(VoiceEvent::Error(error)) => {
                self.dictation.cancel_all();
                self.toast(Kind::Error, error);
            }
            TakeResult::Capture(_) => {}
        }
    }

    pub(crate) fn skip_dictation_cleanup(&mut self, pane: PaneId) {
        if let Some(take) = self
            .dictation
            .take
            .as_mut()
            .filter(|take| take.pane == pane)
            && let Some(raw) = take.raw.take()
        {
            self.insert_dictation(raw);
        }
    }

    fn insert_dictation(&mut self, text: String) {
        let Some(take) = self.dictation.take.take() else {
            return;
        };
        self.dictation.pressed = None;
        if take.terminal {
            if !self.can_dictate(take.pane) {
                return self.toast(
                    Kind::Info,
                    "Dictation ended while the terminal was waiting for an answer.",
                );
            }
            if let Some(term) = self.panes.get_mut(&take.pane).and_then(|p| p.as_term()) {
                term.insert_dictation(&text, take.send);
            }
            return;
        }
        let Some(d) = self.focus.drafts.get_mut(&take.pane) else {
            return;
        };
        insert(
            &mut d.area,
            &take.draft,
            take.selection,
            &text,
            take.language.as_deref(),
        );
        d.keep_tokens();
        d.recall = None;
        if take.send && self.focus_view(take.pane) && self.focus_choice(take.pane).is_none() {
            self.focus_submit(take.pane);
        }
    }
}

impl Take {
    fn context(&self) -> [String; 3] {
        let [before, selected, after] = [
            &self.draft[..self.selection.start],
            &self.draft[self.selection.clone()],
            &self.draft[self.selection.end..],
        ]
        .map(crate::focus::visible);
        let start = before.char_indices().rev().nth(1023).map_or(0, |(i, _)| i);
        [
            before[start..].to_owned(),
            selected.chars().take(1024).collect(),
            after.chars().take(1024).collect(),
        ]
    }
}

fn insert(
    area: &mut TextArea,
    original: &str,
    selection: Range<usize>,
    text: &str,
    language: Option<&str>,
) {
    let current = area.text();
    let range = if current == original {
        selection
    } else {
        let prefix = common_prefix(original, current);
        let suffix = common_suffix(original, current);
        let right_of_change = |prefix, suffix| {
            if selection.end < prefix {
                selection.end
            } else {
                current.len() - (original.len() - selection.end).min(suffix)
            }
        };
        // Repeated text can match on either side of an edit. Insert after
        // both possible anchors so an attachment never gets overwritten.
        let at = right_of_change(
            prefix,
            common_suffix(&original[prefix..], &current[prefix..]),
        )
        .max(right_of_change(
            common_prefix(
                &original[..original.len() - suffix],
                &current[..current.len() - suffix],
            ),
            suffix,
        ));
        at..at
    };
    let lang = language
        .and_then(|lang| lang.split(['-', '_']).next())
        .unwrap_or("auto")
        .to_ascii_lowercase();
    let mut insertion = text.to_owned();
    if range.is_empty()
        && !matches!(
            lang.as_str(),
            "ja" | "zh" | "yue" | "th" | "lo" | "km" | "my"
        )
    {
        let auto = lang == "auto";
        let before = current[..range.start].chars().next_back();
        let after = current[range.end..].chars().next();
        let word = |c: char| {
            if auto {
                c.is_ascii_alphanumeric()
            } else {
                c.is_alphanumeric()
            }
        };
        let transcript_edge = |c: char| {
            static LATIN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(r"^[\p{Latin}\p{N}]$").expect("fixed character class")
            });
            !auto || LATIN.is_match(c.encode_utf8(&mut [0; 4]))
        };
        if before.is_some_and(|c| word(c) || ".!?,:;)]}'\"".contains(c))
            && after.is_none_or(char::is_whitespace)
            && text.chars().next().is_some_and(transcript_edge)
        {
            insertion.insert(0, ' ');
        }
        if after.is_some_and(|c| word(c) || "([{'\"".contains(c))
            && before.is_none_or(char::is_whitespace)
            && text.chars().next_back().is_some_and(transcript_edge)
        {
            insertion.push(' ');
        }
    }
    area.select(range.start, range.end);
    area.insert(&insertion);
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum()
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum()
}

fn normalize(text: &str, settings: &DictationSettings, language: Option<&str>) -> String {
    let mut text = apply_dictionary(text, settings);
    if settings.remove_fillers {
        let local = match language.and_then(|lang| lang.split(['-', '_']).next()) {
            Some("en") => "|um|ah|eh",
            Some("de") => "|äh|ähm",
            Some("fr") => "|euh",
            _ => "",
        };
        let fillers = regex::Regex::new(&format!(
            r"(?i)\b(uh|uhm|umm|uhh|uhhh|ehh|ehm|ahm|hmm|hm|mmm|хм|ммм{local})\b[,.]?[ \t]*"
        ))
        .expect("fixed filler pattern");
        text = fillers
            .replace_all(&text, |m: &regex::Captures<'_>| {
                if settings
                    .correction_cue
                    .split_whitespace()
                    .any(|word| word.eq_ignore_ascii_case(&m[1]))
                {
                    m[0].to_owned()
                } else {
                    String::new()
                }
            })
            .into_owned();
    }
    text.trim().to_owned()
}

fn apply_dictionary(text: &str, settings: &DictationSettings) -> String {
    let mut words: Vec<_> = settings
        .dictionary
        .iter()
        .filter(|(canonical, _)| !canonical.trim().is_empty())
        .flat_map(|(canonical, aliases)| {
            std::iter::once(canonical)
                .chain(aliases)
                .map(move |alias| (alias.trim(), canonical.trim()))
        })
        .filter(|(alias, _)| !alias.is_empty())
        .collect();
    words.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.len()));
    if words.is_empty() {
        return text.to_owned();
    }
    let search = words
        .iter()
        .map(|(alias, _)| format!(r"(\b{{start}}{}\b{{end}})", regex::escape(alias)))
        .collect::<Vec<_>>()
        .join("|");
    let Ok(pattern) = regex::Regex::new(&format!("(?i){search}")) else {
        return text.to_owned();
    };
    pattern
        .replace_all(text, |m: &regex::Captures<'_>| {
            let index = (1..m.len())
                .find(|&i| m.get(i).is_some())
                .expect("one alias matched");
            words[index - 1].1.to_owned()
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictation_replaces_the_original_selection_in_utf8() {
        let mut draft = TextArea::new("Fix 🧪 then $review please");
        insert(
            &mut draft,
            "Fix 🧪 then $review please",
            14..21,
            "use the mobile skill",
            Some("en"),
        );
        assert_eq!(draft.text(), "Fix 🧪 then use the mobile skill please");
        assert_eq!(draft.cursor(), 34);
    }

    #[test]
    fn dictation_keeps_text_added_during_recording() {
        let mut draft = TextArea::new("hello hello world");
        insert(&mut draft, "hello world", 0..5, "new text", Some("en"));
        assert_eq!(draft.text(), "hello hello new text world");
        let mut draft = TextArea::new("🤔");
        insert(&mut draft, "🔔", 0..0, "spoken", Some("en"));
        assert_eq!(draft.text(), "🤔spoken");
    }

    #[test]
    fn dictation_spaces_words_without_splitting_existing_words_or_cjk() {
        for (before, cursor, transcript, expected) in [
            ("fix it", 4, "all", "fix all it"),
            ("hello", 5, "world", "hello world"),
            ("filename", 4, "new", "filenewname"),
            ("你好", 6, "世界", "你好世界"),
        ] {
            let mut draft = TextArea::new(before);
            insert(&mut draft, before, cursor..cursor, transcript, None);
            assert_eq!(draft.text(), expected);
        }
    }

    #[test]
    fn dictation_normalizes_dictionary_and_optionally_removes_fillers() {
        let settings = DictationSettings {
            dictionary: [("Bro CLI".into(), vec!["bro sea el eye".into()])].into(),
            correction_cue: "um actually".into(),
            remove_fillers: true,
            ..DictationSettings::default()
        };
        assert_eq!(
            normalize(
                "Uhh, Bro Sea El Eye, um actually Bro cli.",
                &settings,
                Some("en-US")
            ),
            "Bro CLI, um actually Bro CLI."
        );
        assert_eq!(
            normalize("An umbrella for bro CLIx.", &settings, Some("en")),
            "An umbrella for bro CLIx."
        );
    }
}
