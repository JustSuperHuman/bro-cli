//! Command-line parsing. Add a subcommand by adding a `Command` variant, a match arm in [`parse`], a line in
//! [`HELP`], and a dispatch arm in `main`.

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The TUI (`bro`, `bro --demo`).
    /// `new`: don't hand off to a running bro (`bro --new`)
    /// `dir`: `bro <folder>` opens that folder (default: where you ran bro)
    Tui { demo: bool, new: bool, dir: Option<std::path::PathBuf> },
    Version,
    Help,
    /// `bro proxy <upstream> …` — headless translating proxy.
    Proxy(crate::proxy_cmd::ProxyArgs),
    /// `bro open <path>[:line]` — show a file to the user (from inside a bro pane).
    Open(crate::open_cmd::OpenArgs),
    OpenHelp,
    /// `bro install [tools…]` — Quick Install: JustTools, Git, Node.js, .NET, the agents….
    Install(crate::install_cmd::InstallArgs),
    InstallHelp,
    /// `bro update [--check]` — install the newest bro release over this one (or only look).
    Update { check_only: bool },
    /// `bro env encrypt | decrypt | list` — env files (`--env` itself is taken off in `main`).
    Env(crate::env_cmd::EnvCmd),
    EnvHelp,
    /// `bro imagine` — open the running JustImagine gallery, signed in.
    Imagine,
}

/// Usage text (`{version}` is substituted).
pub const HELP: &str = "bro {version} — the agentic terminal workspace for Claude Code, Codex, Pi and omp

usage
  bro              open the workspace
  bro [folder]     open the workspace with that folder (default: here) as the current project;
                   if bro is already running, the folder opens there instead
  bro --new        a separate bro even when one is running
  bro --demo       open with realistic fake data (screenshots; touches no accounts)
  bro proxy <upstream> [-m model] [--small-model m] [--port n]
                   run the translating proxy headless (upstream: pool, claude:<profile>,
                   codex:<profile> or a provider id) and print harness env vars
  bro open <path>[:line] [--editor]
                   from inside a bro session: show a file (at a line) in bro's preview and
                   on connected phones / browsers, or open it in the desktop editor
  bro update [--check]
                   install the newest bro release over this one (--check: only look)
  bro install [tools… | recommended | agents | all] [--force]
                   Quick Install: JustTools, JustWatch, Git, Node.js, Bun, Python, .NET 9/10/11,
                   Claude Code, Codex, Pi, omp… (no names: pick from a list)
  bro --env <file|url> [--save] [command]
                   load API keys and settings from an env file (encrypted ones ask for a
                   password) for bro and everything it starts; --save keeps them for every
                   new program and service
  bro env encrypt | decrypt | list <file>
                   make an encrypted env file, read one back, or list what it sets
  bro --export-env [file] [--plain]
                   your own environment variables as an encrypted env file (bro.env by
                   default, asks for a passphrase), to load anywhere with --env
  bro imagine      open your JustImagine gallery in the browser, signed in
  bro --version    print the version
  bro --help       this help

inside
  alt+n launch an agent · alt+b sidebar · alt+p palette · F1 keys · ctrl+space prefix
";

/// Parse arguments (without the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    if args.first().map(String::as_str) == Some("proxy") {
        return crate::proxy_cmd::parse(&args[1..]).map(Command::Proxy);
    }
    if args.first().map(String::as_str) == Some("open") {
        return crate::open_cmd::parse(&args[1..]).map(|a| a.map_or(Command::OpenHelp, Command::Open));
    }
    if args.first().map(String::as_str) == Some("update") {
        return match args.get(1).map(String::as_str) {
            None => Ok(Command::Update { check_only: false }),
            Some("--check" | "-c") if args.len() == 2 => Ok(Command::Update { check_only: true }),
            Some(other) => Err(format!("bro update: unknown argument '{other}' (bro update [--check])")),
        };
    }
    // `bro --export-env [file] [--all] [--plain]` is `bro env export …`
    if args.first().map(String::as_str) == Some("--export-env") {
        let rest: Vec<String> = std::iter::once("export".to_string()).chain(args[1..].iter().cloned()).collect();
        return crate::env_cmd::parse(&rest).map(|a| a.map_or(Command::EnvHelp, Command::Env));
    }
    if args.first().map(String::as_str) == Some("env") {
        return crate::env_cmd::parse(&args[1..]).map(|a| a.map_or(Command::EnvHelp, Command::Env));
    }
    if args.first().map(String::as_str) == Some("imagine") {
        return match args.get(1).map(String::as_str) {
            None => Ok(Command::Imagine),
            Some(other) => Err(format!("bro imagine: unknown argument '{other}'")),
        };
    }
    if args.first().map(String::as_str) == Some("install") {
        return crate::install_cmd::parse(&args[1..]).map(|a| a.map_or(Command::InstallHelp, Command::Install));
    }
    let (mut demo, mut new, mut dir) = (false, false, None);
    for a in args {
        match a.as_str() {
            "-h" | "--help" | "help" => return Ok(Command::Help),
            "-V" | "--version" | "version" => return Ok(Command::Version),
            "--demo" | "demo" => demo = true,
            "--new" | "-n" => new = true,
            other if !other.starts_with('-') && dir.is_none() => dir = Some(std::path::PathBuf::from(other)),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Command::Tui { demo, new, dir })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Command, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses() {
        assert_eq!(p(&[]), Ok(Command::Tui { demo: false, new: false, dir: None }));
        assert_eq!(p(&["--new"]), Ok(Command::Tui { demo: false, new: true, dir: None }));
        assert_eq!(p(&["F:/x"]), Ok(Command::Tui { demo: false, new: false, dir: Some("F:/x".into()) }));
        assert_eq!(p(&["--demo"]), Ok(Command::Tui { demo: true, new: false, dir: None }));
        assert_eq!(p(&["--version"]), Ok(Command::Version));
        assert_eq!(p(&["-h"]), Ok(Command::Help));
        assert!(p(&["--nope"]).is_err());
        assert!(matches!(p(&["open", "src/a.rs:3"]), Ok(Command::Open(ref a)) if a.target == "src/a.rs:3"));
        assert_eq!(p(&["open", "--help"]), Ok(Command::OpenHelp));
        assert!(p(&["open"]).is_err());
        assert!(matches!(p(&["install", "claude"]), Ok(Command::Install(ref a)) if a.tools == [bro_core::quick_install::Tool::Agent(bro_core::Harness::Claude)]));
        assert_eq!(p(&["install", "-h"]), Ok(Command::InstallHelp));
        assert_eq!(p(&["update"]), Ok(Command::Update { check_only: false }));
        assert_eq!(p(&["update", "--check"]), Ok(Command::Update { check_only: true }));
        assert!(p(&["update", "now"]).is_err());
        assert!(matches!(p(&["env", "list", "k.env"]), Ok(Command::Env(crate::env_cmd::EnvCmd::List { .. }))));
        assert_eq!(p(&["env"]), Ok(Command::EnvHelp));
        assert_eq!(p(&["imagine"]), Ok(Command::Imagine));
        assert_eq!(p(&["--export-env", "me.env"]), Ok(Command::Env(crate::env_cmd::EnvCmd::Export { out: "me.env".into(), plain: false, all: false, force: false })));
        assert!(matches!(p(&["--export-env"]), Ok(Command::Env(crate::env_cmd::EnvCmd::Export { ref out, plain: false, .. })) if out == "bro.env"));
    }
}
