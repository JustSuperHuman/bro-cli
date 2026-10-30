//! bro-core — the blocking data layer shared by the TUI, proxy and bridge.
//!
//! Everything here is synchronous (ureq, std::fs). Callers that must not block
//! (the TUI render loop, tokio tasks) run these on a worker thread /
//! `spawn_blocking`. Storage is **v1-compatible**: bro v1 (Node) and v2 read and
//! write the same files, so both can be installed side by side.
//!
//! Contract rule: the public items declared in this file and its modules are the
//! API other crates build against. Add freely; do not rename or remove.

pub mod util;
pub mod audio;
pub mod http;
pub mod paths;
pub mod config;
pub mod providers;
pub mod profiles;
pub mod creds;
pub mod gemini;
pub mod gemini_api;
pub mod sessions;
pub mod recap;
pub mod projects;
pub mod usage;
pub mod launch;
pub mod browser;
pub mod skills;
pub mod skill_repos;
pub mod skill_inventory;
pub mod skill_market;
pub mod disabled_skills;
pub mod justtools;
pub mod claude_shared;
pub mod catalogue;
pub mod quips;
pub mod claude_env;
pub mod agent_summaries;
pub mod coauthor;
pub mod claude_mod;
pub mod mcp_apps;
pub mod peers;
pub mod caveman;
pub mod agent_install;
pub mod quick_install;
pub mod justwatch;
pub mod justpaste;
pub mod justimagine;
pub mod justbrowse;
pub mod envfile;
pub mod managed_skill;
pub mod self_update;
pub mod proc_tree;
pub mod orchestrator;
pub mod peon;
pub mod agent_teams;

use serde::{Deserialize, Serialize};

/// The agent CLIs bro drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    Pi,
    Omp,
}

impl Harness {
    pub const ALL: [Harness; 4] = [Harness::Claude, Harness::Codex, Harness::Pi, Harness::Omp];
    pub fn label(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::Omp => "omp",
        }
    }
}
