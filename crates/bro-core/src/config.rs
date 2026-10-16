//! `~/.bro/config.json` and `~/.bro/state.json` (v1 schema, unknown keys preserved)
//! plus `~/.bro/v2.toml` for v2-only settings (keybindings, theme, bridge, proxy).
use crate::paths;
use crate::util::{atomic_write, env_str, merge_ordered, read_json, write_json_pretty};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// v1 `config.json`. Unknown fields are kept in `extra` and written back untouched.
///
/// The raw file is loaded as-is (including v1's `#`-prefixed notes and examples);
/// `#` stripping happens where the data is used (e.g. [`crate::providers::load`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// provider id (or `provider@tier`) -> API key
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// user-defined / overriding providers, merged over models.json by id
    #[serde(default)]
    pub providers: Vec<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Config {
    /// Missing file → empty config. Malformed JSON → error (so a caller never saves
    /// over a file it could not read).
    pub fn load() -> anyhow::Result<Config> {
        let path = paths::config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let raw: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .with_context(|| format!("{} is not valid JSON", path.display()))?;
        Ok(Config::from_value(raw))
    }

    /// Lenient conversion from the raw JSON document: non-string key values and a
    /// non-array `providers` are ignored rather than failing the whole load.
    pub fn from_value(raw: Value) -> Config {
        let Value::Object(mut map) = raw else { return Config::default() };
        let keys = match map.remove("keys") {
            Some(Value::Object(k)) => {
                k.into_iter().filter_map(|(id, v)| v.as_str().map(|s| (id, s.to_string()))).collect()
            }
            _ => BTreeMap::new(),
        };
        let providers = match map.remove("providers") {
            Some(Value::Array(p)) => p,
            _ => Vec::new(),
        };
        Config { keys, providers, extra: map }
    }

    /// Atomic write (temp + rename), preserving unknown keys — and the order keys had
    /// in the existing file, so a hand-edited config doesn't get shuffled.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::config_path();
        let old = match read_json(&path) {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        let old_keys = match old.get("keys") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        let mut keys: Map<String, Value> =
            self.keys.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        // Entries `Config` can't represent (non-string values) are carried over as-is.
        for (k, v) in &old_keys {
            if !v.is_string() && !keys.contains_key(k) {
                keys.insert(k.clone(), v.clone());
            }
        }
        let mut top = Map::new();
        top.insert("keys".into(), Value::Object(merge_ordered(&old_keys, keys)));
        top.insert("providers".into(), Value::Array(self.providers.clone()));
        for (k, v) in &self.extra {
            top.insert(k.clone(), v.clone());
        }
        let merged = merge_ordered(&old, top);
        write_json_pretty(&path, &Value::Object(merged)).with_context(|| format!("writing {}", path.display()))
    }

    /// Resolve a key: config.keys[id] then the provider's keyEnv env var.
    ///
    /// `provider_id` may be `provider@tier` (a new-api relay token for one tier); the
    /// provider's plain key is the fallback, as in v1 `newApiKey`.
    pub fn key_for(&self, provider_id: &str, key_env: Option<&str>) -> Option<String> {
        let nonempty = |s: &String| !s.trim().is_empty();
        if let Some(k) = self.keys.get(provider_id).filter(|k| nonempty(k)) {
            return Some(k.clone());
        }
        if let Some((base, _tier)) = provider_id.split_once('@')
            && let Some(k) = self.keys.get(base).filter(|k| nonempty(k))
        {
            return Some(k.clone());
        }
        key_env.filter(|e| !e.is_empty()).and_then(env_str)
    }

    /// Store (or, with `None`/empty, forget) a key — v1 `setKey`.
    pub fn set_key(&mut self, provider_id: &str, key: Option<&str>) {
        match key.filter(|k| !k.is_empty()) {
            Some(k) => {
                self.keys.insert(provider_id.to_string(), k.to_string());
            }
            None => {
                self.keys.remove(provider_id);
            }
        }
    }

    /// v1 `configPermissionMode`: "auto" | "manual" | "bypass".
    pub fn permission_mode(&self) -> &'static str {
        match self.extra.get("permissionMode").and_then(Value::as_str) {
            Some("auto") => "auto",
            Some("manual") => "manual",
            Some("bypass") => "bypass",
            _ => match self.extra.get("dangerouslySkipPermissions") {
                Some(Value::Bool(true)) => "bypass",
                Some(Value::Bool(false)) => "manual",
                _ => "auto",
            },
        }
    }
}

