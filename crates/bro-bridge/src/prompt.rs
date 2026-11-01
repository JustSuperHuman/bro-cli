use crate::agents;
use crate::model::TerminalSessionSummary;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod codex;

fn stable_id(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("prompt-{:x}", digest)[..31].to_string()
}

pub(crate) fn detect_prompt(text: &str) -> Option<Value> {
    if let Some(prompt) = codex::detect(text) {
        return Some(prompt);
    }
    let rows: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(choice) = bro_core::terminal_prompt::choice(&rows) {
        let options: Vec<Value> = choice.options.iter().enumerate().map(|(index, o)| json!({
            "id": format!("option-{}", o.key), "label": o.label, "description": o.detail,
            "key": o.key, "index": index, "focused": o.focused,
            "selected": false, "disabled": false, "custom": false
        })).collect();
        let identity: Vec<_> = choice.options.iter().map(|o| (&o.key, &o.label, &o.detail)).collect();
        let identity = format!("single-select\n{}\n{}\n{}", choice.question,
            serde_json::to_string(&choice.context).ok()?, serde_json::to_string(&identity).ok()?);
        return Some(json!({
            "id": stable_id(&identity), "kind": "single-select",
            "title": if choice.question.is_empty() { "Choose an option" } else { &choice.question },
            "interaction": "cursor", "details": choice.context,
            "acceptsNotes": false, "canSubmit": false, "cancelLabel": "Cancel",
            "options": options, "submit": "enter", "cancel": true
        }));
    }

    let trimmed = text.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.ends_with('?')
        && (lower.ends_with("(y/n)?") || lower.ends_with("[y/n]?"))
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
    let limit = observation.agent.and_then(|_| bro_core::sessions::limits::UsageLimit::from_screen(text))
        .or_else(|| session.attention.as_ref().and_then(|attention| attention.limit.clone()));
    json!({
        "sessionId": session.id, "status": session.status, "at": crate::model::iso_now(),
        "pasteSafe": bracketed_paste || observation.agent.is_some(),
        "agent": agent,
        "agentLabel": label,
        "cwd": session.cwd,
        "busy": observation.busy && limit.is_none() && !bro_core::terminal_prompt::waiting(&text.lines().map(str::to_string).collect::<Vec<_>>()),
        "limit": limit,
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
        .filter(|prompt| prompt.get("id").and_then(Value::as_str).is_some())
        .ok_or("No actionable prompt is currently rendered.")?;
    if body.get("promptId").and_then(Value::as_str) != prompt.get("id").and_then(Value::as_str) {
        return Err("The prompt changed before the response was delivered.");
    }
    if prompt.get("protocol").and_then(Value::as_str).is_some_and(|p| p.starts_with("codex-question")) {
        let response = codex::response(prompt, body)?;
        return Ok(if context["applicationCursor"] == true && body["action"] == "select" {
            response.replace("\x1b[A", "\x1bOA").replace("\x1b[B", "\x1bOB")
        } else { response });
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
            if prompt["interaction"] == "cursor" {
                let options = prompt["options"].as_array().ok_or("No options are available.")?;
                let current = options.iter().find(|o| o["focused"] == true)
                    .and_then(|o| o["index"].as_u64()).ok_or("The option focus is not visible.")?;
                let target = option["index"].as_u64().ok_or("The option index is unavailable.")?;
                let delta = target as i64 - current as i64;
                if delta.unsigned_abs() > 99 { return Err("The selected option is out of range."); }
                let application = context["applicationCursor"] == true;
                let arrow = match (delta < 0, application) {
                    (true, true) => "\x1bOA", (false, true) => "\x1bOB",
                    (true, false) => "\x1b[A", (false, false) => "\x1b[B",
                };
                Ok(format!("{}\r", arrow.repeat(delta.unsigned_abs() as usize)))
            } else if let Some(key) = option.get("key").and_then(Value::as_str) {
                Ok(key.into())
            } else {
                Err("The option has no response key.")
            }
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
            assert_eq!(prompt["interaction"], if prompt["kind"] == "confirm" { "direct-key" } else { "cursor" });
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

    #[test]
    fn desktop_and_mobile_share_plan_text_details_and_cursor_independent_identity() {
        let screen = include_str!("../../bro-tui/tests/fixtures/claude-plan-approval.txt");
        let desktop = bro_core::terminal_prompt::choice(&screen.lines().map(str::to_string).collect::<Vec<_>>()).unwrap();
        let phone = detect_prompt(screen).unwrap();
        assert_eq!(phone["title"], desktop.question);
        assert_eq!(phone["options"][0]["description"], desktop.options[0].detail.as_deref().unwrap());
        let moved = screen.replace("❯ 1.", "  1.").replace("  2.", "❯ 2.");
        let moved = detect_prompt(&moved).unwrap();
        assert_eq!(phone["id"], moved["id"], "moving the cursor does not replace the request");
        let context = json!({"prompt":moved,"applicationCursor":true});
        let body = json!({"promptId":phone["id"],"action":"select","optionId":"option-1"});
        assert_eq!(prompt_response_bytes(&context, &body).unwrap(), "\x1bOA\r");
        assert!(prompt_response_bytes(&context, &json!({"promptId":phone["id"],"action":"text","text":"unrequested input"})).is_err());
    }

    #[test]
    fn mobile_does_not_turn_stale_menus_or_checkbox_forms_into_single_select_cards() {
        for screen in [
            "Choose?\n❯ 1. One\n  2. Two\nEnter to confirm\n❯\n? for shortcuts",
            "Choose?\n❯ 1. [ ] One\n  2. [x] Two\nSpace to select · Enter to confirm · Esc to cancel",
            "The guide says continue?",
            "Question 1/2\nChoose?\n› 1. One\n  2. Two\nenter to submit answer | esc to interrupt\n›\nGPT-6 · ? for shortcuts",
        ] { assert!(detect_prompt(screen).is_none(), "{screen}"); }
    }
}
