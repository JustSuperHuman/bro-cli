//! Quick Install: the tools a coding machine wants (JustTools first, JustPaste and JustWatch on Windows, JustImagine, then Git, Node.js, Bun, Python,
//! the .NET SDKs, the agent CLIs and a few utilities), which of them are already here, and how to
//! install the rest on this OS.
//!
//! Windows uses WinGet, macOS Homebrew (installed first when it's missing), Linux its own package
//! manager (apt, dnf, pacman, zypper or apk). Bun, uv and the .NET SDKs on macOS / Linux use their
//! official install scripts. JustTools and the agents keep their own installers
//! ([`crate::justtools::install`], [`crate::agent_install::install`]). Every step runs with
//! inherited stdio, so the installer's progress and its password / UAC prompts reach the terminal.
use crate::Harness;
use crate::util::home_dir;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Something Quick Install can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    JustTools,
    /// JustTools' Smart Paste for File Explorer, switched on (Windows only; comes with JustTools)
    JustPaste,
    /// Windows only
    JustWatch,
    /// runs as a service; needs Git and Node.js
    JustImagine,
    Git,
    GitHubCli,
    Node,
    Bun,
    Python,
    Uv,
    Dotnet9,
    Dotnet10,
    Dotnet11,
    Agent(Harness),
    Ripgrep,
    Ffmpeg,
    /// video / audio downloader; always the newest release (sites break old ones quickly)
    YtDlp,
    /// private mesh VPN: reach this machine (and bro's bridge) from your phone anywhere
    Tailscale,
    /// Windows only
    PowerToys,
    /// JustSuperHuman's ShareX fork; Windows only
    ShareX,
}

/// The heading a tool is listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Main,
    Essentials,
    Runtimes,
    Agents,
    Utilities,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Main => "Just apps",
            Group::Essentials => "essentials",
            Group::Runtimes => "runtimes & SDKs",
            Group::Agents => "coding agents",
            Group::Utilities => "utilities",
        }
    }
}

impl Tool {
    /// Every tool, in list order, which is install order except for [Tool::install_rank].
    pub const ALL: [Tool; 23] = [
        Tool::JustTools,
        Tool::JustPaste,
        Tool::JustWatch,
        Tool::JustImagine,
        Tool::Git,
        Tool::GitHubCli,
        Tool::Node,
        Tool::Bun,
        Tool::Python,
        Tool::Uv,
        Tool::Dotnet9,
        Tool::Dotnet10,
        Tool::Dotnet11,
        Tool::Agent(Harness::Claude),
        Tool::Agent(Harness::Codex),
        Tool::Agent(Harness::Pi),
        Tool::Agent(Harness::Omp),
        Tool::Ripgrep,
        Tool::Ffmpeg,
        Tool::YtDlp,
        Tool::Tailscale,
        Tool::PowerToys,
        Tool::ShareX,
    ];

