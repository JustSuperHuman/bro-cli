//! Installing the agent CLIs bro launches (Claude Code, Codex, Pi, omp) with their own installers.
//!
//! Claude Code uses Anthropic's native installer; Codex and Pi are npm packages (Bun when npm is
//! missing); omp is a Bun package, so Bun is installed first when it isn't there. Every step runs
//! with inherited stdio, so the installer's own progress and prompts reach the terminal.
use crate::Harness;
use crate::launch::{program_and_args, which};
use std::path::PathBuf;
use std::process::Command;

/// One command an install runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    /// What to print before running it.
    pub display: String,
}

/// The display name of an agent.
pub fn display_name(h: Harness) -> &'static str {
    match h {
        Harness::Claude => "Claude Code",
        Harness::Codex => "Codex",
        Harness::Pi => "Pi",
        Harness::Omp => "omp (oh-my-pi)",
    }
}

/// The package a package-manager install fetches (None: Claude Code's native installer).
pub fn package(h: Harness) -> Option<&'static str> {
    match h {
        Harness::Claude => None,
        Harness::Codex => Some("@openai/codex"),
        Harness::Pi => Some("@mariozechner/pi-coding-agent"),
        Harness::Omp => Some("@oh-my-pi/pi-coding-agent"),
    }
}

/// Where the agent's executable is, if it is installed.
pub fn installed(h: Harness) -> Option<PathBuf> {
    which(h.label())
}

/// A shell one-liner (`irm … | iex` on Windows, `curl … | bash` elsewhere).
fn script(windows_ps: &str, unix_sh: &str) -> Step {
    if cfg!(windows) {
        Step {
            program: "powershell.exe".into(),
            args: ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", windows_ps].map(String::from).to_vec(),
            display: windows_ps.into(),
        }
    } else {
        Step { program: "sh".into(), args: vec!["-c".into(), unix_sh.into()], display: unix_sh.into() }
    }
}

fn bun_install() -> Step {
    script("irm https://bun.sh/install.ps1 | iex", "curl -fsSL https://bun.sh/install | bash")
}

/// A global package install with `tool` (`npm` or `bun`), resolved like a launch would be.
fn global(tool: &str, pkg: &str) -> Step {
    let (program, args) = program_and_args(tool, vec!["install".into(), "-g".into(), pkg.into()]);
    Step { program, args, display: format!("{tool} install -g {pkg}") }
}

/// The commands that install `h` on this machine.
pub fn plan(h: Harness) -> Result<Vec<Step>, String> {
    plan_with(h, which("npm").is_some(), which("bun").is_some())
}

fn plan_with(h: Harness, has_npm: bool, has_bun: bool) -> Result<Vec<Step>, String> {
    let Some(pkg) = package(h) else {
        return Ok(vec![script("irm https://claude.ai/install.ps1 | iex", "curl -fsSL https://claude.ai/install.sh | bash")]);
    };
    Ok(match h {
        Harness::Omp if has_bun => vec![global("bun", pkg)],
        Harness::Omp => vec![bun_install(), global("bun", pkg)],
        _ if has_npm => vec![global("npm", pkg)],
        _ if has_bun => vec![global("bun", pkg)],
        _ => return Err(format!("{} needs Node.js (npm): install it from https://nodejs.org, then try again", display_name(h))),
    })
}

/// Install `h`, running each step in the current terminal. `say` gets progress lines.
/// Ok holds where the executable now is (None: installed, but not found on PATH yet).
pub fn install(h: Harness, mut say: impl FnMut(&str)) -> Result<Option<PathBuf>, String> {
    for step in plan(h)? {
        // a step planned before Bun existed: resolve it now that the previous step installed it
        let step = if step.program == "bun" { global("bun", package(h).unwrap_or_default()) } else { step };
        say(&format!("$ {}", step.display));
        let status = Command::new(&step.program)
            .args(&step.args)
            .status()
            .map_err(|e| format!("couldn't run {}: {e}", step.program))?;
        if !status.success() {
            return Err(format!("`{}` failed ({status})", step.display));
        }
    }
    Ok(installed(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        let claude = plan_with(Harness::Claude, false, false).unwrap();
        assert_eq!(claude.len(), 1);
        assert!(claude[0].display.contains("claude.ai/install"));

        assert_eq!(plan_with(Harness::Codex, true, true).unwrap()[0].display, "npm install -g @openai/codex");
        assert_eq!(plan_with(Harness::Pi, false, true).unwrap()[0].display, "bun install -g @mariozechner/pi-coding-agent");
        assert!(plan_with(Harness::Pi, false, false).unwrap_err().contains("nodejs.org"));

        let omp = plan_with(Harness::Omp, true, false).unwrap();
        assert_eq!(omp.len(), 2);
        assert!(omp[0].display.contains("bun.sh/install"));
        assert_eq!(omp[1].display, "bun install -g @oh-my-pi/pi-coding-agent");
        assert_eq!(plan_with(Harness::Omp, true, true).unwrap().len(), 1);
    }
}
