//! Optional request compression with llmtrim-core (MPL-2.0, <https://github.com/fkiene/llmtrim>).
//!
//! Runs on the inbound JSON before translation and upstream dispatch, so Anthropic
//! `cache_control` markers (and with them llmtrim's frozen zone) are still visible. Two levels,
//! both lossless for the model and with every lossy or behaviour-changing stage off:
//!
//! * [`TrimLevel::Safe`]: tool output loses ANSI escapes and carriage-return progress frames
//!   (what a terminal would have shown). Nothing else changes.
//! * [`TrimLevel::Lossless`]: also llmtrim's lossless data stages (JSON minify, columnar
//!   arrays). The agent then sees minified JSON in tool output.
//!
//! Prompt caching: content stages skip the cached prefix, and a turn memo replays the exact
//! bytes forwarded for every earlier message, so a conversation's prefix stays byte-identical
//! from turn to turn. The memo is salted with the request's original system/tools envelope:
//! when those change (an MCP server connects, the model switches) the conversation starts
//! cold instead of having an older envelope replayed over the new one. Claude Code marks
//! nearly its whole request as cached, so on its traffic savings are small (the newest tool
//! output only); requests without markers (Codex, Pi) compress further.
//!
//! Never makes a request worse: errors, oversized bodies and results that would not be
//! smaller all forward the original.

use crate::Dialect;
use llmtrim_core::config::DenseConfig;
use llmtrim_core::ir::{ProviderKind, Request};
use llmtrim_core::memo::{self, Memo};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Bodies larger than this are forwarded untouched (compression is CPU-bound).
const MAX_BODY: usize = 16 * 1024 * 1024;
/// Memo entries per generation (two generations live): roughly one entry per message.
const MEMO_CAP: usize = 4096;

/// How much to compress. Both levels keep every lossy stage off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrimLevel {
    /// Terminal-noise normalization of tool output only
    #[default]
    Safe,
    /// Also JSON minify and columnar encoding of uniform arrays
    Lossless,
}

impl TrimLevel {
    /// "lossless" → Lossless, anything else → Safe.
    pub fn parse(s: &str) -> TrimLevel {
        if s.trim().eq_ignore_ascii_case("lossless") { TrimLevel::Lossless } else { TrimLevel::Safe }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TrimLevel::Safe => "safe",
            TrimLevel::Lossless => "lossless",
        }
    }
    fn index(self) -> u8 {
        match self {
            TrimLevel::Safe => 0,
            TrimLevel::Lossless => 1,
        }
    }
    fn from_index(i: u8) -> TrimLevel {
        if i == 1 { TrimLevel::Lossless } else { TrimLevel::Safe }
    }
}

/// The llmtrim stage configuration for a level: never `auto`, no output shaping, no tool
/// selection/trimming, no n-gram (it disables the memo), no retrieval/skeleton/crush, no base64
/// stripping, no cache stage (clients manage their own breakpoints), no images.
pub(crate) fn dense_config(level: TrimLevel) -> DenseConfig {
    let mut c = DenseConfig::lossless();
    c.auto = false;
    c.memo = true;
    c.quality_gate = true;
    c.cache = false;
    c.output_control = false;
    c.output_frugal_tools = false;
    c.output_anti_overthink = false;
    c.output_compact_code = false;
    c.tool_select = false;
    c.tool_trim_desc = false;
    c.tool_minify_schema = false;
    c.ngram = false;
    c.retrieve = false;
    c.skeletonize = false;
    c.minify_code = false;
    c.json_crush = false;
    c.strip_base64 = false;
    c.numeric_sig_figs = None;
    c.normalize_unicode = false;
    c.multimodal = false;
    // Exact-duplicate line folding also merges non-adjacent lines of prose and code: off.
    c.dedup = false;
    c.dedup_near = false;
    // Tool output: normalization only. No segment is ever "long enough" to window, so the
    // stage strips ANSI / collapses CR progress and never drops a line.
    c.toolout = true;
    c.toolout_min_lines = usize::MAX;
    c.toolout_template = false;
    if level == TrimLevel::Safe {
        c.hygiene = false;
        c.serialize = false;
    }
    c
}

