//! Caveman mode: terse replies that keep every technical fact (rules adapted from
//! <https://github.com/JuliusBrussee/caveman>, see `rules/NOTICE`). `[caveman]` in `~/.bro/v2.toml`.
//!
//! bro applies it per launch and installs nothing in the harness: no plugin, no Node,
//! no edits to `settings.json`.
//!
//! * **Claude Code**: the rules go in the system prompt (`--append-system-prompt-file`), so
//!   they hold on every request and survive compaction. A `--settings` file adds two hooks
//!   that `cat` a ready-made JSON reply: `SubagentStart` gives subagents the rules, and
//!   `SessionStart` (resume) restates the mode, because Claude Code reuses a resumed
//!   conversation's recorded system prompt until it is compacted. `CAVEMAN_DEFAULT_MODE=off`
//!   keeps an installed upstream plugin from injecting a second copy.
//! * **Codex**: `-c developer_instructions=…`, after any the login's `config.toml` already has.
//! * **Pi / omp**: `--append-system-prompt <rules file>`.
use crate::util::{atomic_write, short_hash};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Terse full sentences: answer first, no filler.
    #[default]
    Caveman,
    /// Fragments, each fact once.
    Ultracave,
    /// Classical Chinese (文言文).
    Megacave,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Caveman, Level::Ultracave, Level::Megacave];

    pub fn id(self) -> &'static str {
        match self {
            Level::Caveman => "caveman",
            Level::Ultracave => "ultracave",
            Level::Megacave => "megacave",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Level::Caveman => "answer first, no filler, every fact kept",
            Level::Ultracave => "fragments, each fact once: shortest replies",
            Level::Megacave => "replies in Classical Chinese (文言文); code stays as is",
        }
    }

    /// The rules text the agent gets.
    pub fn rules(self) -> &'static str {
        match self {
            Level::Caveman => include_str!("rules/caveman.md"),
            Level::Ultracave => include_str!("rules/ultracave.md"),
            Level::Megacave => include_str!("rules/megacave.md"),
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.id() == s)
    }
}

/// `[caveman]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CavemanSettings {
    pub on: bool,
    /// Kept while off, so turning it back on restores it.
    pub level: Level,
    /// Claude Code subagents get the rules too (their reports to the main agent get shorter).
    pub subagents: bool,
    /// Has ever been on. Then a Claude session resumed with it off is told so, since the
    /// conversation may still carry the rules from the launch that started it.
    pub used: bool,
}

impl Default for CavemanSettings {
    fn default() -> Self {
        Self { on: false, level: Level::Caveman, subagents: true, used: false }
    }
}

impl CavemanSettings {
    /// The level new launches get, or None when off.
    pub fn active(&self) -> Option<Level> {
        self.on.then_some(self.level)
    }

    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        self.used |= on;
    }

    /// "off", or the level's id.
    pub fn summary(&self) -> &'static str {
        self.active().map_or("off", Level::id)
    }
}

/// What a resumed Claude session is told when bro's caveman mode is off.
const OFF_NOTE: &str = "Caveman mode is now off for this session (turned off in bro). Any earlier instruction to reply in \
caveman, ultracave or megacave style no longer applies: write normal prose.";

/// `~/.bro/caveman`: the rules and hook replies bro hands to launches.
pub fn dir() -> PathBuf {
    crate::paths::bro_dir().join("caveman")
}

/// Write `bytes` unless the file already holds exactly them.
fn ensure(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|b| b == bytes) {
        return Ok(());
    }
    atomic_write(path, bytes)
}

/// `~/.bro/caveman/<level>.md`, written from the built-in text.
pub fn rules_file(level: Level) -> std::io::Result<PathBuf> {
    let path = dir().join(format!("{}.md", level.id()));
    ensure(&path, level.rules().as_bytes())?;
    Ok(path)
}

/// A path for a hook command: forward slashes so bash (Git Bash on Windows) and PowerShell
/// both read it, double-quoted.
fn shell_path(p: &Path) -> String {
    format!("\"{}\"", p.to_string_lossy().replace('\\', "/"))
}

/// A hook that prints a JSON file holding `additionalContext` for `event`.
fn cat_hook(event: &str, name: &str, context: &str) -> std::io::Result<serde_json::Value> {
    let reply = dir().join(name);
    let body = json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": context } });
    ensure(&reply, serde_json::to_string(&body).unwrap_or_default().as_bytes())?;
    Ok(json!({ "type": "command", "command": format!("cat {}", shell_path(&reply)), "timeout": 10 }))
}

