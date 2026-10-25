//! Claude Code environment presets: the privacy, update and behaviour switches bro sets on
//! every Claude launch, picked as one preset plus per-switch overrides (`[claude_env]` in
//! `~/.bro/v2.toml`). Reference: https://code.claude.com/docs/en/env-vars
//!
//! Each [`Knob`] has options; the first is the default and is usually "Claude's default",
//! which sets nothing. The `standard` preset leaves Claude Code as it ships apart from
//! [`ALWAYS`] (the fullscreen renderer), agent teams, focus view, tool search behind the proxy, and
//! messages between sessions kept to their project ([`crate::peers`]).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `[claude_env]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeEnvSettings {
    /// A [`PRESETS`] id; unknown ids act like "standard".
    pub preset: String,
    /// knob id -> option id, on top of the preset.
    pub overrides: BTreeMap<String, String>,
    /// Raw variables set last on every Claude launch (escape hatch for anything not in [`KNOBS`]).
    pub extra: BTreeMap<String, String>,
}

impl Default for ClaudeEnvSettings {
    fn default() -> Self {
        Self { preset: "standard".into(), overrides: BTreeMap::new(), extra: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Agent teams: drawn in a group of their own above the preset, which never touches them.
    Teams,
    Privacy,
    Updates,
    LongRuns,
    Features,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Teams => "agent teams",
            Group::Privacy => "privacy",
            Group::Updates => "updates",
            Group::LongRuns => "long runs",
            Group::Features => "features",
        }
    }
}

/// When a knob's variables apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    /// Only when the launch goes through bro-proxy to an Anthropic-format upstream.
    ProxiedAnthropic,
}

pub struct Opt {
    pub id: &'static str,
    pub label: &'static str,
    pub vars: &'static [(&'static str, &'static str)],
    /// Claude Code settings (key, JSON value) bro passes in its `--settings` file. Unlike
    /// settings.json, these stay switchable inside the session (`/focus` drops `viewMode`).
    pub settings: &'static [(&'static str, &'static str)],
}

pub struct Knob {
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    /// One line for the settings view: what it does and what it costs.
    pub help: &'static str,
    pub when: When,
    /// `options[0]` is the default when neither the preset nor an override picks one.
    pub options: &'static [Opt],
}

/// The knob that turns agent teams on ([`crate::agent_teams`]).
pub const AGENT_TEAMS: &str = "agent_teams";

const DEFAULT: Opt = Opt { id: "default", label: "Claude default", vars: &[], settings: &[] };