    /// The name `bro install` takes.
    pub fn id(self) -> &'static str {
        match self {
            Tool::JustTools => "justtools",
            Tool::JustPaste => "justpaste",
            Tool::JustWatch => "justwatch",
            Tool::JustImagine => "justimagine",
            Tool::Git => "git",
            Tool::GitHubCli => "gh",
            Tool::Node => "node",
            Tool::Bun => "bun",
            Tool::Python => "python",
            Tool::Uv => "uv",
            Tool::Dotnet9 => "dotnet9",
            Tool::Dotnet10 => "dotnet10",
            Tool::Dotnet11 => "dotnet11",
            Tool::Agent(h) => h.label(),
            Tool::Ripgrep => "rg",
            Tool::Ffmpeg => "ffmpeg",
            Tool::YtDlp => "yt-dlp",
            Tool::Tailscale => "tailscale",
            Tool::PowerToys => "powertoys",
            Tool::ShareX => "sharex",
        }
    }

    /// The tool `name` names: its id or a common alias ("nodejs", "claude-code", "net10").
    pub fn from_id(name: &str) -> Option<Tool> {
        let n = name.trim().to_ascii_lowercase();
        let alias = match n.as_str() {
            "just" | "just-tools" => Some(Tool::JustTools),
            "just-paste" | "paste" | "smart-paste" | "smartpaste" => Some(Tool::JustPaste),
            "just-watch" | "watch" => Some(Tool::JustWatch),
            "just-imagine" | "imagine" => Some(Tool::JustImagine),
            "github-cli" | "github" => Some(Tool::GitHubCli),
            "nodejs" | "node.js" | "npm" => Some(Tool::Node),
            "python3" | "py" => Some(Tool::Python),
            "ripgrep" => Some(Tool::Ripgrep),
            "power-toys" => Some(Tool::PowerToys),
            "ytdlp" | "yt_dlp" => Some(Tool::YtDlp),
            "share-x" => Some(Tool::ShareX),
            "claude-code" => Some(Tool::Agent(Harness::Claude)),
            "codex-cli" => Some(Tool::Agent(Harness::Codex)),
            "oh-my-pi" => Some(Tool::Agent(Harness::Omp)),
            _ => None,
        };
        if alias.is_some() {
            return alias;
        }
        // dotnet9, dotnet-9, net9, .net9
        let dotnet = n.trim_start_matches('.').trim_start_matches("dotnet").trim_start_matches("net").trim_start_matches(['-', '@']);
        if dotnet != n {
            return match dotnet {
                "9" => Some(Tool::Dotnet9),
                "10" => Some(Tool::Dotnet10),
                "11" => Some(Tool::Dotnet11),
                _ => None,
            };
        }
        Tool::ALL.into_iter().find(|t| t.id() == n)
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::JustTools => "JustTools",
            Tool::JustPaste => "JustPaste",
            Tool::JustWatch => "JustWatch",
            Tool::JustImagine => "JustImagine",
            Tool::Git => "Git",
            Tool::GitHubCli => "GitHub CLI",
            Tool::Node => "Node.js LTS",
            Tool::Bun => "Bun",
            Tool::Python => "Python 3",
            Tool::Uv => "uv",
            Tool::Dotnet9 => ".NET 9 SDK",
            Tool::Dotnet10 => ".NET 10 SDK",
            Tool::Dotnet11 => ".NET 11 SDK",
            Tool::Agent(Harness::Claude) => "Claude Code",
            Tool::Agent(Harness::Codex) => "Codex CLI",
            Tool::Agent(Harness::Pi) => "Pi",
            Tool::Agent(Harness::Omp) => "omp (oh-my-pi)",
            Tool::Ripgrep => "ripgrep",
            Tool::Ffmpeg => "FFmpeg",
            Tool::YtDlp => "yt-dlp",
            Tool::Tailscale => "Tailscale",
            Tool::PowerToys => "PowerToys",
            Tool::ShareX => "ShareX (JSH fork)",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Tool::JustTools => "image, video, PDF, QR and port commands, JustCommit and JustReady in one binary",
            Tool::JustPaste => "Ctrl+V in Explorer saves copied images, downloads copied links, clones repos; file copies stay Windows'",
            Tool::JustWatch => "tray app that keeps your dev servers running; agents restart them and read their logs",
            Tool::JustImagine => "your own image, video, song and voice studio, always on in the background; agents generate through its API",
            Tool::Git => "version control; agents expect it",
            Tool::GitHubCli => "issues, pull requests and releases from the terminal",
            Tool::Node => "JavaScript runtime and npm; Codex and Pi install with it",
            Tool::Bun => "fast JavaScript runtime and package manager; omp needs it",
            Tool::Python => "Python with pip",
            Tool::Uv => "fast Python package and project manager",
            Tool::Dotnet9 => "standard-term support release",
            Tool::Dotnet10 => "current long-term support release",
            Tool::Dotnet11 => "next release (a preview until it ships)",
            Tool::Agent(Harness::Claude) => "Anthropic's coding agent, native installer",
            Tool::Agent(Harness::Codex) => "OpenAI's coding agent",
            Tool::Agent(Harness::Pi) => "minimal, extensible coding agent",
            Tool::Agent(Harness::Omp) => "Pi with batteries included",
            Tool::Ripgrep => "fast code search (rg)",
            Tool::Ffmpeg => "audio and video conversion; JustTools' media commands use it",
            Tool::YtDlp => "download video and audio from YouTube and 1000+ sites; pairs with FFmpeg",
            Tool::Tailscale => "private network between your devices: reach bro's bridge from your phone anywhere",
            Tool::PowerToys => "Microsoft's Windows utilities: FancyZones, PowerToys Run, Text Extractor, color picker",
            Tool::ShareX => "screenshots and screen recording; JustSuperHuman's fork with smooth cursor and device frames",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Tool::JustTools | Tool::JustPaste | Tool::JustWatch | Tool::JustImagine => Group::Main,
            Tool::Git | Tool::GitHubCli => Group::Essentials,
            Tool::Node | Tool::Bun | Tool::Python | Tool::Uv | Tool::Dotnet9 | Tool::Dotnet10 | Tool::Dotnet11 => Group::Runtimes,
            Tool::Agent(_) => Group::Agents,
            Tool::Ripgrep | Tool::Ffmpeg | Tool::YtDlp | Tool::Tailscale | Tool::PowerToys | Tool::ShareX => Group::Utilities,
        }
    }

    /// It can be installed on this OS (JustPaste, JustWatch, PowerToys and ShareX are Windows-only).
    pub fn supported(self) -> bool {
        match self {
            Tool::JustPaste => crate::justpaste::supported(),
            Tool::JustWatch => crate::justwatch::supported(),
            Tool::PowerToys | Tool::ShareX => cfg!(windows),
            _ => true,
        }
    }

    /// Every tool this OS can install, in install order.
    pub fn here() -> Vec<Tool> {
        Tool::ALL.into_iter().filter(|t| t.supported()).collect()
    }

    /// Where it goes in install order: its place in [`Tool::ALL`], except JustImagine, which comes
    /// after Git and Node.js because it installs with them.
    pub fn install_rank(self) -> usize {
        if self == Tool::JustImagine { Tool::ALL.len() } else { Tool::ALL.iter().position(|t| *t == self).unwrap_or(Tool::ALL.len()) }
    }

    /// Ticked by default when it's missing.
    pub fn recommended(self) -> bool {
        matches!(self, Tool::JustTools | Tool::Git | Tool::Node | Tool::Bun | Tool::Agent(Harness::Claude) | Tool::Agent(Harness::Codex))
    }

    /// The .NET major version this installs.
    fn dotnet_major(self) -> Option<u32> {
        match self {
            Tool::Dotnet9 => Some(9),
            Tool::Dotnet10 => Some(10),
            Tool::Dotnet11 => Some(11),
            _ => None,
        }
    }

    /// Executables that show it's installed.
    fn exes(self) -> &'static [&'static str] {
        match self {
            Tool::Git => &["git"],
            Tool::GitHubCli => &["gh"],
            Tool::Node => &["node"],
            Tool::Bun => &["bun"],
            Tool::Python => &["python3", "python", "py"],
            Tool::Uv => &["uv"],
            Tool::Agent(h) => match h {
                Harness::Claude => &["claude"],
                Harness::Codex => &["codex"],
                Harness::Pi => &["pi"],
                Harness::Omp => &["omp"],
            },
            Tool::Ripgrep => &["rg"],
            Tool::Ffmpeg => &["ffmpeg"],
            Tool::YtDlp => &["yt-dlp"],
            Tool::Tailscale => &["tailscale"],
            Tool::PowerToys => &["PowerToys"],
            Tool::ShareX => &["ShareX"],
            Tool::JustTools | Tool::JustPaste | Tool::JustWatch | Tool::JustImagine | Tool::Dotnet9 | Tool::Dotnet10 | Tool::Dotnet11 => &[],
        }
    }
}

