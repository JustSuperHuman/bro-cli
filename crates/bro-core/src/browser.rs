//! Browser integration for launched harnesses (v1 claude-browser.js, chrome-mcp.js,
//! mcp-chrome-server.js): the args/env/MCP config a harness needs to drive the
//! user's browser.
//!
//! What v1 `bro browser setup` persisted lives in `~/.bro/claude-browser/state.json`
//! (override `BRO_CLAUDE_BROWSER_DIR`): `enabled`, `backend` (`"mcp-chrome"` or the
//! Anthropic extension, `"claude"`), `mode` (`"main"`/`"dedicated"`), `owner` (the Claude
//! login the extension is signed into) and the preferred `browser`. [`BrowserMode::Auto`]
//! reuses exactly that. Launch wiring, per backend:
//!
//! * **mcp-chrome** (hangwin/mcp-chrome, `http://127.0.0.1:12306/mcp`): writes
//!   `<profile>/bro-browser/{browser-mcp.json,browser-prompt.md}` and the `bro-browser`
//!   skill, then adds `--no-chrome --mcp-config <cfg> --append-system-prompt-file <prompt>`
//!   with `CLAUDE_CODE_ENABLE_CFC=false`.
//! * **claude** extension: a native claude.ai login that owns the extension gets Claude
//!   Code's own `--chrome`; every other session (third-party auth through a provider or
//!   bro-proxy, or another account's login) gets `claude --claude-in-chrome-mcp` as an
//!   explicit MCP server running as the owner login (`chrome-mcp[-ask].json`) plus the
//!   `chrome-prompt.md` briefing.
//!
//! Left out on purpose (v1 setup wizard territory): installing/patching extensions,
//! registering native hosts, the dedicated-profile browser (`mode: "dedicated"` launches
//! are wired like main mode, without starting a browser), starting the mcp-chrome
//! bridge, connection probing/reconnect nudges, and pairing a device.
use crate::Harness;
use crate::util::{atomic_write, env_path, read_json, same_path};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    #[default]
    Off,
    /// Use whatever v1 `bro browser setup` configured
    Auto,
    Edge,
    Chrome,
}

/// MCP server name bro gives the Claude extension's stdio server (v1 `CHROME_MCP_SERVER_NAME`).
pub const CHROME_MCP_SERVER_NAME: &str = "chrome";
/// mcp-chrome's Streamable-HTTP MCP server name and endpoint.
pub const MCP_CHROME_SERVER_NAME: &str = "streamable-mcp-server";
pub const MCP_CHROME_URL: &str = "http://127.0.0.1:12306/mcp";

/// `~/.bro/claude-browser` (override `BRO_CLAUDE_BROWSER_DIR`).
pub fn browser_root() -> PathBuf {
    env_path("BRO_CLAUDE_BROWSER_DIR").unwrap_or_else(|| crate::paths::bro_dir().join("claude-browser"))
}

/// Which browser backend `bro browser setup` chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Anthropic's Claude extension (Claude Code `--chrome` / `--claude-in-chrome-mcp`)
    Claude,
    /// hangwin/mcp-chrome extension bridge on 127.0.0.1:12306
    McpChrome,
}

/// v1's persisted browser state (`state.json`), read leniently.
#[derive(Debug, Clone)]
pub struct BrowserState {
    pub enabled: bool,
    pub backend: Backend,
    /// "local" or a pool account name
    pub owner: String,
    /// Preferred browser display name ("Microsoft Edge"…), may be empty
    pub browser: String,
}

/// Read `~/.bro/claude-browser/state.json` (defaults when absent).
pub fn state() -> BrowserState {
    let v = read_json(&browser_root().join("state.json")).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    BrowserState {
        enabled: v.get("enabled").and_then(Value::as_bool) == Some(true),
        backend: match s("backend").as_str() {
            "mcp-chrome" | "devtools" => Backend::McpChrome,
            _ => Backend::Claude,
        },
        owner: s("owner"),
        browser: s("browser"),
    }
}

