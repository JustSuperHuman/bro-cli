//! Recaps for agent panes (`docs/agent-insights.md` section 3): picking what to show
//! ([`choose`]), and the pure parts of generating one with Claude Code's built-in `/recap`
//! ([`command`], [`parse_output`]) plus its cache, `~/.bro/recaps/<sid>.json`.
//! Scheduling (180 s unseen, one at a time) belongs to the TUI.
use crate::paths;
use crate::sessions::live::{AgentFacts, Stamped};
use crate::util::{atomic_write, read_json, system_time_ms};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// How long a `/recap` run may take.
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// How long a pane must wait unseen in the same state before a recap is generated.
pub const UNSEEN_AFTER: std::time::Duration = std::time::Duration::from_secs(180);

/// `claude -p --resume <sid> --fork-session --no-session-persistence --output-format json /recap`:
/// run it in the session's cwd with the pane's env (`CLAUDE_CONFIG_DIR`, …) minus
/// variables for which [`strip_env`] is true. The fork and no-persistence flags leave the
/// original transcript untouched.
pub fn command(claude_bin: &str, session_id: &str) -> (String, Vec<String>) {
    (
        claude_bin.to_string(),
        [
            "-p",
            "--resume",
            session_id,
            "--fork-session",
            "--no-session-persistence",
            "--output-format",
            "json",
            "/recap",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    )
}

/// Environment variables the recap child must not inherit from bro's pane env.
pub fn strip_env(key: &str) -> bool {
    key == "BRO" || key.starts_with("BRO_")
}

/// Why a `/recap` run produced nothing usable.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// Not JSON / no `result`
    Malformed,
    /// `is_error: true` (message)
    Error(String),
    /// An empty result
    Empty,
    /// "Nothing to recap yet", "Couldn't generate a recap", "Recap cancelled"
    Declined(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Malformed => write!(f, "unreadable /recap output"),
            Failure::Error(m) => write!(f, "/recap failed: {m}"),
            Failure::Empty => write!(f, "/recap returned nothing"),
            Failure::Declined(m) => write!(f, "{m}"),
        }
    }
}

const DECLINED: &[&str] = &[
    "nothing to recap",
    "couldn't generate",
    "could not generate",
    "recap cancelled",
    "recap canceled",
];

/// Parse `--output-format json` stdout: `{ result, is_error, total_cost_usd }`. Returns the
/// recap text (trimmed) and the cost when reported. Tolerates log lines before the JSON.
pub fn parse_output(stdout: &str) -> Result<(String, Option<f64>), Failure> {
    let v: Value = serde_json::from_str(stdout.trim())
        .ok()
        .or_else(|| {
            stdout.lines().rev().find_map(|l| {
                serde_json::from_str::<Value>(l.trim())
                    .ok()
                    .filter(Value::is_object)
            })
        })
        .ok_or(Failure::Malformed)?;
    let result = v.get("result").and_then(Value::as_str);
    if v.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(Failure::Error(
            result.unwrap_or("unknown error").trim().to_string(),
        ));
    }
    let text = result.ok_or(Failure::Malformed)?.trim();
    if text.is_empty() {
        return Err(Failure::Empty);
    }
    let lower = text.to_lowercase();
    if DECLINED.iter().any(|d| lower.starts_with(d)) {
        return Err(Failure::Declined(
            text.lines().next().unwrap_or(text).to_string(),
        ));
    }
    Ok((
        text.to_string(),
        v.get("total_cost_usd").and_then(Value::as_f64),
    ))
}

/// A generated recap, cached against the transcript it summarised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedRecap {
    pub text: String,
    /// When it was generated (unix ms)
    pub at: i64,
    /// Transcript size and mtime (unix ms) when generated: fresh while both are unchanged
    pub size: u64,
    pub mtime: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// `~/.bro/recaps/<sid>.json` (`None` for ids that can't be file names).
pub fn cache_path(session_id: &str) -> Option<PathBuf> {
    let ok = !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    ok.then(|| {
        paths::bro_dir()
            .join("recaps")
            .join(format!("{session_id}.json"))
    })
}