pub const KNOBS: &[Knob] = &[
    Knob {
        id: AGENT_TEAMS,
        label: "Agent teams",
        group: Group::Teams,
        help: "Ask Claude for teammates and it spawns them: each its own Claude, sharing a task list and messaging the others. Off: named subagents stay subagents.",
        when: When::Always,
        options: &[
            Opt { id: "on", label: "on", vars: &[("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1")], settings: &[] },
            Opt { id: "default", label: "off", vars: &[], settings: &[] },
        ],
    },
    Knob {
        id: "teammate_model",
        label: "Teammate model",
        group: Group::Teams,
        help: "What teammates and subagents run on when Claude doesn't name a model. Sonnet or Haiku keep a big team's tokens down.",
        when: When::Always,
        options: &[
            Opt { id: "default", label: "lead's", vars: &[], settings: &[] },
            Opt { id: "opus", label: "Opus", vars: &[("CLAUDE_CODE_SUBAGENT_MODEL", "opus")], settings: &[] },
            Opt { id: "sonnet", label: "Sonnet", vars: &[("CLAUDE_CODE_SUBAGENT_MODEL", "sonnet")], settings: &[] },
            Opt { id: "haiku", label: "Haiku", vars: &[("CLAUDE_CODE_SUBAGENT_MODEL", "haiku")], settings: &[] },
        ],
    },
    Knob {
        id: "error_reporting",
        label: "Error reports",
        group: Group::Privacy,
        help: "Crash and error reports sent to Anthropic. Turning them off costs no features.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("DISABLE_ERROR_REPORTING", "1")], settings: &[] }],
    },
    Knob {
        id: "feedback",
        label: "Feedback prompts",
        group: Group::Privacy,
        help: "\"How is Claude doing?\" surveys and Claude-drafted feedback. /bug still works.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY", "1"), ("CLAUDE_CODE_SEND_FEEDBACK", "0")], settings: &[] }],
    },
    Knob {
        id: "fingerprint",
        label: "Prompt fingerprint",
        group: Group::Privacy,
        help: "Attribution block (client version + prompt fingerprint) at the top of the system prompt.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("CLAUDE_CODE_ATTRIBUTION_HEADER", "0")], settings: &[] }],
    },
    Knob {
        id: "telemetry",
        label: "Usage telemetry",
        group: Group::Privacy,
        help: "Off also stops feature-flag fetches: no Remote Control, artifact comments, advisor or claude.ai skill sync.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("DISABLE_TELEMETRY", "1")], settings: &[] }],
    },
    Knob {
        id: "network",
        label: "Nonessential traffic",
        group: Group::Privacy,
        help: "Off blocks telemetry, error reports, auto-updates, release notes, PR badges and feature flags in one go.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"), ("CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL", "1")], settings: &[] }],
    },
    Knob {
        id: "connectors",
        label: "claude.ai connectors",
        group: Group::Privacy,
        help: "MCP servers configured on claude.ai (Gmail, Drive, ...) loaded into every session.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("ENABLE_CLAUDEAI_MCP_SERVERS", "false")], settings: &[] }],
    },
    Knob {
        id: "secrets",
        label: "Secrets in tool shells",
        group: Group::Privacy,
        help: "Scrubbed: Bash, hooks and MCP servers don't see API keys/tokens from your env (limits prompt injection).",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "scrubbed", label: "scrubbed", vars: &[("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1")], settings: &[] }],
    },
    Knob {
        id: "mcp_env",
        label: "MCP server env",
        group: Group::Privacy,
        help: "Minimal: stdio MCP servers get a safe baseline + their own configured env, not your whole shell env.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "minimal", label: "minimal", vars: &[("CLAUDE_CODE_MCP_ALLOWLIST_ENV", "1")], settings: &[] }],
    },
    Knob {
        id: "history",
        label: "Transcripts & history",
        group: Group::Privacy,
        help: "Off writes nothing to disk: those sessions can't be continued, resumed or recapped by bro.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("CLAUDE_CODE_SKIP_PROMPT_HISTORY", "1")], settings: &[] }],
    },
    Knob {
        id: "memory",
        label: "Auto memory",
        group: Group::Privacy,
        help: "Claude's own notes about you and your projects, written and read across sessions.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "off", label: "off", vars: &[("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")], settings: &[] }],
    },
    Knob {
        id: "screen_share",
        label: "Screen-share safe",
        group: Group::Privacy,
        help: "Hides your email, org name and working directory from Claude's header and /status.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "on", label: "on", vars: &[("IS_DEMO", "1"), ("CLAUDE_CODE_HIDE_CWD", "1")], settings: &[] }],
    },
    Knob {
        id: crate::peers::KNOB,
        label: "Messages between sessions",
        group: Group::Privacy,
        help: "This project: Claude only finds and messages sessions in the same repo or its worktrees. Off: none. Claude default: any.",
        when: When::Always,
        // the hooks behind "this project" and "off" are bro's own (see `crate::peers`), added per launch
        options: &[
            Opt { id: "project", label: "this project", vars: &[], settings: &[] },
            Opt { id: "off", label: "off", vars: &[], settings: &[("crossSessionInbound", "\"refuse\""), ("isolatePeerMachines", "true")] },
            Opt { id: "default", label: "Claude default", vars: &[], settings: &[] },
        ],
    },
    Knob {
        id: "updates",
        label: "Claude updates",
        group: Group::Updates,
        help: "Manual: no background self-updates mid-session (claude update still works). Frozen blocks both.",
        when: When::Always,
        options: &[
            DEFAULT,
            Opt { id: "manual", label: "manual", vars: &[("DISABLE_AUTOUPDATER", "1")], settings: &[] },
            Opt { id: "frozen", label: "frozen", vars: &[("DISABLE_UPDATES", "1")], settings: &[] },
        ],
    },
    Knob {
        id: "retry",
        label: "Overload errors",
        group: Group::LongRuns,
        help: "Ride out: retry 429/529 capacity errors indefinitely instead of giving up after 10 tries.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "ride_out", label: "ride out", vars: &[("CLAUDE_CODE_RETRY_WATCHDOG", "1")], settings: &[] }],
    },
    Knob {
        id: "shell_timeout",
        label: "Shell command time limit",
        group: Group::LongRuns,
        help: "Long: 10 min default, 60 min max for Bash/PowerShell commands (builds, test suites) instead of 2 / 10.",
        when: When::Always,
        options: &[DEFAULT, Opt { id: "long", label: "long", vars: &[("BASH_DEFAULT_TIMEOUT_MS", "600000"), ("BASH_MAX_TIMEOUT_MS", "3600000")], settings: &[] }],
    },
    Knob {
        id: "cache_ttl",
        label: "Prompt cache lifetime",
        group: Group::LongRuns,
        help: "1h survives long pauses and covers subagents too; on API keys cache writes cost 2x instead of 1.25x.",
        when: When::Always,
        options: &[
            DEFAULT,
            Opt { id: "1h", label: "1 hour", vars: &[("CLAUDE_CODE_PROMPT_CACHE_TTL", "1h"), ("CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL", "1h")], settings: &[] },
            Opt { id: "5m", label: "5 minutes", vars: &[("FORCE_PROMPT_CACHING_5M", "1")], settings: &[] },
        ],
    },
    Knob {
        id: "compact",
        label: "Auto-compact",
        group: Group::LongRuns,
        help: "Early compacts at 70% of the window (sharper long sessions); off leaves /compact to you.",
        when: When::Always,
        options: &[
            DEFAULT,
            Opt { id: "early", label: "early (70%)", vars: &[("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "70")], settings: &[] },
            Opt { id: "off", label: "off", vars: &[("DISABLE_AUTO_COMPACT", "1")], settings: &[] },
        ],
    },
    Knob {
        id: "tool_search",
        label: "Tool search via proxy",
        group: Group::Features,
        help: "Claude Code turns tool search off behind a proxy, loading every MCP tool upfront. Auto keeps it on for Claude upstreams.",
        when: When::ProxiedAnthropic,
        options: &[Opt { id: "auto", label: "auto", vars: &[("ENABLE_TOOL_SEARCH", "true")], settings: &[] }, Opt { id: "off", label: "Claude default", vars: &[], settings: &[] }],
    },
    Knob {
        id: "focus",
        label: "Focus view",
        group: Group::Features,
        help: "Only your prompt, a one-line summary of tool steps and the reply. /focus flips it for one session.",
        when: When::Always,
        options: &[
            Opt { id: "on", label: "on", vars: &[], settings: &[("viewMode", "\"focus\"")] },
            Opt { id: "off", label: "Claude default", vars: &[], settings: &[] },
        ],
    },
    Knob {
        id: "spinner_tips",
        label: "Spinner tips",
        group: Group::Features,
        help: "The \"Tip: ...\" line Claude shows under its spinner while it works. Off keeps that space for the status.",
        when: When::Always,
        options: &[
            Opt { id: "off", label: "off", vars: &[], settings: &[("spinnerTipsEnabled", "false")] },
            Opt { id: "on", label: "Claude default", vars: &[], settings: &[] },
        ],
    },
];

/// Set on every Claude launch, before the knobs: the fullscreen renderer, which focus view
/// needs and which keeps long chats flicker-free. `[claude_env.extra]` can still undo it.
pub const ALWAYS: &[(&str, &str)] = &[("CLAUDE_CODE_NO_FLICKER", "1")];

pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// (knob id, option id)
    pub picks: &'static [(&'static str, &'static str)],
}

const QUIET: &[(&str, &str)] = &[("error_reporting", "off"), ("feedback", "off")];

pub const PRESETS: &[Preset] = &[
    Preset { id: "standard", label: "Standard", help: "Claude Code as it ships.", picks: &[] },
    Preset { id: "quiet", label: "Quiet", help: "No error reports or feedback prompts. Every feature still works.", picks: QUIET },
    Preset {
        id: "private",
        label: "Private",
        help: "Quiet + no telemetry, fingerprint or claude.ai connectors. Loses Remote Control, artifact comments, advisor.",
        picks: &[("error_reporting", "off"), ("feedback", "off"), ("telemetry", "off"), ("fingerprint", "off"), ("connectors", "off")],
    },
    Preset {
        id: "lockdown",
        label: "Lockdown",
        help: "Private + no nonessential traffic, secrets scrubbed from tool shells, minimal MCP env, no auto memory.",
        picks: &[
            ("error_reporting", "off"),
            ("feedback", "off"),
            ("telemetry", "off"),
            ("fingerprint", "off"),
            ("connectors", "off"),
            ("network", "off"),
            ("secrets", "scrubbed"),
            ("mcp_env", "minimal"),
            ("memory", "off"),
            ("updates", "manual"),
        ],
    },
    Preset {
        id: "incognito",
        label: "Incognito",
        help: "Quiet + nothing written to disk and no auto memory. Sessions can't be continued later.",
        picks: &[("error_reporting", "off"), ("feedback", "off"), ("history", "off"), ("memory", "off")],
    },
    Preset {
        id: "presenter",
        label: "Presenter",
        help: "Quiet + email, org and paths hidden, for streams and screen shares.",
        picks: &[("error_reporting", "off"), ("feedback", "off"), ("screen_share", "on")],
    },
    Preset {
        id: "marathon",
        label: "Marathon",
        help: "Quiet + rides out overloads, long shell commands, 1h prompt cache, no mid-run self-updates.",
        picks: &[("error_reporting", "off"), ("feedback", "off"), ("retry", "ride_out"), ("shell_timeout", "long"), ("cache_ttl", "1h"), ("updates", "manual")],
    },
];

pub fn knob(id: &str) -> Option<&'static Knob> {
    KNOBS.iter().find(|k| k.id == id)
}

