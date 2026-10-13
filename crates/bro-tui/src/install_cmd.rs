//! `bro install [tools…] [--force] [--wait]` — Quick Install from the command line: JustTools, Git,
//! Node.js, Bun, Python, the .NET SDKs, the agent CLIs bro launches and a few utilities. With no
//! tools named it lists them and asks which to install (Enter: the recommended ones that are missing).

use bro_core::Harness;
use bro_core::agent_install;
use bro_core::quick_install::{self, Group, Status, Tool};
use std::io::{BufRead, IsTerminal, Write};

/// Parsed `bro install` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallArgs {
    /// Empty: ask.
    pub tools: Vec<Tool>,
    /// Reinstall (update) tools that are already installed.
    pub force: bool,
    /// Wait for Enter before exiting (bro's install tab closes when this exits).
    pub wait: bool,
}

pub const HELP: &str = "bro install — Quick Install: JustTools, Git, Node.js, Bun, .NET, the coding agents and more

usage
  bro install                     list everything and pick what to install
  bro install justtools git node  install those (skips ones already installed)
  bro install recommended         the recommended ones that are missing
  bro install agents              claude codex pi omp
  bro install all                 everything that's missing
  bro install <tool> --force      reinstall / update an installed tool

tools
  justtools  JustTools: media, PDF, QR, port commands, JustCommit, JustReady
  git        Git                        gh        GitHub CLI
  node       Node.js LTS                bun       Bun
  python     Python 3                   uv        uv (Python packages)
  dotnet9    .NET 9 SDK                 dotnet10  .NET 10 SDK
  dotnet11   .NET 11 SDK (preview until it ships)
  claude     Claude Code                codex     Codex CLI
  pi         Pi coding agent            omp       oh-my-pi (needs Bun)
  rg         ripgrep                    ffmpeg    FFmpeg

Windows installs with WinGet, macOS with Homebrew, Linux with apt / dnf / pacman / zypper / apk;
Bun, uv, Claude Code and .NET on macOS / Linux use their official installers.
";

/// `None` = print [`HELP`].
pub fn parse(args: &[String]) -> Result<Option<InstallArgs>, String> {
    let mut out = InstallArgs { tools: vec![], force: false, wait: false };
    for a in args {
        match a.to_ascii_lowercase().as_str() {
            "-h" | "--help" => return Ok(None),
            "-f" | "--force" | "--update" => out.force = true,
            "--wait" => out.wait = true,
            "all" => out.tools.extend(Tool::ALL),
            "agents" => out.tools.extend(Harness::ALL.map(Tool::Agent)),
            "recommended" | "rec" => out.tools.extend(Tool::ALL.into_iter().filter(|t| t.recommended())),
            s => out.tools.push(Tool::from_id(s).ok_or_else(|| format!("unknown tool '{a}' (bro install --help lists them)"))?),
        }
    }
    // install order: what others need first
    out.tools.sort_by_key(|t| Tool::ALL.iter().position(|x| x == t));
    out.tools.dedup();
    Ok(Some(out))
}

/// Parse the answer to the "which ones?" prompt. Empty: `default`.
fn pick(answer: &str, default: &[Tool]) -> Result<Vec<Tool>, String> {
    let answer = answer.trim().to_ascii_lowercase();
    if answer.is_empty() {
        return Ok(default.to_vec());
    }
    if matches!(answer.as_str(), "none" | "n" | "no" | "q" | "skip") {
        return Ok(vec![]);
    }
    let words: Vec<String> = answer.split([' ', ',']).filter(|w| !w.is_empty()).map(String::from).collect();
    parse(&words)?.map(|a| a.tools).ok_or_else(|| "pick tools by name".into())
}

fn list(status: &Status) {
    let mut group = None;
    for t in Tool::ALL {
        if group != Some(t.group()) {
            group = Some(t.group());
            println!("{}{}", if t.group() == Group::Main { "" } else { "\n" }, t.group().label());
        }
        let star = if t.recommended() { "*" } else { " " };
        match status.get(&t) {
            Some(detail) => println!("  {star} {:<10}{:<17}installed  {detail}", t.id(), t.name()),
            None => println!("  {star} {:<10}{:<17}-", t.id(), t.name()),
        }
    }
    println!("\n* recommended");
}

/// PATH plus whatever the last installer added, so the next step (npm after Node.js) finds it.
fn refresh_path() {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = quick_install::fresh_path_dirs();
    dirs.extend(bro_core::launch::extra_bin_dirs());
    if let Some(p) = quick_install::path_with(&path, &dirs) {
        // SAFETY: `bro install` is single-threaded here; no other thread reads the environment.
        unsafe { std::env::set_var("PATH", p) };
    }
}

