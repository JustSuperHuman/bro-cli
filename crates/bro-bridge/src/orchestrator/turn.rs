//! One orchestrator turn: the user's message, then model calls and tool
//! rounds until the model answers, streamed into the shared transcript.

use super::{
    ActiveTurn, MAX_STEPS, MAX_TOOL_RESULT, Orchestrator, OrchestratorConfig,
    STREAM_PUBLISH_INTERVAL, ToolCall, ToolRecord, Usage, context, llm, resolve_key, tools,
};
use crate::model::iso_now;
use crate::state::AppState;
use serde_json::{Value, json};
use std::time::Instant;

/// Appends the user's message and runs one turn in the background. Returns
/// the status the caller should show, or an HTTP status + message.
pub async fn send_message(app: &AppState, text: String) -> Result<Value, (u16, String)> {
    let orchestrator = app.orchestrator.clone();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err((400, "Say something first.".into()));
    }
    let config = orchestrator.config();
    let (key, _) = resolve_key(&config);
    let Some(api_key) = key else {
        return Err((
            409,
            format!(
                "No API key is configured. Set {} in your environment or enter a key in the orchestrator settings.",
                config.key_env
            ),
        ));
    };
    if orchestrator.state.lock().turn.is_some() {
        return Err((
            409,
            "The orchestrator is still working on the previous message; stop it or wait.".into(),
        ));
    }

    let turn_id = uuid::Uuid::new_v4().simple().to_string();
    let mut user = Orchestrator::new_item(&turn_id, "user", "done");
    user.text = text;
    orchestrator.push_item(user);

    let app_for_turn = app.clone();
    let turn_for_task = turn_id.clone();
    let handle = tokio::spawn(async move {
        run_turn(app_for_turn, turn_for_task, config, api_key).await;
    });
    {
        let mut state = orchestrator.state.lock();
        state.turn = Some(ActiveTurn {
            id: turn_id,
            started_at: iso_now(),
            step: "thinking".into(),
            abort: handle.abort_handle(),
        });
        state.error = None;
        state.seq += 1;
        orchestrator.persist_history_locked(&state);
    }
    orchestrator.publish_status();
    Ok(orchestrator.status(None, false))
}

fn usage_delta(usage: &Option<Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    Usage {
        prompt_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        completion_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cost: usage.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        turns: 0,
    }
}