// ---- finding what's installed ----

/// The installed tools, each with a short detail ("2.8.0", "9.0.305", a path).
pub type Status = HashMap<Tool, String>;

#[cfg(windows)]
const ENV_KEYS: [&str; 2] = [r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment", r"HKCU\Environment"];

/// `%NAME%` replaced by its value (a `REG_EXPAND_SZ` PATH).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn expand_vars(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => out.push_str(&rest[start..start + end + 2]),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of a `reg query … /v Path` answer.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn reg_value(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let l = l.trim();
        let (_, rest) = l.split_once("REG_")?;
        let (_, value) = rest.split_once(char::is_whitespace)?;
        Some(value.trim().to_string())
    })
}

/// PATH as a new terminal would see it: on Windows the machine + user PATH from the registry
/// (an installer that just ran changed it, not ours); elsewhere the dirs installers drop tools in.
pub fn fresh_path_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    #[cfg(windows)]
    for key in ENV_KEYS {
        let out = quiet(Command::new("reg").args(["query", key, "/v", "Path"])).output();
        if let Some(v) = out.ok().filter(|o| o.status.success()).and_then(|o| reg_value(&String::from_utf8_lossy(&o.stdout))) {
            dirs.extend(std::env::split_paths(&expand_vars(&v)));
        }
    }
    #[cfg(not(windows))]
    {
        let home = home_dir();
        for d in [home.join(".dotnet"), home.join(".cargo").join("bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/home/linuxbrew/.linuxbrew/bin")] {
            dirs.push(d);
        }
    }
    dirs
}

/// Where to look: this process's PATH, the fresh one, then the usual shim dirs.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(fresh_path_dirs());
    dirs.extend(crate::launch::extra_bin_dirs());
    if cfg!(windows) {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            let pf = PathBuf::from(pf);
            dirs.extend([pf.join("Git").join("cmd"), pf.join("nodejs"), pf.join("dotnet"), pf.join("GitHub CLI"), pf.join("Tailscale"), pf.join("PowerToys"), pf.join("ShareX")]);
        }
        // PowerToys' per-user install
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("PowerToys"));
        }
    } else {
        dirs.push(home_dir().join(".dotnet"));
    }
    if cfg!(target_os = "macos") {
        // the Tailscale app's own CLI, before (or without) "install CLI" from its menu
        dirs.push(PathBuf::from("/Applications/Tailscale.app/Contents/MacOS"));
    }
    let mut seen: Vec<PathBuf> = vec![];
    dirs.retain(|d| !d.as_os_str().is_empty() && !seen.iter().any(|s| crate::util::same_path(s, d)) && {
        seen.push(d.clone());
        true
    });
    dirs
}

/// Every `name` in `dirs`, in order.
fn find_all<'a>(name: &'a str, dirs: &'a [PathBuf]) -> impl Iterator<Item = PathBuf> + 'a {
    let exts: &[&str] = if cfg!(windows) { &["exe", "cmd", "bat"] } else { &[""] };
    dirs.iter().flat_map(move |d| exts.iter().map(move |e| if e.is_empty() { d.join(name) } else { d.join(format!("{name}.{e}")) })).filter(|p| p.is_file())
}

/// `name` in one of `dirs`.
pub fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    find_all(name, dirs).next()
}

/// "3.14.3" from a Python that runs. Windows' Microsoft Store stand-in (`python.exe` in
/// WindowsApps that only opens the Store) prints nothing; the Python Install Manager's
/// alias in the same folder is a real Python.
fn python_version(exe: &std::path::Path) -> Option<String> {
    let out = quiet(Command::new(exe).arg("--version")).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v = text.trim().strip_prefix("Python ")?.trim();
    (out.status.success() && v.starts_with(|c: char| c.is_ascii_digit())).then(|| v.to_string())
}

fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null()).stderr(Stdio::null())
}

/// The SDK versions in `dotnet --list-sdks` ("9.0.305 [C:\Program Files\dotnet\sdk]").
fn parse_sdks(out: &str) -> Vec<String> {
    out.lines().filter_map(|l| l.split_whitespace().next()).filter(|v| v.starts_with(|c: char| c.is_ascii_digit())).map(str::to_string).collect()
}

fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// Every .NET SDK each `dotnet` in `dirs` knows about.
fn dotnet_sdks(dirs: &[PathBuf]) -> Vec<String> {
    let mut hosts: Vec<PathBuf> = dirs.iter().filter_map(|d| find("dotnet", std::slice::from_ref(d))).collect();
    hosts.dedup_by(|a, b| crate::util::same_path(a, b));
    let mut sdks: Vec<String> = vec![];
    for host in hosts.iter().take(4) {
        if let Ok(o) = quiet(Command::new(host).arg("--list-sdks").env("DOTNET_CLI_TELEMETRY_OPTOUT", "1").env("DOTNET_NOLOGO", "1")).output() {
            for v in parse_sdks(&String::from_utf8_lossy(&o.stdout)) {
                if !sdks.contains(&v) {
                    sdks.push(v);
                }
            }
        }
    }
    sdks
}

