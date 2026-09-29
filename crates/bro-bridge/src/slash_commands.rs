// The slash commands a session will actually accept: each agent's built-ins
// plus the custom commands defined in the project and in the user's home. A
// remote client (the mobile composer) cannot see the agent's own popup, so the
// catalog is assembled here and served as data. Port of the Node host's
// `server/slash-commands.ts`.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(10);
const MAX_CUSTOM_COMMANDS: usize = 300;
const MAX_DEPTH: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SlashCommandSource {
    Builtin,
    Project,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlashCommand {
    /// Without the leading slash, e.g. "compact" or "review:security".
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Hint for what follows the command, e.g. "[pr-number]".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    pub source: SlashCommandSource,
}

type Builtin = (&'static str, &'static str, Option<&'static str>);

const CLAUDE_BUILTINS: &[Builtin] = &[
    ("add-dir", "Add another working directory", Some("<path>")),
    ("agents", "Manage subagents", None),
    ("bug", "Report a bug to Anthropic", None),
    ("clear", "Clear the conversation history", None),
    (
        "compact",
        "Summarise the conversation to free context",
        Some("[focus]"),
    ),
    ("config", "Open the config panel", None),
    ("context", "Show what is using the context window", None),
    ("cost", "Show token usage and cost", None),
    ("doctor", "Check the health of this installation", None),
    ("export", "Export the conversation", None),
    ("help", "List commands and shortcuts", None),
    ("hooks", "Manage hook configuration", None),
    ("ide", "Connect to an IDE", None),
    ("init", "Create or refresh CLAUDE.md", None),
    ("mcp", "Manage MCP servers", None),
    ("memory", "Edit the memory files", None),
    ("model", "Change the model", Some("[model]")),
    ("output-style", "Change the output style", None),
    ("permissions", "Manage tool permissions", None),
    ("pr-comments", "Fetch comments from a pull request", None),
    ("release-notes", "Show what changed in Claude Code", None),
    ("resume", "Resume an earlier conversation", None),
    ("review", "Review a pull request", Some("[pr]")),
    ("rewind", "Rewind the conversation or the code", None),
    (
        "security-review",
        "Review the changes for vulnerabilities",
        None,
    ),
    ("status", "Show version, account and connectivity", None),
    ("statusline", "Configure the status line", None),
    ("todos", "Show the current todo list", None),
    ("usage", "Show plan usage limits", None),
    ("vim", "Toggle vim editing mode", None),
];

const CODEX_BUILTINS: &[Builtin] = &[
    ("new", "Start a new chat", None),
    ("init", "Create an AGENTS.md for this repo", None),
    (
        "compact",
        "Summarise the conversation to free context",
        None,
    ),
    ("diff", "Show the git diff, including untracked files", None),
    ("mention", "Mention a file", Some("<file>")),
    ("status", "Show session configuration and token usage", None),
    ("model", "Choose the model and reasoning effort", None),
    ("approvals", "Choose what Codex can do without asking", None),
    ("review", "Review the current changes", None),
    ("undo", "Undo the last Codex edit", None),
    ("mcp", "List the configured MCP tools", None),
    ("logout", "Log out of Codex", None),
    ("quit", "Exit Codex", None),
];

fn builtins_for(agent: &str) -> Vec<SlashCommand> {
    let table = match agent {
        "claude" => CLAUDE_BUILTINS,
        "codex" => CODEX_BUILTINS,
        _ => &[],
    };
    table
        .iter()
        .map(|(name, description, hint)| SlashCommand {
            name: (*name).to_string(),
            description: Some((*description).to_string()),
            argument_hint: hint.map(str::to_string),
            source: SlashCommandSource::Builtin,
        })
        .collect()
}

struct FrontMatter<'a> {
    description: Option<String>,
    argument_hint: Option<String>,
    rest: &'a str,
}

fn strip_line_break(text: &str) -> Option<&str> {
    text.strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
}

// Claude's command files carry an optional YAML header. Only two keys matter
// for a picker, so this reads them directly instead of pulling in a parser.
fn parse_front_matter(body: &str) -> FrontMatter<'_> {
    let none = FrontMatter {
        description: None,
        argument_hint: None,
        rest: body,
    };
    let Some(after_open) = body.strip_prefix("---").and_then(strip_line_break) else {
        return none;
    };
    // The header ends at the first line that is exactly `---`.
    let mut offset = 0;
    let mut header_end = None;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" && offset > 0 {
            header_end = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((header_len, rest_start)) = header_end else {
        return none;
    };
    let header = &after_open[..header_len];

    let mut description = None;
    let mut argument_hint = None;
    for line in header.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
            continue;
        }
        let value = value.trim();
        let value = value.strip_prefix(['"', '\'']).unwrap_or(value);
        let value = value.strip_suffix(['"', '\'']).unwrap_or(value).to_string();
        match key.to_ascii_lowercase().as_str() {
            "description" => description = Some(value),
            "argument-hint" => argument_hint = Some(value),
            _ => {}
        }
    }

    FrontMatter {
        description,
        argument_hint,
        rest: &after_open[rest_start..],
    }
}

