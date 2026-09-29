//! Dialect translation. Anthropic Messages is the hub: every cross-dialect request is
//! converted to Anthropic and then to the upstream dialect, and every upstream
//! response/stream is converted to Anthropic and then to the inbound dialect.
//!
//! Round-tripping opaque reasoning through a foreign client uses these markers:
//! - thinking.signature `bro.chat` — thinking that came from Chat `reasoning_content`
//! - thinking.signature `bro.rs:{enc}` — Responses reasoning `encrypted_content`
//! - reasoning.encrypted_content `bro.at:{sig}` — an Anthropic thinking signature
//! - reasoning.encrypted_content `bro.rd:{data}` — Anthropic redacted_thinking data

pub mod a2chat;
pub mod a2responses;
pub mod chat2a;
pub mod reasoning_cache;
pub mod responses2a;
pub mod schema;
pub mod stream_a2chat;
pub mod stream_a2responses;
pub mod stream_chat2a;
pub mod stream_responses2a;
#[cfg(test)]
mod stream_tests;

use crate::anthropic::{MessagesRequest, Usage};
use crate::openai::{ChatUsage, ResponsesUsage};
use serde_json::{Value, json};

pub const SIG_CHAT: &str = "bro.chat";
pub const SIG_RESPONSES: &str = "bro.rs:";
pub const ENC_ANTHROPIC: &str = "bro.at:";
pub const ENC_REDACTED: &str = "bro.rd:";

pub const EFFORT_ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// Everything a request translator needs to know about the upstream.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// Model to send upstream (already mapped, suffix stripped)
    pub model: String,
    /// Forced effort from a `model:effort` suffix
    pub effort_override: Option<String>,
    /// ChatGPT Codex backend quirks (instructions required, no max_output_tokens…)
    pub chatgpt: bool,
    /// Strip JSON-schema keywords strict upstreams reject (Gemini)
    pub strict_schema: bool,
    /// Send `max_completion_tokens` instead of `max_tokens` (api.openai.com)
    pub max_completion_tokens: bool,
    /// Efforts the model accepts, if known
    pub supported_efforts: Vec<String>,
    /// Stable key for upstream prompt caching / stickiness
    pub cache_key: Option<String>,
    /// Upstream is a Claude subscription login (needs the Claude Code system prefix)
    pub claude_oauth: bool,
}

/// Map an Anthropic thinking budget to an effort level (v1 thresholds).
pub fn budget_to_effort(budget: u64) -> &'static str {
    if budget < 4096 {
        "low"
    } else if budget < 16384 {
        "medium"
    } else if budget < 32768 {
        "high"
    } else {
        "xhigh"
    }
}

/// Budget for an OpenAI effort level; None = thinking off.
pub fn effort_to_budget(effort: &str) -> Option<u64> {
    match effort {
        "none" | "minimal" => None,
        "low" => Some(2048),
        "medium" => Some(8192),
        "high" => Some(24576),
        "xhigh" | "max" => Some(32768),
        _ => Some(8192),
    }
}

/// Desired effort from an Anthropic request (thinking budget / adaptive effort).
pub fn anthropic_effort(req: &MessagesRequest) -> Option<String> {
    let t = req.thinking.as_ref()?;
    match t.kind.as_str() {
        "enabled" => Some(budget_to_effort(t.budget_tokens.unwrap_or(8192)).to_string()),
        "adaptive" => {
            let e = req
                .extra
                .get("output_config")
                .and_then(|o| o.get("effort"))
                .and_then(Value::as_str)
                .unwrap_or("high");
            Some(if e == "max" {
                "xhigh".into()
            } else {
                e.to_string()
            })
        }
        _ => None,
    }
}

/// Clamp to the nearest supported effort (v1 semantics).
pub fn clamp_effort(effort: &str, supported: &[String]) -> String {
    if supported.is_empty() || supported.iter().any(|s| s == effort) {
        return effort.to_string();
    }
    let rank = |e: &str| EFFORT_ORDER.iter().position(|x| *x == e).unwrap_or(2) as i32;
    let want = rank(effort);
    supported
        .iter()
        .min_by_key(|s| (rank(s) - want).abs())
        .cloned()
        .unwrap_or_else(|| effort.to_string())
}