async fn run_turn(app: AppState, turn_id: String, config: OrchestratorConfig, api_key: String) {
    let orchestrator = app.orchestrator.clone();
    orchestrator.ensure_catalog().await;
    let reasoning = orchestrator.reasoning_parameter(&config);
    let tools = tools::definitions();
    let mut turn_usage = Usage::default();

    let mut messages: Vec<Value> = Vec::new();
    messages.push(json!({ "role": "system", "content": context::system_prompt(&app, &config) }));
    let history = context::history_messages(&orchestrator.state.lock().items);
    messages.extend(history);

    let mut error: Option<String> = None;
    for step in 0..MAX_STEPS {
        orchestrator.set_step("thinking");
        let assistant =
            orchestrator.push_item(Orchestrator::new_item(&turn_id, "assistant", "streaming"));
        let assistant_id = assistant.id.clone();
        let mut last_publish = Instant::now();
        let request = llm::ChatRequest {
            base_url: &config.base_url,
            api_key: &api_key,
            model: &config.model,
            openrouter: config.is_openrouter(),
            reasoning: reasoning.clone(),
            messages: &messages,
            tools: &tools,
        };
        let completion = llm::stream_chat(&orchestrator.http, request, |text, reasoning_text| {
            if last_publish.elapsed() < STREAM_PUBLISH_INTERVAL {
                return;
            }
            last_publish = Instant::now();
            let text = text.to_string();
            let reasoning_text = (!reasoning_text.is_empty()).then(|| reasoning_text.to_string());
            orchestrator.update_item(&assistant_id, false, |item| {
                item.text = text;
                item.reasoning = reasoning_text;
            });
        })
        .await;

        let completion = match completion {
            Ok(completion) => completion,
            Err(message) => {
                orchestrator.update_item(&assistant_id, false, |item| {
                    item.status = if item.text.is_empty() {
                        "error".into()
                    } else {
                        "done".into()
                    };
                    item.finished_at = Some(iso_now());
                });
                let mut failure = Orchestrator::new_item(&turn_id, "error", "error");
                failure.text = message.clone();
                failure.finished_at = Some(iso_now());
                orchestrator.push_item(failure);
                error = Some(message);
                break;
            }
        };

        let delta = usage_delta(&completion.usage);
        turn_usage.prompt_tokens += delta.prompt_tokens;
        turn_usage.completion_tokens += delta.completion_tokens;
        turn_usage.cost += delta.cost;

        let tool_calls: Vec<ToolCall> = completion
            .tool_calls
            .iter()
            .map(|call| ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: if call.arguments.trim().is_empty() {
                    "{}".into()
                } else {
                    call.arguments.clone()
                },
            })
            .collect();
        let final_text = completion.text.clone();
        let final_reasoning =
            (!completion.reasoning.is_empty()).then(|| completion.reasoning.clone());
        let model = completion.model.clone();
        let calls_for_item = tool_calls.clone();
        orchestrator.update_item(&assistant_id, true, |item| {
            item.text = final_text;
            item.reasoning = final_reasoning;
            item.tool_calls = calls_for_item;
            item.model = model;
            item.status = "done".into();
            item.finished_at = Some(iso_now());
        });

        let mut assistant_message = json!({
            "role": "assistant",
            "content": if completion.text.is_empty() { Value::Null } else { Value::String(completion.text.clone()) }
        });
        if !tool_calls.is_empty() {
            assistant_message["tool_calls"] = json!(
                tool_calls
                    .iter()
                    .map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments }
                    }))
                    .collect::<Vec<_>>()
            );
        }
        messages.push(assistant_message);

        if tool_calls.is_empty() {
            break;
        }

        for call in &tool_calls {
            let arguments: Value =
                serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
            let summary = tools::summarize(&app, &call.name, &arguments);
            orchestrator.set_step(&call.name);
            let mut record = Orchestrator::new_item(&turn_id, "tool", "streaming");
            record.tool = Some(ToolRecord {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: arguments.clone(),
                summary: summary.clone(),
                result: String::new(),
                ok: true,
            });
            let record = orchestrator.push_item(record);
            let outcome = if serde_json::from_str::<Value>(&call.arguments).is_err() {
                Err(format!(
                    "The arguments for {} were not valid JSON: {}",
                    call.name, call.arguments
                ))
            } else {
                tools::execute(&app, &call.name, &arguments).await
            };
            let (ok, mut result) = match outcome {
                Ok(result) => (true, result),
                Err(message) => (false, message),
            };
            if result.chars().count() > MAX_TOOL_RESULT {
                let kept: String = result.chars().take(MAX_TOOL_RESULT).collect();
                result = format!("{kept}\n…[result trimmed]");
            }
            let result_for_item = result.clone();
            orchestrator.update_item(&record.id, true, |item| {
                if let Some(tool) = item.tool.as_mut() {
                    tool.result = result_for_item;
                    tool.ok = ok;
                }
                item.status = if ok { "done".into() } else { "error".into() };
                item.finished_at = Some(iso_now());
            });
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": if ok { result } else { format!("Error: {result}") }
            }));
        }

        if step + 1 == MAX_STEPS {
            messages.push(json!({
                "role": "user",
                "content": "[You have used the tool budget for this turn. Answer the user now with what you know.]"
            }));
            let assistant =
                orchestrator.push_item(Orchestrator::new_item(&turn_id, "assistant", "streaming"));
            let request = llm::ChatRequest {
                base_url: &config.base_url,
                api_key: &api_key,
                model: &config.model,
                openrouter: config.is_openrouter(),
                reasoning: reasoning.clone(),
                messages: &messages,
                tools: &[],
            };
            let closing = llm::stream_chat(&orchestrator.http, request, |_, _| {}).await;
            let (text, status) = match closing {
                Ok(completion) => (completion.text, "done"),
                Err(message) => (message, "error"),
            };
            orchestrator.update_item(&assistant.id, true, |item| {
                item.text = text;
                item.status = status.into();
                item.finished_at = Some(iso_now());
            });
        }
    }

    orchestrator.finish_turn(&turn_id, error, turn_usage);
}
