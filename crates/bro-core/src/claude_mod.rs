//! bro's Claude Code mod (`crates/bro-core/claude-mod`): a plugin of function hooks bro loads into
//! every Claude session it starts (`--plugin-dir`). `[claude_mod]` in `~/.bro/v2.toml`. Two halves:
//!
//! * **The look.** Claude Code wears bro's theme: bro writes it as a Claude Code theme and selects
//!   it per launch ([`crate::claude_theme`], `theme` below), and the mod gives the spinner and the
//!   turn line the theme's voice ("Forging…", "Forged for 12s" on Ember).
//! * **The link.** The login and its limits at the end of the hint line, a status line when another
//!   session waits on you, `/bro` (a pane of every session and limit; jump to one, rename this one),
//!   and tools Claude can call: show you a file in bro's preview, ping your phone, check the other
//!   sessions. It all goes through bro's bridge on loopback.
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
    /// New Claude sessions load the mod. Off by default: you turn it on in Settings › Claude Code.
    pub on: bool,
    /// Claude Code takes bro's theme (its colours, live). Works with the mod off too.
    pub theme: bool,
    /// The spinner and the turn line speak in the theme's voice.
    pub flair: bool,
    /// The login and its limits at the end of the hint line.
    pub hud: bool,
    /// A status line when another session waits on you or has finished.
    pub radar: bool,
    /// Claude can show you files in bro's preview, ping your phone and check your other sessions.
    pub tools: bool,
}

impl Default for ClaudeModSettings {
    fn default() -> Self {
        Self { on: false, theme: true, flair: true, hud: true, radar: true, tools: true }
    }
}

impl ClaudeModSettings {
    /// What a Claude launch's `--settings` file carries for the mod and the theme: Claude's
    /// `theme` (when bro wrote one for this launch) and the mod's options.
    pub fn launch_settings(&self, themed: bool) -> Map<String, Value> {
        let mut out = Map::new();
        if themed {
            out.insert("theme".into(), Value::String(crate::claude_theme::SETTING.into()));
        }
        if self.on {
            let options = json!({ "flair": self.flair, "hud": self.hud, "radar": self.radar, "tools": self.tools });
            out.insert("pluginConfigs".into(), json!({ NAME: { "options": options } }));
        }
        out
    }

    /// "on", "theme only" or "off", for the settings list.
    pub fn summary(&self) -> &'static str {
        match (self.on, self.theme) {
            (true, _) => "on",
            (false, true) => "theme only",
            (false, false) => "off",
        }
    }
}

/// The plugin's name: its tools are `mcp__bro__*`, its command `/bro`.
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
    "hooks/shared/look.ts",
    "hooks/style/index.ts",
    "hooks/style/words.ts",
    "hooks/link/index.tsx",
    "hooks/link/data.ts",
    "hooks/link/dashboard.tsx",
    "hooks/link/poll.ts",
    "hooks/link/tools.ts",
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
    fn off_by_default_apart_from_the_theme() {
        let s = ClaudeModSettings::default();
        assert!(!s.on && s.theme);
        assert_eq!(s.summary(), "theme only");
        assert!(s.launch_settings(true).get("pluginConfigs").is_none());
    }

    #[test]
    fn launch_settings_pick_the_theme_and_pass_the_options() {
        let mut s = ClaudeModSettings { on: true, radar: false, ..Default::default() };
        let m = s.launch_settings(true);
        assert_eq!(m["theme"], "custom:bro");
        assert_eq!(m["pluginConfigs"]["bro"]["options"]["radar"], false);
        assert_eq!(m["pluginConfigs"]["bro"]["options"]["tools"], true);
        assert!(s.launch_settings(false).get("theme").is_none(), "no theme written: Claude keeps its own");
        s.on = false;
        assert!(s.launch_settings(true).get("pluginConfigs").is_none());
        assert_eq!(s.summary(), "theme only");
    }

    #[test]
    fn manifest_options_match_the_settings() {
        let (_, manifest) = FILES.iter().find(|(p, _)| *p == ".claude-plugin/plugin.json").unwrap();
        let manifest: Value = serde_json::from_str(manifest).unwrap();
        let declared: Vec<&str> = manifest["userConfig"].as_object().unwrap().keys().map(String::as_str).collect();
        let passed = ClaudeModSettings { on: true, ..Default::default() }.launch_settings(false);
        let passed: Vec<&str> = passed["pluginConfigs"]["bro"]["options"].as_object().unwrap().keys().map(String::as_str).collect();
        let (mut declared, mut passed) = (declared, passed);
        declared.sort_unstable();
        passed.sort_unstable();
        assert_eq!(declared, passed);
    }
}