fn newest(a: &str, b: &str) -> std::cmp::Ordering {
    use crate::justtools::is_newer;
    if is_newer(a, b) {
        std::cmp::Ordering::Greater
    } else if is_newer(b, a) {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    }
}

/// What's installed. Blocking (runs `just --version`, `dotnet --list-sdks`): call off the UI thread.
pub fn detect() -> Status {
    let dirs = search_dirs();
    let mut status = Status::new();
    if let Some(found) = crate::justtools::installed() {
        // switched on is what counts as installed
        if let Some(paste) = crate::justpaste::supported().then(|| crate::justpaste::status(&found).ok().flatten()).flatten().filter(|p| p.running) {
            status.insert(Tool::JustPaste, crate::justpaste::describe(&paste).into());
        }
        status.insert(Tool::JustTools, found.version.unwrap_or_else(|| "installed".into()));
    }
    if let Some(found) = crate::justwatch::installed() {
        status.insert(Tool::JustWatch, found.version.unwrap_or_else(|| "installed".into()));
    }
    if let Some(found) = crate::justimagine::installed() {
        status.insert(Tool::JustImagine, found.describe());
    }
    let sdks = dotnet_sdks(&dirs);
    for tool in Tool::ALL {
        if let Some(want) = tool.dotnet_major() {
            // the newest SDK of that major
            if let Some(v) = sdks.iter().filter(|v| major(v) == Some(want)).max_by(|a, b| newest(a, b)) {
                status.insert(tool, v.clone());
            }
            continue;
        }
        if tool == Tool::Python {
            if let Some(v) = tool.exes().iter().flat_map(|e| find_all(e, &dirs)).take(6).find_map(|p| python_version(&p)) {
                status.insert(tool, v);
            }
            continue;
        }
        if let Some(p) = tool.exes().iter().find_map(|e| find(e, &dirs)) {
            status.insert(tool, p.display().to_string());
        }
    }
    status
}

// ---- installing ----

/// A Linux package manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pkg {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Apk,
}

impl Pkg {
    const ALL: [(Pkg, &'static str); 5] = [(Pkg::Apt, "apt-get"), (Pkg::Dnf, "dnf"), (Pkg::Pacman, "pacman"), (Pkg::Zypper, "zypper"), (Pkg::Apk, "apk")];

    /// The shell command that installs `pkgs`.
    fn install(self, sudo: &str, pkgs: &str) -> String {
        match self {
            // a fresh machine's package lists may be empty or stale
            Pkg::Apt => format!("{sudo}apt-get install -y {pkgs} || ({sudo}apt-get update && {sudo}apt-get install -y {pkgs})"),
            Pkg::Dnf => format!("{sudo}dnf install -y {pkgs}"),
            Pkg::Pacman => format!("{sudo}pacman -S --needed --noconfirm {pkgs}"),
            Pkg::Zypper => format!("{sudo}zypper --non-interactive install {pkgs}"),
            Pkg::Apk => format!("{sudo}apk add {pkgs}"),
        }
    }
}

/// The OS and what it has to install with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Os {
    Windows { winget: bool },
    Mac { brew: Option<PathBuf> },
    Linux { pkg: Option<Pkg>, root: bool },
}

impl Os {
    /// This machine. Blocking on Linux (`id -u`).
    pub fn detect() -> Os {
        let dirs = search_dirs();
        if cfg!(windows) {
            Os::Windows { winget: find("winget", &dirs).is_some() }
        } else if cfg!(target_os = "macos") {
            Os::Mac { brew: find("brew", &dirs) }
        } else {
            let pkg = Pkg::ALL.iter().find(|(_, exe)| find(exe, &dirs).is_some()).map(|(p, _)| *p);
            let root = Command::new("id").arg("-u").output().ok().is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
            Os::Linux { pkg, root }
        }
    }

    /// What installs things here, for the view's footer.
    pub fn describe(&self) -> &'static str {
        match self {
            Os::Windows { winget: true } => "Windows · WinGet and official installers",
            Os::Windows { winget: false } => "Windows · WinGet is missing: install App Installer from the Microsoft Store",
            Os::Mac { brew: Some(_) } => "macOS · Homebrew and official installers",
            Os::Mac { brew: None } => "macOS · Homebrew is installed first, then the rest",
            Os::Linux { pkg: Some(Pkg::Apt), .. } => "Linux · apt and official installers",
            Os::Linux { pkg: Some(Pkg::Dnf), .. } => "Linux · dnf and official installers",
            Os::Linux { pkg: Some(Pkg::Pacman), .. } => "Linux · pacman and official installers",
            Os::Linux { pkg: Some(Pkg::Zypper), .. } => "Linux · zypper and official installers",
            Os::Linux { pkg: Some(Pkg::Apk), .. } => "Linux · apk and official installers",
            Os::Linux { pkg: None, .. } => "Linux · no apt, dnf, pacman, zypper or apk found: only official installers work",
        }
    }
}

/// One command an install runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    /// What to print before running it.
    pub display: String,
    /// Exit codes that also mean it worked (WinGet's "already installed").
    pub ok_codes: Vec<i32>,
    /// Run when this one fails (the .NET 11 preview while 11 hasn't shipped).
    pub or_else: Option<Box<Step>>,
}

fn sh(cmd: impl Into<String>) -> Step {
    let cmd = cmd.into();
    Step { program: "sh".into(), args: vec!["-c".into(), cmd.clone()], display: cmd, ok_codes: vec![], or_else: None }
}

