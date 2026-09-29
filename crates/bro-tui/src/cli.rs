//! Command-line parsing. Add a subcommand by adding a `Command` variant, a match arm in [`parse`], a line in
//! [`HELP`], and a dispatch arm in `main`.

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The TUI (`bro`, `bro --demo`).
    Tui { demo: bool },
    Version,
    Help,
}

/// Usage text (`{version}` is substituted).
pub const HELP: &str = "bro {version} — the agentic terminal workspace for Claude Code, Codex, Pi and omp

usage
  bro              open the workspace
  bro --demo       open with realistic fake data (screenshots; touches no accounts)
  bro --version    print the version
  bro --help       this help

inside
  alt+n launch an agent · alt+b sidebar · alt+p palette · F1 keys · ctrl+space prefix
";

/// Parse arguments (without the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut demo = false;
    for a in args {
        match a.as_str() {
            "-h" | "--help" | "help" => return Ok(Command::Help),
            "-V" | "--version" | "version" => return Ok(Command::Version),
            "--demo" | "demo" => demo = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Command::Tui { demo })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Command, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses() {
        assert_eq!(p(&[]), Ok(Command::Tui { demo: false }));
        assert_eq!(p(&["--demo"]), Ok(Command::Tui { demo: true }));
        assert_eq!(p(&["--version"]), Ok(Command::Version));
        assert_eq!(p(&["-h"]), Ok(Command::Help));
        assert!(p(&["--nope"]).is_err());
    }
}
