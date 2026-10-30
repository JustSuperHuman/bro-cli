//! bro's Claude Code mod (`crates/bro-core/claude-mod`): a plugin of function hooks bro loads into
//! every Claude session it starts (`--plugin-dir`). `[claude_mod]` in `~/.bro/v2.toml`. Three parts:
//!
//! * **The link.** `/bro`: a pane of every session and limit; jump to one, rename this one. It goes
//!   through bro's bridge on loopback.
//! * **The questions.** A chime when Claude asks you something, and with the timer on, a countdown above the
//!   question that ticks for its last 15 seconds; at zero Claude goes with its best judgement. bro plays the
//!   sounds itself (`bro hook sound`, `bro_voice::cue`), the same on every OS.
//! * **The apps.** MCP work for bro's focus mode (a widget's HTML, the tools it calls), done through
//!   Claude Code's own MCP connections: the mod long-polls the bridge for jobs and answers them
//!   (`bro_bridge::Bridge::mod_ask`).
//!
//! The files are built into bro and written to `~/.bro/claude-mod/bro-<hash>/`: one folder per
//! version, so a running session keeps the copy it started with and two bros never fight over one.
//! Outside a bro session (no `BRO_SESSION_ID`) the mod does nothing.
use crate::util::short_hash;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;

/// `[claude_mod]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeModSettings {
    /// New Claude sessions load the mod. Off by default: you turn it on in Settings › bro mod.
    pub on: bool,
    /// A chime when Claude asks you a question.
    pub chime: bool,
    /// Claude's questions wait [`Self::timer_secs`] for you, then Claude goes with its best judgement.
    pub timer: bool,
    /// How long a question waits while the timer is on; Settings steps through [`TIMER_CHOICES`].
    pub timer_secs: u32,
    /// The timer ticks for its last 15 seconds.
    pub ticks: bool,
}

/// The question timer's lengths Settings offers, in seconds.
pub const TIMER_CHOICES: [u32; 6] = [15, 30, 60, 120, 300, 600];

impl Default for ClaudeModSettings {
    fn default() -> Self {
        Self { on: false, chime: true, timer: false, timer_secs: 30, ticks: true }
    }
}

impl ClaudeModSettings {
    /// What a Claude launch's `--settings` file carries for the mod: its options, while it loads.
    pub fn launch_settings(&self) -> Map<String, Value> {
        let mut out = Map::new();
        if self.on {
            let options = json!({
                "chime": self.chime,
                "timer": self.question_secs(),
                "ticks": self.ticks,
            });
            out.insert("pluginConfigs".into(), json!({ NAME: { "options": options } }));
        }
        out
    }

    /// How long a question waits for you, in seconds; 0 with the timer off.
    pub fn question_secs(&self) -> u32 {
        if self.timer { self.timer_secs.clamp(5, 3600) } else { 0 }
    }

    /// "on" or "off", for the settings list.
    pub fn summary(&self) -> &'static str {
        if self.on { "on" } else { "off" }
    }
}

/// The plugin's name, and its command: `/bro`.
pub const NAME: &str = "bro";

macro_rules! mod_files {
    ($($path:literal),* $(,)?) => {
        /// Every file of the mod, by its path in the plugin folder.
        pub const FILES: &[(&str, &str)] = &[$(($path, include_str!(concat!("../claude-mod/", $path)))),*];
    };
}

mod_files![
    ".claude-plugin/plugin.json",
    "hooks/hooks.json",
    "hooks/register.tsx",
    "hooks/shared/env.ts",
    "hooks/shared/bridge.ts",
    "hooks/link/index.tsx",
    "hooks/link/data.ts",
    "hooks/link/dashboard.tsx",
    "hooks/link/poll.ts",
    "hooks/questions/index.tsx",
    "hooks/apps/index.ts",
    "types/index.d.ts",
];

/// Set once a folder holds every file, so later launches skip the reads.
const MARKER: &str = ".bro-complete";

/// This build's folder: `~/.bro/claude-mod/bro-<hash of the files>`.
pub fn dir() -> PathBuf {
    static HASH: OnceLock<String> = OnceLock::new();
    let hash = HASH.get_or_init(|| {
        let all: String = FILES.iter().map(|(path, body)| format!("{path}\n{body}\n")).collect();
        short_hash(&all, 10)
    });
    crate::paths::bro_dir().join("claude-mod").join(format!("{NAME}-{hash}"))
}

