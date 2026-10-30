//! Codex's request-user-input overlay, including freeform questions and its notes field.
//! Digits commit an option immediately: appending Enter would answer the next question too.

use serde_json::{Value, json};

fn option(line: &str) -> Option<(usize, &str, bool)> {
    let trimmed = line.trim();
    let focused = trimmed.starts_with('›') || trimmed.starts_with('❯') || trimmed.starts_with('>');
    let trimmed = trimmed.trim_start_matches(['›', '❯', '>']).trim_start();
    let (number, label) = trimmed
        .split_once('.')
        .or_else(|| trimmed.split_once(')'))?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((
        number.parse::<usize>().ok()?.checked_sub(1)?,
        label.trim(),
        focused,
    ))
}

pub(super) fn detect(text: &str) -> Option<Value> {
    blocking(text)
        .or_else(|| asynchronous(text))
        .or_else(|| collapsed(text))
}

fn collapsed(text: &str) -> Option<Value> {
    let lines: Vec<_> = text.lines().collect();
    let (start, count) = lines.iter().enumerate().rev().find_map(|(i, line)| {
        let mut words = line.trim().strip_prefix("? ")?.split_whitespace();
        let count = words.next()?.parse::<usize>().ok()?;
        (count > 0 && matches!(words.next(), Some("question" | "questions"))).then_some((i, count))
    })?;
    let hint = lines.get(start + 1)?.trim().to_ascii_lowercase();
    if !hint.ends_with("to answer") {
        return None;
    }
    let key = if hint.contains("⌥↑") || hint.contains("alt+↑") || hint.contains("alt+up") {
        "\x1b[1;3A"
    } else if hint.contains("⇧←") || hint.contains("shift+←") || hint.contains("shift+left") {
        "\x1b[1;2D"
    } else {
        return None;
    };
    Some(
        json!({ "id": super::stable_id(&format!("codex-questions-open-{count}-{key}")), "protocol": "codex-question-open", "openKey": key,
        "kind":"single-select", "interaction":"direct-key", "title": if count == 1 { "Codex has a question".to_string() } else { format!("Codex has {count} questions") },
        "details":[], "options":[{"id":"open-questions", "label":"Answer questions", "focused":true,"selected":false,"disabled":false,"custom":false}],
        "acceptsNotes":false,"canSubmit":false,"cancelLabel":"Cancel","canCancel":false,
        "hint":"Open Codex’s questions to answer here." }),
    )
}

fn asynchronous(text: &str) -> Option<Value> {
    let lines: Vec<_> = text.lines().collect();
    let footer = lines
        .iter()
        .rposition(|line| line.to_ascii_lowercase().contains("enter submit"))?;
    let hints = lines[footer..].join(" ").to_ascii_lowercase();
    if !hints.contains("skip")
        || !(hints.contains("main prompt") || hints.contains("prev question"))
    {
        return None;
    }
    let progress = lines[..footer]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, line)| {
            let (current, total) = line.trim().split_once(" of ")?;
            let (current, total) = (current.parse::<usize>().ok()?, total.parse::<usize>().ok()?);
            (current > 0 && current <= total && total <= 100).then_some((i, current, total))
        });
    let lower = progress.map_or(0, |p| p.0 + 1);
    let first_option = (lower..footer)
        .rev()
        .find(|&i| option(lines[i]).is_some_and(|o| o.0 == 0));
    let end = first_option
        .or_else(|| {
            lines[..footer]
                .iter()
                .rposition(|line| line.trim() == "Type your answer")
        })
        .or_else(|| {
            let input_end = (lower..footer)
                .rev()
                .find(|&i| !lines[i].trim().is_empty())?;
            (lower..input_end)
                .rev()
                .find(|&i| lines[i].trim().is_empty())
                .map(|i| i + 1)
        })
        .unwrap_or(footer);
    let question_end = (0..end).rev().find(|&i| !lines[i].trim().is_empty())?;
    let start = progress.map(|p| p.0 + 1).unwrap_or_else(|| {
        (0..question_end)
            .rev()
            .find(|&i| lines[i].trim().is_empty())
            .map_or(0, |i| i + 1)
    });
    if start > question_end {
        return None;
    }
    let question = lines[start..=question_end]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let mut options: Vec<Value> = Vec::new();
    for line in &lines[question_end + 1..footer] {
        if let Some((index, label, focused)) = option(line) {
            options.push(json!({"id":format!("option-{}",index+1),"index":index,"label":label,"focused":focused,"selected":false,"disabled":false,"custom":false}));
        } else if !line.trim().is_empty() && !options.is_empty() {
            let last = options.last_mut()?;
            last["label"] = json!(format!("{} {}", last["label"].as_str()?, line.trim()));
        }
    }
    // The async editor always adds a final Other row, which becomes an inline editor when selected.
    let other_visible = options.last().is_some_and(|o| {
        let label = o["label"].as_str().unwrap_or_default();
        label == "Other" || label == "Other (write an answer)" || label.is_empty()
    });
    let other_focused = other_visible && options.last().is_some_and(|o| o["focused"] == true);
    if let Some(other) = options.last_mut().filter(|_| other_visible) {
        other["label"] = json!("Other (write an answer)");
        other["custom"] = json!(true);
        other["disabled"] = json!(other_focused);
    }
    let (current, total) = progress.map(|p| (p.1, p.2)).unwrap_or((1, 1));
    let identity: Vec<_> = options.iter().map(|o| (&o["id"], &o["label"])).collect();
    let id = super::stable_id(&format!(
        "codex-async\n{current}/{total}\n{question}\n{other_focused}\n{}",
        serde_json::to_string(&identity).ok()?
    ));
    Some(
        json!({"id":id,"protocol":"codex-question-async","kind":if options.is_empty(){"freeform"}else{"single-select"},
        "interaction":"cursor","question":question,"details":[],"options":options,"progress":{"current":current,"total":total},
        "textInput":{"kind":"answer","placeholder":"Your answer","optional":false},"acceptsNotes":false,"canSubmit":true,
        "submitLabel":"Submit answer","cancelLabel":"Cancel", "hint":"Choose an option or write your own answer."}),
    )
}

