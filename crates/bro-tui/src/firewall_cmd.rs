//! `bro firewall [status | allow]`: the firewall rules that let a phone reach this computer without the system
//! stopping the first connection for an "Allow access?" prompt nobody's there to answer (Windows Firewall, the
//! macOS firewall; Linux firewalls don't prompt, so `status` says what to run).
//! The app runs `allow` itself (elevated, with the programs named) when you turn on phone access or remote desktop.

use bro_core::firewall::{self, Outcome, Readiness, Report};

pub const HELP: &str = "bro firewall [status | allow]

  status   which of bro's programs the firewall lets in from other devices
  allow    add the rules now (Windows or macOS asks for administrator rights once): bro for
           phone access and the web client, voxl for remote desktop
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallCmd {
    Status,
    /// With paths (`--bridge …`, `--remote-desktop …`): the elevated helper the app starts.
    Allow(Vec<String>),
    Help,
}

pub fn parse(args: &[String]) -> Result<FirewallCmd, String> {
    match args.first().map(String::as_str) {
        None | Some("status") if args.len() <= 1 => Ok(FirewallCmd::Status),
        Some("allow") => Ok(FirewallCmd::Allow(args[1..].to_vec())),
        Some("--help" | "-h" | "help") => Ok(FirewallCmd::Help),
        Some(other) => Err(format!("bro firewall: unknown argument '{other}' (bro firewall [status | allow])")),
        None => Ok(FirewallCmd::Status),
    }
}

/// This bro's programs: the running bro.exe (when the bridge is reachable from other devices) and Bro RMM's.
pub fn default_targets() -> Vec<firewall::Target> {
    let bridge_open = firewall::reachable_bind(&bro_core::config::Settings::load().bridge.bind);
    let exe = std::env::current_exe().ok();
    let rmm = firewall::remote_desktop_program(&bro_core::paths::bro_dir());
    firewall::targets(exe.as_deref().filter(|_| bridge_open), Some(&rmm))
}

pub fn run(cmd: FirewallCmd) -> anyhow::Result<()> {
    match cmd {
        FirewallCmd::Help => {
            print!("{HELP}");
            Ok(())
        }
        FirewallCmd::Status => {
            print_report(&firewall::check(&default_targets()));
            Ok(())
        }
        FirewallCmd::Allow(args) if args.is_empty() => {
            let targets = default_targets();
            let exe = std::env::current_exe()?;
            match firewall::request(&exe, &targets) {
                Outcome::Done(report) => {
                    print_report(&report);
                    anyhow::ensure!(report.ready(), "the rules didn't take effect");
                    Ok(())
                }
                Outcome::Declined => anyhow::bail!("permission wasn't given, so nothing changed"),
                Outcome::Failed(error) => anyhow::bail!(error),
            }
        }
        FirewallCmd::Allow(args) => {
            let (targets, result) = firewall::parse_allow_args(&args)?;
            if !firewall::elevated() {
                let exe = std::env::current_exe()?;
                return match firewall::request(&exe, &targets) {
                    Outcome::Done(report) => {
                        print_report(&report);
                        Ok(())
                    }
                    Outcome::Declined => anyhow::bail!("permission wasn't given, so nothing changed"),
                    Outcome::Failed(error) => anyhow::bail!(error),
                };
            }
            let written = firewall::allow(&targets);
            let answer = match &written {
                Ok(done) => done.clone(),
                Err(error) => firewall::AllowResult { ok: false, error: Some(error.to_string()), ..Default::default() },
            };
            if let Some(result) = result {
                std::fs::write(&result, serde_json::to_vec(&answer)?)?;
            }
            written.map(|_| ())
        }
    }
}

fn print_report(report: &Report) {
    let line = match &report.readiness {
        Readiness::Ready => format!("{} lets phones in on every network.", firewall::name()),
        Readiness::NotNeeded => "Nothing to do: the firewall is off, or nothing here needs to be reachable.".into(),
        Readiness::NeedsRules => format!("{} Run `bro firewall allow`.", firewall::prompt_risk()),
        Readiness::BlocksAll(profiles) => firewall::blocks_all_fix(*profiles),
        Readiness::Managed => "Your organisation manages Windows Firewall on this PC; ask its administrator to allow bro.exe and voxl.exe.".into(),
        Readiness::Manual(advice) => advice.clone(),
        Readiness::Unknown(error) => format!("Couldn't read the {}: {error}", firewall::name()),
    };
    println!("{line}");
    for app in &report.apps {
        let state = if app.open() {
            "allowed".to_string()
        } else {
            firewall::missing_line(app)
        };
        println!("  {:<19} {}  {}", app.target.app.rule_name(), state, app.target.program.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<FirewallCmd, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_status_allow_and_the_helpers_arguments() {
        assert_eq!(p(&[]), Ok(FirewallCmd::Status));
        assert_eq!(p(&["status"]), Ok(FirewallCmd::Status));
        assert_eq!(p(&["allow"]), Ok(FirewallCmd::Allow(vec![])));
        assert_eq!(p(&["allow", "--bridge", r"C:\b\bro.exe"]), Ok(FirewallCmd::Allow(vec!["--bridge".into(), r"C:\b\bro.exe".into()])));
        assert_eq!(p(&["--help"]), Ok(FirewallCmd::Help));
        assert!(p(&["open"]).is_err());
        assert!(p(&["status", "extra"]).is_err());
    }
}
