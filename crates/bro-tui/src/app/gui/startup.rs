//! Seeing the window through its start. A graphics driver can crash bro while the window starts the GPU (Intel's
//! Vulkan driver does over Remote Desktop): an access violation inside the driver, which no error handling sees,
//! so the window never appears and nothing says why. The copy of bro that starts the window, the launcher
//! ([`watch`]), stays until the window is on screen:
//!
//! - The window writes how far it got to a status file the launcher names (`--window-status <file>`): the
//!   graphics API it's starting and on which GPU, then `ready` after its first frame, or `failed` and why.
//! - A window gone before `ready` with a crash's exit code (an NTSTATUS such as 0xC0000005, or a fatal signal)
//!   while it was starting a graphics API gets that API written down in `~/.bro/v2-gpu.json`, and the launcher
//!   opens it once more. Every start from then on skips that API ([`skip`]); bro-gfx tries the others.
//! - Whatever went wrong is said: on the terminal bro was started from, else in a message box.

use bro_gfx::wgpu::Backends;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// A window not on screen after this long is starting slowly, not failing: the launcher leaves it to it.
const PATIENCE: Duration = Duration::from_secs(30);
/// Starts in a row before the launcher gives up (one per graphics API, and one more).
const MAX_STARTS: u32 = 4;

/// The status file this window reports to (none when nothing waits for it).
static STATUS: OnceLock<PathBuf> = OnceLock::new();

/// Take `--window-status <file>` (what [`watch`] gives the window) off the command line.
pub fn take_flag(args: &mut Vec<String>) {
    if let Some(i) = args.iter().position(|a| a == "--window-status") {
        args.remove(i);
        if i < args.len() {
            let _ = STATUS.set(PathBuf::from(args.remove(i)));
        }
    }
}

/// What the window wrote in its status file.
#[derive(Debug, Clone, PartialEq)]
enum Said {
    /// starting these graphics APIs (named as `WGPU_BACKEND` names them), and on this GPU once one is picked
    Gpu { backends: String, on: Option<String> },
    /// its first frame is on screen
    Ready,
    /// it couldn't start, and this is why
    Failed(String),
}

impl Said {
    fn line(&self) -> String {
        match self {
            Said::Gpu { backends, on } => format!("gpu\t{backends}\t{}", on.as_deref().unwrap_or("")),
            Said::Ready => "ready".into(),
            Said::Failed(why) => format!("failed\t{}", why.replace(['\r', '\n'], " ")),
        }
    }

    fn parse(text: &str) -> Option<Said> {
        let mut parts = text.trim_end_matches(['\r', '\n']).splitn(3, '\t');
        match parts.next()? {
            "ready" => Some(Said::Ready),
            "failed" => Some(Said::Failed(parts.next().unwrap_or("").trim().to_string())),
            "gpu" => {
                let backends = parts.next()?.trim().to_string();
                let on = parts.next().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                Some(Said::Gpu { backends, on })
            }
            _ => None,
        }
    }
}

/// Tell the launcher (a small file written whole, before going on: if the driver crashes the process next,
/// this is what's left to say what it was doing).
fn say(said: &Said) {
    if let Some(path) = STATUS.get() {
        let _ = crate::util::atomic_write(path, said.line().as_bytes());
    }
}

/// Have every graphics API the renderer starts written down. Before the window makes its renderer.
pub fn install() {
    bro_gfx::on_start(note_gpu);
}

fn note_gpu(s: bro_gfx::Starting<'_>) {
    let on = s.adapter.map(|a| if a.driver_info.is_empty() { a.name.clone() } else { format!("{} ({})", a.name, a.driver_info) });
    say(&Said::Gpu { backends: bro_gfx::backend_names(s.backends), on });
}

/// The window's first frame is on screen: its launcher can go.
pub fn ready() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| say(&Said::Ready));
}

/// The window can't start, for this reason (told on stderr too).
pub fn failed(why: &str) {
    eprintln!("bro: {why}");
    say(&Said::Failed(why.to_string()));
}

