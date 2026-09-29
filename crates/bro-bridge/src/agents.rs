//! Screen-derived recognition of the agent TUIs (Claude Code, Codex) running
//! inside terminals, and what they are doing right now.
//!
//! The session summaries, the composed-input context and the orchestrator's
//! workspace snapshot all read the same observation, so every client agrees
//! about a tab's state. Detection is deliberately heuristic: it scores the
//! furniture each TUI paints (footers, prompts, mode banners) over the last
//! screenful, which is what lets a `pwsh` tab that later ran `claude` become
//! an agent terminal, and a `claude` tab that exited back to its shell stop
//! being one.

use crate::model::TerminalSessionSummary;

/// How many trailing screen lines participate in fingerprinting.
pub const WINDOW_LINES: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub agent: Option<Agent>,
    /// `osc` (explicit handshake), `screen` (fingerprint) or `command`
    /// (launch line). Absent when no agent is recognised.
    pub source: Option<&'static str>,
    /// The agent is mid-turn rather than waiting at its prompt.
    pub busy: bool,
}

pub fn tail_lines(text: &str, count: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].to_vec()
}

fn claude_score(lines: &[&str], lower: &str) -> u32 {
    let mut score = 0;
    if lower.contains("shift+tab to cycle") {
        score += 4;
    }
    if lower.contains("bypass permissions on")
        || lower.contains("accept edits on")
        || lower.contains("plan mode on")
    {
        score += 4;
    }
    if lower.contains("⏵⏵") {
        score += 3;
    }
    if lower.contains("welcome to claude code") {
        score += 3;
    }
    if lower.contains("? for shortcuts") {
        score += 3;
    }
    if lower.contains("ctrl+t to hide tasks") || lower.contains("ctrl+t to show tasks") {
        score += 2;
    }
    if lower.contains("claude code") {
        score += 1;
    }
    if lines.iter().any(|line| line.starts_with("❯ ")) {
        score += 1;
    }
    score
}

fn codex_score(lines: &[&str], lower: &str) -> u32 {
    let mut score = 0;
    if lower.contains("ctrl+j") && lower.contains("newline") {
        score += 4;
    }
    if lines.iter().any(|line| line.trim_start().starts_with("› ")) {
        score += 3;
    }
    if lower.contains("/approvals") {
        score += 3;
    }
    if lines
        .iter()
        .any(|line| line.trim_start().to_lowercase().starts_with("• working"))
    {
        score += 2;
    }
    if lower.contains("openai codex") {
        score += 3;
    }
    if lower.contains("codex") {
        score += 1;
    }
    // Codex's status line: "gpt-5.3 high · 42% context left".
    if lines.iter().any(|line| {
        let trimmed = line.trim_start().to_lowercase();
        (trimmed.starts_with("gpt") || trimmed.starts_with("o3") || trimmed.starts_with("o4"))
            && trimmed.contains(" · ")
    }) {
        score += 3;
    }
    score
}

pub fn is_busy(lines: &[&str], lower: &str) -> bool {
    lower.contains("esc to interrupt")
        || lines
            .iter()
            .any(|line| line.trim_start().to_lowercase().starts_with("• working ("))
}

fn executable_name(shell: &str) -> String {
    let trimmed = shell.trim();
    let first = if let Some(rest) = trimmed.strip_prefix('"') {
        rest.split('"').next().unwrap_or_default()
    } else {
        trimmed.split_whitespace().next().unwrap_or_default()
    };
    let name = first
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(first)
        .to_lowercase();
    name.trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .to_string()
}

fn command_mentions(command: &str, word: &str) -> bool {
    command
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '\\' | '/' | ';' | '&' | '|' | '(' | ')'
                )
        })
        .any(|token| token.trim_end_matches(".exe") == word)
}

/// The last non-empty row reads like a shell prompt (`PS C:\x>`, `$ `,
/// `%`, `#`), which is what an agent leaves behind when it exits.
fn ends_at_shell_prompt(lines: &[&str]) -> bool {
    let Some(last) = lines
        .iter()
        .rev()
        .map(|line| line.trim_end())
        .find(|line| !line.trim().is_empty())
    else {
        return false;
    };
    let trimmed = last.trim();
    if trimmed.chars().count() > 200 {
        return false;
    }
    let ends_like_prompt = trimmed.ends_with('>')
        || trimmed.ends_with('$')
        || trimmed.ends_with('%')
        || trimmed.ends_with('#');
    ends_like_prompt || (trimmed.starts_with("PS ") && trimmed.contains('>'))
}

fn launched_agent(summary: &TerminalSessionSummary) -> Option<Agent> {
    let executable = executable_name(&summary.shell);
    let command = format!("{} {}", summary.shell, summary.args.join(" ")).to_lowercase();
    if executable.starts_with("claude") || command_mentions(&command, "claude") {
        return Some(Agent::Claude);
    }
    if executable.starts_with("codex") || command_mentions(&command, "codex") {
        return Some(Agent::Codex);
    }
    None
}

/// Classifies a session from its rendered screen, the private OSC handshake
/// (when a wrapper announced itself) and its launch command, most trusted
/// signal first.
pub fn observe(
    summary: &TerminalSessionSummary,
    osc_agent: Option<Agent>,
    screen: &str,
) -> Observation {
    observe_with_hint(summary, osc_agent, screen, None)
}