pub fn preset(id: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

impl Knob {
    fn option(&self, id: &str) -> Option<usize> {
        self.options.iter().position(|o| o.id == id)
    }
}

/// Where a knob's current option comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    Preset,
    Override,
}

impl ClaudeEnvSettings {
    /// The option index in effect for `k` and where it came from.
    pub fn choice(&self, k: &Knob) -> (usize, Source) {
        if let Some(i) = self.overrides.get(k.id).and_then(|o| k.option(o)) {
            return (i, Source::Override);
        }
        match preset(&self.preset).picks.iter().find(|(id, _)| *id == k.id).and_then(|(_, o)| k.option(o)) {
            Some(i) => (i, Source::Preset),
            None => (0, Source::Default),
        }
    }

    /// Pick option `i` for `k`; picking what the preset already gives drops the override.
    pub fn set_choice(&mut self, k: &Knob, i: usize) {
        self.overrides.remove(k.id);
        if self.choice(k).0 != i {
            self.overrides.insert(k.id.into(), k.options[i % k.options.len()].id.into());
        }
    }

    /// Switch preset and forget its overrides (a preset is a fresh starting point). Agent teams stay as they
    /// are: no preset touches them.
    pub fn use_preset(&mut self, id: &str) {
        self.preset = preset(id).id.into();
        self.clear_preset_changes();
    }

