//! One browser/desktop policy for every harness, independent of the legacy browser launcher.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub justbrowse: bool,
    pub cua: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            justbrowse: true,
            cua: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Active {
    pub browser: bool,
    pub desktop: Option<PathBuf>,
    pub browser_off: bool,
    pub desktop_off: bool,
    installed: bool,
}

impl Active {
    pub fn for_launch(ctx: &crate::launch::LaunchCtx) -> Self {
        ctx.automation.as_ref().map(Self::detect).unwrap_or_else(Self::load)
    }

    pub fn load() -> Self {
        let settings = crate::config::Settings::load().automation;
        Self::detect(&settings)
    }

    pub fn detect(settings: &Settings) -> Self {
        let browser = crate::justbrowse::installed().is_some();
        let desktop = crate::cua::installed();
        Self {
            installed: browser || desktop.is_some(),
            browser: settings.justbrowse && browser,
            desktop: if settings.cua { desktop } else { None },
            browser_off: !settings.justbrowse,
            desktop_off: !settings.cua,
        }
    }

    pub fn managed(&self) -> bool {
        self.installed
    }

    pub fn prompt(&self, harness: crate::Harness) -> String {
        if !self.managed() {
            return String::new();
        }
        let mut text = String::from(
            "Bro manages browser and computer use for this session. Use only the Bro-configured tools below for UI work. Built-in Claude/Codex browser and computer tools are disabled. Do not enable or install another browser or desktop integration as a fallback. If a tool is disconnected, report the problem and ask the user to reconnect it in Bro's Browser and computer settings.\n",
        );
        if self.browser {
            text.push_str("When a browser page needs the user's interaction, call chrome_request_handoff with a short reason on your owned tab and pass your BRO_SESSION_ID as broSessionId. Bro opens that exact signed-in tab on mobile or desktop. A handoff is a request for help, not a blocker: keep working and inspect the page to check whether the user has finished, even while phase is requested, connecting or human. Continue the original task as soon as the page shows it is ready; Done is optional. Use chrome_wait_for_handoff with timeoutMs:0 when you need handoff status. A [browser handoff ...] message notifies you that the user pressed Done: recheck the named tab if the task still needs it, without repeating completed actions. A disconnected viewer leaves the request visible and does not prevent agent work. If the tab closed, recover the task from your remaining owned tabs. Never copy cookies or a live profile into another browser to perform the handoff.\n");
            if matches!(harness, crate::Harness::Pi | crate::Harness::Omp) {
                text.push_str("For browser work use `bro browse` to list JustBrowse tools, and `bro browse <tool> key=value` or `bro browse <tool> '<JSON object>'` to call them. Bro retains a separate MCP connection for your BRO_SESSION_ID across calls. Images are saved to files you can read. Start with chrome_navigate without a tabId to open a background tab of your own. Keep all calls on your owned tabs and close them when finished.\n");
            } else {
                text.push_str("For browser work use the justbrowse MCP server and the justbrowse-browser skill. Start with chrome_navigate without a tabId, keep work in your owned background tabs, and close those tabs when finished.\n");
            }
        }
        if let Some(binary) = &self.desktop {
            text.push_str(&format!(
                "For desktop apps use Cua Driver ({}), its cua-driver skill and get_guide guidance. ",
                binary.display()
            ));
            if matches!(harness, crate::Harness::Pi | crate::Harness::Omp) {
                text.push_str("Use `cua-driver call <tool> '<JSON object>'` through the shell; `cua-driver --help` lists commands.\n");
            } else {
                text.push_str("Use the cua-driver MCP server.\n");
            }
            if cfg!(windows) {
                text.push_str("For Windows work that must not interrupt the user, first reuse an inactive, non-minimized window. Background does not require minimizing: start_minimized:true can hide UWP/WinUI controls and pixels. An empty tree in that state does not establish that the app is unsupported. Use only driver-supported launch or restore routes that preserve focus; if unavailable, report the limitation. Do not silently activate a window or retry in foreground. Act by fresh element_token with delivery_mode:\"background\" and foreground_fallback:false where supported. Verify the requested result by reading app-owned content; a selected flag or moved window alone does not prove a workflow succeeded. Doctor and health checks establish driver prerequisites, not app-specific automation.\n");
            }
            if !self.browser {
                text.push_str("Browser UI work also uses Cua Driver.\n");
            }
        } else {
            text.push_str("Desktop automation is unavailable until CUA is installed and enabled in Bro.\n");
        }
        if !self.browser && self.desktop.is_none() {
            text.push_str("Browser automation is unavailable until JustBrowse or CUA is enabled in Bro.\n");
        }
        text
    }

    pub fn prompt_file(&self, harness: crate::Harness) -> std::io::Result<PathBuf> {
        let text = self.prompt(harness);
        let file = crate::paths::bro_dir()
            .join("automation")
            .join(format!("instructions-{}.md", crate::util::short_hash(&text, 16)));
        crate::util::atomic_write(&file, text.as_bytes())?;
        Ok(file)
    }

    pub fn cua_mcp_config(&self, home: &Path) -> std::io::Result<PathBuf> {
        let file = home.join("bro-browser/cua-mcp.json");
        crate::util::write_json_pretty(
            &file,
            &serde_json::json!({"mcpServers": {
                crate::cua::SERVER_NAME: {"type": "stdio", "command": self.desktop, "args": ["mcp"]}
            }}),
        )?;
        Ok(file)
    }
}

/// Public Codex feature switches plus the desktop plugins that can inject competing UI tools.
pub fn codex_native_off() -> Vec<String> {
    let features = ["computer_use", "browser_use", "browser_use_external", "in_app_browser"];
    let mut settings: Vec<String> = features.into_iter().map(|feature| format!("features.{feature}=false")).collect();
    for plugin in ["unified-computer-use", "computer-use", "browser", "chrome"] {
        settings.push(format!("plugins.\"{plugin}@openai-bundled\".enabled=false"));
    }
    let mut args: Vec<String> = settings.into_iter().flat_map(|value| ["-c".into(), value]).collect();
    // Codex applies --enable after -c overrides. --disable wins over either, including saved
    // launch arguments that enabled a native UI backend before Bro owned the integration.
    for feature in features {
        args.extend(["--disable".into(), feature.into()]);
    }
    args
}

pub const CLAUDE_NATIVE_DENY: &str = "mcp__computer,mcp__computer-use,mcp__cua,mcp__claude-in-chrome,mcp__Claude_Browser,mcp__remote-devices__computer_*,mcp__remote-devices__Claude_Browser_*";

/// Commander keeps the last variadic --disallowedTools. Merge existing denials before adding
/// Bro's so an extra flag cannot drop either list. Values after a literal -- are always prompts.
pub fn claude_denials(args: &mut Vec<String>, extra: &[&str]) {
    let mut denied: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        let name = ["--disallowedTools", "--disallowed-tools"]
            .into_iter()
            .find(|name| args[i] == *name || args[i].starts_with(&format!("{name}=")));
        if let Some(name) = name {
            let value = args.remove(i);
            if let Some(value) = value.strip_prefix(&format!("{name}=")) {
                denied.push(value.into());
            } else {
                while i < args.len() && !args[i].starts_with('-') {
                    denied.push(args.remove(i));
                }
            }
        } else {
            i += 1;
        }
    }
    denied.extend(extra.iter().map(|s| s.to_string()));
    if !denied.is_empty() {
        let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
        // Equals form keeps a following positional prompt out of the variadic flag.
        args.insert(end, format!("--disallowedTools={}", denied.join(",")));
    }
}
