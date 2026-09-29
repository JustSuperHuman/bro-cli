use crate::agents;
use crate::model::TerminalSessionSummary;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn stable_id(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("prompt-{:x}", digest)[..31].to_string()
}

fn selected_prefix(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('❯')
        || trimmed.starts_with('>')
        || trimmed.starts_with('›')
        || trimmed.starts_with('●')
}

fn parse_options(text: &str) -> Vec<Value> {
    let mut options = Vec::new();
    for line in text
        .lines()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let trimmed = line.trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, '❯' | '>' | '›' | '●' | '○')
        });
        let Some((number, rest)) = trimmed.split_once('.') else {
            continue;
        };
        if number.is_empty() || !number.chars().all(|character| character.is_ascii_digit()) {
            continue;
        }
        let label = rest.trim();
        if label.is_empty() {
            continue;
        }
        let index = number.parse::<usize>().unwrap_or(options.len() + 1);
        options.push(json!({
            "id": format!("option-{index}"),
            "label": label,
            "key": number,
            "index": index.saturating_sub(1),
            "focused": selected_prefix(line),
            "selected": false, "disabled": false, "custom": false
        }));
    }
    options
}

pub(crate) fn detect_prompt(text: &str) -> Option<Value> {
    let tail = text
        .lines()
        .rev()
        .take(40)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let options = parse_options(&tail);
    // Conservative compatibility rule: ordinary numbered shell output is not
    // a prompt. A focused caret/marker or an explicit confirmation trailer is
    // required, matching the existing Terminal Assist safety boundary.
    let focused = options
        .iter()
        .any(|option| option.get("focused").and_then(Value::as_bool) == Some(true));
    let trailer = tail.to_ascii_lowercase();
    let actionable = focused
        || trailer.contains("enter to confirm")
        || trailer.contains("esc to cancel")
        || trailer.contains("use arrow keys");
    if options.len() >= 2 && actionable {
        let title = tail
            .lines()
            .rev()
            .find(|line| line.contains('?'))
            .unwrap_or("Choose an option")
            .trim();
        let identity = format!(
            "single-select\n{title}\n{}",
            serde_json::to_string(&options).ok()?
        );
        return Some(json!({
            "id": stable_id(&identity),
            "kind": "single-select",
            "title": title,
            "interaction": "direct-key", "details": [],
            "acceptsNotes": false, "canSubmit": false, "cancelLabel": "Cancel",
            "options": options,
            "submit": "enter",
            "cancel": true
        }));
    }

    let trimmed = tail.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.ends_with('?')
        && (lower.ends_with("(y/n)?") || lower.ends_with("[y/n]?") || lower.contains("continue?"))
    {
        let identity = format!("confirm\n{trimmed}");
        return Some(json!({
            "id": stable_id(&identity),
            "kind": "confirm",
            "title": trimmed.lines().last().unwrap_or("Confirm"),
            "interaction": "direct-key", "details": [],
            "acceptsNotes": false, "canSubmit": false, "cancelLabel": "Cancel",
            "options": [
                { "id": "yes", "label": "Yes", "key": "y", "index": 0, "focused": false, "selected": false, "disabled": false, "custom": false },
                { "id": "no", "label": "No", "key": "n", "index": 1, "focused": false, "selected": false, "disabled": false, "custom": false }
            ],
            "submit": "key",
            "cancel": true
        }));
    }
    None
}

pub fn input_context(
    session: &TerminalSessionSummary,
    text: &str,
    bracketed_paste: bool,
    application_cursor: bool,
) -> Value {
    let osc_agent = (session.agent_source.as_deref() == Some("osc"))
        .then(|| session.agent.as_deref().and_then(agents::Agent::parse))
        .flatten();
    let observation = agents::observe(session, osc_agent, text);
    let (agent, label) = match observation.agent {
        Some(agent) => (agent.id(), agent.label()),
        None => ("terminal", "Terminal"),
    };
    let prompt = observation.agent.and_then(|_| detect_prompt(text));
    json!({
        "sessionId": session.id, "status": session.status, "at": crate::model::iso_now(),
        "pasteSafe": bracketed_paste || observation.agent.is_some(),
        "agent": agent,
        "agentLabel": label,
        "cwd": session.cwd,
        "busy": observation.busy,
        // A confident agent detection implies paste support even when this
        // mirror never saw the mode switch (a host restart starts it empty).
        "bracketedPaste": bracketed_paste || observation.agent.is_some(),
        "applicationCursor": application_cursor,
        "prompt": prompt
    })
}

pub fn prompt_response_bytes(context: &Value, body: &Value) -> Result<String, &'static str> {
    let prompt = context
        .get("prompt")
        .ok_or("No actionable prompt is currently rendered.")?;
    if body.get("promptId").and_then(Value::as_str) != prompt.get("id").and_then(Value::as_str) {
        return Err("The prompt changed before the response was delivered.");
    }
    let action = body
        .get("action")
        .and_then(Value::as_str)
        .ok_or("A response action is required.")?;
    match action {
        "cancel" => Ok("\u{1b}".into()),
        "submit" | "select" => {
            let option_id = body
                .get("optionId")
                .and_then(Value::as_str)
                .ok_or("An option is required.")?;
            let option = prompt
                .get("options")
                .and_then(Value::as_array)
                .and_then(|options| {
                    options
                        .iter()
                        .find(|option| option.get("id").and_then(Value::as_str) == Some(option_id))
                })
                .ok_or("The selected option is no longer available.")?;
            if let Some(key) = option.get("key").and_then(Value::as_str) {
                Ok(format!("{key}\r"))
            } else {
                Ok("\r".into())
            }
        }
        "text" => {
            let text = body.get("text").and_then(Value::as_str).unwrap_or_default();
            Ok(format!("{text}\r"))
        }
        _ => Err("Unsupported prompt response action."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TerminalSessionSummary;

    fn session() -> TerminalSessionSummary {
        let mut session = TerminalSessionSummary::native(
            "id".into(),
            "Claude".into(),
            "pwsh".into(),
            "C:\\work".into(),
            1,
            100,
            30,
        );
        session.agent = Some("claude".into());
        session
    }

    #[test]
    fn detects_focused_numbered_prompt_but_not_plain_shell_list() {
        let active = input_context(
            &session(),
            "Choose\r\n❯ 1. Yes\r\n  2. No\r\nEnter to confirm",
            true,
            false,
        );
        assert_eq!(
            active.pointer("/prompt/kind").and_then(Value::as_str),
            Some("single-select")
        );
        for text in [
            "Choose\r\n❯ 1. Yes\r\n  2. No\r\nEnter to confirm",
            "Continue? (y/n)?",
        ] {
            let context = input_context(&session(), text, true, false);
            let prompt = &context["prompt"];
            assert!(prompt["details"].is_array(), "mobile reads details.length");
            assert_eq!(prompt["interaction"], "direct-key");
            assert_eq!(prompt["cancelLabel"], "Cancel");
            assert_eq!(prompt["canSubmit"], false);
            for option in prompt["options"].as_array().unwrap() {
                assert!(option["selected"].is_boolean());
                assert!(option["disabled"].is_boolean());
                assert!(option["custom"].is_boolean());
            }
            assert_eq!(context["sessionId"], "id");
        }
        let plain = input_context(&session(), "1. src\r\n2. tests", false, false);
        assert!(plain.get("prompt").unwrap().is_null());
    }
}