/// Writes this build's copy of the mod, once; the folder to hand Claude Code.
pub fn install() -> std::io::Result<PathBuf> {
    let dir = dir();
    if dir.join(MARKER).is_file() {
        return Ok(dir);
    }
    for (path, body) in FILES {
        crate::browser::write_if_changed(&dir.join(path), body)?;
    }
    crate::util::atomic_write(&dir.join(MARKER), b"written by bro\n")?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;
    use std::path::Path;

    /// Every file the plugin folder holds is built in (tests and notes aside), so the copy bro
    /// writes is the mod in the repo.
    #[test]
    fn every_mod_file_is_built_in() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("claude-mod");
        let mut found = vec![];
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                // Claude Code lays its own types and a tsconfig beside a mod it loads
                if rel == "tests" || rel == ".claude-plugin/types" || rel == "node_modules" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if !rel.ends_with(".md") && rel != "tsconfig.json" && rel != ".gitignore" {
                    found.push(rel);
                }
            }
        }
        found.sort();
        let mut listed: Vec<String> = FILES.iter().map(|(p, _)| (*p).to_string()).collect();
        listed.sort();
        assert_eq!(found, listed);
    }

    #[test]
    fn installs_once_per_build() {
        let sb = sandbox();
        let dir = install().unwrap();
        assert!(dir.starts_with(sb.bro().join("claude-mod")));
        let manifest: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], NAME);
        assert!(dir.join("hooks/link/index.tsx").is_file());
        // a second launch finds it complete and writes nothing
        std::fs::write(dir.join("hooks/hooks.json"), "edited").unwrap();
        assert_eq!(install().unwrap(), dir);
        assert_eq!(std::fs::read_to_string(dir.join("hooks/hooks.json")).unwrap(), "edited");
    }

    #[test]
    fn off_by_default() {
        let s = ClaudeModSettings::default();
        assert!(!s.on);
        assert_eq!(s.summary(), "off");
        assert!(s.launch_settings().is_empty());
    }

    #[test]
    fn launch_settings_pass_the_options() {
        let mut s = ClaudeModSettings { on: true, ticks: false, ..Default::default() };
        let m = s.launch_settings();
        assert_eq!(m["pluginConfigs"]["bro"]["options"]["ticks"], false);
        assert_eq!(m["pluginConfigs"]["bro"]["options"]["chime"], true);
        assert_eq!(m["pluginConfigs"]["bro"]["options"]["timer"], 0, "no timer until you turn it on");
        assert_eq!(s.summary(), "on");
        s.timer = true;
        assert_eq!(s.launch_settings()["pluginConfigs"]["bro"]["options"]["timer"], 30);
        s.timer_secs = 1;
        assert_eq!(s.question_secs(), 5, "a hand-edited length stays one the mod takes");
        s.on = false;
        assert!(s.launch_settings().is_empty());
    }

    /// Settings saved before the theme and the mod's other switches went still load.
    #[test]
    fn old_switches_are_ignored() {
        let s: ClaudeModSettings = toml::from_str("on = true
theme = false
flair = false
hud = false
radar = false
tools = false
").unwrap();
        assert_eq!(s, ClaudeModSettings { on: true, ..Default::default() });
    }

    #[test]
    fn manifest_options_match_the_settings() {
        let (_, manifest) = FILES.iter().find(|(p, _)| *p == ".claude-plugin/plugin.json").unwrap();
        let manifest: Value = serde_json::from_str(manifest).unwrap();
        let declared: Vec<&str> = manifest["userConfig"].as_object().unwrap().keys().map(String::as_str).collect();
        let passed = ClaudeModSettings { on: true, ..Default::default() }.launch_settings();
        let passed: Vec<&str> = passed["pluginConfigs"]["bro"]["options"].as_object().unwrap().keys().map(String::as_str).collect();
        let (mut declared, mut passed) = (declared, passed);
        declared.sort_unstable();
        passed.sort_unstable();
        assert_eq!(declared, passed);
    }
}
