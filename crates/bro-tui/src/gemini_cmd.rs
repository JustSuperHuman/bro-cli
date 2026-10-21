//! `bro gemini login | status | logout` — the Google account behind the `gemini` provider (Gemini 4 Argon and
//! friends on a Google AI subscription, served to Claude Code / Codex / Pi through bro-proxy).

use bro_core::gemini;

pub const HELP: &str = "bro gemini — Gemini on your Google AI subscription

usage
  bro gemini login [--wait]   sign in with Google in the browser (Google AI Pro or Ultra)
  bro gemini status           who is signed in, the Code Assist project, and quota left per model
  bro gemini logout           forget the sign-in

Then pick the gemini provider in the launcher (alt+n) for Claude Code, Codex, Pi or omp:
bro-proxy translates their requests for Gemini.
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiCmd {
    Login { wait: bool },
    Status,
    Logout,
}

/// None = print help.
pub fn parse(args: &[String]) -> Result<Option<GeminiCmd>, String> {
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => Ok(None),
        Some("login") => match rest.as_slice() {
            [] => Ok(Some(GeminiCmd::Login { wait: false })),
            ["--wait"] => Ok(Some(GeminiCmd::Login { wait: true })),
            _ => Err(format!("bro gemini login: unknown argument '{}' (bro gemini login [--wait])", rest.join(" "))),
        },
        Some("status") if rest.is_empty() => Ok(Some(GeminiCmd::Status)),
        Some("logout") if rest.is_empty() => Ok(Some(GeminiCmd::Logout)),
        Some(other) => Err(format!("bro gemini: unknown command '{other}' (login, status or logout)")),
    }
}

pub fn run(cmd: GeminiCmd) -> Result<(), String> {
    match cmd {
        GeminiCmd::Login { wait } => {
            let result = login();
            if wait {
                if let Err(e) = &result {
                    eprintln!("\nbro gemini login: {e}");
                }
                crate::install_cmd::wait_for_enter();
            }
            result
        }
        GeminiCmd::Status => status(),
        GeminiCmd::Logout => {
            if gemini::logout(&gemini::creds_path()).map_err(|e| format!("{e:#}"))? {
                println!("Signed out of Gemini. `bro gemini login` signs in again.");
            } else {
                println!("Gemini wasn't signed in.");
            }
            Ok(())
        }
    }
}

fn login() -> Result<(), String> {
    println!("Signing in to Gemini with your Google account.");
    println!();
    println!("This uses the same Google sign-in as Google's Antigravity CLI, so requests count against");
    println!("your Google AI subscription. Google's terms only cover its own apps: other tools using that");
    println!("sign-in can get an account restricted. Use an account you're fine taking that risk with.");
    println!();
    let open = |url: &str| {
        println!("Opening your browser. If it doesn't open, paste this link into it:\n\n  {url}\n");
        crate::panes::links::open(url).is_ok()
    };
    let say = |line: &str| println!("  {line}");
    let account = gemini::login(&gemini::creds_path(), &open, &say).map_err(|e| format!("{e:#}"))?;
    println!();
    println!("Signed in as {}.", account.email.as_deref().unwrap_or("your Google account"));
    if let Some(tier) = account.tier_name.as_deref().or(account.tier.as_deref()) {
        println!("Plan: {tier}");
    }
    println!("Pick the gemini provider in bro's launcher (alt+n) to run Claude Code, Codex or Pi on it.");
    Ok(())
}

fn status() -> Result<(), String> {
    let path = gemini::creds_path();
    let Some(account) = gemini::account(&path) else {
        println!("Not signed in. Run `bro gemini login`.");
        return Ok(());
    };
    println!("Signed in as {}", account.email.as_deref().unwrap_or("(unknown account)"));
    if let Some(tier) = account.tier_name.as_deref().or(account.tier.as_deref()) {
        println!("Plan          {tier}");
    }
    if let Some(p) = &account.project {
        println!("Project       {p}");
    }
    match gemini::quotas(&path) {
        Ok(models) if models.is_empty() => println!("No models offered to this account."),
        Ok(models) => {
            println!();
            for m in models {
                let left = m.remaining.map(|r| format!("{:>3.0}% left", r * 100.0)).unwrap_or_else(|| "        ".into());
                let reset = m.reset.map(|r| format!("  resets {r}")).unwrap_or_default();
                println!("  {:<28} {left}{reset}", m.id);
            }
        }
        Err(e) => println!("Couldn't read quota: {e:#}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Option<GeminiCmd>, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses() {
        assert_eq!(p(&[]), Ok(None));
        assert_eq!(p(&["login"]), Ok(Some(GeminiCmd::Login { wait: false })));
        assert_eq!(p(&["login", "--wait"]), Ok(Some(GeminiCmd::Login { wait: true })));
        assert_eq!(p(&["status"]), Ok(Some(GeminiCmd::Status)));
        assert_eq!(p(&["logout"]), Ok(Some(GeminiCmd::Logout)));
        assert!(p(&["login", "now"]).is_err());
        assert!(p(&["nope"]).is_err());
    }
}