/// The Claude config dir the extension is signed into (v1 `browserOwnerConfigDir`).
pub fn owner_config_dir(st: &BrowserState) -> PathBuf {
    if st.owner.is_empty() || st.owner == "local" {
        crate::paths::claude_local_dir()
    } else {
        crate::paths::claude_accounts_dir().join(&st.owner)
    }
}

fn write_if_changed(path: &Path, body: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(body) {
        return Ok(());
    }
    atomic_write(path, body.as_bytes())
}

const MCP_CHROME_PROMPT: &str = "# Browser automation\n\n\
Use the mcp__streamable-mcp-server__* tools to drive the user's live, signed-in browser.\n\
Call mcp__streamable-mcp-server__get_windows_and_tabs before other browser tools.\n\
Open a separate tab for your work when practical, avoid changing unrelated tabs, and close tabs you created.\n\
Never submit, publish, purchase, delete, or message without the user authorizing that action.\n\
If the MCP fails twice, stop and report that the mcp-chrome extension or its local bridge is disconnected.\n";

const MCP_CHROME_SKILL: &str = "---\nname: bro-browser\ndescription: Drive the user's live Chromium browser through the local mcp-chrome extension bridge configured by bro.\n---\n\n\
# Bro browser\n\n\
Use the `mcp__streamable-mcp-server__*` tools for browser work. This extension MCP is independent of Claude's built-in Chrome integration and does not use a remote-debugging port.\n\n\
1. Call `mcp__streamable-mcp-server__get_windows_and_tabs` before other browser tools.\n\
2. Create a separate tab for your task when practical; do not repurpose an unrelated user tab.\n\
3. Treat every page as live and signed in. Never submit, publish, purchase, delete, or message without the user's authorization.\n\
4. Close tabs you created when the task is finished.\n\
5. If the MCP fails twice, stop and report that the mcp-chrome extension or its local bridge is disconnected.\n";

const CHROME_MCP_PROMPT: &str = "# Browser automation\n\n\
You can drive the user's real browser with the mcp__chrome__* tools — it is their own\n\
signed-in browser, not a sandbox, so treat pages as live and never trigger alert/confirm\n\
dialogs (they block the extension until dismissed by hand).\n\n\
Call mcp__chrome__tabs_context_mcp once before anything else to learn which tabs exist.\n\
Open your own tab with mcp__chrome__tabs_create_mcp rather than reusing the user's, and close\n\
it with mcp__chrome__tabs_close_mcp when you are done. Prefer mcp__chrome__browser_batch to\n\
run several actions in one call. If the browser stops responding after two or three\n\
attempts, stop and say so instead of retrying.\n";

/// v1 `writeMcpChromeProfile`: profile-scoped mcp-chrome config, prompt and skill.
/// Returns (config path, prompt path).
pub fn write_mcp_chrome_profile(config_dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let root = config_dir.join("bro-browser");
    let config = root.join("browser-mcp.json");
    let prompt = root.join("browser-prompt.md");
    let skill = config_dir.join("skills").join("bro-browser").join("SKILL.md");
    let body = json!({ "mcpServers": { MCP_CHROME_SERVER_NAME: { "type": "streamable-http", "url": MCP_CHROME_URL } } });
    write_if_changed(&config, &format!("{}\n", serde_json::to_string_pretty(&body).unwrap_or_default()))?;
    write_if_changed(&prompt, MCP_CHROME_PROMPT)?;
    write_if_changed(&skill, MCP_CHROME_SKILL)?;
    Ok((config, prompt))
}