/// What to forward, and what to remember once it was forwarded.
pub(crate) struct Trimmed {
    pub json: Value,
    /// Estimated input tokens before / after (llmtrim's counter), when compression ran
    pub tokens: Option<(u64, u64)>,
    pub commit: Option<Commit>,
}

/// Memo entries for a request; recorded only after the request actually went upstream.
pub(crate) struct Commit {
    salt: Vec<u8>,
    original: Value,
    forwarded: Value,
}

pub(crate) struct Trimmer {
    enabled: AtomicBool,
    level: AtomicU8,
    memo: Arc<Memo>,
}

impl Trimmer {
    pub fn new(enabled: bool, level: TrimLevel) -> Self {
        Trimmer { enabled: AtomicBool::new(enabled), level: AtomicU8::new(level.index()), memo: Arc::new(Memo::with_capacity(MEMO_CAP)) }
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn level(&self) -> TrimLevel {
        TrimLevel::from_index(self.level.load(Ordering::Relaxed))
    }
    pub fn set(&self, enabled: bool, level: TrimLevel) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.level.store(level.index(), Ordering::Relaxed);
    }

    /// Compress `json` if trimming is on (off the async workers). Never fails: on any problem
    /// the original is returned unchanged.
    pub async fn apply(&self, inbound: Dialect, json: Value) -> Trimmed {
        if !self.enabled() {
            return Trimmed { json, tokens: None, commit: None };
        }
        let level = self.level();
        let memo = self.memo.clone();
        let original = json.clone();
        match tokio::task::spawn_blocking(move || compress(&memo, inbound, level, json)).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("request compression task failed: {e}");
                Trimmed { json: original, tokens: None, commit: None }
            }
        }
    }

    /// Remember what was forwarded so the next turn replays it byte for byte.
    pub fn commit(&self, c: Commit) {
        let memo = self.memo.clone();
        // Hashing every message of a long conversation: keep it off the async workers.
        tokio::task::spawn_blocking(move || memo::record(&memo, &c.salt, &c.original, &c.forwarded));
    }
}

fn kind_for(d: Dialect) -> ProviderKind {
    match d {
        Dialect::Anthropic => ProviderKind::Anthropic,
        Dialect::Chat | Dialect::Responses => ProviderKind::OpenAi,
    }
}

/// Memo scope: provider kind, level, the stage lineup that ran, and the request's original
/// envelope. A changed system prompt or tool list therefore never gets an older one replayed.
fn salt_for(kind: ProviderKind, level: TrimLevel, lineup: &str, original: &Value) -> Vec<u8> {
    let envelope: Vec<String> = ["system", "instructions", "tools"]
        .iter()
        .map(|k| original.get(*k).map(|v| v.to_string()).unwrap_or_default())
        .collect();
    format!("bro|{}|{}|{lineup}|{}", kind.as_str(), level.as_str(), crate::util::short_hash(&envelope.join("\u{0}"))).into_bytes()
}

