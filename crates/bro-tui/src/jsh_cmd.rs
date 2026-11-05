//! `bro jsh login | status | logout`: the Just Superhuman account behind the `justsuperhuman` provider (Claude Code
//! on your organization's credits).

use bro_core::jsh;

pub const HELP: &str = "bro jsh: Claude on your Just Superhuman account

usage
  bro jsh login    sign in in the browser; approve \"bro\" and this computer
  bro jsh status   who is signed in, the organization and the credits left
  bro jsh logout   revoke this computer's sign-in and forget it

Then pick Just Superhuman in the launcher (alt+n) to run Claude Code on it. BRO_JSH_ORIGIN points bro at another
accounts server (http://auth.localhost:8790 for local development).
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JshCmd {
    Login,
    Status,
    Logout,
}

/// None = print help.
pub fn parse(args: &[String]) -> Result<Option<JshCmd>, String> {
    match (args.first().map(String::as_str), args.len()) {
        (None, _) | (Some("-h" | "--help" | "help"), _) => Ok(None),
        (Some("login"), 1) => Ok(Some(JshCmd::Login)),
        (Some("status"), 1) => Ok(Some(JshCmd::Status)),
        (Some("logout"), 1) => Ok(Some(JshCmd::Logout)),
        (Some(c @ ("login" | "status" | "logout")), _) => Err(format!("bro jsh {c}: takes no arguments")),
        (Some(other), _) => Err(format!("bro jsh: unknown command '{other}' (login, status or logout)")),
    }
}

pub fn run(cmd: JshCmd) -> Result<(), String> {
    match cmd {
        JshCmd::Login => login(),
        JshCmd::Status => status(),
        JshCmd::Logout => {
            let out = jsh::login().sign_out();
            match (out.was_signed_in, out.revoked) {
                (false, _) => println!("Not signed in to Just Superhuman."),
                (true, true) => println!("Signed out. This computer can no longer spend your Just Superhuman credits."),
                (true, false) => println!("Signed out here, but Just Superhuman couldn't be reached to revoke this computer. Revoke it under Account > Devices on the website."),
            }
            Ok(())
        }
    }
}

fn login() -> Result<(), String> {
    if let Some((account, _)) = jsh::cached_account().filter(|_| jsh::signed_in()) {
        println!("Already signed in as {}. `bro jsh logout` first to switch accounts.", account.who());
        return Ok(());
    }
    let (pending, _) = jsh::login().begin().map_err(|e| format!("{e:#}"))?;
    println!("Your code is  {}", pending.user_code);
    println!("Opening {}", pending.verification_uri_complete);
    println!("If the browser doesn't open, go to {} and enter the code.", pending.verification_uri);
    println!("Waiting for you to approve it...");
    let _ = crate::panes::links::open(&pending.verification_uri_complete);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let snap = jsh::login().snapshot();
        if snap.signed_in {
            let who = snap.account.as_ref().map(|a| a.who().to_string()).unwrap_or_default();
            println!("\nSigned in as {who}.");
            if let Some(a) = &snap.account {
                println!("{}", a.summary());
            }
            println!("Pick Just Superhuman in bro's launcher (alt+n) to run Claude Code on it.");
            return Ok(());
        }
        if snap.pending.is_none() {
            return Err(snap.error.unwrap_or_else(|| "the sign-in ended without an approval".into()));
        }
    }
}

fn status() -> Result<(), String> {
    if !jsh::signed_in() {
        println!("Not signed in. Run `bro jsh login`.");
        return Ok(());
    }
    // a fresh reading when we can; a revoked device signs out right here
    let fresh = jsh::login().refresh();
    let snap = jsh::login().snapshot();
    let Some(a) = snap.account.filter(|_| snap.signed_in) else {
        println!("{}", snap.error.unwrap_or_else(|| "Not signed in. Run `bro jsh login`.".into()));
        return Ok(());
    };
    println!("Signed in as {} <{}>", a.user.name, a.user.email);
    println!("Organization  {}", a.organization.name);
    println!("Credits       {} available ({} granted)", jsh::grouped(a.credits.available), jsh::grouped(a.credits.granted));
    if a.credits.monthly_allowance > 0 {
        println!("Allowance     {} a month{}", jsh::grouped(a.credits.monthly_allowance), a.credits.resets_at.map(|r| format!(", {}", jsh::renews_text(r))).unwrap_or_default());
    }
    if !a.claude.available {
        println!("Claude        unavailable: {}", a.claude.reason);
    }
    if let Err(e) = fresh {
        println!("\nShowing the last reading: {e:#}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Option<JshCmd>, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses() {
        assert_eq!(p(&[]), Ok(None));
        assert_eq!(p(&["login"]), Ok(Some(JshCmd::Login)));
        assert_eq!(p(&["status"]), Ok(Some(JshCmd::Status)));
        assert_eq!(p(&["logout"]), Ok(Some(JshCmd::Logout)));
        assert!(p(&["login", "now"]).is_err());
        assert!(p(&["nope"]).is_err());
    }
}