fn blocking(text: &str) -> Option<Value> {
    let lines: Vec<_> = text.lines().collect();
    let (start, current, total) = lines.iter().enumerate().rev().find_map(|(i, line)| {
        let progress = line
            .trim()
            .strip_prefix("Question ")?
            .split_whitespace()
            .next()?;
        let (current, total) = progress.split_once('/')?;
        let (current, total) = (current.parse::<usize>().ok()?, total.parse::<usize>().ok()?);
        (current > 0 && current <= total && total <= 100).then_some((i, current, total))
    })?;
    let body = &lines[start + 1..];
    let footer = body.iter().position(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("to submit answer")
            || line.contains("to submit all")
            || line.contains("to add notes")
            || line.contains("to clear notes")
    })?;
    let lower_footer = body[footer..].join(" ").to_ascii_lowercase();
    // Wait for a complete overlay, not a header left in the transcript or a partial repaint.
    if !lower_footer.contains("enter") || !lower_footer.contains("to submit") {
        return None;
    }
    let notes = lower_footer.contains("to clear notes");
    let mut options: Vec<Value> = Vec::new();
    let mut question = Vec::new();
    let mut in_answer = false;
    for line in &body[..footer] {
        if let Some((index, label, focused)) = option(line) {
            let custom = matches!(
                label.to_ascii_lowercase().as_str(),
                "none of the above" | "other" | "other (type your own answer)"
            );
            options.push(json!({ "id": format!("option-{}", index + 1), "index": index,
                "label": label, "focused": focused, "selected": false, "disabled": false, "custom": custom }));
        } else {
            let trimmed = line.trim();
            if trimmed.starts_with('›') || trimmed.starts_with('>') || trimmed.starts_with('❯')
            {
                in_answer = true;
            }
            if !in_answer && !trimmed.is_empty() {
                if let Some(last) = options.last_mut() {
                    let description = last["description"]
                        .as_str()
                        .map(|s| format!("{s} {trimmed}"))
                        .unwrap_or_else(|| trimmed.to_string());
                    last["description"] = json!(description);
                } else {
                    question.push(trimmed);
                }
            }
        }
    }
    let question = question.join(" ");
    if question.is_empty() {
        return None;
    }
    let freeform = options.is_empty();
    let identity: Vec<_> = options
        .iter()
        .map(|o| (&o["id"], &o["label"], &o["description"]))
        .collect();
    let id = super::stable_id(&format!(
        "codex-question\n{current}/{total}\n{question}\n{notes}\n{}",
        serde_json::to_string(&identity).ok()?
    ));
    Some(json!({
        "id": id, "protocol": "codex-question", "kind": if freeform { "freeform" } else { "single-select" },
        "interaction": "cursor", "question": question, "details": [], "options": options,
        "progress": { "current": current, "total": total }, "notesVisible": notes,
        "textInput": if freeform || notes { json!({ "kind": if freeform { "answer" } else { "notes" },
            "placeholder": if freeform { "Your answer" } else { "Add notes" }, "optional": false }) } else { Value::Null },
        "acceptsNotes": !freeform && !notes, "canSubmit": freeform || notes,
        "submitLabel": if current < total { "Next question" } else { "Submit answer" }, "cancelLabel": "Interrupt"
    }))
}