/// [`observe`] with the agent the host says it launched (bro's
/// `SessionMeta::agent`), used like the launch command: a fallback below the
/// handshake and the screen fingerprint.
pub fn observe_with_hint(
    summary: &TerminalSessionSummary,
    osc_agent: Option<Agent>,
    screen: &str,
    launched: Option<Agent>,
) -> Observation {
    let lines = tail_lines(screen, WINDOW_LINES);
    let joined = lines.join("\n");
    let lower = joined.to_lowercase();
    let claude = claude_score(&lines, &lower);
    let codex = codex_score(&lines, &lower);
    let busy = is_busy(&lines, &lower);

    let screen_agent = if claude >= 4 && claude > codex {
        Some(Agent::Claude)
    } else if codex >= 4 && codex > claude {
        Some(Agent::Codex)
    } else {
        None
    };

    // An agent recognised earlier stays recognised while its dialogs cover
    // the footer that identified it; only a shell prompt or another agent
    // taking over the screen ends it.
    let sticky_agent = match summary.agent_source.as_deref() {
        Some("screen") | None => summary.agent.as_deref().and_then(Agent::parse),
        _ => None,
    }
    .filter(|_| !ends_at_shell_prompt(&lines));

    let (agent, source) = if let Some(agent) = osc_agent {
        (Some(agent), Some("osc"))
    } else if let Some(agent) = screen_agent {
        (Some(agent), Some("screen"))
    } else if let Some(agent) = sticky_agent {
        (Some(agent), Some("screen"))
    } else {
        match launched.or_else(|| launched_agent(summary)) {
            Some(Agent::Claude) if codex < 4 => (Some(Agent::Claude), Some("command")),
            Some(Agent::Codex) if claude < 4 => (Some(Agent::Codex), Some("command")),
            _ => (None, None),
        }
    };

    Observation {
        agent,
        source,
        busy: agent.is_some() && busy,
    }
}

/// `working` while the agent is mid-turn, `awaiting` when it is blocked on a
/// rendered question, `idle` at its prompt; nothing for plain shells.
pub fn activity(observation: &Observation, prompt_waiting: bool) -> Option<&'static str> {
    observation.agent?;
    Some(if prompt_waiting {
        "awaiting"
    } else if observation.busy {
        "working"
    } else {
        "idle"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(shell: &str) -> TerminalSessionSummary {
        TerminalSessionSummary::native(
            "id".into(),
            "tab".into(),
            shell.into(),
            "C:\\work".into(),
            1,
            120,
            30,
        )
    }

    #[test]
    fn claude_footer_is_recognised_and_busy_state_follows_the_screen() {
        let screen = "❯ fix the tests\n\n· Thinking… (esc to interrupt)\n\n? for shortcuts        shift+tab to cycle";
        let observation = observe(&session("pwsh.exe"), None, screen);
        assert_eq!(observation.agent, Some(Agent::Claude));
        assert_eq!(observation.source, Some("screen"));
        assert!(observation.busy);
        assert_eq!(activity(&observation, false), Some("working"));

        let idle = observe(
            &session("pwsh.exe"),
            None,
            "❯ \n\n? for shortcuts   shift+tab to cycle",
        );
        assert_eq!(activity(&idle, false), Some("idle"));
        assert_eq!(activity(&idle, true), Some("awaiting"));
    }

    #[test]
    fn codex_prompt_and_status_line_are_recognised() {
        let screen = "› what should I do next?\n\ngpt-5.3 high · 87% context left   /approvals";
        let observation = observe(&session("pwsh.exe"), None, screen);
        assert_eq!(observation.agent, Some(Agent::Codex));
    }

    #[test]
    fn launch_command_is_the_fallback_and_a_plain_shell_has_no_agent() {
        let launched = observe(&session("C:\\tools\\claude.exe"), None, "PS C:\\work>");
        assert_eq!(launched.agent, Some(Agent::Claude));
        assert_eq!(launched.source, Some("command"));
        assert_eq!(activity(&launched, false), Some("idle"));

        let shell = observe(
            &session("pwsh.exe"),
            None,
            "PS C:\\work> dir\n1. src\n2. tests",
        );
        assert_eq!(shell.agent, None);
        assert_eq!(activity(&shell, false), None);
    }

    #[test]
    fn a_recognised_agent_survives_its_dialogs_but_not_a_shell_prompt() {
        let mut summary = session("pwsh.exe");
        summary.agent = Some("claude".into());
        summary.agent_source = Some("screen".into());
        let dialog =
            "Do you want to proceed?\n❯ 1. Yes\n  2. No\n\nEnter to confirm · Esc to cancel";
        let observation = observe(&summary, None, dialog);
        assert_eq!(observation.agent, Some(Agent::Claude));
        assert_eq!(observation.source, Some("screen"));

        let exited = observe(&summary, None, "Goodbye!\n\nPS C:\\work>");
        assert_eq!(exited.agent, None);
    }

    #[test]
    fn handshake_outranks_the_screen() {
        let observation = observe(
            &session("pwsh.exe"),
            Some(Agent::Codex),
            "❯ \n? for shortcuts  shift+tab to cycle",
        );
        assert_eq!(observation.agent, Some(Agent::Codex));
        assert_eq!(observation.source, Some("osc"));
    }
}