/// The synchronous core: compress, replay the memo, gate on size.
pub(crate) fn compress(memo: &Memo, inbound: Dialect, level: TrimLevel, json: Value) -> Trimmed {
    let passthrough = |json: Value| Trimmed { json, tokens: None, commit: None };
    let Ok(original_text) = serde_json::to_string(&json) else { return passthrough(json) };
    if original_text.len() > MAX_BODY {
        return passthrough(json);
    }
    let kind = kind_for(inbound);
    let config = dense_config(level);
    let result = match llmtrim_core::compress_with_config(&original_text, Some(kind), &config) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("request compression skipped: {e:#}");
            return passthrough(json);
        }
    };
    let lineup: Vec<&str> = result.stages.iter().map(|s| s.name.as_str()).collect();
    let salt = salt_for(kind, level, &lineup.join(","), &json);
    let Ok(mut compressed) = serde_json::from_str::<Value>(&result.request_json) else { return passthrough(json) };
    let before = result.input_tokens_before.0 as u64;
    let mut after = result.input_tokens_after.0 as u64;
    if memo::replay(memo, &salt, &json, &mut compressed) > 0 {
        // Count what is actually sent now that earlier turns were replayed.
        if let Ok(counter) = llmtrim_core::tokenizer::counter_for(kind, result.model.as_deref()) {
            let adapter = llmtrim_core::provider::for_kind(kind);
            let req = Request::from_value(kind, compressed.clone());
            after = llmtrim_core::pipeline::content_tokens(&req, adapter.as_ref(), counter.as_ref()) as u64;
        }
    }
    let forwarded = if after > before || compressed == json {
        // Never forward something bigger; an unchanged body goes out as received.
        after = before;
        json.clone()
    } else {
        compressed
    };
    Trimmed {
        commit: Some(Commit { salt, original: json, forwarded: forwarded.clone() }),
        json: forwarded,
        tokens: Some((before, after)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ANSI_LOG: &str = "\u{1b}[32m   Compiling\u{1b}[0m bro v2\n\u{1b}[1m\u{1b}[31merror\u{1b}[0m: mismatched types\n  0% [    ]\r 50% [==  ]\r100% [====] done\n";

    fn tool_turn(id: &str, output: &str) -> [Value; 2] {
        [
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "cargo build"}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": output}]}),
        ]
    }

    /// Claude Code's shape: cached system, tools, and the newest message carries the marker.
    fn claude_code_request(turns: &[[Value; 2]]) -> Value {
        let mut messages = vec![json!({"role": "user", "content": [{"type": "text", "text": "build it and fix the errors"}]})];
        for t in turns {
            messages.extend(t.iter().cloned());
        }
        let last = messages.last_mut().unwrap();
        last["content"][0]["cache_control"] = json!({"type": "ephemeral"});
        json!({
            "model": "claude-opus-5-5", "max_tokens": 32000, "stream": true, "temperature": 1,
            "system": [{"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}}],
            "tools": [{"name": "Bash", "description": "Run a command", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}}}],
            "messages": messages,
        })
    }

    fn strip_markers(mut v: Value) -> Value {
        fn walk(v: &mut Value) {
            match v {
                Value::Object(m) => {
                    m.remove("cache_control");
                    m.values_mut().for_each(walk);
                }
                Value::Array(a) => a.iter_mut().for_each(walk),
                _ => {}
            }
        }
        walk(&mut v);
        v
    }

    #[test]
    fn levels_keep_lossy_and_behaviour_changing_stages_off() {
        for level in [TrimLevel::Safe, TrimLevel::Lossless] {
            let c = dense_config(level);
            assert!(!c.auto && c.memo && !c.output_control && !c.tool_select && !c.tool_trim_desc && !c.ngram);
            assert!(!c.retrieve && !c.skeletonize && !c.json_crush && !c.strip_base64 && !c.cache && !c.multimodal && !c.dedup);
            assert!(c.toolout && c.toolout_min_lines == usize::MAX);
        }
        assert!(!dense_config(TrimLevel::Safe).hygiene && dense_config(TrimLevel::Lossless).hygiene);
        assert_eq!(TrimLevel::parse("LOSSLESS"), TrimLevel::Lossless);
        assert_eq!(TrimLevel::parse("whatever"), TrimLevel::Safe);
    }

    #[test]
    fn newest_tool_output_loses_terminal_noise_and_later_turns_replay_it_byte_for_byte() {
        let memo = Memo::with_capacity(MEMO_CAP);
        let t1 = [tool_turn("toolu_1", ANSI_LOG)];
        let req1 = claude_code_request(&t1);
        let out1 = compress(&memo, Dialect::Anthropic, TrimLevel::Safe, req1.clone());
        let (before, after) = out1.tokens.unwrap();
        assert!(after < before, "ANSI and progress frames removed: {before} -> {after}");
        let sent1 = out1.json.clone();
        let result1 = sent1["messages"][2]["content"][0]["content"].as_str().unwrap();
        assert!(!result1.contains('\u{1b}') && !result1.contains('\r'));
        assert!(result1.contains("Compiling") && result1.contains("error: mismatched types") && result1.contains("100% [====] done"));
        assert_eq!(sent1["system"], req1["system"], "instructions untouched");
        assert_eq!(sent1["tools"], req1["tools"], "tools untouched");
        memo::record(&memo, &out1.commit.as_ref().unwrap().salt, &req1, &sent1);

        // Turn 2: the old result is now inside the cached prefix (the marker moved on). Without
        // the memo it would go out raw and bust the cache; with it the bytes repeat exactly.
        let t2 = [tool_turn("toolu_1", ANSI_LOG), tool_turn("toolu_2", "ok\n")];
        let req2 = claude_code_request(&t2);
        let out2 = compress(&memo, Dialect::Anthropic, TrimLevel::Safe, req2.clone());
        let sent2 = out2.json;
        for i in 0..3 {
            assert_eq!(
                serde_json::to_string(&strip_markers(sent2["messages"][i].clone())).unwrap(),
                serde_json::to_string(&strip_markers(sent1["messages"][i].clone())).unwrap(),
                "message {i} is byte-identical across turns"
            );
        }
        assert!(sent2["messages"][2]["content"][0].get("cache_control").is_none(), "markers follow the client, not the memo");
        assert_eq!(sent2["messages"][4]["content"][0]["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(serde_json::to_string(&sent2["system"]).unwrap(), serde_json::to_string(&req2["system"]).unwrap());
    }

    #[test]
    fn without_the_memo_the_prefix_would_change_between_turns() {
        // Control for the test above: an inert memo lets the frozen result go out raw on turn 2.
        let memo = Memo::with_capacity(0);
        let sent1 = compress(&memo, Dialect::Anthropic, TrimLevel::Safe, claude_code_request(&[tool_turn("toolu_1", ANSI_LOG)])).json;
        let req2 = claude_code_request(&[tool_turn("toolu_1", ANSI_LOG), tool_turn("toolu_2", "ok
")]);
        let sent2 = compress(&memo, Dialect::Anthropic, TrimLevel::Safe, req2).json;
        assert_ne!(strip_markers(sent2["messages"][2].clone()), strip_markers(sent1["messages"][2].clone()));
    }

    #[test]
    fn a_changed_tool_list_is_never_replaced_by_the_remembered_one() {
        let memo = Memo::with_capacity(MEMO_CAP);
        let req1 = claude_code_request(&[tool_turn("toolu_1", ANSI_LOG)]);
        let out1 = compress(&memo, Dialect::Anthropic, TrimLevel::Lossless, req1.clone());
        let c = out1.commit.unwrap();
        memo::record(&memo, &c.salt, &c.original, &c.forwarded);
        let mut req2 = claude_code_request(&[tool_turn("toolu_1", ANSI_LOG), tool_turn("toolu_2", "ok")]);
        req2["tools"].as_array_mut().unwrap().push(json!({"name": "mcp__new__tool", "description": "new", "input_schema": {"type": "object"}}));
        let out2 = compress(&memo, Dialect::Anthropic, TrimLevel::Lossless, req2.clone());
        assert_eq!(out2.json["tools"], req2["tools"]);
    }

    #[test]
    fn clean_requests_and_failures_pass_through_unchanged() {
        let memo = Memo::with_capacity(MEMO_CAP);
        let req = claude_code_request(&[tool_turn("toolu_1", "plain output\n")]);
        let out = compress(&memo, Dialect::Anthropic, TrimLevel::Safe, req.clone());
        assert_eq!(serde_json::to_string(&out.json).unwrap(), serde_json::to_string(&req).unwrap(), "byte-identical when there is nothing to trim");
        let (b, a) = out.tokens.unwrap();
        assert_eq!(a, b);
        let odd = json!({"not": "a request"});
        assert_eq!(compress(&memo, Dialect::Anthropic, TrimLevel::Safe, odd.clone()).json, odd);
    }

    #[test]
    fn lossless_minifies_json_tool_output_without_markers() {
        // Codex / Pi shape: no cache_control, so the whole conversation is compressible.
        let memo = Memo::with_capacity(MEMO_CAP);
        let pretty = serde_json::to_string_pretty(&json!({"items": [{"id": 1, "name": "a"}, {"id": 2, "name": "b"}, {"id": 3, "name": "c"}]})).unwrap();
        let req = json!({"model": "gpt-5.3-codex", "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "list items"}]},
            {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": "{\"cmd\":\"cat items.json\"}"},
            {"type": "function_call_output", "call_id": "c1", "output": pretty},
        ]});
        let safe = compress(&memo, Dialect::Responses, TrimLevel::Safe, req.clone());
        assert_eq!(safe.json, req, "safe never reshapes JSON");
        let lossless = compress(&memo, Dialect::Responses, TrimLevel::Lossless, req.clone());
        let (b, a) = lossless.tokens.unwrap();
        assert!(a < b, "{b} -> {a}");
        assert_ne!(lossless.json["input"][2]["output"], req["input"][2]["output"]);
        assert_eq!(lossless.json["input"][1], req["input"][1], "call arguments untouched");
    }

    #[test]
    fn numbers_and_flattened_fields_survive_arbitrary_precision() {
        // llmtrim-core turns on serde_json `arbitrary_precision` for the whole binary.
        let body = json!({"model": "m", "max_tokens": 1024, "temperature": 0.7, "top_k": 5, "messages": [{"role": "user", "content": "hi"}], "metadata": {"user_id": "u"}});
        let req: crate::anthropic::MessagesRequest = serde_json::from_value(body.clone()).unwrap();
        let back = serde_json::to_value(&req).unwrap();
        assert_eq!(back["max_tokens"].as_u64(), Some(1024));
        assert_eq!(back["temperature"].as_f64(), Some(0.7));
        assert_eq!(back["top_k"].as_u64(), Some(5));
        assert_eq!(serde_json::to_string(&json!(0.1)).unwrap(), "0.1");
    }

    /// Measure savings on a captured request: `BRO_TRIM_FIXTURE=<request.json> cargo test -p bro-proxy
    /// measure_fixture -- --ignored --nocapture`. Reports the Claude Code shape as given and the
    /// same conversation without cache markers (how Codex / Pi requests arrive).
    #[test]
    #[ignore]
    fn measure_fixture() {
        let Ok(path) = std::env::var("BRO_TRIM_FIXTURE") else { return };
        let req: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let unmarked = strip_markers(req.clone());
        for (label, body) in [("claude code (cache markers)", &req), ("no markers (codex / pi)", &unmarked)] {
            for level in [TrimLevel::Safe, TrimLevel::Lossless] {
                let memo = Memo::with_capacity(MEMO_CAP);
                let started = std::time::Instant::now();
                let out = compress(&memo, Dialect::Anthropic, level, body.clone());
                let (b, a) = out.tokens.unwrap_or((0, 0));
                println!("{label:<28} {:<9} {b:>8} -> {a:>8} tokens  saved {:>5.2}%  {:?}", level.as_str(), if b > 0 { 100.0 * (b - a) as f64 / b as f64 } else { 0.0 }, started.elapsed());
            }
            // For comparison only (bro never enables these): llmtrim's lossy presets.
            let text = serde_json::to_string(body).unwrap();
            for preset in ["agent", "auto", "aggressive"] {
                let r = llmtrim_core::compress_with_config(&text, Some(ProviderKind::Anthropic), &DenseConfig::preset(preset).unwrap()).unwrap();
                let (b, a) = (r.input_tokens_before.0, r.input_tokens_after.0);
                println!("{label:<28} {preset:<9} {b:>8} -> {a:>8} tokens  saved {:>5.2}%  (lossy, not used)", 100.0 * (b - a) as f64 / b as f64);
            }
        }
    }
}
