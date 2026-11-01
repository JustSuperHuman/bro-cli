//! Prompts read from the live terminal, shared by desktop and mobile.
//! Numbered single-select menus can be drawn as buttons. Other interactive
//! forms keep their native terminal controls.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub question: String,
    pub context: Vec<String>,
    pub options: Vec<Opt>,
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    pub key: String,
    pub label: String,
    pub detail: Option<String>,
    pub focused: bool,
}

impl Choice {
    /// Cursor movement does not change the request; a new command, question,
    /// option or description does.
    pub fn same_request(&self, other: &Self) -> bool {
        self.question == other.question
            && self.context == other.context
            && self.options.len() == other.options.len()
            && self
                .options
                .iter()
                .zip(&other.options)
                .all(|(a, b)| a.key == b.key && a.label == b.label && a.detail == b.detail)
    }

    /// Move the visible cursor, then commit exactly once. A digit followed by
    /// Enter can answer two questions when the harness commits digits itself.
    pub fn response(&self, key: &str, application_cursor: bool) -> Option<String> {
        let target = self.options.iter().position(|o| o.key == key)?;
        let current = self.options.iter().position(|o| o.focused)?;
        let delta = target as isize - current as isize;
        let arrow = match (delta < 0, application_cursor) {
            (true, true) => "\x1bOA",
            (false, true) => "\x1bOB",
            (true, false) => "\x1b[A",
            (false, false) => "\x1b[B",
        };
        Some(format!("{}\r", arrow.repeat(delta.unsigned_abs())))
    }
}

fn unframed(row: &str) -> String {
    const FRAME: &[char] = &['│', '┃', '║', '╭', '╰', '╮', '╯', '▌', '▐'];
    let t = row.trim_end_matches(|c: char| FRAME.contains(&c) || c.is_whitespace());
    let edge = t.trim_start();
    if edge.starts_with(|c: char| FRAME.contains(&c)) {
        edge.trim_start_matches(|c: char| FRAME.contains(&c))
            .to_string()
    } else {
        t.to_string()
    }
}

fn indent(row: &str) -> usize {
    row.chars().take_while(|c| c.is_whitespace()).count()
}

pub fn option_key(line: &str) -> Option<String> {
    let rest = line.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '❯' | '>' | '›' | '●' | '○' | '│' | '┃' | '║')
    });
    let (number, label) = rest.split_once(['.', ')'])?;
    (!number.is_empty()
        && number.len() <= 2
        && number.chars().all(|c| c.is_ascii_digit())
        && !label.trim().is_empty())
    .then(|| number.to_string())
}

fn option(row: &str) -> Option<(String, usize, String, bool)> {
    let key = option_key(row)?;
    let col = row.find(&*key)?;
    let focused = row[..col]
        .chars()
        .any(|c| matches!(c, '❯' | '›' | '>' | '●'));
    Some((
        key.clone(),
        row[..col].chars().count(),
        row[col + key.len() + 1..].trim().to_string(),
        focused,
    ))
}

fn is_hint(row: &str) -> bool {
    let l = row.trim().to_lowercase();
    l.starts_with("tab to ")
        || [
            "enter to",
            "esc to",
            "arrow keys",
            "to navigate",
            "to cancel",
            "to select",
            "to confirm",
            "ctrl+g to edit in",
            "enter select",
            "enter confirm",
            "enter continue",
        ]
        .iter()
        .any(|m| l.contains(m))
}

fn is_rule(row: &str) -> bool {
    let t = row.trim();
    !t.is_empty()
        && t.chars().all(|c| {
            matches!(
                c,
                '─' | '━'
                    | '═'
                    | '-'
                    | '╌'
                    | '┄'
                    | '╭'
                    | '╮'
                    | '╰'
                    | '╯'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
            )
        })
}

fn lines(rows: &[String]) -> Vec<String> {
    rows.iter().map(|r| unframed(r)).collect()
}