fn powershell(cmd: impl Into<String>) -> Step {
    let cmd = cmd.into();
    let args = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &cmd].map(String::from).to_vec();
    Step { program: "powershell.exe".into(), args, display: cmd, ok_codes: vec![], or_else: None }
}

/// WinGet's "no applicable upgrade" and "already installed".
const WINGET_ALREADY: [i32; 2] = [0x8A15002Bu32 as i32, 0x8A150061u32 as i32];

fn winget(id: &str) -> Step {
    let args = ["install", "--id", id, "--exact", "--source", "winget", "--accept-package-agreements", "--accept-source-agreements"].map(String::from).to_vec();
    Step { program: "winget".into(), display: format!("winget {}", args.join(" ")), args, ok_codes: WINGET_ALREADY.to_vec(), or_else: None }
}

/// The newest setup from JustSuperHuman's ShareX fork, run elevated (it installs into Program Files) and
/// without starting ShareX afterwards. No double quotes: they don't survive powershell.exe's -Command parsing.
const SHAREX_INSTALL: &str = "$r = irm https://api.github.com/repos/JustSuperHuman/ShareX/releases/latest; \
$a = $r.assets | Where-Object name -like '*-setup-x64.exe' | Select-Object -First 1; \
if (-not $a) { throw ('no ShareX setup in ' + $r.tag_name) }; \
Write-Host ('ShareX ' + $r.tag_name); \
$f = Join-Path $env:TEMP $a.name; Invoke-WebRequest $a.browser_download_url -OutFile $f -UseBasicParsing; \
$p = Start-Process $f -ArgumentList '/SILENT','/NORESTART','/NORUN' -Verb RunAs -Wait -PassThru; \
Remove-Item $f -ErrorAction SilentlyContinue; exit $p.ExitCode";

const HOMEBREW_INSTALL: &str = r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#;

fn brew(brew: &Option<PathBuf>, args: &str) -> Step {
    // a Homebrew installed a step ago isn't on PATH yet
    let exe = brew.as_ref().map_or_else(|| "$( [ -x /opt/homebrew/bin/brew ] && echo /opt/homebrew/bin/brew || echo /usr/local/bin/brew )".into(), |p| p.display().to_string());
    let mut s = sh(format!("{exe} install {args}"));
    s.display = format!("brew install {args}");
    s
}

fn dotnet_script(major: u32) -> Step {
    let script = "curl -fsSL https://dot.net/v1/dotnet-install.sh | bash -s --";
    let mut step = sh(format!("{script} --channel {major}.0"));
    if major >= 11 {
        step.or_else = Some(Box::new(sh(format!("{script} --channel {major}.0 --quality preview"))));
    }
    step
}

