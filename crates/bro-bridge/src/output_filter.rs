//! Removes the private `OSC 1337;TerminalWeb.Agent=<base64url json>` metadata
//! envelope from session output before clients see it, and applies it to the
//! session summary. Ordinary terminal controls pass through untouched.
//!
//! The scanner owns incomplete OSC frames across output calls, so neither a
//! split marker nor a malformed oversized marker can leak a printable tail to
//! web/mobile clients.

use crate::model::TerminalSessionSummary;
use base64::Engine;
use serde_json::Value;

const PRIVATE_AGENT_OSC: &str = "1337;TerminalWeb.Agent=";
/// OSC frames longer than this are discarded instead of forwarded.
pub(crate) const MAX_FILTERED_OSC_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    DiscardOsc,
    DiscardOscEscape,
}

/// Streaming filter for one session's output.
#[derive(Debug, Default)]
pub(crate) struct OutputFilter {
    state: State,
    frame: String,
    /// The agent a wrapper announced through the private OSC handshake and
    /// has not cleared yet; outranks screen fingerprinting.
    pub osc_agent: Option<String>,
}

impl OutputFilter {
    /// Feeds one chunk and returns the part clients should see.
    pub fn feed(&mut self, data: &str, summary: &mut TerminalSessionSummary) -> String {
        use State::*;
        let mut visible = String::with_capacity(data.len());
        for character in data.chars() {
            match self.state {
                Ground => {
                    if character == '\u{1b}' {
                        self.frame.clear();
                        self.frame.push(character);
                        self.state = Escape;
                    } else {
                        visible.push(character);
                    }
                }
                Escape => {
                    if character == ']' {
                        self.frame.push(character);
                        self.state = Osc;
                    } else if character == '\u{1b}' {
                        visible.push_str(&self.frame);
                        self.frame.clear();
                        self.frame.push(character);
                    } else {
                        self.frame.push(character);
                        visible.push_str(&self.frame);
                        self.frame.clear();
                        self.state = Ground;
                    }
                }
                Osc => {
                    self.frame.push(character);
                    if character == '\u{7}' {
                        self.finish_osc(summary, &mut visible, 1);
                    } else if character == '\u{1b}' {
                        self.state = OscEscape;
                    } else if self.frame.len() > MAX_FILTERED_OSC_BYTES {
                        self.frame.clear();
                        self.state = DiscardOsc;
                    }
                }
                OscEscape => {
                    self.frame.push(character);
                    if character == '\\' {
                        self.finish_osc(summary, &mut visible, 2);
                    } else if character != '\u{1b}' {
                        self.state = Osc;
                    }
                }
                DiscardOsc => {
                    if character == '\u{7}' {
                        self.state = Ground;
                    } else if character == '\u{1b}' {
                        self.state = DiscardOscEscape;
                    }
                }
                DiscardOscEscape => {
                    self.state = if character == '\\' {
                        Ground
                    } else {
                        DiscardOsc
                    };
                }
            }
        }
        visible
    }

    fn finish_osc(
        &mut self,
        summary: &mut TerminalSessionSummary,
        visible: &mut String,
        terminator_bytes: usize,
    ) {
        let body_end = self.frame.len().saturating_sub(terminator_bytes);
        let body = self.frame.get(2..body_end).unwrap_or_default();
        if let Some(encoded) = body.strip_prefix(PRIVATE_AGENT_OSC) {
            apply_agent_metadata(summary, &mut self.osc_agent, encoded);
        } else {
            visible.push_str(&self.frame);
        }
        self.frame.clear();
        self.state = State::Ground;
    }
}

/// Applies a decoded `{"v":1,"agent":"claude|codex","state":"active|inactive"}`
/// handshake. Anything malformed is ignored.
fn apply_agent_metadata(
    summary: &mut TerminalSessionSummary,
    osc_agent: &mut Option<String>,
    encoded: &str,
) {
    if encoded.is_empty()
        || encoded.len() > 512
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return;
    }
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        return;
    };
    if decoded.len() > 256 {
        return;
    }
    let Ok(Value::Object(value)) = serde_json::from_slice::<Value>(&decoded) else {
        return;
    };
    if value.len() != 3
        || value.get("v").and_then(Value::as_u64) != Some(1)
        || !value.contains_key("agent")
        || !value.contains_key("state")
    {
        return;
    }
    let agent = value.get("agent").and_then(Value::as_str);
    let state = value.get("state").and_then(Value::as_str);
    if !matches!(agent, Some("claude" | "codex")) || !matches!(state, Some("active" | "inactive")) {
        return;
    }
    if state == Some("active") {
        *osc_agent = agent.map(str::to_owned);
        summary.agent = agent.map(str::to_owned);
        summary.agent_source = Some("osc".into());
    } else if osc_agent.as_deref() == agent {
        // Ignore stale or unpaired clears so one wrapper cannot clear a
        // newer agent that already became active in the same terminal.
        *osc_agent = None;
        if summary.agent.as_deref() == agent {
            summary.agent = None;
            summary.agent_source = None;
            summary.agent_activity = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> TerminalSessionSummary {
        TerminalSessionSummary::native(
            "session".into(),
            "Shell".into(),
            "pwsh".into(),
            "C:\\work".into(),
            1,
            80,
            24,
        )
    }

    #[test]
    fn private_agent_osc_is_removed_at_every_chunk_boundary() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"v":1,"agent":"codex","state":"active"}"#);
        let marker = format!("before\u{1b}]1337;TerminalWeb.Agent={payload}\u{1b}\\after");
        let boundaries: Vec<usize> = marker
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(marker.len()))
            .collect();
        for split in boundaries {
            let mut summary = summary();
            let mut filter = OutputFilter::default();
            let mut visible = filter.feed(&marker[..split], &mut summary);
            visible.push_str(&filter.feed(&marker[split..], &mut summary));
            assert_eq!(visible, "beforeafter", "split at byte {split}");
            assert_eq!(summary.agent.as_deref(), Some("codex"));
        }
    }

    #[test]
    fn ordinary_osc_is_preserved_and_oversized_osc_is_dropped() {
        let mut summary = summary();
        let mut filter = OutputFilter::default();
        let title = "\u{1b}]0;useful title\u{7}";
        assert_eq!(filter.feed(title, &mut summary), title);
        let malformed = format!(
            "\u{1b}]1337;TerminalWeb.Agent={}\u{1b}\\safe",
            "x".repeat(MAX_FILTERED_OSC_BYTES + 1)
        );
        assert_eq!(filter.feed(&malformed, &mut summary), "safe");
    }

    #[test]
    fn inactive_clears_only_the_matching_agent() {
        let mut summary = summary();
        let mut filter = OutputFilter::default();
        let encode = |json: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        let active = encode(r#"{"v":1,"agent":"claude","state":"active"}"#);
        let stale = encode(r#"{"v":1,"agent":"codex","state":"inactive"}"#);
        let clear = encode(r#"{"v":1,"agent":"claude","state":"inactive"}"#);
        filter.feed(
            &format!("\u{1b}]1337;TerminalWeb.Agent={active}\u{7}"),
            &mut summary,
        );
        filter.feed(
            &format!("\u{1b}]1337;TerminalWeb.Agent={stale}\u{7}"),
            &mut summary,
        );
        assert_eq!(summary.agent.as_deref(), Some("claude"));
        filter.feed(
            &format!("\u{1b}]1337;TerminalWeb.Agent={clear}\u{7}"),
            &mut summary,
        );
        assert_eq!(summary.agent, None);
    }
}