    /// Drop the overrides on top of the preset, keeping the agent teams ones.
    pub fn clear_preset_changes(&mut self) {
        self.overrides.retain(|k, _| knob(k).is_some_and(|k| k.group == Group::Teams));
    }

    /// How many switches differ from the preset (agent teams aside: no preset touches them).
    pub fn preset_changes(&self) -> usize {
        self.overrides.keys().filter(|k| knob(k).is_some_and(|k| k.group != Group::Teams)).count()
    }

    /// The variables a Claude launch gets, in order. `proxied_anthropic`: the launch goes
    /// through bro-proxy to an Anthropic-format upstream.
    pub fn vars(&self, proxied_anthropic: bool) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = ALWAYS.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        for k in KNOBS {
            if k.when == When::ProxiedAnthropic && !proxied_anthropic {
                continue;
            }
            for (name, value) in k.options[self.choice(k).0].vars {
                out.retain(|(n, _)| n != name);
                out.push((name.to_string(), value.to_string()));
            }
        }
        for (name, value) in &self.extra {
            out.retain(|(n, _)| n != name);
            out.push((name.clone(), value.clone()));
        }
        out
    }

    /// The Claude Code settings a launch's `--settings` file carries, in knob order.
    pub fn settings(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        for k in KNOBS {
            for (key, value) in k.options[self.choice(k).0].settings {
                out.insert(key.to_string(), serde_json::from_str(value).unwrap_or_else(|_| (*value).into()));
            }
        }
        out
    }

    /// New Claude sessions can form agent teams.
    pub fn agent_teams(&self) -> bool {
        knob(AGENT_TEAMS).is_some_and(|k| k.options[self.choice(k).0].id == "on")
    }

    /// Short status for toasts and the sidebar: "Private" or "Private + 2 changes".
    pub fn summary(&self) -> String {
        let p = preset(&self.preset).label;
        match self.preset_changes() {
            0 => p.into(),
            1 => format!("{p} + 1 change"),
            n => format!("{p} + {n} changes"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(v: &'a [(String, String)], k: &str) -> Option<&'a str> {
        v.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn standard_sets_the_renderer_agent_teams_focus_view_no_tips_and_proxy_tool_search() {
        let s = ClaudeEnvSettings::default();
        let flicker = ("CLAUDE_CODE_NO_FLICKER".to_string(), "1".to_string());
        let teams = ("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(), "1".to_string());
        assert_eq!(s.vars(false), vec![flicker.clone(), teams.clone()]);
        assert_eq!(s.vars(true), vec![flicker, teams, ("ENABLE_TOOL_SEARCH".to_string(), "true".to_string())]);
        assert_eq!(serde_json::Value::Object(s.settings()), serde_json::json!({ "viewMode": "focus", "spinnerTipsEnabled": false }));
    }

    #[test]
    fn settings_switch_back_to_claude_defaults_and_extra_can_drop_the_renderer() {
        let mut s = ClaudeEnvSettings::default();
        s.set_choice(knob("focus").unwrap(), 1);
        s.set_choice(knob("spinner_tips").unwrap(), 1);
        assert!(s.settings().is_empty());
        s.extra.insert("CLAUDE_CODE_NO_FLICKER".into(), "0".into());
        assert_eq!(get(&s.vars(false), "CLAUDE_CODE_NO_FLICKER"), Some("0"));
    }

    #[test]
    fn presets_and_overrides() {
        let mut s = ClaudeEnvSettings::default();
        s.use_preset("private");
        let v = s.vars(false);
        assert_eq!(get(&v, "DISABLE_TELEMETRY"), Some("1"));
        assert_eq!(get(&v, "DISABLE_ERROR_REPORTING"), Some("1"));
        assert_eq!(get(&v, "CLAUDE_CODE_SKIP_PROMPT_HISTORY"), None);

        let telemetry = knob("telemetry").unwrap();
        s.set_choice(telemetry, 0);
        assert_eq!(s.choice(telemetry), (0, Source::Override));
        assert_eq!(get(&s.vars(false), "DISABLE_TELEMETRY"), None);
        assert_eq!(s.summary(), "Private + 1 change");
        s.set_choice(telemetry, 1);
        assert!(s.overrides.is_empty(), "back to the preset's pick drops the override");

        s.extra.insert("DISABLE_TELEMETRY".into(), "".into());
        assert_eq!(get(&s.vars(false), "DISABLE_TELEMETRY"), Some(""), "extra wins");

        s.use_preset("nope");
        assert_eq!(s.preset, "standard");
    }

    #[test]
    fn every_preset_pick_names_a_real_option() {
        for p in PRESETS {
            for (k, o) in p.picks {
                let knob = knob(k).unwrap_or_else(|| panic!("{}: unknown knob {k}", p.id));
                assert!(knob.option(o).is_some(), "{}: {k} has no option {o}", p.id);
            }
        }
        let mut ids: Vec<_> = KNOBS.iter().map(|k| k.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), KNOBS.len());
    }

    #[test]
    fn agent_teams_are_on_until_switched_off_and_no_preset_touches_them() {
        let mut s = ClaudeEnvSettings::default();
        assert!(s.agent_teams());
        let k = knob(AGENT_TEAMS).unwrap();
        s.set_choice(k, 1);
        assert!(!s.agent_teams());
        assert_eq!(get(&s.vars(false), "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"), None, "off is Claude's own default");
        // a v2.toml from before teams were on by default
        let old: ClaudeEnvSettings = toml::from_str("[overrides]
agent_teams = \"on\"
").unwrap();
        assert!(old.agent_teams());
        let model = knob("teammate_model").unwrap();
        s.set_choice(model, 2);
        assert_eq!(get(&s.vars(false), "CLAUDE_CODE_SUBAGENT_MODEL"), Some("sonnet"));
        for p in PRESETS {
            assert!(p.picks.iter().all(|(k, _)| knob(k).is_some_and(|k| k.group != Group::Teams)), "{} picks a team knob", p.id);
        }
        // and switching presets keeps them
        s.set_choice(knob("telemetry").unwrap(), 1);
        assert_eq!((s.preset_changes(), s.summary().as_str()), (1, "Standard + 1 change"), "teams aren't preset changes");
        s.use_preset("private");
        assert!(!s.agent_teams() && get(&s.vars(false), "CLAUDE_CODE_SUBAGENT_MODEL") == Some("sonnet"), "a preset leaves agent teams as they were");
        assert_eq!(s.preset_changes(), 0);
        assert!(KNOBS.iter().take_while(|k| k.group == Group::Teams).count() == KNOBS.iter().filter(|k| k.group == Group::Teams).count(), "team knobs come first");
    }

    #[test]
    fn toml_shape() {
        let s: ClaudeEnvSettings = toml::from_str("preset = \"marathon\"\n[overrides]\nretry = \"default\"\n").unwrap();
        assert_eq!(s.choice(knob("retry").unwrap()), (0, Source::Override));
        assert_eq!(s.choice(knob("cache_ttl").unwrap()).1, Source::Preset);
        assert!(s.extra.is_empty());
    }
}