fn transcript_stamp(transcript: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(transcript).ok()?;
    Some((m.len(), system_time_ms(m.modified().ok()?) as i64))
}

/// The cached recap for a session, fresh or not.
pub fn load(session_id: &str) -> Option<CachedRecap> {
    serde_json::from_value(read_json(&cache_path(session_id)?)?).ok()
}

/// Whether `cached` still describes `transcript` (same size and mtime).
pub fn is_fresh(cached: &CachedRecap, transcript: &Path) -> bool {
    transcript_stamp(transcript)
        .is_some_and(|(size, mtime)| size == cached.size && mtime == cached.mtime)
}

/// The cached recap when it is still fresh for `transcript`.
pub fn load_fresh(session_id: &str, transcript: &Path) -> Option<CachedRecap> {
    load(session_id).filter(|c| is_fresh(c, transcript))
}

/// Cache a generated recap against the transcript's current size and mtime (atomic write).
pub fn store(
    session_id: &str,
    text: &str,
    cost_usd: Option<f64>,
    transcript: &Path,
) -> std::io::Result<CachedRecap> {
    let path = cache_path(session_id).ok_or_else(|| std::io::Error::other("invalid session id"))?;
    let (size, mtime) = transcript_stamp(transcript)
        .ok_or_else(|| std::io::Error::other("transcript unavailable"))?;
    let cached = CachedRecap {
        text: text.trim().to_string(),
        at: crate::util::now_ms(),
        size,
        mtime,
        cost_usd,
    };
    atomic_write(
        &path,
        serde_json::to_string_pretty(&cached)
            .map_err(std::io::Error::other)?
            .as_bytes(),
    )?;
    Ok(cached)
}

/// Where a shown recap came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Claude's own `away_summary`
    Claude,
    /// A `/recap` bro ran
    Generated,
    /// The first sentence of the last assistant text
    LastMessage,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Generated => "generated",
            Source::LastMessage => "last-message",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recap {
    pub text: String,
    pub at_ms: Option<i64>,
    pub source: Source,
}

/// The recap to show: Claude's `away_summary` when it is at least as new as the last
/// assistant text, else a fresh generated recap (pass only a fresh one), else the last
/// assistant text's first sentence.
pub fn choose(facts: &AgentFacts, fresh_generated: Option<&CachedRecap>) -> Option<Recap> {
    if facts.away_summary_is_fresh()
        && let Some(Stamped { text, at_ms }) = &facts.away_summary
    {
        return Some(Recap {
            text: text.clone(),
            at_ms: *at_ms,
            source: Source::Claude,
        });
    }
    if let Some(c) = fresh_generated {
        return Some(Recap {
            text: c.text.clone(),
            at_ms: Some(c.at),
            source: Source::Generated,
        });
    }
    facts.last_assistant.as_ref().map(|s| Recap {
        text: s.text.clone(),
        at_ms: s.at_ms,
        source: Source::LastMessage,
    })
}