/// v2-only settings, `~/.bro/v2.toml`. Every field has a default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    /// Prefix chord, default "ctrl+space"
    pub prefix: String,
    pub shell: Option<String>,
    pub nerd_font: bool,
    /// Sidebar usage block shows every profile (true) or just the Claude / Codex totals (false)
    pub usage_expanded: bool,
    /// Sidebar width in terminal columns; None uses the adaptive default.
    pub sidebar_width: Option<u16>,
    /// Harness logos as images: "auto" (detect the terminal), "sixel", "kitty", "iterm" or "text"
    pub icons: String,
    /// Digital rain over any theme: off (the theme decides), everywhere, or only in sessions that are working
    pub rain: RainMode,
    /// A font bro bundles, by id ("departure-mono"), for bro's own window; None = the terminal's font
    pub font: Option<String>,
    pub bridge: BridgeSettings,
    pub proxy: ProxySettings,
    pub voice: VoiceSettings,
    pub insights: InsightsSettings,
    pub auto_resume: AutoResumeSettings,
    /// Answer Yes for you when Claude stops on a "Do you want to proceed?" permission prompt (the safety
    /// checks bypass mode still asks, e.g. "Dangerous rm operation on statically-unresolvable target"). Off by default.
    pub auto_approve: bool,
    /// Summary screensaver: after this many seconds with no keys or mouse, every session on screen turns over
    /// to its summary (the card back), and they turn back at the next touch. 0 = off.
    pub summary_screensaver_secs: u64,
    /// Permission mode new sessions start on: the one last picked in the launcher.
    pub permission: crate::launch::Permission,
    /// Privacy / update / behaviour env for Claude launches (preset + overrides).
    pub claude_env: crate::claude_env::ClaudeEnvSettings,
    /// Caveman mode: terse replies for new Claude / Codex / Pi sessions.
    pub caveman: crate::caveman::CavemanSettings,
    /// JustTools integration: its skill in new Claude / Codex sessions.
    pub justtools: crate::justtools::JustToolsSettings,
    /// JustWatch integration: its skill in new Claude / Codex sessions.
    pub justwatch: crate::justwatch::JustWatchSettings,
    /// Looking for new bro releases.
    pub updates: crate::self_update::UpdateSettings,
    /// Watchdogs: a fresh reviewer agent that hands an idle session its next task.
    pub watchdog: WatchdogSettings,
    /// action name -> key chord, overriding defaults (e.g. "palette" = "ctrl+k")
    pub keys: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgeSettings {
    pub enabled: bool,
    pub port: u16,
    pub automatic_port: bool,
    pub bind: String,
    pub web_interface: bool,
    /// While a remote client (phone, web) watches a session, its terminal is at least this
    /// many columns wide, even if the desktop tile is narrower (the tile then shows the
    /// bottom-left of it). 0 turns the column minimum off.
    pub remote_min_cols: u16,
    /// Same for rows. 0 turns the row minimum off.
    pub remote_min_rows: u16,
    /// How long the minimum is kept after the last remote viewer leaves, so a phone that
    /// briefly backgrounds the app doesn't make the session resize back and forth.
    pub remote_min_grace_secs: u64,
}

/// Agent insights (why a pane waits, recaps, files it touched or sent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InsightsSettings {
    /// Ask Claude for a `/recap` of a pane left waiting unseen for 3 minutes (a few cents each).
    pub generate_recaps: bool,
}

impl Default for InsightsSettings {
    fn default() -> Self {
        Self { generate_recaps: true }
    }
}

/// Nudge an agent that stopped on an API error (connection lost, overloaded, rate / usage limit) to carry on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoResumeSettings {
    pub enabled: bool,
    /// What to type to resume
    pub prompt: String,
    /// Retries per stall before giving up (a usage limit's wait for its reset doesn't count)
    pub max_attempts: u32,
    /// After a session / weekly limit, resume once it resets
    pub after_limit_reset: bool,
}

impl Default for AutoResumeSettings {
    fn default() -> Self {
        Self { enabled: true, prompt: "continue".into(), max_attempts: 6, after_limit_reset: true }
    }
}

