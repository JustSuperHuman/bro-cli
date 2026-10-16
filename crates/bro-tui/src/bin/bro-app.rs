//! `bro-app` — open bro in its own window, like an app (the Start menu / desktop entry point).
//!
//! It hosts `bro` in the JustTerminal fork's app mode (`--app`: no tab strip, no Terminal shortcuts — so
//! ctrl+tab and friends reach bro — and bro's icon on the window and taskbar). Without the fork it falls back
//! to Windows Terminal in a new window, then to a plain console window. `BRO_TERMINAL=<path to wt.exe>` picks
//! the terminal; arguments are passed on to bro (e.g. a folder to open).
//! Each launch uses `--new` and the newest local bro executable; in the fork bro also gets `--app-window`, which
//! turns on its font choice (the fork switches fonts with OSC 50).
//!
//! A windows-subsystem program, so launching it never flashes a console of its own.
//!
//! Every way in is tried in turn until one works: a terminal alias can outlive the app it pointed to (a dev
//! build removed, a package re-registered), and a launcher that starts but exits with an error within a few
//! seconds didn't open anything, so bro-app moves on to the next one rather than leaving you with nothing.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// A terminal launcher that fails says so quickly (a stale alias, an unknown flag); one still running after this
/// long is the terminal itself, hosting bro.
const LAUNCH_CHECK: Duration = Duration::from_secs(4);

/// bro's icon, written next to bro's data so terminals can point at a real file.
const ICON: &[u8] = include_bytes!("../../assets/bro.ico");

fn main() {
    let args = new_instance_args(std::env::args().skip(1).collect());
    let bro = find_bro();
    let cwd = std::env::current_dir().unwrap_or_else(|_| dirs::home_dir().unwrap_or_default());
    let icon = icon_file();
    let relaunch = std::env::current_exe().ok().map(|launcher| relaunch_command(&launcher, &cwd, &args));
    // BRO_TERMINAL first, then the fork's builds in order: each in app mode until one opens
    let chosen = std::env::var_os("BRO_TERMINAL").map(PathBuf::from).filter(|p| present(p));
    let app_mode = chosen.into_iter().chain(fork_terminals()).any(|wt| {
        let mut cmd = Command::new(&wt);
        cmd.args(app_mode_args(&bro, &cwd, icon.as_deref(), relaunch.as_deref(), &args));
        // the side-by-side build keeps its own phone-bridge data (like the fork's launch:next script)
        if wt.file_name().is_some_and(|n| n.eq_ignore_ascii_case("wtdn.exe"))
            && std::env::var_os("TERMINAL_WEB_DATA_ROOT").is_none()
            && let Some(local) = dirs::data_local_dir()
        {
            cmd.env("TERMINAL_WEB_DATA_ROOT", local.join("TerminalWebNext"));
        }
        launch(&mut cmd)
    });
    let started = app_mode
        || which("wt.exe").is_some_and(|wt| launch(Command::new(wt).args(plain_wt_args(&bro, &cwd, &args))))
        || launch(Command::new("conhost.exe").arg(&bro).args(&args).current_dir(&cwd));
    if !started {
        std::process::exit(1);
    }
}

/// Always start a fresh process, even when an older bro is still running.
fn new_instance_args(mut args: Vec<String>) -> Vec<String> {
    if !args.iter().any(|a| a == "--new") {
        args.insert(0, "--new".into());
    }
    args
}

/// Compare the sibling, Cargo install and every PATH copy by modification time.
/// Start menu launches can inherit a PATH that predates the latest Cargo install.
fn find_bro() -> PathBuf {
    let name = exe("bro");
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&name)));
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".cargo")));
    let installed = cargo_home.map(|d| d.join("bin").join(&name));
    let path = std::env::var_os("PATH").unwrap_or_default();
    let candidates = here.into_iter().chain(installed).chain(std::env::split_paths(&path).map(|d| d.join(&name)));
    newest_bro(candidates).unwrap_or_else(|| PathBuf::from(name))
}

/// Keep the first candidate on a tie so a matching sibling remains preferred.
fn newest_bro(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().filter_map(|path| {
        let metadata = path.metadata().ok()?;
        metadata.is_file().then(|| (path, metadata.modified().unwrap_or(std::time::UNIX_EPOCH)))
    }).reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best }).map(|(path, _)| path)
}

/// The fork's builds that are installed, in order: the side-by-side build (`wtdn.exe`, where new fork features
/// land first), the everyday `wtd.exe` alias, and the per-user `wt.exe` shim that forwards to it.
fn fork_terminals() -> Vec<PathBuf> {
    let Some(local) = dirs::data_local_dir() else { return vec![] };
    let apps = local.join("Microsoft").join("WindowsApps");
    [apps.join("wtdn.exe"), apps.join("wtd.exe"), local.join("Programs").join("WindowsTerminalDevShim").join("wt.exe")]
        .into_iter()
        .filter(|p| present(p))
        .collect()
}