/// Heuristic: does this OpenAI-side model accept reasoning parameters?
pub fn is_reasoning_model(model: &str) -> bool {
    let m = model.to_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m);
    m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.starts_with("gpt-5")
        || m.contains("gpt-oss")
        || m.contains("codex")
        || m.contains("grok-3-mini")
        || m.contains("reasoner")
        || m.contains("qwen3")
        || m.contains("deepseek-r1")
}

// ------------------------------------------------------------ stop reasons

pub fn chat_finish_to_stop(finish: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        return "tool_use";
    }
    match finish {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("content_filter") => "refusal",
        _ => "end_turn",
    }
}

pub fn stop_to_chat_finish(stop: Option<&str>) -> &'static str {
    match stop {
        Some("max_tokens") | Some("model_context_window_exceeded") => "length",
        Some("tool_use") => "tool_calls",
        Some("refusal") => "content_filter",
        _ => "stop",
    }
}

// ------------------------------------------------------------ usage

pub fn usage_from_chat(u: &ChatUsage) -> Usage {
    let cached = u.cached_tokens();
    Usage {
        input_tokens: u.prompt_tokens.saturating_sub(cached),
        output_tokens: u.completion_tokens,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: (cached > 0).then_some(cached),
    }
}

pub fn usage_from_responses(u: &ResponsesUsage) -> Usage {
    let cached = u.cached_tokens();
    Usage {
        input_tokens: u.input_tokens.saturating_sub(cached),
        output_tokens: u.output_tokens,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: (cached > 0).then_some(cached),
    }
}

/// Total prompt tokens (OpenAI counts cache reads/writes inside prompt tokens).
pub fn total_input(u: &Usage) -> u64 {
    u.input_tokens
        + u.cache_read_input_tokens.unwrap_or(0)
        + u.cache_creation_input_tokens.unwrap_or(0)
}

pub fn chat_usage_json(u: &Usage) -> Value {
    let prompt = total_input(u);
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": u.output_tokens,
        "total_tokens": prompt + u.output_tokens,
        "prompt_tokens_details": { "cached_tokens": u.cache_read_input_tokens.unwrap_or(0) },
        "completion_tokens_details": { "reasoning_tokens": 0 }
    })
}

pub fn responses_usage_json(u: &Usage) -> Value {
    let input = total_input(u);
    json!({
        "input_tokens": input,
        "input_tokens_details": { "cached_tokens": u.cache_read_input_tokens.unwrap_or(0) },
        "output_tokens": u.output_tokens,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": input + u.output_tokens
    })
}

// ------------------------------------------------------------ Anthropic request shaping

/// Default max_tokens when an OpenAI client didn't send one.
pub fn default_max_tokens(model: &str) -> u64 {
    if model.contains("claude-3-") || model.contains("claude-3.") {
        8192
    } else {
        32000
    }
}

/// Configure thinking from an OpenAI effort on an Anthropic request, keeping
/// max_tokens > budget and dropping sampling params thinking forbids.
pub fn apply_effort_to_anthropic(
    req: &mut MessagesRequest,
    effort: Option<&str>,
    explicit_max: bool,
) {
    let Some(budget) = effort.and_then(effort_to_budget) else {
        return;
    };
    let mut max = req
        .max_tokens
        .unwrap_or_else(|| default_max_tokens(&req.model));
    let mut budget = budget;
    if budget + 1024 > max {
        if explicit_max {
            if max < 2048 {
                return; // too small for thinking
            }
            budget = max - 1024;
        } else {
            max = budget + 8192;
        }
    }
    req.max_tokens = Some(max);
    req.thinking = Some(crate::anthropic::ThinkingConfig {
        kind: "enabled".into(),
        budget_tokens: Some(budget),
    });
    req.temperature = None;
    req.top_k = None;
    if req.top_p.is_some_and(|p| p < 0.95) {
        req.top_p = None;
    }
}

/// With thinking on, Anthropic requires the last assistant turn of an ongoing tool
/// loop to start with its signed thinking block. If a foreign client lost it (and
/// our cache can't restore it), turn thinking off for this request instead of failing.
pub fn drop_thinking_if_unsigned(req: &mut MessagesRequest) {
    use crate::anthropic::{ContentBlock, MessageContent, Role};
    if req.thinking.as_ref().is_none_or(|t| t.kind == "disabled") {
        return;
    }
    let Some(last) = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
    else {
        return;
    };
    let MessageContent::Blocks(blocks) = &last.content else {
        return;
    };
    let has_tool = blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
    let starts_signed = matches!(
        blocks.first(),
        Some(ContentBlock::Thinking { signature: Some(s), .. }) if !s.is_empty() && !s.starts_with("bro.")
    ) || matches!(blocks.first(), Some(ContentBlock::RedactedThinking { .. }));
    if has_tool && !starts_signed {
        req.thinking = None;
    }
}