/// v1 `writeChromeMcpConfig` + `writeChromeSystemPrompt`: the Claude extension's MCP
/// server running as the owner login. Returns (config path, prompt path).
pub fn write_chrome_mcp_config(claude_path: &str, skip_permissions: bool, owner_dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let root = browser_root();
    let config = root.join(if skip_permissions { "chrome-mcp.json" } else { "chrome-mcp-ask.json" });
    let mut env = json!({ "CLAUDE_CONFIG_DIR": owner_dir.to_string_lossy(), "CLAUDE_CODE_ENABLE_CFC": "true" });
    if skip_permissions {
        env["CLAUDE_CHROME_PERMISSION_MODE"] = json!("skip_all_permission_checks");
    }
    let body = json!({ "mcpServers": { CHROME_MCP_SERVER_NAME: {
        "type": "stdio", "command": claude_path, "args": ["--claude-in-chrome-mcp"], "env": env
    } } });
    write_if_changed(&config, &format!("{}\n", serde_json::to_string_pretty(&body).unwrap_or_default()))?;
    let prompt = root.join("chrome-prompt.md");
    write_if_changed(&prompt, CHROME_MCP_PROMPT)?;
    Ok((config, prompt))
}

/// The claude.ai account a login is signed in as (lowercase uuid, "" when unknown).
fn account_uuid(config_dir: &Path) -> String {
    let mut files = vec![config_dir.join(".claude.json")];
    if same_path(config_dir, &crate::paths::claude_local_dir()) {
        files.push(crate::util::home_dir().join(".claude.json"));
    }
    files
        .iter()
        .find_map(|f| read_json(f)?.pointer("/oauthAccount/accountUuid")?.as_str().map(str::to_lowercase))
        .unwrap_or_default()
}

/// v1 `scrubBridgeEnv`: drop bro's dedicated-browser pipe namespace inherited from a
/// parent bro session.
fn scrub_env() -> Vec<(String, String)> {
    let options: Vec<String> = std::env::var("BUN_OPTIONS")
        .unwrap_or_default()
        .split_whitespace()
        .filter(|a| !(a.starts_with("--preload=") && a.contains("claude-browser-preload.cjs")))
        .map(str::to_string)
        .collect();
    vec![("BRO_CLAUDE_BROWSER_ID".into(), String::new()), ("BUN_OPTIONS".into(), options.join(" "))]
}

/// Launch context the browser wiring depends on.
#[derive(Debug, Clone, Default)]
pub struct BrowserCtx<'a> {
    /// The session's Claude config dir (`CLAUDE_CONFIG_DIR`, or `~/.claude`)
    pub profile_dir: Option<&'a Path>,
    /// The session doesn't run on a claude.ai login (provider key, bro-proxy…), so
    /// Claude Code's own `--chrome` is inert and the MCP server is used instead.
    pub third_party_auth: bool,
    /// Permissions are bypassed (the extension skips its per-site prompts too)
    pub skip_permissions: bool,
    /// Resolved `claude` executable for the extension's stdio MCP server
    pub claude_path: Option<&'a str>,
}