/// The commands that install `tool` on `os`. JustTools, JustImagine and the agents have their own
/// installers and get an empty plan.
pub fn plan(tool: Tool, os: &Os) -> Result<Vec<Step>, String> {
    if matches!(tool, Tool::JustTools | Tool::JustImagine | Tool::Agent(_)) {
        return Ok(vec![]);
    }
    if tool == Tool::JustPaste {
        // switched on with JustTools' own `justpaste` ([`crate::justpaste::set`])
        return match os {
            Os::Windows { .. } => Ok(vec![]),
            _ => Err("JustPaste's Smart Paste is for Windows' File Explorer".into()),
        };
    }
    if tool == Tool::JustWatch {
        return match os {
            Os::Windows { .. } => Ok(vec![powershell(crate::justwatch::installer_command())]),
            _ => Err("JustWatch is a Windows app".into()),
        };
    }
    if matches!(tool, Tool::ShareX | Tool::PowerToys) && !matches!(os, Os::Windows { .. }) {
        return Err(format!("{} is a Windows app", tool.name()));
    }
    if tool == Tool::ShareX {
        return Ok(vec![powershell(SHAREX_INSTALL)]);
    }
    // official scripts, the same everywhere
    match (tool, os) {
        // distro packages lag behind and sites break old yt-dlp quickly: the newest standalone build
        (Tool::YtDlp, Os::Linux { pkg, root }) => {
            let sudo = if *root { "" } else { "sudo " };
            let libc = if *pkg == Some(Pkg::Apk) { "musllinux" } else { "linux" };
            return Ok(vec![sh(format!(
                "case $(uname -m) in aarch64|arm64) a=_aarch64;; *) a=;; esac; \
                 {sudo}curl -fL https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_{libc}$a -o /usr/local/bin/yt-dlp && {sudo}chmod a+rx /usr/local/bin/yt-dlp"
            ))]);
        }
        // its script picks the repository for the distro and asks for sudo itself
        (Tool::Tailscale, Os::Linux { .. }) => return Ok(vec![sh("curl -fsSL https://tailscale.com/install.sh | sh")]),
        (Tool::Bun, Os::Windows { .. }) => return Ok(vec![powershell("irm https://bun.sh/install.ps1 | iex")]),
        (Tool::Uv, Os::Windows { .. }) => return Ok(vec![powershell("irm https://astral.sh/uv/install.ps1 | iex")]),
        (Tool::Uv, _) => return Ok(vec![sh("curl -LsSf https://astral.sh/uv/install.sh | sh")]),
        (Tool::Bun, Os::Linux { pkg, root }) => {
            // its installer unpacks a zip
            let mut steps = vec![];
            if let Some(p) = pkg.filter(|_| find("unzip", &search_dirs()).is_none()) {
                steps.push(sh(p.install(if *root { "" } else { "sudo " }, "unzip")));
            }
            steps.push(sh("curl -fsSL https://bun.sh/install | bash"));
            return Ok(steps);
        }
        (Tool::Bun, Os::Mac { .. }) => return Ok(vec![sh("curl -fsSL https://bun.sh/install | bash")]),
        _ => {}
    }
    match os {
        Os::Windows { winget: false } => Err(format!("{} needs WinGet: install App Installer from the Microsoft Store, then try again", tool.name())),
        Os::Windows { winget: true } => {
            let id = match tool {
                Tool::Git => "Git.Git",
                Tool::GitHubCli => "GitHub.cli",
                Tool::Node => "OpenJS.NodeJS.LTS",
                Tool::Python => "Python.Python.3.14",
                Tool::Dotnet9 => "Microsoft.DotNet.SDK.9",
                Tool::Dotnet10 => "Microsoft.DotNet.SDK.10",
                Tool::Dotnet11 => {
                    let mut step = winget("Microsoft.DotNet.SDK.11");
                    step.or_else = Some(Box::new(winget("Microsoft.DotNet.SDK.Preview")));
                    return Ok(vec![step]);
                }
                Tool::Ripgrep => "BurntSushi.ripgrep.MSVC",
                Tool::Ffmpeg => "Gyan.FFmpeg",
                Tool::YtDlp => "yt-dlp.yt-dlp",
                Tool::Tailscale => "Tailscale.Tailscale",
                Tool::PowerToys => "Microsoft.PowerToys",
                _ => unreachable!("planned above"),
            };
            Ok(vec![winget(id)])
        }
        Os::Mac { brew: b } => {
            if let Some(m) = tool.dotnet_major() {
                return Ok(vec![dotnet_script(m)]);
            }
            let pkg = match tool {
                Tool::Git => "git",
                Tool::GitHubCli => "gh",
                Tool::Node => "node",
                Tool::Python => "python",
                Tool::Ripgrep => "ripgrep",
                Tool::Ffmpeg => "ffmpeg",
                Tool::YtDlp => "yt-dlp",
                // the app (menu bar + system extension); the plain formula is a headless daemon
                Tool::Tailscale => "--cask tailscale-app",
                _ => unreachable!("planned above"),
            };
            let mut steps = vec![];
            if b.is_none() && find("brew", &search_dirs()).is_none() {
                steps.push(sh(HOMEBREW_INSTALL));
            }
            steps.push(brew(b, pkg));
            Ok(steps)
        }
        Os::Linux { pkg, root } => {
            if let Some(m) = tool.dotnet_major() {
                return Ok(vec![dotnet_script(m)]);
            }
            let Some(pkg) = *pkg else {
                return Err(format!("no apt, dnf, pacman, zypper or apk here: install {} with your package manager", tool.name()));
            };
            let sudo = if *root { "" } else { "sudo " };
            if tool == Tool::Node && matches!(pkg, Pkg::Apt | Pkg::Dnf) {
                // the distro's Node.js is often too old for the agents: NodeSource's current LTS
                let (url, pm) = if pkg == Pkg::Apt { ("https://deb.nodesource.com/setup_lts.x", "apt-get") } else { ("https://rpm.nodesource.com/setup_lts.x", "dnf") };
                return Ok(vec![sh(format!("curl -fsSL {url} | {sudo}bash - && {sudo}{pm} install -y nodejs"))]);
            }
            let names = match (tool, pkg) {
                (Tool::Git, _) => "git",
                (Tool::GitHubCli, Pkg::Pacman | Pkg::Apk) => "github-cli",
                (Tool::GitHubCli, _) => "gh",
                (Tool::Node, _) => "nodejs npm",
                (Tool::Python, Pkg::Apt) => "python3 python3-pip python3-venv",
                (Tool::Python, Pkg::Pacman) => "python python-pip",
                (Tool::Python, Pkg::Apk) => "python3 py3-pip",
                (Tool::Python, _) => "python3 python3-pip",
                (Tool::Ripgrep, _) => "ripgrep",
                (Tool::Ffmpeg, Pkg::Dnf) => "ffmpeg-free",
                (Tool::Ffmpeg, _) => "ffmpeg",
                _ => unreachable!("planned above"),
            };
            Ok(vec![sh(pkg.install(sudo, names))])
        }
    }
}

/// One line on how `tool` gets installed here, for the view.
pub fn how(tool: Tool, os: &Os) -> String {
    match tool {
        Tool::JustTools => "JustTools' own installer; the download is checked against its SHA-256".into(),
        Tool::JustPaste if !matches!(os, Os::Windows { .. }) => "JustPaste's Smart Paste is for Windows' File Explorer".into(),
        Tool::JustPaste => "comes with JustTools: justpaste --enable && justpaste --startup on (turn it off in settings)".into(),
        Tool::JustWatch if !matches!(os, Os::Windows { .. }) => "JustWatch is a Windows app".into(),
        Tool::JustWatch => format!("{} · then its skill for new sessions", crate::justwatch::installer_command()),
        Tool::ShareX if matches!(os, Os::Windows { .. }) => "the newest setup-x64.exe from github.com/JustSuperHuman/ShareX releases, run elevated (/SILENT)".into(),
        Tool::JustImagine => crate::justimagine::how(),
        Tool::Agent(h) => match crate::agent_install::plan(h) {
            Ok(steps) => steps.iter().map(|s| s.display.as_str()).collect::<Vec<_>>().join(" && "),
            Err(e) => e,
        },
        _ => match plan(tool, os) {
            Ok(steps) => steps.iter().map(|s| s.display.as_str()).collect::<Vec<_>>().join(" && "),
            Err(e) => e,
        },
    }
}