/// JustTools with its own installer, then its skill for new sessions (as the settings view does).
fn install_justtools() -> Result<(), String> {
    use bro_core::justtools::{self, Step};
    let found = justtools::install(&|step| {
        let what = match step {
            Step::Installer => "fetching the installer",
            Step::Download => "downloading the release",
            Step::Verify => "checking its SHA-256",
            Step::Install => "installing the commands",
            Step::Skill => "adding the skill",
        };
        println!("  {what}…");
    })
    .map_err(|e| format!("{e:#}"))?;
    println!("  JustTools {} in {}", found.version.as_deref().unwrap_or("installed"), found.bin.display());
    match justtools::fetch_skill(found.version.as_deref()) {
        Ok(()) => {
            let mut s = bro_core::config::Settings::load();
            if !s.justtools.skill {
                s.justtools.skill = true;
                let _ = s.save();
            }
            println!("  new Claude and Codex sessions get its skill");
        }
        Err(e) => println!("  couldn't download its skill: {e:#}"),
    }
    Ok(())
}

fn install_one(t: Tool) -> Result<(), String> {
    match t {
        Tool::JustTools => install_justtools(),
        Tool::Agent(h) => agent_install::install(h, |line| println!("{line}")).map(|_| ()),
        _ => quick_install::install(t, |line| println!("{line}")),
    }
}

fn wait_for_enter() {
    if std::io::stdin().is_terminal() {
        print!("\nPress Enter to close. ");
        std::io::stdout().flush().ok();
        let _ = std::io::stdin().lock().read_line(&mut String::new());
    }
}

/// Run it; Err when an install failed.
pub fn run(args: InstallArgs) -> Result<(), String> {
    let result = install(&args);
    if args.wait {
        if let Err(e) = &result {
            eprintln!("\nbro install: {e}");
        }
        wait_for_enter();
    }
    result
}

fn install(args: &InstallArgs) -> Result<(), String> {
    let status = quick_install::detect();
    let mut tools = args.tools.clone();
    if tools.is_empty() {
        list(&status);
        let missing: Vec<Tool> = Tool::ALL.into_iter().filter(|t| !status.contains_key(t)).collect();
        if missing.is_empty() && !args.force {
            println!("\nEverything's installed. `bro install <tool> --force` reinstalls one.");
            return Ok(());
        }
        if !std::io::stdin().is_terminal() {
            println!("\nName the tools to install, e.g.: bro install recommended");
            return Ok(());
        }
        let default: Vec<Tool> = missing.iter().copied().filter(|t| t.recommended()).collect();
        let shown = if default.is_empty() { "none".to_string() } else { default.iter().map(|t| t.id()).collect::<Vec<_>>().join(" ") };
        print!("\nInstall which? (names, 'recommended', 'agents', 'all', 'none'; Enter: {shown}) ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
        tools = pick(&answer, &default)?;
    }
    let (mut done, mut failed, mut notes) = (vec![], vec![], vec![]);
    for t in tools {
        if !args.force
            && let Some(detail) = status.get(&t)
        {
            println!("{} is already installed ({detail}); --force reinstalls it", t.name());
            continue;
        }
        println!("\n── Installing {} ──", t.name());
        match install_one(t) {
            Ok(()) => {
                println!("{} installed.", t.name());
                done.push(t.name());
                notes.extend(quick_install::after_install(t));
            }
            Err(e) => {
                eprintln!("{}: {e}", t.name());
                failed.push(t.id());
            }
        }
        refresh_path();
    }
    if !done.is_empty() {
        println!("\nInstalled: {}.", done.join(", "));
        for n in &notes {
            println!("  {n}");
        }
        println!("Open a new terminal (or restart bro) so everything is on PATH.");
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("failed: {}", failed.join(", "))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Option<InstallArgs>, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses() {
        assert_eq!(p(&[]), Ok(Some(InstallArgs { tools: vec![], force: false, wait: false })));
        // install order, not the order given, and no repeats
        assert_eq!(p(&["codex", "Node", "claude", "codex"]).unwrap().unwrap().tools, vec![Tool::Node, Tool::Agent(Harness::Claude), Tool::Agent(Harness::Codex)]);
        assert_eq!(p(&["all", "-f", "--wait"]), Ok(Some(InstallArgs { tools: Tool::ALL.to_vec(), force: true, wait: true })));
        assert_eq!(p(&["agents"]).unwrap().unwrap().tools, Harness::ALL.map(Tool::Agent).to_vec());
        assert_eq!(p(&["recommended"]).unwrap().unwrap().tools[0], Tool::JustTools);
        assert_eq!(p(&["dotnet-9", "net11"]).unwrap().unwrap().tools, vec![Tool::Dotnet9, Tool::Dotnet11]);
        assert_eq!(p(&["--help"]), Ok(None));
        assert!(p(&["vim"]).is_err());
    }

    #[test]
    fn picks() {
        let default = [Tool::Git, Tool::Agent(Harness::Omp)];
        assert_eq!(pick("\n", &default), Ok(default.to_vec()));
        assert_eq!(pick("none", &default), Ok(vec![]));
        assert_eq!(pick("omp, git", &default), Ok(vec![Tool::Git, Tool::Agent(Harness::Omp)]));
        assert!(pick("nope", &default).is_err());
    }
}