/// Whether generating a recap is worthwhile now: a Claude pane with a known session id and
/// no fresh `away_summary` or cached recap. (The caller checks the 180 s unseen rule.)
pub fn worth_generating(facts: &AgentFacts, fresh_generated: Option<&CachedRecap>) -> bool {
    facts.harness == crate::Harness::Claude
        && facts.session_id.is_some()
        // something to recap: prose, or a turn of only tool calls (an edit, then a question)
        && (facts.last_assistant.is_some() || facts.pending.is_some() || !facts.touched.is_empty())
        && !facts.away_summary_is_fresh()
        && fresh_generated.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Harness;
    use crate::util::test_env::sandbox;

    #[test]
    fn command_and_env() {
        let (prog, args) = command(
            "C:\\bin\\claude.exe",
            "11111111-1111-4111-8111-111111111111",
        );
        assert_eq!(prog, "C:\\bin\\claude.exe");
        assert_eq!(
            args.join(" "),
            "-p --resume 11111111-1111-4111-8111-111111111111 --fork-session --no-session-persistence --output-format json /recap"
        );
        assert!(strip_env("BRO_SESSION_ID") && strip_env("BRO"));
        assert!(!strip_env("CLAUDE_CONFIG_DIR") && !strip_env("BROWSER"));
    }

    #[test]
    fn parses_results_and_failures() {
        assert_eq!(
            parse_output(
                r#"{"type":"result","result":"  Fixed the parser; README pending.\n","is_error":false,"total_cost_usd":0.021}"#
            ),
            Ok(("Fixed the parser; README pending.".into(), Some(0.021)))
        );
        assert_eq!(
            parse_output("warming up\n{\"result\":\"ok text\"}\n"),
            Ok(("ok text".into(), None))
        );
        assert_eq!(
            parse_output(r#"{"result":"Nothing to recap yet.","is_error":false}"#),
            Err(Failure::Declined("Nothing to recap yet.".into()))
        );
        assert_eq!(
            parse_output(r#"{"result":"Couldn't generate a recap"}"#),
            Err(Failure::Declined("Couldn't generate a recap".into()))
        );
        assert_eq!(
            parse_output(r#"{"result":"boom","is_error":true}"#),
            Err(Failure::Error("boom".into()))
        );
        assert_eq!(parse_output(r#"{"result":"   "}"#), Err(Failure::Empty));
        assert_eq!(parse_output("not json"), Err(Failure::Malformed));
    }

    #[test]
    fn cache_round_trip_and_freshness() {
        let sb = sandbox();
        let t = sb.home().join("t.jsonl");
        std::fs::write(&t, "{}\n").unwrap();
        let sid = "11111111-1111-4111-8111-111111111111";
        assert!(cache_path("../evil").is_none());
        let c = store(sid, " Recap text ", Some(0.02), &t).unwrap();
        assert_eq!(c.text, "Recap text");
        assert!(cache_path(sid).unwrap().starts_with(sb.bro()));
        assert_eq!(load_fresh(sid, &t), Some(c.clone()));
        std::fs::write(&t, "{}\n{\"more\":1}\n").unwrap();
        assert_eq!(load_fresh(sid, &t), None, "the transcript grew");
        assert_eq!(load(sid), Some(c));
    }

    #[test]
    fn choose_prefers_fresh_claude_summary_then_generated_then_last_message() {
        let mut f = AgentFacts::new(Harness::Claude);
        assert_eq!(choose(&f, None), None);
        f.session_id = Some("s".into());
        f.last_assistant = Some(Stamped {
            text: "Done.".into(),
            at_ms: Some(100),
        });
        assert_eq!(choose(&f, None).unwrap().source, Source::LastMessage);
        assert!(worth_generating(&f, None));
        let generated = CachedRecap {
            text: "Generated".into(),
            at: 150,
            size: 1,
            mtime: 1,
            cost_usd: None,
        };
        assert_eq!(
            choose(&f, Some(&generated)).unwrap().source,
            Source::Generated
        );
        assert!(!worth_generating(&f, Some(&generated)));
        f.away_summary = Some(Stamped {
            text: "Away".into(),
            at_ms: Some(200),
        });
        let r = choose(&f, Some(&generated)).unwrap();
        assert_eq!((r.text.as_str(), r.source), ("Away", Source::Claude));
        assert!(!worth_generating(&f, None));
        f.away_summary = Some(Stamped {
            text: "Old away".into(),
            at_ms: Some(50),
        });
        assert_eq!(choose(&f, None).unwrap().source, Source::LastMessage);
        let mut codex = f.clone();
        codex.harness = Harness::Codex;
        assert!(!worth_generating(&codex, None));

        // A turn of only tool calls (an edit, then a question) is still worth a recap;
        // a session with nothing in it yet is not.
        let mut tools_only = AgentFacts::new(Harness::Claude);
        tools_only.session_id = Some("s".into());
        assert!(!worth_generating(&tools_only, None));
        tools_only.pending = Some(crate::sessions::live::Pending {
            tool: "AskUserQuestion".into(),
            reason: "Question: Color".into(),
            at_ms: Some(10),
        });
        assert!(worth_generating(&tools_only, None));
    }
}
