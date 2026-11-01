//! Vercel's agent-desktop CLI: native desktop automation and the matching OS skill.
use std::path::PathBuf;

pub const REPO: &str = "vercel-labs/agent-desktop";
pub const SKILL: &str = "agent-desktop";

pub fn platform_skill() -> &'static str {
    if cfg!(windows) { "agent-desktop-windows" } else if cfg!(target_os = "macos") { "agent-desktop-macos" } else { "agent-desktop-linux" }
}

pub fn skill_names() -> [&'static str; 2] {
    [SKILL, platform_skill()]
}

/// Filesystem detection only; no process or network on a session-launch path.
pub fn installed() -> Option<PathBuf> {
    crate::launch::which("agent-desktop")
}

/// Install/update the CLI, then its complete Bro-managed skill trees. Does not request Accessibility
/// permission, start desktop automation or send desktop content to a model provider.
pub fn install(mut say: impl FnMut(&str)) -> Result<(), String> {
    if crate::launch::which("npm").is_none() {
        return Err("agent-desktop needs Node.js and npm: run `bro install node` first".into());
    }
    if crate::quick_install::find("git", &crate::quick_install::search_dirs()).is_none() {
        return Err("Bro needs Git to keep agent-desktop's skills up to date: run `bro install git` first".into());
    }
    let (program, args) = crate::launch::program_and_args("npm", vec!["install".into(), "-g".into(), "agent-desktop".into()]);
    say("$ npm install -g agent-desktop");
    let status = std::process::Command::new(program).args(args).status().map_err(|e| format!("starting npm: {e}"))?;
    if !status.success() {
        return Err(format!("npm install -g agent-desktop failed ({status})"));
    }
    say("Adding agent-desktop and this platform's guide to Bro's managed skills…");
    crate::tool_skills::refresh_desktop().map_err(|e| format!("agent-desktop installed, but its skills need attention: {e}. Run `bro install agent-desktop --force` to retry"))
}

pub fn after_install() -> String {
    let platform = if cfg!(target_os = "macos") {
        "On macOS, run `agent-desktop permissions --guide` and grant Accessibility access in System Settings."
    } else if cfg!(target_os = "linux") {
        "On Linux, run in your graphical session with at-spi2-core available. Mouse input needs X11 and explicit --headed permission; Wayland clipboard needs wl-clipboard."
    } else {
        "On Windows, agents can control apps at their own privilege level. Physical input still needs your permission."
    };
    format!("Run `agent-desktop status` to check readiness. {platform} Bro keeps agent-desktop and {} organized under Skills and supplies them to new profiles.", platform_skill())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_only_the_main_and_host_platform_skills() {
        let names = skill_names();
        assert_eq!(names[0], SKILL);
        assert!(names[1].starts_with("agent-desktop-"));
        assert_eq!(names[1] == "agent-desktop-windows", cfg!(windows));
        assert!(!names.contains(&"agent-desktop-ffi") && !names.contains(&"jev-desktop"));
        assert!(after_install().contains("agent-desktop status"));
    }
}
