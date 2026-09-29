//! `bro-app` — open bro in its own window, like an app (the Start menu / desktop entry point).
//!
//! It hosts `bro` in the JustTerminal fork's app mode (`--app`: no tab strip, no Terminal shortcuts — so
//! ctrl+tab and friends reach bro — and bro's icon on the window and taskbar). Without the fork it falls back
//! to Windows Terminal in a new window, then to a plain console window. `BRO_TERMINAL=<path to wt.exe>` picks
//! the terminal; arguments are passed on to bro (e.g. a folder to open).
//!
//! A windows-subsystem program, so launching it never flashes a console of its own.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// bro's icon, written next to bro's data so terminals can point at a real file.
const ICON: &[u8] = include_bytes!("../../assets/bro.ico");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bro = find_bro();
    let cwd = std::env::current_dir().unwrap_or_else(|_| dirs::home_dir().unwrap_or_default());
    let icon = icon_file();
    let terminal = std::env::var_os("BRO_TERMINAL").map(PathBuf::from).filter(|p| p.is_file()).or_else(fork_terminal);

    let started = match &terminal {
        Some(wt) if supports_app_mode(wt) => spawn(Command::new(wt).args(app_mode_args(&bro, &cwd, icon.as_deref(), &args))),
        _ => false,
    } || which("wt.exe").is_some_and(|wt| spawn(Command::new(wt).args(plain_wt_args(&bro, &cwd, &args))))
        || spawn(Command::new("conhost.exe").arg(&bro).args(&args).current_dir(&cwd));
    if !started {
        std::process::exit(1);
    }
}

/// `bro.exe` next to this launcher, else on PATH.
fn find_bro() -> PathBuf {
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(exe("bro"))));
    here.filter(|p| p.is_file()).or_else(|| which(&exe("bro"))).unwrap_or_else(|| PathBuf::from(exe("bro")))
}

/// The fork installs a per-user `wt.exe` shim that opens the dev build.
fn fork_terminal() -> Option<PathBuf> {
    let shim = dirs::data_local_dir()?.join("Programs").join("WindowsTerminalDevShim").join("wt.exe");
    shim.is_file().then_some(shim)
}

/// Only the fork understands `--app`; assume the shim is the fork, anything else (BRO_TERMINAL) is trusted.
fn supports_app_mode(_wt: &Path) -> bool {
    true
}

fn app_mode_args(bro: &Path, cwd: &Path, icon: Option<&Path>, args: &[String]) -> Vec<String> {
    let mut v = vec!["-w".into(), "new".into(), "--app".into(), "--title".into(), "bro".into()];
    if let Some(i) = icon {
        v.extend(["--app-icon".into(), i.display().to_string()]);
    }
    v.extend(["-d".into(), cwd.display().to_string(), "--".into(), bro.display().to_string()]);
    v.extend(args.iter().cloned());
    v
}

fn plain_wt_args(bro: &Path, cwd: &Path, args: &[String]) -> Vec<String> {
    let mut v = vec!["-w".into(), "new".into(), "--title".into(), "bro".into(), "-d".into(), cwd.display().to_string(), "--".into(), bro.display().to_string()];
    v.extend(args.iter().cloned());
    v
}

/// `~/.bro/bro.ico`, refreshed when it differs from the embedded one.
fn icon_file() -> Option<PathBuf> {
    let dir = std::env::var_os("BRO_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".bro")))?;
    let path = dir.join("bro.ico");
    if std::fs::read(&path).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, ICON).ok()?;
    }
    Some(path)
}

fn spawn(cmd: &mut Command) -> bool {
    cmd.spawn().is_ok()
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_mode_command_line() {
        let a = app_mode_args(Path::new("C:/bin/bro.exe"), Path::new("F:/code/x"), Some(Path::new("C:/u/.bro/bro.ico")), &["F:/code/x".into()]);
        assert_eq!(a[..5], ["-w", "new", "--app", "--title", "bro"]);
        assert!(a.windows(2).any(|w| w[0] == "--app-icon"));
        let dash = a.iter().position(|s| s == "--").unwrap();
        assert_eq!(a[dash + 1], "C:/bin/bro.exe");
        assert_eq!(a.last().unwrap(), "F:/code/x");
    }
}