/// `~/.bro/v2-gpu.json`: the graphics APIs whose driver crashed a starting window on this PC. Delete it to have
/// them tried again.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Crashes {
    #[serde(default)]
    crashed: Vec<Crash>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Crash {
    /// as `WGPU_BACKEND` names them (`vulkan`)
    backends: String,
    /// the GPU and driver it was starting on, when it got that far
    #[serde(default)]
    gpu: String,
    #[serde(default)]
    exit: String,
    #[serde(default)]
    at: String,
}

fn crashes_file() -> PathBuf {
    crate::util::bro_dir().join("v2-gpu.json")
}

fn read_crashes(path: &Path) -> Crashes {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn skipped(c: &Crashes) -> Backends {
    c.crashed.iter().fold(Backends::empty(), |all, c| all | Backends::from_comma_list(&c.backends))
}

/// The graphics APIs to leave alone: ones whose driver crashed a starting window here before.
pub fn skip() -> Backends {
    static SKIP: OnceLock<Backends> = OnceLock::new();
    *SKIP.get_or_init(|| skipped(&read_crashes(&crashes_file())))
}

/// Write down that starting `backends` crashed the window; what's skipped from now on.
fn note_crash(path: &Path, backends: &str, gpu: Option<&str>, exit: &str) -> Backends {
    let mut c = read_crashes(path);
    c.crashed.retain(|old| old.backends != backends);
    c.crashed.push(Crash {
        backends: backends.to_string(),
        gpu: gpu.unwrap_or_default().to_string(),
        exit: exit.to_string(),
        at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    });
    if let Ok(json) = serde_json::to_vec_pretty(&c) {
        let _ = crate::util::atomic_write(path, &json);
    }
    skipped(&c)
}

/// Whether a start that skips `skip` has a graphics API left that hasn't crashed.
fn untried_left(skip: Backends) -> bool {
    bro_gfx::attempts(skip).iter().all(|b| !b.intersects(skip))
}

/// "Vulkan on Intel(R) HD Graphics 630 (31.0.101.2115)".
fn describe(backends: &str, on: Option<&str>) -> String {
    let api = bro_gfx::backend_titles(Backends::from_comma_list(backends));
    let api = if api.is_empty() { backends.to_string() } else { api };
    match on {
        Some(gpu) => format!("{api} on {gpu}"),
        None => api,
    }
}

/// How a window's start went, as its launcher saw it.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// on screen; or gone in good order (it handed the folder to the bro that's running); or still starting
    /// after [`PATIENCE`]
    Fine,
    /// it said why it couldn't start
    Failed(String),
    /// it died with a crash's exit code (`exit` says which), while starting `gpu`'s graphics API (and GPU) if it was
    Crashed { exit: String, gpu: Option<(String, Option<String>)> },
    /// it exited with an ordinary error code, saying nothing
    Closed(i32),
}

/// A crash's exit status, said for people ("exit code 0xC0000005", "signal 11"); None for an ordinary exit.
#[cfg(windows)]
fn crash(status: ExitStatus) -> Option<String> {
    // the system's NTSTATUS errors (an access violation, a stack overflow, a fast-fail abort) and a breakpoint;
    // not 0xFFFFFFFF, what Stop-Process and the like end a process with
    let code = status.code()? as u32;
    (code & 0xFFFF_0000 == 0xC000_0000 || code == 0x8000_0003).then(|| format!("exit code 0x{code:08X}"))
}

#[cfg(unix)]
fn crash(status: ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    // SIGILL, SIGABRT, SIGFPE, SIGSEGV and SIGBUS (not SIGTERM or SIGKILL: someone ended it)
    let bus = if cfg!(target_os = "linux") { 7 } else { 10 };
    status.signal().filter(|n| [4, 6, 8, 11, bus].contains(n)).map(|n| format!("signal {n}"))
}