/// First readable prose line, used when a command file has no description.
fn summarise(body: &str) -> Option<String> {
    for line in body.lines() {
        let text = line.trim_start_matches('#').trim();
        if !text.is_empty() && !text.starts_with("---") {
            return Some(text.chars().take(120).collect());
        }
    }
    None
}

fn read_command_directory(root: &Path, source: SlashCommandSource) -> Vec<SlashCommand> {
    fn walk(
        directory: &Path,
        prefix: &str,
        depth: usize,
        source: SlashCommandSource,
        found: &mut Vec<SlashCommand>,
    ) {
        if depth > MAX_DEPTH || found.len() >= MAX_CUSTOM_COMMANDS {
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_CUSTOM_COMMANDS {
                return;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                // Nested folders namespace their commands, matching how the
                // agents address them: commands/review/api.md -> /review:api.
                walk(
                    &entry.path(),
                    &format!("{prefix}{name}:"),
                    depth + 1,
                    source,
                    found,
                );
                continue;
            }
            if !kind.is_file() || !name.to_ascii_lowercase().ends_with(".md") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let header = parse_front_matter(&body);
            found.push(SlashCommand {
                name: format!("{prefix}{}", &name[..name.len() - 3]),
                description: header.description.or_else(|| summarise(header.rest)),
                argument_hint: header.argument_hint,
                source,
            });
        }
    }

    let mut found = Vec::new();
    walk(root, "", 0, source, &mut found);
    found
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn collect(agent: &str, cwd: &str, home: Option<&Path>) -> Vec<SlashCommand> {
    let builtins = builtins_for(agent);
    let folder = match agent {
        "claude" => [".claude", "commands"],
        "codex" => [".codex", "prompts"],
        _ => return builtins,
    };

    let mut project = Vec::new();
    if !cwd.is_empty() {
        project = read_command_directory(
            &Path::new(cwd).join(folder[0]).join(folder[1]),
            SlashCommandSource::Project,
        );
    }
    let user = home
        .map(|home| {
            read_command_directory(
                &home.join(folder[0]).join(folder[1]),
                SlashCommandSource::User,
            )
        })
        .unwrap_or_default();

    // Project definitions shadow user ones, and both shadow a same-named
    // built-in — the same precedence the agents apply.
    let mut by_name: BTreeMap<String, SlashCommand> = BTreeMap::new();
    for command in builtins.into_iter().chain(user).chain(project) {
        by_name.insert(command.name.clone(), command);
    }
    let mut commands: Vec<SlashCommand> = by_name.into_values().collect();
    commands.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    commands
}

/// The agent's built-ins plus the project's and user's own command files,
/// cached briefly per agent + directory. Blocking: call off the async runtime.
pub(crate) fn list_slash_commands(agent: &str, cwd: &str) -> Vec<SlashCommand> {
    type Cache = Mutex<HashMap<String, (Instant, Vec<SlashCommand>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{agent} {cwd}");
    if let Some((at, commands)) = cache.lock().get(&key)
        && at.elapsed() < CACHE_TTL
    {
        return commands.clone();
    }
    let commands = collect(agent, cwd, home_dir().as_deref());
    let mut cache = cache.lock();
    cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
    cache.insert(key, (Instant::now(), commands.clone()));
    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn front_matter_reads_description_and_hint() {
        let parsed = parse_front_matter(
            "---\r\ndescription: \"Ship it\"\nargument-hint: [pr]\nother: x\n---\n# Body\n",
        );
        assert_eq!(parsed.description.as_deref(), Some("Ship it"));
        assert_eq!(parsed.argument_hint.as_deref(), Some("[pr]"));
        assert_eq!(parsed.rest, "# Body\n");
        assert!(parse_front_matter("no header").description.is_none());
    }

    #[test]
    fn summarise_skips_headings_marks() {
        assert_eq!(
            summarise("\n## Review the diff\nmore").as_deref(),
            Some("Review the diff")
        );
        assert_eq!(summarise("   \n"), None);
    }

    #[test]
    fn shell_sessions_have_no_commands() {
        assert!(collect("shell", "", None).is_empty());
    }

    #[test]
    fn collects_builtins_and_custom_commands_with_precedence() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let project_commands = project.path().join(".claude").join("commands");
        let user_commands = home.path().join(".claude").join("commands");
        fs::create_dir_all(project_commands.join("review")).unwrap();
        fs::create_dir_all(&user_commands).unwrap();
        fs::write(
            project_commands.join("deploy.md"),
            "---\ndescription: Deploy it\nargument-hint: <env>\n---\nbody",
        )
        .unwrap();
        fs::write(
            project_commands.join("review").join("api.md"),
            "# Review the API\n",
        )
        .unwrap();
        fs::write(project_commands.join("notes.txt"), "ignored").unwrap();
        fs::write(user_commands.join("deploy.md"), "user version").unwrap();
        fs::write(user_commands.join("mine.md"), "Mine").unwrap();
        fs::write(user_commands.join("clear.md"), "Custom clear").unwrap();

        let commands = collect(
            "claude",
            &project.path().to_string_lossy(),
            Some(home.path()),
        );
        let find = |name: &str| commands.iter().find(|c| c.name == name).cloned();

        let deploy = find("deploy").unwrap();
        assert_eq!(deploy.source, SlashCommandSource::Project);
        assert_eq!(deploy.argument_hint.as_deref(), Some("<env>"));
        assert_eq!(
            find("review:api").unwrap().description.as_deref(),
            Some("Review the API")
        );
        assert_eq!(find("mine").unwrap().source, SlashCommandSource::User);
        assert_eq!(find("clear").unwrap().source, SlashCommandSource::User);
        assert_eq!(find("compact").unwrap().source, SlashCommandSource::Builtin);
        assert!(find("notes").is_none());
        let names: Vec<_> = commands.iter().map(|c| c.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let json = serde_json::to_value(&deploy).unwrap();
        assert_eq!(json["argumentHint"], "<env>");
        assert_eq!(json["source"], "project");
    }

    #[test]
    fn codex_reads_prompts_folder() {
        let project = tempfile::tempdir().unwrap();
        let prompts = project.path().join(".codex").join("prompts");
        fs::create_dir_all(&prompts).unwrap();
        fs::write(prompts.join("triage.md"), "Triage the issue").unwrap();
        let commands = collect("codex", &project.path().to_string_lossy(), None);
        assert!(
            commands
                .iter()
                .any(|c| c.name == "triage" && c.source == SlashCommandSource::Project)
        );
        assert!(commands.iter().any(|c| c.name == "approvals"));
        assert!(!commands.iter().any(|c| c.name == "vim"));
    }
}
