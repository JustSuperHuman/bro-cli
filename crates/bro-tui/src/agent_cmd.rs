//! `bro agent`: Bro as a Windows service, for computers the RMM manages.
//!
//! A person's Bro lives in their Windows profile and runs while they are signed in. A server in a closet, a
//! domain controller nobody signs in to, a computer after a reboot: none of those has anyone signed in, so the
//! account could reach the RMM's monitoring service there but not Bro. The agent closes that gap the way
//! remote-management tools run their own always-on work: a service the computer owns.
//!
//! - It is installed machine-wide by the RMM (as SYSTEM): `C:\Program Files\Bro`, data in
//!   `C:\ProgramData\Bro\agent` (readable only by SYSTEM, administrators and the agent itself).
//! - It runs as its own virtual account, `NT SERVICE\BroAgent`: no password to keep, works on a domain controller
//!   (where a "local" account would be a domain one), and isn't an administrator, so the agents it runs aren't
//!   either.
//! - From boot it keeps the background bro running (`bro background`, which drives the session server and the
//!   account connection), and starts it again if it stops.
//! - Installing takes over the Just Superhuman sign-in of the Windows profile that put this computer on the
//!   account, so the computer keeps its identity, its place on the account and its history. A computer nobody
//!   connected yet is signed in with `bro agent login`.
//! - From then on the account connection is the agent's alone: a Bro someone opens here leaves it to the agent
//!   (`bro_core::justconnect::agent::owns`), so the computer never answers twice.

use anyhow::{Context, bail};
use bro_core::justconnect::agent;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const HELP: &str = "bro agent — Bro as a Windows service, for computers the RMM manages

  bro agent install [--computer <id>] [--from <profile folder>]
                       install or update the service from this bro (run as administrator or SYSTEM)
                       and take over the Just Superhuman sign-in of the profile that connected this
                       computer (the newest one, or the one connected as <id>, or <profile folder>)
  bro agent status     whether the service runs, who it is signed in as, and its sessions
  bro agent login      sign the service in to Just Superhuman (for a computer nobody connected yet)
  bro agent uninstall  stop and remove the service (its data stays in C:\\ProgramData\\Bro)

The service runs as NT SERVICE\\BroAgent from boot, whether or not anyone is signed in to Windows,
and keeps Bro's sessions and the account connection running.
";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => {
            print!("{HELP}");
            Ok(())
        }
        ["run"] => imp::serve(),
        ["status"] => status(),
        ["login"] => login(),
        ["uninstall" | "remove"] => imp::uninstall(),
        ["install", rest @ ..] => {
            let mut computer = None;
            let mut from = None;
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match *flag {
                    "--computer" => computer = Some(it.next().context("--computer needs a computer ID")?.to_string()),
                    "--from" => from = Some(PathBuf::from(it.next().context("--from needs a profile folder")?)),
                    other => bail!("unknown argument '{other}' (see bro agent help)"),
                }
            }
            imp::install(computer.as_deref(), from.as_deref())
        }
        [other, ..] => bail!("unknown command '{other}' (see bro agent help)"),
    }
}

/// A Windows profile whose Bro connected a computer to the account: where its sign-in is, and which computer.
#[derive(Debug, Clone, PartialEq)]
pub struct Signed {
    pub profile: PathBuf,
    pub computer_id: String,
    /// When its sign-in was last written, for choosing the newest.
    pub at: std::time::SystemTime,
}

/// Every profile under `users` (e.g. `C:\Users`) with a Bro signed in to Just Superhuman that put this computer on
/// the account, newest sign-in first.
pub fn signed_profiles(users: &Path) -> Vec<Signed> {
    let mut found = vec![];
    let Ok(entries) = std::fs::read_dir(users) else { return found };
    for entry in entries.flatten() {
        if let Some(signed) = signed_profile(&entry.path()) {
            found.push(signed);
        }
    }
    found.sort_by(|a, b| b.at.cmp(&a.at));
    found
}