/// Stay with `child` until it says it's on screen or can't be, goes, or `patience` runs out.
fn wait(child: &mut Child, status: &Path, patience: Duration) -> Outcome {
    let until = Instant::now() + patience;
    loop {
        // exited first, then read: what it wrote just before it went counts
        let exited = child.try_wait();
        let said = std::fs::read_to_string(status).ok().and_then(|t| Said::parse(&t));
        match (said, exited) {
            (Some(Said::Ready), _) => return Outcome::Fine,
            (Some(Said::Failed(why)), _) => return Outcome::Failed(why),
            (said, Ok(Some(code))) => {
                let gpu = match said {
                    Some(Said::Gpu { backends, on }) => Some((backends, on)),
                    _ => None,
                };
                return match crash(code) {
                    Some(exit) => Outcome::Crashed { exit, gpu },
                    None if code.success() => Outcome::Fine,
                    None => Outcome::Closed(code.code().unwrap_or(-1)),
                };
            }
            (_, Err(_)) => return Outcome::Fine,
            _ if Instant::now() >= until => return Outcome::Fine,
            _ => std::thread::sleep(Duration::from_millis(40)),
        }
    }
}

/// Start the window with `start` (given the status file to name to it) and stay until it's on screen. A window
/// its graphics driver crashed is started again without that graphics API, while there's another to try.
/// Problems are told on the terminal (`console`), else in a message box. False when it didn't start at all.
pub fn watch(start: impl FnMut(&Path) -> Option<Child>, console: bool) -> bool {
    watch_with(start, &crashes_file(), console, PATIENCE)
}

fn watch_with(mut start: impl FnMut(&Path) -> Option<Child>, crashes: &Path, console: bool, patience: Duration) -> bool {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let chosen = std::env::var("WGPU_BACKEND").ok().filter(|v| !v.trim().is_empty());
    let mut starts = 0;
    loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let status = std::env::temp_dir().join(format!("bro-window-{}-{n}.status", std::process::id()));
        let _ = std::fs::remove_file(&status);
        let Some(mut child) = start(&status) else { return starts > 0 };
        starts += 1;
        let outcome = wait(&mut child, &status, patience);
        let _ = std::fs::remove_file(&status);
        let problem = match outcome {
            Outcome::Fine => return true,
            Outcome::Failed(why) => format!("{why}\n(bro-cli opens the terminal UI)"),
            Outcome::Closed(code) => format!("the window closed as it opened (exit code {code})\n(bro-cli opens the terminal UI)"),
            Outcome::Crashed { exit, gpu: Some((backends, on)) } => {
                let what = describe(&backends, on.as_deref());
                if let Some(chosen) = &chosen {
                    format!("the graphics driver crashed as the window started ({what}, {exit}). WGPU_BACKEND={chosen} chose it: unset it to let bro pick, or name another (dx12, vulkan, gl)\n(bro-cli opens the terminal UI)")
                } else {
                    let skip = note_crash(crashes, &backends, on.as_deref(), &exit);
                    if untried_left(skip) && starts < MAX_STARTS {
                        if console {
                            eprintln!("bro: the graphics driver crashed as the window started ({what}, {exit}). Opening it again without {}.", describe(&backends, None));
                        }
                        continue;
                    }
                    format!("the graphics driver crashed as the window started ({what}, {exit}), and every graphics API has crashed here now. WGPU_BACKEND=dx12, vulkan or gl tries one again; deleting {} lets bro try them all\n(bro-cli opens the terminal UI)", crashes.display())
                }
            }
            Outcome::Crashed { exit, gpu: None } => format!("the window crashed as it opened ({exit})\n(bro-cli opens the terminal UI)"),
        };
        tell(&problem, console);
        return true;
    }
}

