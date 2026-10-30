//! A provider limit that stops a turn, distinct from an agent thinking or asking a question.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimit {
    pub title: String,
    /// The provider's words, including its reset/retry information. No inferred reset timestamp.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
}

fn clean(text: &str) -> String {
    text.replace('’', "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bounded(text: &str, length: usize) -> String {
    let mut out: String = text.chars().take(length).collect();
    if text.chars().count() > length {
        out.push('…');
    }
    out
}

fn body(line: &str) -> &str {
    line.trim()
        .trim_start_matches(|c: char| {
            matches!(
                c,
                '*' | '✻'
                    | '✽'
                    | '✶'
                    | '✳'
                    | '✢'
                    | '✺'
                    | '✹'
                    | '⚠'
                    | '\u{fe0f}'
                    | '!'
                    | '●'
                    | '•'
                    | '⏺'
                    | '✕'
                    | '×'
            )
        })
        .trim()
}

fn title(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    [
        ("weekly limit reached", "Weekly limit reached"),
        ("weekly usage limit reached", "Weekly limit reached"),
        ("5-hour limit reached", "5-hour limit reached"),
        ("5 hour limit reached", "5-hour limit reached"),
        ("session limit reached", "Session limit reached"),
        ("rate limit reached", "Rate limit reached"),
        ("rate limit exceeded", "Rate limit reached"),
        ("usage limit reached", "Usage limit reached"),
        ("usage limit exceeded", "Usage limit reached"),
        ("you've hit your usage limit", "Usage limit reached"),
        ("you have hit your usage limit", "Usage limit reached"),
        ("you've reached your usage limit", "Usage limit reached"),
        ("you've hit your limit", "Usage limit reached"),
        ("you have hit your limit", "Usage limit reached"),
        ("you're out of extra usage", "Usage limit reached"),
        ("credit balance is too low", "Insufficient credits"),
        ("insufficient credits", "Insufficient credits"),
        ("api error: 429", "Rate limit reached"),
        ("api error: rate_limit", "Rate limit reached"),
    ]
    .into_iter()
    .find_map(|(prefix, label)| {
        lower
            .strip_prefix(prefix)
            .filter(|tail| {
                tail.is_empty()
                    || tail.starts_with(|c: char| {
                        c.is_whitespace() || matches!(c, '.' | '·' | ':' | '(' | '—' | '-')
                    })
            })
            .map(|_| label)
    })
}

impl UsageLimit {
    /// The harness is already scheduling its own next attempt.
    pub fn is_retrying(&self) -> bool {
        let text = self.message.to_ascii_lowercase();
        text.contains("retrying in ") || text.contains("retrying at ")
    }
    fn message(text: &str, label: &str) -> Self {
        let text = clean(text);
        let lower = text.to_ascii_lowercase();
        let retry = [
            "retrying in ",
            "retrying at ",
            "resets ",
            "reset at ",
            "try again ",
            "available again ",
        ]
        .into_iter()
        .filter_map(|needle| lower.find(needle))
        .min()
        .map(|at| {
            text[at..]
                .split(" · attempt ")
                .next()
                .unwrap_or(&text[at..])
                .trim()
                .to_string()
        });
        Self {
            title: label.into(),
            message: bounded(&text, 320),
            retry: retry.map(|text| bounded(&text, 180)),
        }
    }

    /// Only the current screen's bottom rows. Quoted prompts, progress percentages and historical scrollback
    /// are not limit banners. New agent output below a banner clears it even if the old line is still visible.
    pub fn from_screen(screen: &str) -> Option<Self> {
        let rows: Vec<_> = screen
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        // Anything typed into the current composer belongs to the user, even if it names a limit.
        let end = rows
            .iter()
            .rposition(|line| line.trim_start().starts_with(['❯', '›']))
            .unwrap_or(rows.len());
        let rows = &rows[end.saturating_sub(20)..end];
        let mut result = None;
        let mut fenced = false;
        for (index, line) in rows.iter().enumerate() {
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            let text = clean(body(line));
            if let Some(label) = title(&text) {
                let mut message = text;
                for next in rows.iter().skip(index + 1).take(4) {
                    let next = clean(next);
                    let lower = next.to_ascii_lowercase();
                    let continuation = [
                        "retrying ",
                        "resets ",
                        "reset at ",
                        "try again ",
                        "available again ",
                        "attempt ",
                        "(",
                    ]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix))
                        || message.to_ascii_lowercase().ends_with(" in")
                        || message.to_ascii_lowercase().ends_with(" at")
                        || message.ends_with('·');
                    if !continuation || next.starts_with(['❯', '›', '>', '⏵', '─', '━']) {
                        break;
                    }
                    message.push(' ');
                    message.push_str(&next);
                }
                result = Some(Self::message(&message, label));
            } else if result.is_some() {
                let trimmed = line.trim();
                let decorated = body(trimmed) != trimmed;
                let lower = text.to_ascii_lowercase();
                if decorated
                    && !lower.starts_with("you've used")
                    && !lower.starts_with("attempt ")
                    && !lower.starts_with("auto-update")
                    && !lower.starts_with("retrying ")
                {
                    result = None;
                }
            }
        }
        result
    }

    /// Structured errors can supply a limit before (or without) a terminal banner.
    pub fn from_error(code: Option<&str>, text: &str) -> Option<Self> {
        let text = clean(body(text));
        if let Some(label) = title(&text) {
            return Some(Self::message(&text, label));
        }
        let code = code.unwrap_or_default().to_ascii_lowercase();
        let label = if code.contains("rate_limit") || code == "429" {
            "Rate limit reached"
        } else if code.contains("usage_limit") || code.contains("quota") {
            "Usage limit reached"
        } else {
            return None;
        };
        Some(Self::message(
            if text.is_empty() { label } else { &text },
            label,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_retry_banner_preserves_when_and_does_not_mean_thinking() {
        let screen = "● Finding the website's share code\n✻ Weekly limit reached · Retrying in 5h (Nov 1, 7pm) · attempt 1/30\nYou've used 99% of your weekly limit\n❯\n⏵⏵ bypass permissions on (shift+tab to cycle) · esc to interrupt";
        let limit = UsageLimit::from_screen(screen).unwrap();
        assert_eq!(limit.title, "Weekly limit reached");
        assert_eq!(limit.retry.as_deref(), Some("Retrying in 5h (Nov 1, 7pm)"));
        assert!(limit.message.contains("attempt 1/30"));
        assert!(limit.is_retrying());
        assert!(
            !UsageLimit::from_screen("You've hit your limit · resets 7pm")
                .unwrap()
                .is_retrying()
        );
    }

    #[test]
    fn wrapped_resets_and_other_provider_limits_are_kept() {
        let limit = UsageLimit::from_screen("✻ Weekly limit reached ·\nRetrying in\n5h (Nov 1, 7pm) · attempt 2/30\n❯\nesc to interrupt").unwrap();
        assert_eq!(limit.retry.as_deref(), Some("Retrying in 5h (Nov 1, 7pm)"));
        for text in [
            "You've hit your limit · resets 7pm (America/New_York)",
            "You’ve hit your usage limit. Try again at 7pm.",
            "Rate limit reached · Retrying in 30s",
            "API Error: 429 Too many requests",
            "Credit balance is too low",
        ] {
            assert!(UsageLimit::from_screen(text).is_some(), "{text}");
        }
    }

    #[test]
    fn warnings_quoted_text_and_resumed_work_are_not_limits() {
        for screen in [
            "You've used 99% of your weekly limit",
            "Weekly limit is almost reached",
            "❯ Explain Weekly limit reached",
            "❯ Explain this:\nWeekly limit reached",
            "The server returned a weekly limit reached error",
            "`Weekly limit reached`",
            "```\nWeekly limit reached\n```",
            "Weekly limit reachedness",
            "✻ Weekly limit reached · Retrying in 5h\n● Read src/app.ts\n✻ Thinking…\n❯\nesc to interrupt",
        ] {
            assert!(UsageLimit::from_screen(screen).is_none(), "{screen}");
        }
        let old = format!("✻ Weekly limit reached\n{}", "new output\n".repeat(21));
        assert!(UsageLimit::from_screen(&old).is_none());
    }

    #[test]
    fn structured_errors_are_classified_without_mislabeling_other_errors() {
        assert_eq!(
            UsageLimit::from_error(Some("rate_limit"), "Too many requests")
                .unwrap()
                .title,
            "Rate limit reached"
        );
        assert!(
            UsageLimit::from_error(Some("server_error"), "API Error: 529 overloaded").is_none()
        );
        assert!(UsageLimit::from_error(Some("authentication_failed"), "Please sign in").is_none());
    }
}