pub(crate) fn run(step: &Step) -> Result<(), String> {
    let status = Command::new(&step.program).args(&step.args).status().map_err(|e| format!("couldn't run {}: {e}", step.program))?;
    if status.success() || status.code().is_some_and(|c| step.ok_codes.contains(&c)) {
        Ok(())
    } else {
        Err(format!("`{}` failed ({status})", step.display))
    }
}

/// Install `tool` (not JustTools or an agent) in the current terminal. `say` gets progress lines.
pub fn install(tool: Tool, mut say: impl FnMut(&str)) -> Result<(), String> {
    for step in plan(tool, &Os::detect())? {
        say(&format!("$ {}", step.display));
        if let Err(e) = run(&step) {
            let Some(alt) = &step.or_else else { return Err(e) };
            say(&format!("$ {}", alt.display));
            run(alt)?;
        }
    }
    Ok(())
}

/// After an install: what to do before the tool can be used, if anything.
pub fn after_install(tool: Tool) -> Option<String> {
    if tool == Tool::Tailscale {
        return Some(if cfg!(target_os = "linux") { "sign in: sudo tailscale up".into() } else { "sign in from the Tailscale app (tray / menu bar)".into() });
    }
    if tool.dotnet_major().is_some() && !cfg!(windows) {
        let dir = home_dir().join(".dotnet");
        if find("dotnet", &std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default()).is_none() {
            return Some(format!(
                "add .NET to your shell profile: export DOTNET_ROOT=\"{0}\"; export PATH=\"$PATH:{0}\"",
                dir.display()
            ));
        }
    }
    None
}