fn tell(problem: &str, console: bool) {
    if console {
        eprintln!("bro: {problem}");
        return;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(&format!("bro couldn't open its window: {problem}")), wide("bro"));
        // SAFETY: two NUL-terminated UTF-16 strings that outlive the call; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn status_lines_read_back() {
        for said in [
            Said::Ready,
            Said::Failed("no graphics adapter: none found".into()),
            Said::Gpu { backends: "vulkan".into(), on: None },
            Said::Gpu { backends: "dx12".into(), on: Some("Intel(R) HD Graphics 630 (31.0.101.2115)".into()) },
        ] {
            assert_eq!(Said::parse(&said.line()), Some(said));
        }
        assert_eq!(Said::parse(&Said::Failed("two\nlines".into()).line()), Some(Said::Failed("two lines".into())));
        assert_eq!(Said::parse(""), None);
        assert_eq!(Said::parse("half a wri"), None);
    }

    #[test]
    fn a_crashed_api_is_skipped_from_then_on() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v2-gpu.json");
        assert_eq!(skipped(&read_crashes(&file)), Backends::empty(), "nothing crashed yet");
        assert_eq!(note_crash(&file, "vulkan", Some("Intel(R) HD Graphics 630 (31.0.101.2115)"), "exit code 0xC0000005"), Backends::VULKAN);
        assert_eq!(note_crash(&file, "vulkan", None, "exit code 0xC0000005"), Backends::VULKAN, "once each");
        assert_eq!(read_crashes(&file).crashed.len(), 1);
        assert_eq!(skipped(&read_crashes(&file)), Backends::VULKAN);
        assert_eq!(describe("vulkan", Some("Intel(R) HD Graphics 630")), "Vulkan on Intel(R) HD Graphics 630");
        if std::env::var_os("WGPU_BACKEND").is_none() {
            assert!(untried_left(Backends::VULKAN));
            assert!(!untried_left(Backends::all()));
        }
    }

    /// A program that exits with `code` (as a crash would, for a code like 0xC0000005).
    #[cfg(windows)]
    fn exits(code: u32) -> Command {
        let mut c = Command::new("cmd");
        c.args(["/c", &format!("exit {}", code as i32)]);
        c
    }

    #[cfg(windows)]
    #[test]
    fn the_launcher_tells_a_crash_from_an_exit() {
        let dir = tempfile::tempdir().unwrap();
        let status = dir.path().join("s.status");
        let run = |code: u32, said: Option<Said>| {
            let _ = std::fs::remove_file(&status);
            if let Some(said) = said {
                std::fs::write(&status, said.line()).unwrap();
            }
            wait(&mut exits(code).spawn().unwrap(), &status, Duration::from_secs(20))
        };
        let gpu = Said::Gpu { backends: "vulkan".into(), on: Some("Intel(R) HD Graphics 630".into()) };
        assert_eq!(run(0xC000_0005, Some(gpu)), Outcome::Crashed { exit: "exit code 0xC0000005".into(), gpu: Some(("vulkan".into(), Some("Intel(R) HD Graphics 630".into()))) });
        assert_eq!(run(0xC000_0005, None), Outcome::Crashed { exit: "exit code 0xC0000005".into(), gpu: None });
        assert_eq!(run(0, None), Outcome::Fine, "handed off to the bro that runs");
        assert_eq!(run(1, None), Outcome::Closed(1));
        assert_eq!(run(0xFFFF_FFFF, Some(Said::Gpu { backends: "dx12".into(), on: None })), Outcome::Closed(-1), "ended by someone (Stop-Process), not a crash");
        assert_eq!(run(1, Some(Said::Failed("there's no session server to watch".into()))), Outcome::Failed("there's no session server to watch".into()));
        assert_eq!(run(0xC000_0005, Some(Said::Ready)), Outcome::Fine, "on screen: what happens after isn't the start's");
    }

    #[cfg(windows)]
    #[test]
    fn a_driver_crash_opens_the_window_again_without_that_api() {
        if std::env::var_os("WGPU_BACKEND").is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let crashes = dir.path().join("v2-gpu.json");
        let mut starts = 0;
        let ok = watch_with(
            |status| {
                starts += 1;
                // the first window crashes in Vulkan's driver; the second, started without it, comes up
                let (said, code) = if starts == 1 { (Said::Gpu { backends: "vulkan".into(), on: None }, 0xC000_0005) } else { (Said::Ready, 0) };
                std::fs::write(status, said.line()).unwrap();
                exits(code).spawn().ok()
            },
            &crashes,
            true,
            Duration::from_secs(20),
        );
        assert!(ok);
        assert_eq!(starts, 2);
        assert_eq!(skipped(&read_crashes(&crashes)), Backends::VULKAN);
        // a window that crashed before it got to the GPU isn't started again, and nothing is blamed
        let mut starts = 0;
        assert!(watch_with(|_| { starts += 1; exits(0xC000_0005).spawn().ok() }, &crashes, true, Duration::from_secs(20)));
        assert_eq!(starts, 1);
        assert_eq!(skipped(&read_crashes(&crashes)), Backends::VULKAN);
        assert!(!watch_with(|_| None, &crashes, true, Duration::from_secs(1)), "never started: the caller runs the window itself");
    }
}
