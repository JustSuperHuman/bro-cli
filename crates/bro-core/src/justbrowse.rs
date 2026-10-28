//! JustBrowseMCP (github.com/JustSuperHuman/mcp-chrome-but-good): your own signed-in Chrome, Edge or Brave as an
//! MCP server (`justbrowse`, `http://127.0.0.1:12306/mcp`), each agent session in tabs of its own.
//!
//! Its own installer (Quick Install runs it) puts the extension and the native bridge in
//! `%LOCALAPPDATA%\JustBrowseMCP` (macOS `~/Library/Application Support/JustBrowseMCP`, Linux
//! `~/.local/share/justbrowse-mcp`), registers the native host, copies the `justbrowse-browser` skill into the
//! agent homes and adds the MCP server to Claude Code and Codex. Loading the unpacked extension is the one step
//! left to you: browsers only take one by hand.
use crate::util::{env_path, home_dir, read_json};
use std::path::PathBuf;

pub const REPO: &str = "JustSuperHuman/mcp-chrome-but-good";
/// The released extension's ID (its manifest key fixes it wherever it's unpacked).
pub const EXTENSION_ID: &str = "dgpjcgoidnfedcpaijeelljbppcfhcll";
/// The MCP server's name and its Streamable HTTP endpoint.
pub const SERVER_NAME: &str = "justbrowse";
pub const MCP_URL: &str = "http://127.0.0.1:12306/mcp";

/// Where the installer puts it (override `JUSTBROWSE_DIR`).
pub fn install_dir() -> PathBuf {
    if let Some(dir) = env_path("JUSTBROWSE_DIR") {
        return dir;
    }
    if cfg!(windows) {
        env_path("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local")).join("JustBrowseMCP")
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library").join("Application Support").join("JustBrowseMCP")
    } else {
        env_path("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local").join("share")).join("justbrowse-mcp")
    }
}

/// The unpacked extension to load in the browser.
pub fn extension_dir() -> PathBuf {
    install_dir().join("extension")
}

/// The shell command that installs (or updates) it: the newest release, checked against its SHA-256.
pub fn installer_command() -> String {
    if cfg!(windows) {
        format!("irm https://raw.githubusercontent.com/{REPO}/master/install.ps1 | iex")
    } else {
        format!("curl -fsSL https://raw.githubusercontent.com/{REPO}/master/install.sh | bash")
    }
}

/// An installed JustBrowseMCP, from the installer's `installed.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub version: Option<String>,
}

/// What's installed (a file read; no network).
pub fn installed() -> Option<Installed> {
    let dir = install_dir();
    let info = read_json(&dir.join("installed.json"));
    if info.is_none() && !dir.join("bridge").join("dist").join("cli.js").is_file() {
        return None;
    }
    let version = info.as_ref().and_then(|v| v.get("version")).and_then(|v| v.as_str()).map(str::to_string);
    Some(Installed { version })
}

/// What to do after the installer ran, for the terminal.
pub fn after_install() -> String {
    format!(
        "load the extension once: chrome://extensions (edge://extensions) → Developer mode → Load unpacked → {} · then Connect in its popup",
        extension_dir().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn installed_reads_the_installers_record() {
        let sb = sandbox();
        // SAFETY: the sandbox holds the env lock
        unsafe { std::env::set_var("JUSTBROWSE_DIR", sb.home().join("jb")) };
        assert_eq!(installed(), None);
        std::fs::create_dir_all(install_dir()).unwrap();
        std::fs::write(install_dir().join("installed.json"), r#"{"version":"1.1.0","extensionId":"dgpjcgoidnfedcpaijeelljbppcfhcll"}"#).unwrap();
        assert_eq!(installed(), Some(Installed { version: Some("1.1.0".into()) }));
        assert!(after_install().contains("Load unpacked") && extension_dir().ends_with("extension"));
        assert!(installer_command().contains(REPO));
    }
}