/// `dirs` that exist and aren't in `path` yet, appended to it: PATH for the next install step.
pub fn path_with(path: &std::ffi::OsStr, dirs: &[PathBuf]) -> Option<std::ffi::OsString> {
    let mut all: Vec<PathBuf> = std::env::split_paths(path).collect();
    let before = all.len();
    for d in dirs {
        if d.is_dir() && !all.iter().any(|a| crate::util::same_path(a, d)) {
            all.push(d.clone());
        }
    }
    (all.len() > before).then(|| std::env::join_paths(all).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_aliases_resolve() {
        for t in Tool::ALL {
            assert_eq!(Tool::from_id(t.id()), Some(t), "{t:?}");
        }
        assert_eq!(Tool::from_id("NodeJS"), Some(Tool::Node));
        assert_eq!(Tool::from_id("claude-code"), Some(Tool::Agent(Harness::Claude)));
        assert_eq!(Tool::from_id("dotnet-10"), Some(Tool::Dotnet10));
        assert_eq!(Tool::from_id("net11"), Some(Tool::Dotnet11));
        assert_eq!(Tool::from_id(".NET9"), Some(Tool::Dotnet9));
        assert_eq!(Tool::from_id("dotnet8"), None);
        assert_eq!(Tool::from_id("vim"), None);
        assert_eq!(Tool::ALL[0], Tool::JustTools, "JustTools comes first");
        let pos = |t| Tool::ALL.iter().position(|x| *x == t).unwrap();
        assert!(pos(Tool::Node) < pos(Tool::Agent(Harness::Codex)) && pos(Tool::Bun) < pos(Tool::Agent(Harness::Omp)));
        assert_eq!(Tool::from_id("imagine"), Some(Tool::JustImagine));
        assert!(Tool::JustImagine.install_rank() > Tool::Node.install_rank() && Tool::JustImagine.install_rank() > Tool::Git.install_rank());
        assert_eq!(Tool::Node.install_rank(), pos(Tool::Node));
    }

    #[test]
    fn windows_plans_use_winget() {
        let os = Os::Windows { winget: true };
        assert_eq!(plan(Tool::Git, &os).unwrap()[0].args[2], "Git.Git");
        assert!(plan(Tool::Node, &os).unwrap()[0].display.contains("OpenJS.NodeJS.LTS"));
        let dn = plan(Tool::Dotnet11, &os).unwrap();
        assert!(dn[0].display.contains("SDK.11") && dn[0].or_else.as_ref().unwrap().display.contains("SDK.Preview"));
        assert!(dn[0].ok_codes.contains(&-1978335189));
        assert!(plan(Tool::Bun, &os).unwrap()[0].display.contains("bun.sh/install.ps1"));
        assert!(plan(Tool::Git, &Os::Windows { winget: false }).unwrap_err().contains("App Installer"));
        assert!(plan(Tool::JustTools, &os).unwrap().is_empty() && plan(Tool::Agent(Harness::Pi), &os).unwrap().is_empty());
        assert!(plan(Tool::JustImagine, &os).unwrap().is_empty() && how(Tool::JustImagine, &os).contains("service install"));
        assert!(plan(Tool::JustWatch, &os).unwrap()[0].display.contains("JustWatch/main/install.ps1"));
        assert!(plan(Tool::JustWatch, &Os::Linux { pkg: Some(Pkg::Apt), root: false }).is_err());
        assert_eq!(plan(Tool::Tailscale, &os).unwrap()[0].args[2], "Tailscale.Tailscale");
        assert_eq!(plan(Tool::PowerToys, &os).unwrap()[0].args[2], "Microsoft.PowerToys");
        assert_eq!(plan(Tool::YtDlp, &os).unwrap()[0].args[2], "yt-dlp.yt-dlp");
        assert_eq!(Tool::from_id("ytdlp"), Some(Tool::YtDlp));
        let sharex = &plan(Tool::ShareX, &os).unwrap()[0];
        assert!(sharex.program == "powershell.exe" && sharex.display.contains("JustSuperHuman/ShareX/releases/latest") && sharex.display.contains("/NORUN"));
        let apt = Os::Linux { pkg: Some(Pkg::Apt), root: false };
        assert!(plan(Tool::ShareX, &apt).is_err() && plan(Tool::PowerToys, &apt).is_err());
        assert!(plan(Tool::JustPaste, &os).unwrap().is_empty() && plan(Tool::JustPaste, &apt).is_err());
        assert!(how(Tool::JustPaste, &os).contains("justpaste --enable"));
        assert_eq!(Tool::from_id("smart-paste"), Some(Tool::JustPaste));
        assert_eq!(Tool::here().contains(&Tool::JustPaste), cfg!(windows));
        assert_eq!(Tool::here().contains(&Tool::ShareX), cfg!(windows));
        assert_eq!(Tool::here().contains(&Tool::PowerToys), cfg!(windows));
        assert_eq!(Tool::from_id("ShareX"), Some(Tool::ShareX));
        assert_eq!(Tool::here().contains(&Tool::JustWatch), cfg!(windows));
    }

    #[test]
    fn mac_and_linux_plans() {
        let mac = Os::Mac { brew: Some(PathBuf::from("/opt/homebrew/bin/brew")) };
        assert_eq!(plan(Tool::Ripgrep, &mac).unwrap()[0].display, "brew install ripgrep");
        assert!(plan(Tool::Dotnet9, &mac).unwrap()[0].display.ends_with("--channel 9.0"));

        let apt = Os::Linux { pkg: Some(Pkg::Apt), root: false };
        let git = &plan(Tool::Git, &apt).unwrap()[0].display;
        assert!(git.starts_with("sudo apt-get install -y git ||") && git.contains("apt-get update"), "{git}");
        assert!(plan(Tool::Node, &apt).unwrap()[0].display.contains("deb.nodesource.com"));
        assert_eq!(plan(Tool::GitHubCli, &Os::Linux { pkg: Some(Pkg::Pacman), root: true }).unwrap()[0].display, "pacman -S --needed --noconfirm github-cli");
        assert_eq!(plan(Tool::Ffmpeg, &Os::Linux { pkg: Some(Pkg::Dnf), root: false }).unwrap()[0].display, "sudo dnf install -y ffmpeg-free");
        let dn = plan(Tool::Dotnet11, &apt).unwrap();
        assert!(dn[0].or_else.as_ref().unwrap().display.contains("--quality preview"));
        assert!(plan(Tool::Git, &Os::Linux { pkg: None, root: false }).is_err());
        assert!(plan(Tool::Uv, &Os::Linux { pkg: None, root: false }).unwrap()[0].display.contains("astral.sh/uv/install.sh"));
        assert_eq!(plan(Tool::Tailscale, &mac).unwrap()[0].display, "brew install --cask tailscale-app");
        assert_eq!(plan(Tool::YtDlp, &mac).unwrap()[0].display, "brew install yt-dlp");
        let yt = &plan(Tool::YtDlp, &apt).unwrap()[0].display;
        assert!(yt.contains("releases/latest/download/yt-dlp_linux$a") && yt.contains("sudo curl"), "{yt}");
        let alpine = &plan(Tool::YtDlp, &Os::Linux { pkg: Some(Pkg::Apk), root: true }).unwrap()[0].display;
        assert!(alpine.contains("yt-dlp_musllinux$a") && !alpine.contains("sudo"), "{alpine}");
        assert!(plan(Tool::Tailscale, &Os::Linux { pkg: None, root: false }).unwrap()[0].display.contains("tailscale.com/install.sh"));
    }

    #[test]
    fn parses_sdks_registry_values_and_vars() {
        let sdks = parse_sdks("9.0.305 [C:\\Program Files\\dotnet\\sdk]\n10.0.100 [C:\\Program Files\\dotnet\\sdk]\n");
        assert_eq!(sdks, vec!["9.0.305", "10.0.100"]);
        assert_eq!(major("11.0.100-rc.2.25502.107"), Some(11));
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    %USERPROFILE%\\bin;C:\\x\r\n\r\n";
        assert_eq!(reg_value(out).as_deref(), Some("%USERPROFILE%\\bin;C:\\x"));
        let (name, value) = std::env::vars().find(|(k, v)| !k.is_empty() && !k.contains('=') && !v.contains('%')).unwrap();
        assert_eq!(expand_vars(&format!("a%{name}%b")), format!("a{value}b"));
        assert_eq!(expand_vars("50% off %NO_SUCH_VAR_BRO%"), "50% off %NO_SUCH_VAR_BRO%");
    }

    #[test]
    fn finds_executables_in_order_and_extends_path() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, store) = (dir.path().join("bin"), dir.path().join("WindowsApps"));
        let exe = |n: &str| if cfg!(windows) { format!("{n}.exe") } else { n.to_string() };
        for d in [&bin, &store] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(exe("rg")), "").unwrap();
        }
        let dirs = [store.clone(), bin.clone()];
        assert_eq!(find("rg", &dirs), Some(store.join(exe("rg"))));
        assert_eq!(find_all("rg", &dirs).count(), 2);
        assert_eq!(find("python", &dirs), None);
        // an empty file isn't a Python that answers --version
        assert_eq!(python_version(&store.join(exe("rg"))), None);
        let p = path_with(bin.as_os_str(), &[bin.clone(), store.clone(), dir.path().join("missing")]).unwrap();
        assert_eq!(std::env::split_paths(&p).count(), 2);
        assert!(path_with(bin.as_os_str(), &[bin.clone()]).is_none());
    }
}