/// The `--settings` file for a Claude launch: `base` (bro's own Claude settings, e.g. focus
/// view) plus hooks that hand subagents the rules and restate the mode on resume. None when
/// nothing needs saying (empty `base` and caveman off and never used, or off and a fresh session).
pub fn claude_settings_file(s: &CavemanSettings, resuming: bool, base: serde_json::Map<String, serde_json::Value>) -> std::io::Result<Option<PathBuf>> {
    let mut hooks = serde_json::Map::new();
    match s.active() {
        Some(level) => {
            let id = level.id();
            if s.subagents {
                let h = cat_hook("SubagentStart", &format!("subagent-{id}.json"), level.rules())?;
                hooks.insert("SubagentStart".into(), json!([{ "hooks": [h] }]));
            }
            let note = format!("Caveman mode ({id}) is on for this session, set in bro. Follow these rules from now on:\n\n{}", level.rules());
            let h = cat_hook("SessionStart", &format!("resume-{id}.json"), &note)?;
            hooks.insert("SessionStart".into(), json!([{ "matcher": "resume", "hooks": [h] }]));
        }
        None if s.used && resuming => {
            let h = cat_hook("SessionStart", "resume-off.json", OFF_NOTE)?;
            hooks.insert("SessionStart".into(), json!([{ "matcher": "resume", "hooks": [h] }]));
        }
        None => {}
    }
    if hooks.is_empty() && base.is_empty() {
        return Ok(None);
    }
    let mut settings = base;
    // next to the hooks `base` brings (bro's peer hooks, see `crate::peers`)
    if !hooks.is_empty()
        && let Some(all) = settings.entry("hooks").or_insert_with(|| json!({})).as_object_mut()
    {
        for (event, list) in hooks {
            match all.get_mut(&event).and_then(serde_json::Value::as_array_mut) {
                Some(have) => have.extend(list.as_array().cloned().unwrap_or_default()),
                None => {
                    all.insert(event, list);
                }
            }
        }
    }
    let body = serde_json::to_string_pretty(&settings).unwrap_or_default();
    let path = dir().join(format!("claude-settings-{}.json", short_hash(&body, 12)));
    ensure(&path, body.as_bytes())?;
    Ok(Some(path))
}

/// Combine several system-prompt appendices into one file (Claude Code keeps only the last
/// `--append-system-prompt-file`). Content-addressed under `~/.bro/caveman/prompts`.
pub fn combined_prompt_file(parts: &[String]) -> std::io::Result<PathBuf> {
    let body = parts.iter().map(|p| p.trim_end()).collect::<Vec<_>>().join("\n\n") + "\n";
    let path = dir().join("prompts").join(format!("{}.md", short_hash(&body, 16)));
    ensure(&path, body.as_bytes())?;
    Ok(path)
}

/// The upstream Caveman plugin or hooks are installed in this Claude config dir.
pub fn upstream_installed(config_dir: &Path) -> bool {
    let has = |file: &str, needle: &str| std::fs::read_to_string(config_dir.join(file)).is_ok_and(|s| s.contains(needle));
    has("plugins/installed_plugins.json", "caveman@") || has("settings.json", "caveman-activate")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn levels_round_trip_and_carry_rules() {
        for l in Level::ALL {
            assert_eq!(Level::parse(l.id()), Some(l));
            assert!(l.rules().contains("## Persistence"), "{}", l.id());
            assert!(!l.rules().contains("/caveman status"), "{} still mentions the upstream status hook", l.id());
        }
    }

    #[test]
    fn toggling_remembers_level_and_use() {
        let mut s = CavemanSettings { level: Level::Ultracave, ..Default::default() };
        assert_eq!(s.active(), None);
        s.set_on(true);
        assert_eq!(s.active(), Some(Level::Ultracave));
        s.set_on(false);
        assert!(s.used);
        assert_eq!(s.summary(), "off");
    }

    #[test]
    fn rules_file_is_written_once() {
        let _sb = sandbox();
        let p = rules_file(Level::Caveman).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), Level::Caveman.rules());
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        rules_file(Level::Caveman).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);
    }

    fn hooks(p: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    /// The JSON a hook command prints.
    fn reply(hook: &serde_json::Value) -> serde_json::Value {
        let cmd = hook["command"].as_str().unwrap();
        let path = cmd.strip_prefix("cat \"").unwrap().strip_suffix('"').unwrap();
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn settings_file_hooks_subagents_and_resume() {
        let _sb = sandbox();
        let mut s = CavemanSettings { level: Level::Ultracave, ..Default::default() };
        assert!(claude_settings_file(&s, true, Default::default()).unwrap().is_none(), "never used: nothing to say");
        s.set_on(true);
        let v = hooks(&claude_settings_file(&s, false, Default::default()).unwrap().unwrap());
        let sub = reply(&v["hooks"]["SubagentStart"][0]["hooks"][0]);
        assert_eq!(sub["hookSpecificOutput"]["hookEventName"], "SubagentStart");
        assert_eq!(sub["hookSpecificOutput"]["additionalContext"], Level::Ultracave.rules());
        assert_eq!(v["hooks"]["SessionStart"][0]["matcher"], "resume");
        let res = reply(&v["hooks"]["SessionStart"][0]["hooks"][0]);
        assert!(res["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("(ultracave) is on"));

        s.subagents = false;
        let v = hooks(&claude_settings_file(&s, false, Default::default()).unwrap().unwrap());
        assert!(v["hooks"].get("SubagentStart").is_none());

        s.set_on(false);
        assert!(claude_settings_file(&s, false, Default::default()).unwrap().is_none(), "off, fresh session: nothing");
        let v = hooks(&claude_settings_file(&s, true, Default::default()).unwrap().unwrap());
        let res = reply(&v["hooks"]["SessionStart"][0]["hooks"][0]);
        assert!(res["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("now off"));
    }

    #[test]
    fn combined_prompt_is_content_addressed() {
        let _sb = sandbox();
        let a = combined_prompt_file(&["one".into(), "two\n".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\n\ntwo\n");
        assert_eq!(combined_prompt_file(&["one".into(), "two".into()]).unwrap(), a);
        assert_ne!(combined_prompt_file(&["two".into(), "one".into()]).unwrap(), a);
    }

    #[test]
    fn detects_upstream_install() {
        let sb = sandbox();
        let dir = sb.home().join("cfg");
        assert!(!upstream_installed(&dir));
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        std::fs::write(dir.join("plugins/installed_plugins.json"), r#"{"plugins":{"caveman@caveman":[]}}"#).unwrap();
        assert!(upstream_installed(&dir));
    }
}
