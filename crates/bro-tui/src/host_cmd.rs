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
    /// what the Slop Collector finds in the sessions now (`collect`: and end it)
    Slop { collect: bool },
}

pub fn parse(args: &[String]) -> Result<HostCmd, String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => Ok(HostCmd::Run),
        ["status"] => Ok(HostCmd::Status),
        ["stop"] => Ok(HostCmd::Stop),
        ["install"] => Ok(HostCmd::Install),
        ["uninstall" | "remove"] => Ok(HostCmd::Uninstall),
        ["slop"] => Ok(HostCmd::Slop { collect: false }),
        ["slop", "collect"] => Ok(HostCmd::Slop { collect: true }),
        [other, ..] => Err(format!("bro host: unknown argument '{other}' (bro host [status | stop | install | uninstall | slop [collect]])")),
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
        HostCmd::Slop { collect } => {
            match bro_host::client::slop(&dir, collect) {
                Ok(Some(r)) => print!("{}", slop_text(&r, collect)),
                Ok(None) => println!("the session server runs a bro from before Slop Collector · restart it from Settings › Session server"),
                Err(bro_host::client::ConnectError::NotRunning) => println!("the session server isn't running, so there's nothing to look through"),
                Err(e) => println!("{e}"),
            }
            Ok(())
        }
    }
}

/// `bro slop`: what the collector found and what it has cleaned up, for a terminal or an agent.
fn slop_text(r: &bro_core::slop::Report, collected: bool) -> String {
    use crate::views::settings as v;
    use bro_core::slop::bytes;
    use std::fmt::Write;
    let mut out = String::new();
    let n = |k: u64, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    if !r.supported {
        return "Slop Collector runs on Windows for now. macOS and Linux are next.
".into();
    }
    let mode = if r.collect { format!("cleans up after {} min unused", r.wait_mins) } else { "lists what it finds until you say".into() };
    let scope = if r.anything_idle { "anything idle" } else { "renders & tests" };
    let state = if r.on { format!("on · {mode} · closes {scope}") } else { "off · turn it on in Settings › Slop Collector, or set [slop] on = true in ~/.bro/v2.toml".into() };
    let _ = writeln!(out, "Slop Collector (beta) · {state}");
    if r.looked_ms > 0 {
        let _ = writeln!(out, "{} · {} · {}", n(r.sessions as u64, "session", "sessions"), n(r.processes as u64, "process", "processes"), bytes(r.memory));
    }
    let size = |c: &bro_core::slop::Cleanup| bytes(if c.temp_only() { c.disk } else { c.memory });
    if collected {
        let fresh: Vec<_> = r.ledger.recent.iter().filter(|c| c.at_ms + 5_000 >= r.looked_ms && c.by_hand).collect();
        if fresh.is_empty() && !r.temp.clearing {
            let _ = writeln!(out, "
nothing to clean up");
        } else if !fresh.is_empty() {
            let memory: u64 = fresh.iter().map(|c| c.memory).sum();
            let disk: u64 = fresh.iter().map(|c| c.disk).sum();
            let back: Vec<String> = [(memory, "of memory"), (disk, "on disk")].into_iter().filter(|(b, _)| *b > 0).map(|(b, w)| format!("{} {w}", bytes(b))).collect();
            let _ = writeln!(out, "
cleaned up {} · {} back", n(fresh.len() as u64, "leftover", "leftovers"), back.join(" and "));
            for c in fresh {
                let _ = writeln!(out, "  {:>8}  {} · {}", size(c), c.name(), v::slop_cleanup_detail(c));
            }
        }
        if r.temp.clearing {
            let _ = writeln!(out, "  still clearing {} ({}): it carries on in the background", v::slop_temp_detail(&r.temp), bytes(r.temp.bytes));
        }
    }
    if r.looked_ms > 0 && r.anything() && !(collected && r.leftovers.is_empty()) {
        let mut held = vec![];
        if !r.leftovers.is_empty() {
            held.push(format!("{} in {}", bytes(r.leftover_memory()), n(r.leftovers.len() as u64, "program", "programs")));
        }
        if r.temp.folders > 0 {
            held.push(format!("{} of temp files", bytes(r.temp.bytes)));
        }
        let _ = writeln!(out, "
left behind · {}", held.join(" · "));
        for l in &r.leftovers {
            let place = v::slop_place(&l.project, &l.session_label);
            let _ = writeln!(out, "  {:>8}  {} · {place} · {} · {}, {} · {}", bytes(l.memory), l.name(), v::slop_why(l), l.what, n(l.processes as u64, "process", "processes"), v::slop_due(l));
        }
        if r.temp.folders > 0 {
            let _ = writeln!(out, "  {:>8}  temp files · {} · {}", bytes(r.temp.bytes), v::slop_temp_detail(&r.temp), v::slop_temp_due(&r.temp, r.collect));
        }
        if !collected {
            let _ = writeln!(out, "  bro slop collect cleans them up now");
        }
    } else if r.looked_ms > 0 && !collected {
        let _ = writeln!(out, "
nothing left behind");
    }
    if r.others.groups > 0 {
        let _ = writeln!(out, "
also running on their own, left to you: {} ({}, {})", r.others.names.join(", "), n(r.others.processes as u64, "process", "processes"), bytes(r.others.memory));
    }
    let l = &r.ledger;
    if l.cleanups > 0 {
        let disk = if l.disk > 0 { format!(" and {} on disk", bytes(l.disk)) } else { String::new() };
        let _ = writeln!(out, "
cleaned up so far · {} of memory{disk} back in {}", bytes(l.memory), n(l.cleanups, "cleanup", "cleanups"));
        for c in l.recent.iter().take(5) {
            let _ = writeln!(out, "  {:>8}  {} · {} · {}", size(c), c.name(), v::slop_cleanup_detail(c), v::slop_ago(c.at_ms));
        }
    }
    out
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
        assert_eq!(parse(&a(&["slop"])), Ok(HostCmd::Slop { collect: false }));
        assert_eq!(parse(&a(&["slop", "collect"])), Ok(HostCmd::Slop { collect: true }));
        assert!(parse(&a(&["start"])).is_err());
        // `bro slop` itself
        assert_eq!(crate::cli::parse(&a(&["slop", "collect"])), Ok(crate::cli::Command::Host(HostCmd::Slop { collect: true })));
    }

    #[test]
    fn bro_slop_says_what_is_left_and_what_came_back() {
        let r = crate::services::demo::slop_report("leftovers").unwrap();
        let text = slop_text(&r, false);
        println!("{text}");
        assert!(text.starts_with("Slop Collector (beta) · on · cleans up after 10 min unused · closes renders & tests
9 sessions · 214 processes · 11.4 GB"), "{text}");
        assert!(text.contains("left behind · 7.7 GB in 2 programs · 14.6 GB of temp files"), "{text}");
        assert!(text.contains("  6.5 GB  Chrome from Remotion · film in Night Before Closing · unused for 4 min · Remotion's headless Chrome, 21 processes · closes in 5 min"), "{text}");
        assert!(text.contains("temp files · 547 folders from Remotion, Puppeteer and Playwright · clears at the next check"), "{text}");
        assert!(text.contains("bro slop collect cleans them up now"));
        assert!(text.contains("cleaned up so far · 61.2 GB of memory and 18.9 GB on disk back in 38 cleanups"), "{text}");
        let off = bro_core::slop::Report { on: false, ..crate::services::demo::slop_report("off").unwrap() };
        assert!(slop_text(&off, false).contains("off · turn it on in Settings › Slop Collector"));
    }
}
