//! `bro host`: bro's session server (see [`bro_host`]). bro starts it on its own; from a shell you can run it in
//! the foreground, see what it holds, stop it (ending its sessions), or have it start when you sign in.

use bro_host::service::Startup;
use std::time::Duration;

/// A server with no sessions and no window stops after this long.
const IDLE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCmd {
    /// run the server here (what bro starts in the background)
    Run,
    Status,
    Stop,
    /// start it when you sign in (for your user: no admin)
    Install,
    Uninstall,
}

pub fn parse(args: &[String]) -> Result<HostCmd, String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => Ok(HostCmd::Run),
        ["status"] => Ok(HostCmd::Status),
        ["stop"] => Ok(HostCmd::Stop),
        ["install"] => Ok(HostCmd::Install),
        ["uninstall" | "remove"] => Ok(HostCmd::Uninstall),
        [other, ..] => Err(format!("bro host: unknown argument '{other}' (bro host [status | stop | install | uninstall])")),
    }
}

pub fn run(cmd: HostCmd) -> anyhow::Result<()> {
    let dir = bro_host::client::default_dir();
    match cmd {
        HostCmd::Run => bro_host::serve(bro_host::ServeOpts { dir, version: bro_core::self_update::CURRENT.to_string(), idle: IDLE, resident: bro_host::service::resident }),
        HostCmd::Status => {
            match bro_host::client::status(&dir) {
                Ok(info) => {
                    let up = (bro_core::util::now_ms().max(0) as u64).saturating_sub(info.started_ms) / 1000;
                    println!("session server · pid {} · bro {} · up {}", info.pid, info.version, ago(up));
                    match info.peers.primary {
                        Some(p) if p.role == bro_host::Role::Background => println!("driven by the background bro (pid {}): no window is open", p.pid),
                        Some(p) => println!("driven by a bro window (pid {})", p.pid),
                        None => println!("no bro drives it: sessions keep running, but no window or background bro looks after them"),
                    }
                    if info.peers.viewers > 0 {
                        println!("{} more window{} watching", info.peers.viewers, if info.peers.viewers == 1 { "" } else { "s" });
                    }
                    let live: Vec<_> = info.sessions.iter().filter(|s| !s.exited).collect();
                    println!("{} session{} running", live.len(), if live.len() == 1 { "" } else { "s" });
                    for s in live {
                        let name = s.blob.get("name").and_then(|v| v.as_str()).or_else(|| s.blob.get("label").and_then(|v| v.as_str())).unwrap_or(&s.id);
                        let cwd = s.blob.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
                        println!("  {name}  {cwd}");
                    }
                    if !info.lost.is_empty() {
                        println!("{} session{} from a server that stopped unexpectedly, for bro to offer back", info.lost.len(), if info.lost.len() == 1 { "" } else { "s" });
                    }
                }
                Err(bro_host::client::ConnectError::NotRunning) => println!("the session server isn't running"),
                Err(e) => println!("{e}"),
            }
            println!("{}", startup_line(&bro_host::service::startup()));
            Ok(())
        }
        HostCmd::Stop => {
            if bro_host::client::stop(&dir, Duration::from_secs(10)) {
                println!("stopped the session server (its sessions ended)");
            } else {
                println!("the session server isn't running");
            }
            Ok(())
        }
        HostCmd::Install => {
            install_startup()?;
            println!("{}", startup_line(&bro_host::service::startup()));
            Ok(())
        }
        HostCmd::Uninstall => {
            bro_host::service::uninstall()?;
            println!("the session server no longer starts when you sign in (bro still starts it when it opens)");
            Ok(())
        }
    }
}

/// Start bro in the background when you sign in (the background bro, which starts the session server), from this
/// bro's folder: on Windows through `bro-app --background`, so no console window opens; elsewhere `bro background`.
pub fn install_startup() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    if cfg!(windows) {
        // the folder, not the name: a running bro an update moved aside is bro.exe.old
        let app = exe.with_file_name("bro-app.exe");
        anyhow::ensure!(app.is_file(), "bro-app.exe isn't next to {}: Windows starts the server through it, so no console window opens", exe.display());
        return bro_host::service::install(&app, &["--background"]);
    }
    // Linux names a running bro an update replaced "bro (deleted)"
    let name = exe.file_name().map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string()).unwrap_or_else(|| "bro".into());
    bro_host::service::install(&exe.with_file_name(name), &["background"])
}

fn startup_line(s: &Startup) -> String {
    match s {
        Startup::On { command } => format!("bro starts in the background when you sign in: {command}"),
        Startup::Disabled { command } => format!("set to start when you sign in, but turned off outside bro ({command}) · bro host install turns it back on"),
        Startup::Off => "bro doesn't start when you sign in · bro host install makes it".into(),
    }
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_its_words() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&a(&[])), Ok(HostCmd::Run));
        assert_eq!(parse(&a(&["status"])), Ok(HostCmd::Status));
        assert_eq!(parse(&a(&["stop"])), Ok(HostCmd::Stop));
        assert_eq!(parse(&a(&["install"])), Ok(HostCmd::Install));
        assert_eq!(parse(&a(&["remove"])), Ok(HostCmd::Uninstall));
        assert!(parse(&a(&["start"])).is_err());
    }
}