fn dialog_end(lines: &[String]) -> usize {
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    // Claude can draw a named session divider and its counted task panel below
    // the controls. Only that bounded trailer is allowed, never arbitrary output.
    if let Some(divider) = (0..end)
        .rev()
        .find(|&i| lines[i].trim().starts_with("───") && !is_rule(&lines[i]))
    {
        let previous = lines[..divider].iter().rev().find(|l| !l.trim().is_empty());
        let trailer = &lines[divider + 1..end];
        let task_panel = trailer.iter().all(|l| {
            let l = l.trim();
            l.is_empty()
                || l.starts_with(['◻', '◼', '✔'])
                || l.starts_with("… +")
                || (l.starts_with(|c: char| c.is_ascii_digit())
                    && l.contains(" task")
                    && l.contains("done")
                    && l.contains("open"))
        });
        if previous.is_some_and(|l| is_hint(l)) && task_panel {
            return divider;
        }
    }
    end
}

fn complex(lines: &[String]) -> bool {
    let lower = lines
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    // These controls belong to checkbox forms, interview editors, MCP forms,
    // or Codex's question stack. Keep the actual editor visible and usable.
    (lower.contains("space")
        && (lower.contains("to toggle")
            || lower.contains("to select")
            || lower.contains("space select")))
        || lower.contains("enter to submit answer")
        || lower.contains("enter to submit all")
        || lower.contains("enter submit")
        || lower.contains("to add notes")
        || lower.contains("to clear notes")
        || lower.contains("requests your input")
        || lower.contains("review your answers")
        || lines.iter().any(|l| {
            let t =
                l.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '❯' | '›' | '>'));
            t.starts_with("[ ]")
                || t.starts_with("[x]")
                || t.starts_with("[✓]")
                || option(l).is_some_and(|o| {
                    let label = o.2.to_lowercase();
                    label.contains("(disabled)")
                        || label.contains("(unavailable)")
                        || label.starts_with("[ ]")
                        || label.starts_with("[x]")
                        || label.starts_with("[✓]")
                        || label.starts_with(['☐', '☑', '◻', '◼'])
                })
                || (l.trim_start().starts_with(['❯', '›', '>'])
                    && (t.starts_with("Type your answer") || t.starts_with("Add notes")))
        })
}

/// A complete, focused single-select menu at the live bottom of the screen.
pub fn choice(rows: &[String]) -> Option<Choice> {
    let lines = lines(rows);
    let end = dialog_end(&lines);
    let start = end.saturating_sub(64);
    if complex(&lines[start..end]) {
        return None;
    }
    let opts: Vec<usize> = (start..end)
        .filter(|&i| option(&lines[i]).is_some())
        .collect();
    let &last = opts.last()?;
    let col = option(&lines[last])?.1;
    if !(last + 1..end).all(|i| {
        lines[i].trim().is_empty()
            || is_hint(&lines[i])
            || is_rule(&lines[i])
            || indent(&lines[i]) > col + 1
    }) {
        return None;
    }
    let mut group = vec![last];
    let mut at = last;
    while let Some(&prev) = opts.iter().rev().find(|&&j| j < at) {
        let col = option(&lines[prev])?.1;
        if (prev + 1..at).all(|k| lines[k].trim().is_empty() || indent(&lines[k]) > col + 1) {
            group.push(prev);
            at = prev;
        } else {
            break;
        }
    }
    group.reverse();
    let mut options = vec![];
    for (n, &i) in group.iter().enumerate() {
        let (key, col, label, focused) = option(&lines[i])?;
        let until = group.get(n + 1).copied().unwrap_or(end);
        let detail: Vec<&str> = (i + 1..until)
            .map(|k| lines[k].as_str())
            .take_while(|l| !l.trim().is_empty())
            .filter(|l| indent(l) > col + 1 && !is_hint(l))
            .map(str::trim)
            .collect();
        options.push(Opt {
            key,
            label,
            detail: (!detail.is_empty()).then(|| detail.join(" ")),
            focused,
        });
    }
    if options.len() < 2
        || !options
            .iter()
            .enumerate()
            .all(|(i, o)| o.key == (i + 1).to_string())
        || !options.iter().any(|o| o.focused)
    {
        return None;
    }
    let mut q = group[0];
    while q > start && lines[q - 1].trim().is_empty() {
        q -= 1;
    }
    let mut question_start = q.saturating_sub(1);
    while question_start > start
        && !lines[question_start - 1].trim().is_empty()
        && !is_rule(&lines[question_start - 1])
    {
        question_start -= 1;
    }
    let question = lines[question_start..q]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let mut context = vec![];
    let mut k = question_start;
    while k > start && context.len() < 6 {
        k -= 1;
        if is_rule(&lines[k]) {
            break;
        }
        if !lines[k].trim().is_empty() {
            context.push(lines[k].trim().to_string());
        }
    }
    context.reverse();
    let hint = (last + 1..end)
        .map(|i| lines[i].trim())
        .find(|l| is_hint(l))
        .map(str::to_string);
    Some(Choice {
        question,
        context,
        options,
        hint,
    })
}

