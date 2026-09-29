//! `~/.bro/config.json` and `~/.bro/state.json` (v1 schema, unknown keys preserved)
//! plus `~/.bro/v2.toml` for v2-only settings (keybindings, theme, bridge, proxy).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// v1 `config.json`. Unknown fields are kept in `extra` and written back untouched.
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
    pub fn load() -> anyhow::Result<Config> { todo!() }
    /// Atomic write (temp + rename), preserving unknown keys.
    pub fn save(&self) -> anyhow::Result<()> { todo!() }
    /// Resolve a key: config.keys[id] then the provider's keyEnv env var.
    pub fn key_for(&self, provider_id: &str, key_env: Option<&str>) -> Option<String> { todo!() }
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
    pub bridge: BridgeSettings,
    pub proxy: ProxySettings,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySettings {
    pub enabled: bool,
    /// 0 = pick a free port
    pub port: u16,
}

impl Default for Settings { fn default() -> Self { todo!() } }
impl Default for BridgeSettings {
    fn default() -> Self { Self { enabled: true, port: 10001, automatic_port: true, bind: "0.0.0.0".into(), web_interface: true } }
}
impl Default for ProxySettings { fn default() -> Self { Self { enabled: true, port: 3458 } } }

impl Settings {
    pub fn load() -> Settings { todo!() }
    pub fn save(&self) -> anyhow::Result<()> { todo!() }
}

/// v1 `state.json` — last choices, used to pre-select the launcher.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
impl State {
    pub fn load() -> State { todo!() }
    pub fn save(&self) -> anyhow::Result<()> { todo!() }
}