pub const CLAUDE_CODE_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Automatic prompt-cache breakpoints for requests translated from OpenAI clients
/// (they never send cache_control): last system block, last tool, and the last
/// block of the final two user turns — at most 4, Anthropic's limit.
pub fn add_cache_breakpoints(req: &mut MessagesRequest) {
    use crate::anthropic::{ContentBlock, MessageContent, Role, SystemPrompt};
    let cc = json!({"type": "ephemeral"});
    if let Some(sys) = req.system.take() {
        let mut blocks = match sys {
            SystemPrompt::Text(t) => vec![ContentBlock::text(t)],
            SystemPrompt::Blocks(b) => b,
        };
        if let Some(last) = blocks.last_mut() {
            last.set_cache_control(cc.clone());
        }
        req.system = Some(SystemPrompt::Blocks(blocks));
    }
    if let Some(last) = req.tools.as_mut().and_then(|t| t.last_mut()) {
        last.cache_control = Some(cc.clone());
    }
    let mut marked = 0;
    for msg in req.messages.iter_mut().rev() {
        if marked == 2 {
            break;
        }
        if msg.role != Role::User {
            continue;
        }
        let mut blocks =
            std::mem::replace(&mut msg.content, MessageContent::Blocks(vec![])).into_blocks();
        if let Some(b) = blocks
            .iter_mut()
            .rev()
            .find(|b| !matches!(b, ContentBlock::Thinking { .. }))
        {
            b.set_cache_control(cc.clone());
            marked += 1;
        }
        msg.content = MessageContent::Blocks(blocks);
    }
}

/// Subscription (OAuth) tokens are only honoured when the system prompt starts
/// with the Claude Code identity line.
pub fn ensure_claude_code_system(req: &mut MessagesRequest) {
    use crate::anthropic::{ContentBlock, SystemPrompt};
    let mut blocks = match req.system.take() {
        None => vec![],
        Some(SystemPrompt::Text(t)) if t.is_empty() => vec![],
        Some(SystemPrompt::Text(t)) => vec![ContentBlock::text(t)],
        Some(SystemPrompt::Blocks(b)) => b,
    };
    let has = matches!(blocks.first(), Some(ContentBlock::Text { text, .. }) if text.starts_with(CLAUDE_CODE_SYSTEM));
    if !has {
        blocks.insert(0, ContentBlock::text(CLAUDE_CODE_SYSTEM));
    }
    req.system = Some(SystemPrompt::Blocks(blocks));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_mapping() {
        assert_eq!(budget_to_effort(1024), "low");
        assert_eq!(budget_to_effort(10000), "medium");
        assert_eq!(budget_to_effort(31999), "high");
        assert_eq!(budget_to_effort(64000), "xhigh");
        let sup = vec!["low".to_string(), "medium".into(), "high".into()];
        assert_eq!(clamp_effort("xhigh", &sup), "high");
        assert_eq!(clamp_effort("minimal", &sup), "low");
        assert_eq!(clamp_effort("medium", &[]), "medium");
    }

    #[test]
    fn effort_keeps_budget_below_max() {
        let mut r = MessagesRequest {
            model: "claude-sonnet-4-5".into(),
            max_tokens: Some(4096),
            ..Default::default()
        };
        r.temperature = Some(0.2);
        apply_effort_to_anthropic(&mut r, Some("high"), true);
        let t = r.thinking.unwrap();
        assert_eq!(t.budget_tokens, Some(3072));
        assert_eq!(r.temperature, None);
        let mut r = MessagesRequest {
            model: "claude-sonnet-4-5".into(),
            ..Default::default()
        };
        apply_effort_to_anthropic(&mut r, Some("xhigh"), false);
        assert_eq!(r.max_tokens, Some(40960));
        let mut r = MessagesRequest::default();
        apply_effort_to_anthropic(&mut r, Some("minimal"), false);
        assert!(r.thinking.is_none());
    }
}