/// Native editors and menus still need attention even when there is no button
/// card. Ordinary prose, old menus above an input prompt, and a busy status do not.
pub fn waiting(rows: &[String]) -> bool {
    if choice(rows).is_some() {
        return true;
    }
    let lines = lines(rows);
    let end = dialog_end(&lines);
    let tail = &lines[end.saturating_sub(32)..end];
    let nonempty: Vec<_> = tail
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !is_rule(l))
        .collect();
    let Some(last) = nonempty.last() else {
        return false;
    };
    let lower = nonempty.join(" ").to_lowercase();
    // A newer empty composer supersedes old question text above it. Native
    // question editors have an explicit submit/cancel footer after their input.
    if let Some(input) = nonempty.iter().rposition(|l| matches!(*l, "❯" | "›" | ">"))
        && !nonempty[input + 1..].iter().any(|l| control_footer(l))
    {
        return false;
    }
    if control_footer(last) {
        return true;
    }
    // In narrow panes a footer can wrap onto rows with no key at their start.
    for count in 2..=3.min(nonempty.len()) {
        let footer = &nonempty[nonempty.len() - count..];
        if footer[1..].iter().all(|l| {
            l.split_whitespace().all(|w| {
                let word = w
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_ascii_lowercase();
                word.is_empty()
                    || [
                        "enter",
                        "esc",
                        "tab",
                        "space",
                        "to",
                        "or",
                        "submit",
                        "answer",
                        "all",
                        "cancel",
                        "interrupt",
                        "confirm",
                        "select",
                        "toggle",
                        "clear",
                        "add",
                        "notes",
                        "navigate",
                        "arrow",
                        "keys",
                        "main",
                        "prompt",
                        "skip",
                        "continue",
                        "question",
                        "prev",
                        "next",
                    ]
                    .contains(&word.as_str())
            })
        }) && control_footer(&footer.join(" "))
        {
            return true;
        }
    }
    if last.ends_with("to answer")
        && (last.contains("alt+")
            || last.contains("⌥")
            || last.contains("shift+")
            || last.contains("⇧"))
    {
        return true;
    }
    if ["(y/n)", "[y/n]", "(y/n)?", "[y/n]?"]
        .iter()
        .any(|m| last.to_lowercase().ends_with(m))
    {
        return true;
    }
    if lower.contains("requests your input") && lower.contains("esc to cancel") {
        return true;
    }
    // A clipped menu may expose just one of its numbered rows.
    nonempty
        .iter()
        .rev()
        .take(8)
        .any(|l| option(l).is_some_and(|o| o.3))
        && !last.to_lowercase().contains("esc to interrupt")
}