pub fn signed_profile(profile: &Path) -> Option<Signed> {
    let jsh = profile.join(".bro").join("jsh");
    let token = jsh.join("token.json");
    let connection: Value = serde_json::from_slice(&std::fs::read(jsh.join("justconnect.json")).ok()?).ok()?;
    let id = connection["computerId"].as_str().filter(|id| bro_core::justconnect::valid_id(id))?;
    let at = std::fs::metadata(&token).and_then(|m| m.modified()).ok()?;
    Some(Signed { profile: profile.to_path_buf(), computer_id: id.to_string(), at })
}

/// The profile whose sign-in the agent takes over: the one named, else the one connected as `computer`, else the
/// newest.
pub fn choose(found: &[Signed], computer: Option<&str>) -> Option<Signed> {
    match computer {
        Some(id) => found.iter().find(|s| s.computer_id == id).cloned(),
        None => found.first().cloned(),
    }
}

/// Copy a profile's sign-in (and which computer it connected) into the agent's data. The files are the agent's
/// own copies from then on; the profile keeps its own, and its Bro leaves the connection to the agent.
pub fn adopt(from: &Signed, data: &Path) -> anyhow::Result<()> {
    let src = from.profile.join(".bro").join("jsh");
    let dst = data.join("jsh");
    std::fs::create_dir_all(&dst)?;
    for name in ["token.json", "justconnect.json", "account.json"] {
        let file = src.join(name);
        if !file.exists() {
            if name == "account.json" {
                continue;
            }
            bail!("{} has no {name}", src.display());
        }
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        bro_core::util::atomic_write(&dst.join(name), &bytes).with_context(|| format!("writing {}", dst.join(name).display()))?;
    }
    Ok(())
}

/// Claude Code greets a new account with a theme picker and security notes before it takes a prompt; nobody sits
/// at the agent's account to answer them, so they are marked done in its own `~/.claude.json` (anything else in
/// there stays as it is).
pub fn seed_claude(home: &Path) {
    let path = home.join(".claude.json");
    let mut config: Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
    let mut changed = false;
    for (key, value) in [("hasCompletedOnboarding", Value::Bool(true)), ("theme", Value::String("dark".into()))] {
        if config.get(key).is_none() {
            config[key] = value;
            changed = true;
        }
    }
    if changed && let Ok(bytes) = serde_json::to_vec_pretty(&config) {
        let _ = bro_core::util::atomic_write(&path, &bytes);
    }
}

/// The agent's settings, written once (a person may change them later): no update checks (the RMM updates it),
/// the bridge only on this computer (phones come in through the account), and no setup guide.
pub const SETTINGS: &str = "setup_done = true\n\n[updates]\ncheck = false\n\n[bridge]\nbind = \"127.0.0.1\"\n";

fn status() -> anyhow::Result<()> {
    let marker = agent::installed();
    println!("service      {}", imp::service_state().unwrap_or_else(|| "not installed".into()));
    match &marker {
        Some(m) => {
            println!("computer     {}", m.computer_id);
            if !m.version.is_empty() {
                println!("installed    bro {}", m.version);
            }
            if !m.adopted_from.is_empty() {
                println!("signed in as the sign-in from {}", m.adopted_from);
            }
        }
        None => println!("computer     none yet"),
    }
    let data = agent::data_dir();
    let account = bro_core::util::read_json(&data.join("jsh").join("account.json"));
    if let Some(account) = account {
        let user = &account["account"]["user"];
        let org = &account["account"]["organization"]["name"];
        println!("account      {} ({}) · {}", user["name"].as_str().unwrap_or(""), user["email"].as_str().unwrap_or(""), org.as_str().unwrap_or(""));
    } else if !data.join("jsh").join("token.json").exists() {
        println!("account      not signed in · bro agent login signs it in");
    }
    match bro_host::client::status(&data) {
        Ok(info) => {
            let live = info.sessions.iter().filter(|s| !s.exited).count();
            println!("sessions     {live} running (session server pid {}, bro {})", info.pid, info.version);
        }
        Err(_) => println!("sessions     the session server isn't running"),
    }
    Ok(())
}

