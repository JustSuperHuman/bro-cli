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
    }
}