/// Defaults for new watchdogs (each one can change them when it starts) and the reviewer's standing
/// instructions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WatchdogSettings {
    /// The reviewer's system prompt; None = bro's own (`bro_bridge::watchdog::DEFAULT_PROMPT`).
    pub prompt: Option<String>,
    /// Seconds a session sits idle after its agent stops before the watchdog wakes.
    pub idle_secs: u64,
    /// Wake-ups a watchdog gets before it stands down.
    pub max_wakes: u32,
    /// Claude model the reviewer runs on (alias or full id). A Codex reviewer uses its login's default.
    pub model: String,
    /// "auto" (the login with the most usage left, Claude first) or a login id ("claude:work").
    pub reviewer: String,
}

impl Default for WatchdogSettings {
    fn default() -> Self {
        Self { prompt: None, idle_secs: 60, max_wakes: 25, model: "opus".into(), reviewer: "auto".into() }
    }
}

/// Push-to-talk to the orchestrator: hold the hotkey anywhere, speak, release.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Only takes effect when an API key is found in `api_key_env`.
    pub enabled: bool,
    /// None = the platform default (right Windows key on Windows); see bro-voice for names.
    pub hotkey: Option<String>,
    pub api_key_env: String,
    /// Transcription model; None = bro-voice's default.
    pub model: Option<String>,
    /// ISO-639-1 hint such as "en"; None = auto-detect.
    pub language: Option<String>,
    /// "live": talk with GPT-Live, which answers out loud and hands work to Hugh. "transcribe": speech to text
    /// only, Hugh's answers as toasts.
    pub mode: String,
    /// GPT-Live voice (cedar, marin, ash, ...); None = bro-voice's default.
    pub live_voice: Option<String>,
    /// GPT-Live model; None = bro-voice's default.
    pub live_model: Option<String>,
}

/// Where digital rain falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RainMode {
    /// only themes that bring their own (digital-rain)
    #[default]
    Off,
    /// every empty corner, on any theme
    Everywhere,
    /// only inside sessions whose agent is working right now
    Working,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySettings {
    pub enabled: bool,
    /// 0 = pick a free port
    pub port: u16,
    /// Compress requests that go through bro-proxy (llmtrim). Off by default.
    pub trim: bool,
    /// While `trim` is on, newly launched native Claude logins and Codex logins are routed
    /// through bro-proxy (as their own login) so compression applies to them too.
    pub trim_route_native: bool,
    /// "safe" (terminal-noise normalization of tool output only) or "lossless" (also llmtrim's
    /// lossless preset: JSON minify, columnar arrays, exact-duplicate lines).
    pub trim_level: String,
}

impl ProxySettings {
    /// Native launches should go through the proxy (trim on and routing allowed).
    pub fn routes_native(&self) -> bool {
        self.enabled && self.trim && self.trim_route_native
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "graphite".into(),
            prefix: "ctrl+space".into(),
            shell: None,
            nerd_font: false,
            usage_expanded: false,
            sidebar_width: None,
            icons: "auto".into(),
            rain: RainMode::Off,
            font: None,
            bridge: BridgeSettings::default(),
            proxy: ProxySettings::default(),
            voice: VoiceSettings::default(),
            insights: InsightsSettings::default(),
            auto_resume: AutoResumeSettings::default(),
            auto_approve: false,
            summary_screensaver_secs: 0,
            permission: Default::default(),
            claude_env: Default::default(),
            caveman: Default::default(),
            justtools: Default::default(),
            justwatch: Default::default(),
            updates: Default::default(),
            watchdog: Default::default(),
            keys: BTreeMap::new(),
        }
    }
}
impl Default for VoiceSettings {
    fn default() -> Self {
        Self { enabled: true, hotkey: None, api_key_env: "OPENAI_API_KEY".into(), model: None, language: None, mode: "live".into(), live_voice: None, live_model: None }
    }
}
impl Default for BridgeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 10001,
            automatic_port: true,
            bind: "0.0.0.0".into(),
            web_interface: true,
            remote_min_cols: 80,
            remote_min_rows: 50,
            remote_min_grace_secs: 15,
        }
    }
}
impl Default for ProxySettings {
    fn default() -> Self {
        Self { enabled: true, port: 3458, trim: false, trim_route_native: true, trim_level: "safe".into() }
    }
}