fn login() -> anyhow::Result<()> {
    let data = agent::data_dir();
    std::fs::create_dir_all(&data).with_context(|| format!("creating {} (run as administrator)", data.display()))?;
    // SAFETY: set before any other thread starts; everything after reads the agent's data.
    unsafe { std::env::set_var("BRO_DIR", &data) };
    crate::jsh_cmd::run(crate::jsh_cmd::JshCmd::LoginNoBrowser).map_err(anyhow::Error::msg)?;
    println!("The service picks the sign-in up within a few seconds.");
    Ok(())
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::OsString;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows_service::service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType};
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn program_files() -> PathBuf {
        let base = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        base.join("Bro")
    }

    fn users_dir() -> PathBuf {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        PathBuf::from(format!("{drive}\\Users"))
    }

    fn log(line: &str) {
        use std::io::Write;
        let path = agent::root().join("agent.log");
        // keep it small: start over past 1 MB
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{} {line}", chrono_like_now());
        }
    }

    fn chrono_like_now() -> String {
        let ms = bro_core::util::now_ms();
        format!("[{}]", ms / 1000)
    }

    fn sc(args: &[&str]) -> anyhow::Result<String> {
        let out = Command::new("sc.exe").args(args).creation_flags_no_window().output().context("running sc.exe")?;
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            bail!("sc {} failed: {}", args.first().unwrap_or(&""), text.trim());
        }
        Ok(text)
    }

    trait NoWindow {
        fn creation_flags_no_window(&mut self) -> &mut Self;
    }
    impl NoWindow for Command {
        fn creation_flags_no_window(&mut self) -> &mut Self {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW)
        }
    }

    pub fn service_state() -> Option<String> {
        let out = Command::new("sc.exe").args(["query", agent::SERVICE]).creation_flags_no_window().output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let state = text.lines().find(|l| l.trim_start().starts_with("STATE"))?;
        Some(state.split_whitespace().last().unwrap_or("UNKNOWN").to_lowercase())
    }

    fn wait_for(state: &str, within: Duration) -> bool {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if service_state().as_deref() == Some(state) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    }

    /// Put this bro's two executables in `dir`, moving any running copies aside (Windows lets a running program be
    /// renamed, not overwritten).
    fn place(dir: &Path) -> anyhow::Result<PathBuf> {
        let here = std::env::current_exe()?.parent().context("bro's folder")?.to_path_buf();
        std::fs::create_dir_all(dir).with_context(|| format!("creating {} (run as administrator or SYSTEM)", dir.display()))?;
        let same = |a: &Path, b: &Path| a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b);
        if !same(&here, dir) {
            let stamp = bro_core::util::now_ms();
            for name in ["bro.exe", "bro-app.exe"] {
                let src = here.join(name);
                anyhow::ensure!(src.is_file(), "{name} isn't next to this bro ({})", here.display());
                let dst = dir.join(name);
                if dst.exists() && std::fs::read(&dst).ok() == std::fs::read(&src).ok() {
                    continue;
                }
                if dst.exists() {
                    std::fs::rename(&dst, dir.join(format!("{name}.old-{stamp}"))).with_context(|| format!("moving the old {name} aside"))?;
                }
                std::fs::copy(&src, &dst).with_context(|| format!("copying {name} to {}", dir.display()))?;
            }
            // copies moved aside by earlier updates, once nothing runs them
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    if e.file_name().to_string_lossy().contains(".exe.old-") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        }
        Ok(dir.join("bro.exe"))
    }

    /// Only SYSTEM, administrators and the agent may read the agent's data: its sign-in is in there.
    fn lock_down(root: &Path) -> anyhow::Result<()> {
        let account = format!("NT SERVICE\\{}:(OI)(CI)M", agent::SERVICE);
        // the folder alone gets the list (inheritance flags only mean something on a folder: on a file they fail
        // and leave it with no access at all) ...
        let out = Command::new("icacls.exe")
            .arg(root)
            .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", &account, "/C", "/Q"])
            .creation_flags_no_window()
            .output()
            .context("running icacls")?;
        anyhow::ensure!(out.status.success(), "icacls failed: {}", String::from_utf8_lossy(&out.stdout).trim());
        // ... and everything in it takes it from the folder
        let out = Command::new("icacls.exe").arg(root.join("*")).args(["/reset", "/T", "/C", "/Q"]).creation_flags_no_window().output().context("running icacls")?;
        anyhow::ensure!(out.status.success(), "icacls /reset failed: {}", String::from_utf8_lossy(&out.stdout).trim());
        // everyone may read which computer the agent answers for, so a person's Bro knows to leave it be
        let _ = Command::new("icacls.exe").arg(agent::marker_path()).args(["/grant", "*S-1-5-32-545:R", "/Q"]).creation_flags_no_window().output();
        Ok(())
    }

    pub fn install(computer: Option<&str>, from: Option<&Path>) -> anyhow::Result<()> {
        let dir = program_files();
        let exe = place(&dir)?;
        let root = agent::root();
        let data = agent::data_dir();
        std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
        let settings = data.join("v2.toml");
        if !settings.exists() {
            bro_core::util::atomic_write(&settings, SETTINGS.as_bytes())?;
        }

        // whose sign-in it takes over
        let had = signed_in(&data);
        let mut adopted = String::new();
        let pick = match from {
            Some(profile) => Some(signed_profile(profile).with_context(|| format!("{} has no Bro connected to Just Superhuman", profile.display()))?),
            None => choose(&signed_profiles(&users_dir()), computer),
        };
        let mut computer_id = computer.map(str::to_string).unwrap_or_default();
        // an agent already signed in keeps its sign-in on an update, unless a profile is named
        if let Some(signed) = pick.as_ref().filter(|_| from.is_some() || !had) {
            adopt(signed, &data)?;
            adopted = signed.profile.display().to_string();
            computer_id = signed.computer_id.clone();
            println!("Took over the Just Superhuman sign-in from {adopted}.");
        }
        if computer_id.is_empty() {
            computer_id = bro_core::util::read_json(&data.join("jsh").join("justconnect.json")).and_then(|v| v["computerId"].as_str().map(str::to_string)).unwrap_or_default();
        }
        let previous = agent::installed();
        let marker = agent::Marker {
            computer_id: computer_id.clone(),
            installed_at: bro_core::util::now_ms(),
            version: bro_core::self_update::CURRENT.to_string(),
            adopted_from: if adopted.is_empty() { previous.map(|m| m.adopted_from).unwrap_or_default() } else { adopted },
        };
        bro_core::util::atomic_write(&agent::marker_path(), serde_json::to_vec_pretty(&marker)?.as_slice())?;

        // the service: created once, pointed at this bro on every install
        let bin = format!("\"{}\" agent run", exe.display());
        let account = format!("NT SERVICE\\{}", agent::SERVICE);
        if service_state().is_some() {
            let _ = sc(&["stop", agent::SERVICE]);
            wait_for("stopped", Duration::from_secs(30));
            sc(&["config", agent::SERVICE, "binPath=", &bin, "start=", "delayed-auto", "obj=", &account, "DisplayName=", "Bro Agent"])?;
        } else {
            sc(&["create", agent::SERVICE, "binPath=", &bin, "start=", "delayed-auto", "obj=", &account, "DisplayName=", "Bro Agent"])?;
        }
        sc(&["description", agent::SERVICE, "Keeps Bro's sessions and its Just Superhuman connection running on this computer, whether or not anyone is signed in."])?;
        sc(&["sidtype", agent::SERVICE, "unrestricted"])?;
        sc(&["failure", agent::SERVICE, "reset=", "86400", "actions=", "restart/5000/restart/30000/restart/60000"])?;
        lock_down(&root)?;
        sc(&["start", agent::SERVICE])?;
        let running = wait_for("running", Duration::from_secs(30));
        log(&format!("installed bro {} for computer {}", marker.version, if computer_id.is_empty() { "(none yet)" } else { &computer_id }));
        println!("Bro Agent {} · service {}", marker.version, if running { "running" } else { "installed, not running yet" });
        if !signed_in(&data) {
            println!("Not signed in yet: run `bro agent login` to connect this computer to Just Superhuman.");
        } else if computer_id.is_empty() {
            println!("Signed in. The computer appears on the account within a few seconds.");
        } else {
            println!("Answers for computer {computer_id} from now on, whether or not anyone is signed in to Windows.");
        }
        Ok(())
    }

    fn signed_in(data: &Path) -> bool {
        data.join("jsh").join("token.json").exists()
    }

    pub fn uninstall() -> anyhow::Result<()> {
        if service_state().is_none() {
            println!("The Bro Agent service isn't installed.");
        } else {
            let _ = sc(&["stop", agent::SERVICE]);
            wait_for("stopped", Duration::from_secs(30));
            sc(&["delete", agent::SERVICE])?;
            println!("Removed the Bro Agent service. Its data stays in {}.", agent::root().display());
        }
        let _ = std::fs::remove_file(agent::marker_path());
        Ok(())
    }

    // ---- the service itself

    define_windows_service!(ffi_service_main, service_main);

    pub fn serve() -> anyhow::Result<()> {
        service_dispatcher::start(agent::SERVICE, ffi_service_main).context("bro agent run is started by Windows as the Bro Agent service")?;
        Ok(())
    }

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            log(&format!("the service stopped: {e:#}"));
        }
    }

    fn status(state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running { ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN } else { ServiceControlAccept::empty() },
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        }
    }

    fn run_service() -> anyhow::Result<()> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = service_control_handler::register(agent::SERVICE, move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;
        handle.set_service_status(status(ServiceState::Running))?;
        log(&format!("started (bro {}, profile {})", bro_core::self_update::CURRENT, std::env::var("USERPROFILE").unwrap_or_default()));
        supervise(&stop_rx);
        handle.set_service_status(status(ServiceState::Stopped))?;
        log("stopped");
        Ok(())
    }

    fn spawn() -> std::io::Result<Child> {
        let exe = std::env::current_exe()?;
        let data = agent::data_dir();
        let _ = std::fs::create_dir_all(&data);
        Command::new(exe)
            .arg("background")
            .env("BRO_DIR", &data)
            .env("BRO_AGENT", "1")
            .current_dir(std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or(data))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags_no_window()
            .spawn()
    }

    /// Keep the background bro running until Windows stops the service. A stop ends the background bro only: the
    /// session server it started keeps the sessions, and the next start picks them up again.
    /// Claude Code and Codex install per account, so the agent's own account starts with neither: install the
    /// missing ones for it once, beside the sessions (a phone asking for a session meanwhile hears it is coming).
    fn ensure_harnesses() {
        let missing: Vec<&str> = ["claude", "codex"].into_iter().filter(|tool| bro_core::launch::which(tool).is_none()).collect();
        if missing.is_empty() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        log(&format!("installing {} for the agent's account", missing.join(" and ")));
        let out = Command::new(exe)
            .arg("install")
            .args(&missing)
            .env("BRO_DIR", agent::data_dir())
            .env("BRO_AGENT", "1")
            .stdin(Stdio::null())
            .creation_flags_no_window()
            .output();
        match out {
            Ok(out) => log(&format!(
                "bro install {} exited {:?}: {}",
                missing.join(" "),
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).lines().chain(String::from_utf8_lossy(&out.stderr).lines()).filter(|l| !l.trim().is_empty()).last().unwrap_or("")
            )),
            Err(e) => log(&format!("bro install couldn't start: {e}")),
        }
    }

    fn supervise(stop: &mpsc::Receiver<()>) {
        seed_claude(&bro_core::util::home_dir());
        std::thread::spawn(ensure_harnesses);
        let mut backoff = Duration::from_secs(2);
        loop {
            let started = Instant::now();
            let mut child = match spawn() {
                Ok(child) => child,
                Err(e) => {
                    log(&format!("couldn't start the background bro: {e}"));
                    if stop.recv_timeout(Duration::from_secs(30)).is_ok() {
                        return;
                    }
                    continue;
                }
            };
            log(&format!("background bro started (pid {})", child.id()));
            let code = loop {
                if stop.recv_timeout(Duration::from_secs(1)).is_ok() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                match child.try_wait() {
                    Ok(Some(code)) => break code.code(),
                    Ok(None) => {}
                    Err(_) => break None,
                }
            };
            // a run that lasted a while starts again soon; one that keeps failing at once waits longer each time
            backoff = if started.elapsed() > Duration::from_secs(120) { Duration::from_secs(2) } else { (backoff * 2).min(Duration::from_secs(60)) };
            log(&format!("background bro exited ({code:?}); starting it again in {}s", backoff.as_secs()));
            if stop.recv_timeout(backoff).is_ok() {
                return;
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn serve() -> anyhow::Result<()> {
        bail!("bro agent runs as a Windows service; on macOS and Linux, run `bro host install` for each user for now")
    }
    pub fn install(_: Option<&str>, _: Option<&Path>) -> anyhow::Result<()> {
        serve()
    }
    pub fn uninstall() -> anyhow::Result<()> {
        serve()
    }
    pub fn service_state() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(users: &Path, name: &str, computer: &str) -> PathBuf {
        let p = users.join(name);
        let jsh = p.join(".bro").join("jsh");
        std::fs::create_dir_all(&jsh).unwrap();
        std::fs::write(jsh.join("token.json"), format!("{{\"token\":\"t-{name}\"}}")).unwrap();
        std::fs::write(jsh.join("justconnect.json"), format!("{{\"computerId\":\"{computer}\",\"enabled\":true}}")).unwrap();
        p
    }

    #[test]
    fn the_agent_takes_over_the_sign_in_that_connected_this_computer() {
        let temp = tempfile::tempdir().unwrap();
        let users = temp.path().join("Users");
        const A: &str = "0eaa740a-6561-4d8a-b72c-44b8b610708f";
        const B: &str = "dbb88f69-0da5-444f-a10c-9c17afcd48e0";
        profile(&users, "Administrator", A);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let newest = profile(&users, "TEMP.OFFICE.248", B);
        // a profile with Bro but no connection is no candidate
        std::fs::create_dir_all(users.join("Public").join(".bro")).unwrap();
        let found = signed_profiles(&users);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].profile, newest, "newest sign-in first");
        assert_eq!(choose(&found, None).unwrap().computer_id, B);
        assert_eq!(choose(&found, Some(A)).unwrap().computer_id, A);
        assert!(choose(&found, Some("missing")).is_none());

        let data = temp.path().join("agent");
        adopt(&found[0], &data).unwrap();
        assert_eq!(std::fs::read_to_string(data.join("jsh/token.json")).unwrap(), "{\"token\":\"t-TEMP.OFFICE.248\"}");
        assert!(data.join("jsh/justconnect.json").exists());
    }

    #[test]
    fn claude_code_skips_its_first_run_screens_for_the_agent() {
        let temp = tempfile::tempdir().unwrap();
        seed_claude(temp.path());
        let config: Value = serde_json::from_slice(&std::fs::read(temp.path().join(".claude.json")).unwrap()).unwrap();
        assert_eq!(config["hasCompletedOnboarding"], true);
        // what Claude Code wrote itself is kept
        std::fs::write(temp.path().join(".claude.json"), r#"{"theme":"light","numStartups":4}"#).unwrap();
        seed_claude(temp.path());
        let config: Value = serde_json::from_slice(&std::fs::read(temp.path().join(".claude.json")).unwrap()).unwrap();
        assert_eq!((config["theme"].as_str(), config["numStartups"].as_i64(), config["hasCompletedOnboarding"].as_bool()), (Some("light"), Some(4), Some(true)));
    }

    #[test]
    fn the_agents_settings_parse() {
        let s: bro_core::config::Settings = toml::from_str(SETTINGS).unwrap();
        assert!(s.setup_done && !s.updates.check);
        assert_eq!(s.bridge.bind, "127.0.0.1");
    }
}
