//! Browser integration for launched harnesses (v1 claude-browser.js, chrome-mcp.js,
//! mcp-chrome-server.js): the args/env/MCP config a harness needs to drive the
//! user's browser.
use serde::{Deserialize, Serialize};

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

/// Extra (args, env) for a launch. Empty when Off or unsupported for the harness.
pub fn launch_additions(
    mode: BrowserMode,
    harness: crate::Harness,
    profile_dir: Option<&std::path::Path>,
) -> (Vec<String>, Vec<(String, String)>) {
    todo!()
}