fn control_footer(line: &str) -> bool {
    let l = line.trim().to_lowercase();
    let starts = [
        "enter ",
        "press enter ",
        "esc ",
        "tab ",
        "ctrl+",
        "shift+",
        "space ",
        "use arrow",
        "arrow",
        "↑",
        "←",
        "⏎",
    ]
    .iter()
    .any(|p| l.starts_with(p));
    starts
        && [
            "to confirm",
            "to select",
            "to submit",
            "to cancel",
            "to continue",
            "to close",
            "to toggle",
            "to amend",
            "to explain",
            "to edit in",
            "enter submit",
            "enter select",
            "enter continue",
            "esc cancel",
            "space select",
        ]
        .iter()
        .any(|p| l.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn single_select_accepts_frames_wrapping_parentheses_and_any_cursor() {
        for cursor in 1..=3 {
            let screen = format!(
                "╭────────────────────╮\n│ Approve this plan? │\n│ {} 1) Approve │\n│      Keep the tests │\n│ {} 2) Edit │\n│ {} 3) Cancel │\n│ Enter to confirm · Esc to cancel │\n╰────────────────────╯",
                if cursor == 1 { "❯" } else { " " },
                if cursor == 2 { "❯" } else { " " },
                if cursor == 3 { "❯" } else { " " }
            );
            let c = choice(&rows(&screen)).unwrap();
            assert!(c.options[cursor - 1].focused);
            assert_eq!(c.options[0].detail.as_deref(), Some("Keep the tests"));
            assert!(waiting(&rows(&screen)));
            let indented = screen
                .lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                choice(&rows(&indented)).unwrap(),
                c,
                "the frame's outer margin is not option indentation"
            );
        }
    }

    #[test]
    fn native_forms_remain_actionable_without_fake_single_select_buttons() {
        for screen in [
            "Which features?\n❯ [ ] Logging\n  [x] Metrics\nSpace to toggle · Enter to confirm · Esc to cancel",
            "Which features?\n❯ 1. [ ] Logging\n  2. [x] Metrics\nSpace to select · Enter to confirm · Esc to cancel",
            "Which features?\n❯ 1. [ ] Logging\n  2. [X] Metrics\nEnter to confirm · Esc to cancel",
            "MCP server \"demo\" requests your input\nName: \n❯ Accept\n  Decline\nEsc to cancel",
            "Question 1/2\nWhere should files go?\n› Type your answer\nenter to submit answer | esc to interrupt",
            "Question 1/2\nWhere should files go?\n› Type your answer\nenter to submit\nanswer | esc to interrupt",
            "Trust this folder?\n❯ Trust and continue\n  Exit\nPress enter to continue",
            "Question 1/2\nChoose one\n› 1. One\n  2. Two\ntab to add notes | enter to submit answer | esc to interrupt",
            "1 of 2\nChoose one\n› 1. One\n  2. Other\nenter submit · esc skip · tab main prompt",
            "? 2 questions\nalt+up to answer",
            "Update available · 1.0 to 2.0\n❯ Update now\n  Skip\nenter continue · esc skip",
            "Type a response\n❯\nEnter to confirm · Esc to cancel",
        ] {
            assert!(
                choice(&rows(screen)).is_none(),
                "must use the native editor: {screen}"
            );
            assert!(waiting(&rows(screen)), "must stay visible: {screen}");
        }
    }

    #[test]
    fn stale_menus_and_numbered_prose_are_not_actionable() {
        for screen in [
            "Steps:\n1. Read\n2. Edit",
            "Continue?\n❯ 1. Yes\n  2. No\nEnter to confirm\n❯",
            "Continue?\n❯ 1. Yes\n  2. No\nEnter to confirm\n❯\n⏵⏵ bypass permissions on (shift+tab to cycle)",
            "The documentation says Enter to confirm and Esc to cancel.",
            "Working again\nesc to interrupt",
        ] {
            assert!(!waiting(&rows(screen)), "{screen}");
            assert!(choice(&rows(screen)).is_none(), "{screen}");
        }
    }

    #[test]
    fn counted_task_trailers_do_not_hide_a_live_menu() {
        let screen = "Continue?\n❯ 1. Yes\n  2. No\nEnter to confirm · Esc to cancel\n─── My session ───\n  2 tasks (1 done, 1 open)\n  ✔ Read the code\n  ◻ Fix the prompt";
        assert!(choice(&rows(screen)).is_some());
        assert!(choice(&rows(&format!("{screen}\nNew agent output"))).is_none());
    }

    #[test]
    fn answering_moves_the_cursor_and_commits_once_in_both_terminal_modes() {
        let c = choice(&rows("Pick one\n❯ 1. One\n  2. Two\n  3. Three")).unwrap();
        assert_eq!(c.response("1", false).as_deref(), Some("\r"));
        assert_eq!(c.response("3", false).as_deref(), Some("\x1b[B\x1b[B\r"));
        assert_eq!(c.response("2", true).as_deref(), Some("\x1bOB\r"));
        let next = choice(&rows("Pick one\n  1. One\n❯ 2. Two\n  3. Three")).unwrap();
        assert!(c.same_request(&next));
        assert_eq!(next.response("1", false).as_deref(), Some("\x1b[A\r"));
        assert!(
            !c.same_request(
                &choice(&rows(
                    "A different question\n❯ 1. One\n  2. Two\n  3. Three"
                ))
                .unwrap()
            )
        );
        assert!(c.response("4", false).is_none());
    }
}