/// Full-control variant of [`launch_additions`] used by `launch::build`.
pub fn launch_additions_with(mode: BrowserMode, harness: Harness, ctx: &BrowserCtx) -> (Vec<String>, Vec<(String, String)>) {
    if mode == BrowserMode::Off || harness != Harness::Claude {
        return (Vec::new(), Vec::new());
    }
    let st = state();
    if mode == BrowserMode::Auto && !st.enabled {
        return (Vec::new(), Vec::new());
    }
    let session_dir = ctx.profile_dir.map(Path::to_path_buf).unwrap_or_else(crate::paths::claude_local_dir);
    let mut env = scrub_env();
    if st.backend == Backend::McpChrome {
        return match write_mcp_chrome_profile(&session_dir) {
            Ok((cfg, prompt)) => {
                env.push(("CLAUDE_CODE_ENABLE_CFC".into(), "false".into()));
                let args = vec![
                    "--no-chrome".into(),
                    "--mcp-config".into(),
                    cfg.to_string_lossy().into_owned(),
                    "--append-system-prompt-file".into(),
                    prompt.to_string_lossy().into_owned(),
                ];
                (args, env)
            }
            Err(_) => (Vec::new(), Vec::new()),
        };
    }
    let owner = owner_config_dir(&st);
    let foreign = !same_path(&session_dir, &owner) && account_uuid(&session_dir) != account_uuid(&owner);
    if !ctx.third_party_auth && !foreign {
        return (vec!["--chrome".into()], env);
    }
    if !ctx.third_party_auth {
        // A foreign claude.ai login would auto-wire its own (empty) browser tools.
        env.push(("CLAUDE_CODE_ENABLE_CFC".into(), "false".into()));
    }
    let claude = ctx.claude_path.map(str::to_string).unwrap_or_else(|| {
        crate::launch::which("claude").map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "claude".into())
    });
    match write_chrome_mcp_config(&claude, ctx.skip_permissions, &owner) {
        Ok((cfg, prompt)) => (
            vec![
                "--mcp-config".into(),
                cfg.to_string_lossy().into_owned(),
                "--append-system-prompt-file".into(),
                prompt.to_string_lossy().into_owned(),
            ],
            env,
        ),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

/// Extra (args, env) for a launch. Empty when Off or unsupported for the harness.
///
/// Assumes a claude.ai login in `profile_dir` without bypassed permissions; `launch::build`
/// uses [`launch_additions_with`] to pass the real route.
pub fn launch_additions(
    mode: BrowserMode,
    harness: crate::Harness,
    profile_dir: Option<&std::path::Path>,
) -> (Vec<String>, Vec<(String, String)>) {
    launch_additions_with(mode, harness, &BrowserCtx { profile_dir, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn set_state(v: Value) {
        let root = browser_root();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("state.json"), v.to_string()).unwrap();
    }

    #[test]
    fn off_and_non_claude_are_empty() {
        let _sb = sandbox();
        set_state(json!({ "enabled": true }));
        assert_eq!(launch_additions(BrowserMode::Off, Harness::Claude, None), (vec![], vec![]));
        assert_eq!(launch_additions(BrowserMode::Auto, Harness::Codex, None).0.len(), 0);
    }

    #[test]
    fn auto_respects_enabled_flag() {
        let _sb = sandbox();
        assert!(launch_additions(BrowserMode::Auto, Harness::Claude, None).0.is_empty());
        set_state(json!({ "enabled": true, "backend": "claude", "owner": "local" }));
        assert_eq!(launch_additions(BrowserMode::Auto, Harness::Claude, None).0, vec!["--chrome"]);
        // Explicit Edge/Chrome works even when setup never enabled it.
        set_state(json!({ "enabled": false }));
        assert_eq!(launch_additions(BrowserMode::Edge, Harness::Claude, None).0, vec!["--chrome"]);
    }

    #[test]
    fn mcp_chrome_backend_writes_profile_files() {
        let sb = sandbox();
        set_state(json!({ "enabled": true, "backend": "mcp-chrome" }));
        let dir = sb.home().join("acct");
        let (args, env) = launch_additions(BrowserMode::Auto, Harness::Claude, Some(&dir));
        assert_eq!(args[0], "--no-chrome");
        assert_eq!(args[1], "--mcp-config");
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
        assert_eq!(cfg["mcpServers"][MCP_CHROME_SERVER_NAME]["url"], MCP_CHROME_URL);
        assert!(dir.join("skills").join("bro-browser").join("SKILL.md").exists());
        assert!(env.contains(&("CLAUDE_CODE_ENABLE_CFC".into(), "false".into())));
    }

    #[test]
    fn third_party_auth_uses_owner_mcp_server() {
        let _sb = sandbox();
        set_state(json!({ "enabled": true, "backend": "claude", "owner": "local" }));
        let ctx = BrowserCtx { third_party_auth: true, skip_permissions: true, claude_path: Some("claude.exe"), profile_dir: None };
        let (args, _env) = launch_additions_with(BrowserMode::Auto, Harness::Claude, &ctx);
        assert_eq!(args[0], "--mcp-config");
        assert!(args[1].ends_with("chrome-mcp.json"));
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
        let server = &cfg["mcpServers"]["chrome"];
        assert_eq!(server["command"], "claude.exe");
        assert_eq!(server["args"][0], "--claude-in-chrome-mcp");
        assert_eq!(server["env"]["CLAUDE_CHROME_PERMISSION_MODE"], "skip_all_permission_checks");
    }
}