pub(super) fn response(prompt: &Value, body: &Value) -> Result<String, &'static str> {
    if prompt["protocol"] == "codex-question-open" {
        return if body["action"] == "select" && body["optionId"] == "open-questions" {
            Ok(prompt["openKey"]
                .as_str()
                .ok_or("The question shortcut is unavailable.")?
                .into())
        } else {
            Err("Open the Codex question before answering.")
        };
    }
    let notes = prompt["notesVisible"] == true;
    match body["action"].as_str() {
        Some("cancel") => Ok(if notes { "\x1b\x1b" } else { "\x1b" }.into()),
        Some("open-notes") if prompt["acceptsNotes"] == true => Ok("\t".into()),
        Some("select") => {
            let options = prompt["options"]
                .as_array()
                .ok_or("No options are available.")?;
            let selected = options
                .iter()
                .find(|o| o["id"] == body["optionId"] && o["disabled"] != true)
                .ok_or("The selected option is no longer available.")?;
            let focused = options
                .iter()
                .find(|o| o["focused"] == true)
                .ok_or("The option focus is not visible. Refresh the question.")?;
            let target = selected["index"]
                .as_u64()
                .ok_or("The option index is unavailable.")? as i64;
            let current = focused["index"]
                .as_u64()
                .ok_or("The option focus is unavailable.")? as i64;
            let delta = target - current;
            if delta.unsigned_abs() > 100 {
                return Err("The selected option is out of range.");
            }
            // Tab leaves the notes editor; arrows then select without committing until the single Enter.
            let submit =
                !(prompt["protocol"] == "codex-question-async" && selected["custom"] == true);
            Ok(format!(
                "{}{}{}",
                if notes { "\t" } else { "" },
                if delta < 0 { "\x1b[A" } else { "\x1b[B" }.repeat(delta.unsigned_abs() as usize),
                if submit { "\r" } else { "" }
            ))
        }
        Some("text") if !prompt["textInput"].is_null() => {
            let text = body["text"]
                .as_str()
                .filter(|t| !t.trim().is_empty())
                .ok_or("Enter an answer before submitting.")?;
            if text.len() > 32_768
                || text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
            {
                return Err("The answer is too long or contains terminal control characters.");
            }
            // Paste as one answer. The handler sends Enter separately after Codex consumes the paste.
            Ok(format!(
                "{}\r",
                crate::commands::compose_payload(text, true)
            ))
        }
        _ => Err("Unsupported response for this Codex question."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices(current: usize, marker: &str, notes: bool) -> String {
        format!(
            "Question {current}/2 (2 unanswered)\nWhich environment should I use?\n{marker} 1. Staging (Recommended)\n   Test before release\n  2. Production\n   Live environment\n  3. None of the above\n   Optionally, add details in notes (tab)\n{}\n{} | enter to submit answer | esc to interrupt",
            if notes { "› Add notes" } else { "" },
            if notes {
                "tab or esc to clear notes"
            } else {
                "tab to add notes"
            }
        )
    }

    fn respond(prompt: &Value, body: Value) -> String {
        let context = json!({ "prompt": prompt });
        let mut body = body;
        body["promptId"] = prompt["id"].clone();
        super::super::prompt_response_bytes(&context, &body).unwrap()
    }

    #[test]
    fn choices_keep_the_question_descriptions_and_progress() {
        let prompt = detect(&choices(1, "›", false)).unwrap();
        assert_eq!(prompt["question"], "Which environment should I use?");
        assert_eq!(prompt["progress"], json!({"current": 1, "total": 2}));
        assert_eq!(prompt["options"][0]["description"], "Test before release");
        assert_eq!(prompt["options"][2]["custom"], true);
        assert_eq!(
            respond(&prompt, json!({"action":"select", "optionId":"option-2"})),
            "\x1b[B\r"
        );
        assert_eq!(respond(&prompt, json!({"action":"open-notes"})), "\t");
    }

    #[test]
    fn an_answer_only_commits_once_and_custom_answers_open_notes() {
        let prompt = detect(&choices(1, "›", false)).unwrap();
        assert_eq!(
            respond(&prompt, json!({"action":"select", "optionId":"option-1"})),
            "\r"
        );
        assert_eq!(
            respond(&prompt, json!({"action":"select", "optionId":"option-3"})),
            "\x1b[B\x1b[B\r"
        );
        let notes = detect(&choices(1, "›", true)).unwrap();
        assert_ne!(
            notes["id"], prompt["id"],
            "notes are a separate actionable step"
        );
        assert_eq!(notes["textInput"]["kind"], "notes");
        assert_eq!(respond(&notes, json!({"action":"cancel"})), "\x1b\x1b");
    }

    #[test]
    fn freeform_answers_are_pasted_and_later_questions_reject_old_cards() {
        let first = detect("Question 1/2\nWhere should the files go?\n› Type your answer (optional)\nenter to submit answer | esc to interrupt").unwrap();
        assert_eq!(first["kind"], "freeform");
        assert_eq!(
            respond(&first, json!({"action":"text", "text":"docs/\n日本語"})),
            "\x1b[200~docs/\n日本語\x1b[201~\r"
        );
        let next = detect(&choices(2, "›", false)).unwrap();
        assert!(
            super::super::prompt_response_bytes(
                &json!({"prompt":next}),
                &json!({"promptId": first["id"], "action":"cancel"})
            )
            .unwrap_err()
            .contains("changed")
        );
        assert!(
            super::super::prompt_response_bytes(
                &json!({"prompt":first}),
                &json!({"promptId":first["id"],"action":"text","text":"bad\x1b[201~\r"})
            )
            .is_err()
        );
    }

    #[test]
    fn ordinary_lists_and_incomplete_questions_do_not_become_question_cards() {
        for text in [
            "1. Staging\n2. Production",
            "Question 1/2\nWhich environment?\n› 1. Staging\n2. Production",
            "Question 0/0\nWhich?\nenter to submit answer",
        ] {
            assert!(detect(text).is_none(), "{text}");
        }
    }

    #[test]
    fn async_questions_open_from_focus_without_typing_into_the_main_prompt() {
        for (hint, key) in [("⇧←", "\x1b[1;2D"), ("shift+←", "\x1b[1;2D"), ("⌥↑", "\x1b[1;3A"), ("alt+↑", "\x1b[1;3A")] {
            let prompt = detect(&format!(
                "Agent is working\n? 2 questions · 15s\n  {hint} to answer"
            ))
            .unwrap();
            assert_eq!(prompt["protocol"], "codex-question-open");
            assert_eq!(prompt["canCancel"], false);
            assert_eq!(
                respond(
                    &prompt,
                    json!({"action":"select","optionId":"open-questions"})
                ),
                key
            );
            assert_eq!(
                prompt["id"],
                detect(&format!("? 2 questions · 3s\n{hint} to answer")).unwrap()["id"],
                "countdown does not reset a card"
            );
        }
    }

    #[test]
    fn async_choices_and_other_use_the_current_question_editor() {
        let screen = "Old numbered list\n1. Ignore this\n2. Ignore that\n\n1 of 2\n\nWhich environment?\n\n› 1. Staging\n  2. Production\n  3. Other\n\nenter submit   ⌃] skip   ⇧→ main prompt   ⇧← next question";
        let prompt = detect(screen).unwrap();
        assert_eq!(prompt["protocol"], "codex-question-async");
        assert_eq!(prompt["question"], "Which environment?");
        assert_eq!(prompt["progress"], json!({"current":1,"total":2}));
        assert_eq!(
            respond(&prompt, json!({"action":"select","optionId":"option-2"})),
            "\x1b[B\r"
        );
        assert_eq!(
            respond(&prompt, json!({"action":"select","optionId":"option-3"})),
            "\x1b[B\x1b[B"
        );
        let selected = screen
            .replace("› 1.", "  1.")
            .replace("  3. Other", "› 3. Other");
        let other = detect(&selected).unwrap();
        assert_ne!(prompt["id"], other["id"]);
        assert_eq!(other["options"][2]["disabled"], true);
        assert_eq!(
            respond(&other, json!({"action":"text","text":"Use local"})),
            "\x1b[200~Use local\x1b[201~\r"
        );
    }

    #[test]
    fn async_freeform_questions_keep_wrapped_text_and_ignore_existing_drafts() {
        let prefix = "Old output\n\n1 of 2\n\nRun this command:\n\nprintf '診断'\n\nThen review diagnostics?\n\n";
        for draft in ["Type your answer", "my existing draft"] {
            let prompt = detect(&format!(
                "{prefix}{draft}\n\nenter submit   ⌃] skip   ⇧→ main prompt   ⇧← next question"
            ))
            .unwrap();
            assert_eq!(prompt["kind"], "freeform");
            assert_eq!(
                prompt["question"],
                "Run this command:\n\nprintf '診断'\n\nThen review diagnostics?"
            );
        }
    }
}