impl Settings {
    /// Missing or malformed file → defaults (never fails).
    pub fn load() -> Settings {
        std::fs::read_to_string(paths::settings_path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Atomic write of `~/.bro/v2.toml`.
    pub fn save(&self) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self).context("serializing settings")?;
        let path = paths::settings_path();
        atomic_write(&path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
    }
}

/// v1 `state.json` — last choices, used to pre-select the launcher.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
impl State {
    /// Missing or malformed → empty state.
    pub fn load() -> State {
        match read_json(&paths::state_path()) {
            Some(Value::Object(extra)) => State { extra },
            _ => State::default(),
        }
    }

    /// Atomic write, 2-space JSON like v1.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::state_path();
        write_json_pretty(&path, &self.extra).with_context(|| format!("writing {}", path.display()))
    }

    fn map_get(&self, map: &str, key: &str) -> Option<String> {
        self.extra.get(map)?.get(key)?.as_str().map(str::to_string)
    }

    fn map_set(&mut self, map: &str, key: &str, value: &str) {
        let entry = self.extra.entry(map.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        if let Value::Object(m) = entry {
            m.insert(key.to_string(), Value::String(value.to_string()));
        }
    }

    /// v1 `lastProvider`.
    pub fn last_provider(&self) -> Option<String> {
        self.extra.get("lastProvider")?.as_str().map(str::to_string)
    }
    /// v1 `lastHarness`.
    pub fn last_harness(&self) -> Option<String> {
        self.extra.get("lastHarness")?.as_str().map(str::to_string)
    }
    /// v1 `lastModelFor`.
    pub fn last_model_for(&self, provider_id: &str) -> Option<String> {
        self.map_get("lastModelByProvider", provider_id)
    }
    /// v1 `lastProfileFor`.
    pub fn last_profile_for(&self, provider_id: &str) -> Option<String> {
        self.map_get("lastProfileByProvider", provider_id)
    }
    /// v1 `rememberSelection` (in memory; call [`State::save`]).
    pub fn remember_selection(&mut self, provider_id: &str, model: Option<&str>, harness: Option<&str>) {
        self.extra.insert("lastProvider".into(), Value::String(provider_id.into()));
        self.map_set("lastModelByProvider", provider_id, model.unwrap_or(""));
        if let Some(h) = harness {
            self.extra.insert("lastHarness".into(), Value::String(h.into()));
        }
    }
    /// v1 `rememberHarness`.
    pub fn remember_harness(&mut self, harness: &str) {
        self.extra.insert("lastHarness".into(), Value::String(harness.into()));
    }
    /// v1 `rememberProfile`.
    pub fn remember_profile(&mut self, provider_id: &str, profile: &str) {
        self.map_set("lastProfileByProvider", provider_id, profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn config_round_trip_preserves_unknown_keys_and_order() {
        let sb = sandbox();
        std::fs::create_dir_all(sb.bro()).unwrap();
        let raw = r##"{
  "#": "note",
  "defaultHarness": "claude",
  "keys": { "#sakana": "fish_xxx", "openrouter": "sk-or", "weird": 5 },
  "providers": [ { "id": "#my-local", "mode": "openai" } ],
  "zzz": { "nested": [1, 2] }
}"##;
        std::fs::write(paths::config_path(), raw).unwrap();
        let mut cfg = Config::load().unwrap();
        assert_eq!(cfg.keys.get("openrouter").map(String::as_str), Some("sk-or"));
        assert_eq!(cfg.extra["defaultHarness"], "claude");
        cfg.set_key("zai", Some("z-key"));
        cfg.save().unwrap();

        let back: Value = serde_json::from_str(&std::fs::read_to_string(paths::config_path()).unwrap()).unwrap();
        let top: Vec<&String> = back.as_object().unwrap().keys().collect();
        assert_eq!(top, ["#", "defaultHarness", "keys", "providers", "zzz"]);
        assert_eq!(back["zzz"]["nested"][1], 2);
        assert_eq!(back["providers"][0]["id"], "#my-local");
        let keys: Vec<&String> = back["keys"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["#sakana", "openrouter", "weird", "zai"]);
        assert_eq!(Config::load().unwrap().key_for("zai", None).as_deref(), Some("z-key"));
    }

    #[test]
    fn key_for_tier_and_env() {
        let _sb = sandbox();
        let mut cfg = Config::default();
        cfg.set_key("openlux", Some("plain"));
        assert_eq!(cfg.key_for("openlux@Codex-1", None).as_deref(), Some("plain"));
        cfg.set_key("openlux@Codex-1", Some("tiered"));
        assert_eq!(cfg.key_for("openlux@Codex-1", None).as_deref(), Some("tiered"));
        unsafe { std::env::set_var("BRO_TEST_KEY_ENV", "from-env") };
        assert_eq!(cfg.key_for("nope", Some("BRO_TEST_KEY_ENV")).as_deref(), Some("from-env"));
        unsafe { std::env::remove_var("BRO_TEST_KEY_ENV") };
        assert!(cfg.key_for("nope", Some("BRO_TEST_KEY_ENV")).is_none());
    }

    #[test]
    fn malformed_config_is_an_error_missing_is_default() {
        let sb = sandbox();
        assert!(Config::load().unwrap().keys.is_empty());
        std::fs::create_dir_all(sb.bro()).unwrap();
        std::fs::write(paths::config_path(), "{ nope").unwrap();
        assert!(Config::load().is_err());
    }

    #[test]
    fn settings_defaults_and_round_trip() {
        let _sb = sandbox();
        let s = Settings::load();
        assert_eq!(s.theme, "graphite");
        assert_eq!(s.prefix, "ctrl+space");
        assert!(!s.nerd_font);
        assert_eq!(s.sidebar_width, None);
        let mut s2 = s.clone();
        s2.theme = "dark".into();
        s2.sidebar_width = Some(58);
        assert_eq!(s2.permission, crate::launch::Permission::Skip, "unattended unless you pick another mode");
        s2.permission = crate::launch::Permission::Auto;
        s2.keys.insert("palette".into(), "ctrl+k".into());
        assert_eq!(s2.claude_env.preset, "standard");
        s2.claude_env.use_preset("private");
        s2.claude_env.overrides.insert("telemetry".into(), "default".into());
        s2.claude_env.extra.insert("FOO".into(), "bar".into());
        s2.save().unwrap();
        let back = Settings::load();
        assert_eq!(back.claude_env, s2.claude_env);
        assert_eq!(back.rain, RainMode::Off);
        let w: Settings = toml::from_str("rain = \"working\"").unwrap();
        assert_eq!(w.rain, RainMode::Working);
        assert_eq!(back.theme, "dark");
        assert_eq!(back.sidebar_width, Some(58));
        assert_eq!(back.permission, crate::launch::Permission::Auto);
        assert_eq!(back.keys["palette"], "ctrl+k");
        assert_eq!(back.bridge.port, 10001);
        assert_eq!((back.bridge.remote_min_cols, back.bridge.remote_min_rows, back.bridge.remote_min_grace_secs), (80, 50, 15));
        assert!(!back.proxy.trim && back.proxy.trim_route_native && back.proxy.trim_level == "safe");
        assert!(!back.proxy.routes_native(), "trim is off by default");
        std::fs::write(paths::settings_path(), "[proxy]\ntrim = true\n").unwrap();
        let trimmed = Settings::load();
        assert!(trimmed.proxy.trim && trimmed.proxy.enabled && trimmed.proxy.port == 3458);
        assert!(trimmed.proxy.routes_native(), "older files without the new keys get the defaults");
        std::fs::write(paths::settings_path(), "[bridge]\nport = 10002\nremote_min_rows = 0\n").unwrap();
        let bridge = Settings::load().bridge;
        assert_eq!((bridge.port, bridge.remote_min_cols, bridge.remote_min_rows, bridge.remote_min_grace_secs), (10002, 80, 0, 15));
        std::fs::write(paths::settings_path(), "theme = 3").unwrap();
        assert_eq!(Settings::load().theme, "graphite");
    }

    #[test]
    fn state_round_trip() {
        let _sb = sandbox();
        let mut st = State::load();
        st.extra.insert("jevRouting".into(), Value::Bool(true));
        st.remember_selection("openrouter", Some("x/y"), Some("claude"));
        st.remember_profile("codex", "work");
        st.save().unwrap();
        let back = State::load();
        assert_eq!(back.last_provider().as_deref(), Some("openrouter"));
        assert_eq!(back.last_model_for("openrouter").as_deref(), Some("x/y"));
        assert_eq!(back.last_harness().as_deref(), Some("claude"));
        assert_eq!(back.last_profile_for("codex").as_deref(), Some("work"));
        assert_eq!(back.extra["jevRouting"], true);
    }
}