/// Exists — including app execution aliases (wtd.exe, wtdn.exe), which are reparse points `is_file` rejects.
fn present(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

fn app_mode_args(bro: &Path, cwd: &Path, icon: Option<&Path>, relaunch: Option<&str>, args: &[String]) -> Vec<String> {
    let mut v = vec!["-w".into(), "new".into(), "--app".into(), "--title".into(), "bro".into()];
    if let Some(i) = icon {
        v.extend(["--app-icon".into(), i.display().to_string()]);
    }
    if let Some(command) = relaunch {
        v.extend(["--app-relaunch".into(), escape_wt(command)]);
    }
    // in the fork bro may switch the window's font (OSC 50), so it offers the choice
    v.extend(["-d".into(), cwd.display().to_string(), "--".into(), bro.display().to_string(), "--app-window".into()]);
    v.extend(args.iter().map(|a| escape_wt(a)));
    v
}

/// A taskbar pin must reopen the launcher, which resolves bro again and adds --new.
/// Include the original folder because Explorer may relaunch from a different cwd.
fn relaunch_command(launcher: &Path, cwd: &Path, args: &[String]) -> String {
    let mut command = vec![launcher.display().to_string()];
    command.extend_from_slice(args);
    if args.iter().all(|a| a.starts_with('-') || matches!(a.as_str(), "demo" | "help" | "version")) {
        command.push(cwd.display().to_string());
    }
    command.iter().map(|a| quote_windows_arg(a)).collect::<Vec<_>>().join(" ")
}

/// Quote one argument for Windows ShellExecute / CommandLineToArgvW.
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.into();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        quoted.push_str(&"\\".repeat(if ch == '"' { backslashes * 2 + 1 } else { backslashes }));
        backslashes = 0;
        quoted.push(ch);
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

/// Terminal splits its command line into commands at `;` — keep bro's arguments whole.
fn escape_wt(a: &str) -> String {
    a.replace(';', r"\;")
}

fn plain_wt_args(bro: &Path, cwd: &Path, args: &[String]) -> Vec<String> {
    let mut v = vec!["-w".into(), "new".into(), "--title".into(), "bro".into(), "-d".into(), cwd.display().to_string(), "--".into(), bro.display().to_string()];
    v.extend(args.iter().map(|a| escape_wt(a)));
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

/// Start it and see that it took: it fails to start, or exits with an error within [`LAUNCH_CHECK`], and the next
/// way in gets its turn. (Terminal launchers hand the window to the running terminal and exit 0; a terminal or
/// console that keeps running is hosting bro.)
fn launch(cmd: &mut Command) -> bool {
    // an app window shouldn't inherit a shell's colour opt-out (NO_COLOR makes crossterm, PowerShell and most
    // CLIs drop every colour) — bro is started from here, not from that shell
    let Ok(mut child) = cmd.env_remove("NO_COLOR").spawn() else { return false };
    launched(|| child.try_wait().ok().map(|s| s.map(|s| s.success())), LAUNCH_CHECK)
}

/// Poll `status` (None: still running; Some(ok): it exited) until it exits or `within` passes.
fn launched(mut status: impl FnMut() -> Option<Option<bool>>, within: Duration) -> bool {
    let until = Instant::now() + within;
    loop {
        match status() {
            Some(Some(ok)) => return ok,
            // can't tell: assume it runs
            None => return true,
            Some(None) if Instant::now() >= until => return true,
            Some(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
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
        let args = new_instance_args(vec!["F:/code/x".into()]);
        let relaunch = relaunch_command(Path::new("C:/bin/bro-app.exe"), Path::new("F:/code/x"), &args);
        let a = app_mode_args(Path::new("C:/bin/bro.exe"), Path::new("F:/code/x"), Some(Path::new("C:/u/.bro/bro.ico")), Some(&relaunch), &args);
        assert_eq!(a[..5], ["-w", "new", "--app", "--title", "bro"]);
        assert!(a.windows(2).any(|w| w[0] == "--app-icon"));
        assert!(a.windows(2).any(|w| w[0] == "--app-relaunch" && w[1] == "C:/bin/bro-app.exe --new F:/code/x"));
        let dash = a.iter().position(|s| s == "--").unwrap();
        assert_eq!(a[dash + 1], "C:/bin/bro.exe");
        assert_eq!(a[dash + 2], "--app-window", "bro knows it's in the fork's window");
        assert_eq!(a[dash + 3], "--new");
        assert_eq!(a.last().unwrap(), "F:/code/x");
        assert_eq!(escape_wt("a;b"), r"a\;b");
    }

    #[test]
    fn pin_reopens_launcher_in_original_folder() {
        let launcher = Path::new(r"C:\Program Files\bro\bro-app.exe");
        let cwd = Path::new(r"F:\my project");
        assert_eq!(relaunch_command(launcher, cwd, &new_instance_args(vec![])), r#""C:\Program Files\bro\bro-app.exe" --new "F:\my project""#);
        let args = new_instance_args(vec![r"J:\another project".into()]);
        assert_eq!(relaunch_command(launcher, cwd, &args), r#""C:\Program Files\bro\bro-app.exe" --new "J:\another project""#);
    }

    #[test]
    fn windows_argument_quoting_handles_quotes_and_trailing_slashes() {
        assert_eq!(quote_windows_arg(""), r#""""#);
        assert_eq!(quote_windows_arg(r"F:\project"), r"F:\project");
        assert_eq!(quote_windows_arg(r"F:\my project\"), r#""F:\my project\\""#);
        assert_eq!(quote_windows_arg("a\"b"), r#""a\"b""#);
    }

    #[test]
    fn terminal_keeps_semicolons_inside_pin_relaunch_command() {
        let args = new_instance_args(vec![]);
        let cwd = Path::new("F:/code/a;b project");
        let relaunch = relaunch_command(Path::new("C:/bin/bro-app.exe"), cwd, &args);
        let a = app_mode_args(Path::new("C:/bin/bro.exe"), cwd, None, Some(&relaunch), &args);
        let pin = a.iter().position(|s| s == "--app-relaunch").unwrap();
        assert_eq!(a[pin + 1], r#"C:/bin/bro-app.exe --new "F:/code/a\;b project""#);
    }

    #[test]
    fn every_launch_gets_new_without_duplicating_it() {
        assert_eq!(new_instance_args(vec![]), ["--new"]);
        let args = vec!["--new".into(), "--demo".into(), "F:/code/my project".into()];
        assert_eq!(new_instance_args(args.clone()), args);
        assert_eq!(new_instance_args(vec!["--demo".into(), "F:/code/my project".into()]), ["--new", "--demo", "F:/code/my project"]);
    }

    #[test]
    fn plain_terminal_preserves_new_and_folder_arguments() {
        let args = new_instance_args(vec!["F:/code/a;b project".into()]);
        let a = plain_wt_args(Path::new("C:/bin/bro.exe"), Path::new("F:/code/x"), &args);
        let dash = a.iter().position(|s| s == "--").unwrap();
        assert_eq!(&a[dash + 1..], ["C:/bin/bro.exe", "--new", r"F:/code/a\;b project"]);
    }

    fn executable(dir: &Path, name: &str, seconds: u64) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))).unwrap();
        path
    }

    #[test]
    fn newer_install_or_path_copy_wins_over_stale_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let sibling = executable(dir.path(), "sibling.exe", 100);
        let installed = executable(dir.path(), "installed.exe", 200);
        let path_copy = executable(dir.path(), "path.exe", 300);
        assert_eq!(newest_bro([sibling.clone(), installed.clone()]), Some(installed.clone()));
        assert_eq!(newest_bro([sibling, installed, path_copy.clone()]), Some(path_copy));
    }

    #[test]
    fn newer_sibling_wins_and_equal_copies_keep_search_order() {
        let dir = tempfile::tempdir().unwrap();
        let sibling = executable(dir.path(), "sibling.exe", 300);
        let installed = executable(dir.path(), "installed.exe", 200);
        let equal = executable(dir.path(), "equal.exe", 300);
        assert_eq!(newest_bro([sibling.clone(), installed, equal]), Some(sibling));
    }

    #[test]
    fn a_launcher_that_fails_hands_over_to_the_next() {
        let fast = Duration::from_millis(200);
        assert!(launched(|| Some(Some(true)), fast), "exited 0: the terminal took the window");
        assert!(!launched(|| Some(Some(false)), fast), "exited with an error: try the next one");
        assert!(launched(|| Some(None), fast), "still running: it hosts bro");
        let mut calls = 0;
        assert!(!launched(|| { calls += 1; if calls < 3 { Some(None) } else { Some(Some(false)) } }, Duration::from_secs(2)), "fails a moment later");
    }

    #[test]
    fn a_real_failing_launcher_is_caught() {
        // a program that starts and exits with an error, like a stale terminal alias's forwarder
        let mut cmd = if cfg!(windows) { let mut c = Command::new("cmd"); c.args(["/c", "exit 1"]); c } else { let mut c = Command::new("sh"); c.args(["-c", "exit 1"]); c };
        assert!(!launch(&mut cmd));
        let mut ok = if cfg!(windows) { let mut c = Command::new("cmd"); c.args(["/c", "exit 0"]); c } else { Command::new("true") };
        assert!(launch(&mut ok));
        assert!(!launch(&mut Command::new("definitely-not-a-terminal-xyz")));
    }

    #[test]
    fn missing_executables_and_directories_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.exe");
        let valid = executable(dir.path(), "bro.exe", 100);
        assert_eq!(newest_bro([missing.clone(), dir.path().to_path_buf(), valid.clone()]), Some(valid));
        assert_eq!(newest_bro([missing, dir.path().to_path_buf()]), None);
    }
}
