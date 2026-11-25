//! Windows Firewall rules for the programs that take connections from your phone.
//!
//! Windows asks "Allow access?" on the PC the first time a program listens on the
//! network without a rule for the current network type (and again when the PC joins
//! a network of another type). Someone connecting from a phone can't answer it, so
//! the connection just fails. bro adds its own inbound rules while you're at the
//! computer to approve one UAC prompt: the bridge (`bro.exe`) and Bro RMM
//! (`voxl.exe`, whose WebRTC media arrives over UDP), for every network type.
//!
//! Checking needs no rights; changing the rules runs `bro firewall allow …` elevated.
//! Rules that block these programs (Windows adds them when that prompt is cancelled,
//! and for the network types left unticked) are switched off, since a block rule
//! wins over any allow rule.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The rule group bro's own rules carry (Windows Defender Firewall's "Group" column).
pub const GROUP: &str = "Bro";

pub const PROFILE_DOMAIN: i32 = 1;
pub const PROFILE_PRIVATE: i32 = 2;
pub const PROFILE_PUBLIC: i32 = 4;
pub const PROFILES: [i32; 3] = [PROFILE_DOMAIN, PROFILE_PRIVATE, PROFILE_PUBLIC];
const ALL_PROFILES: i32 = PROFILE_DOMAIN | PROFILE_PRIVATE | PROFILE_PUBLIC;
pub const TCP: i32 = 6;
pub const UDP: i32 = 17;
pub const ANY_PROTOCOL: i32 = 256;

/// A program bro needs reachable from other devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum App {
    /// The bridge: the phone app and web client (TCP; voice and future media may use UDP).
    Bridge,
    /// Bro RMM: remote desktop video and input over WebRTC (UDP).
    RemoteDesktop,
}

impl App {
    pub fn rule_name(self) -> &'static str {
        match self {
            App::Bridge => "Bro phone access",
            App::RemoteDesktop => "Bro remote desktop",
        }
    }

    /// For sentences: "phone access", "remote desktop".
    pub fn label(self) -> &'static str {
        match self {
            App::Bridge => "phone access",
            App::RemoteDesktop => "remote desktop",
        }
    }

    /// The Windows rule's description (macOS and Linux have nowhere to put one).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn description(self) -> &'static str {
        match self {
            App::Bridge => "Lets your paired phone and bro's web client reach bro on this PC. Added by bro so Windows doesn't hold a phone's first connection for an Allow prompt.",
            App::RemoteDesktop => "Lets your paired phone view and control this PC through Bro RMM. Added by bro when remote desktop was turned on.",
        }
    }

    pub fn file_name(self) -> &'static str {
        match (self, cfg!(windows)) {
            (App::Bridge, true) => "bro.exe",
            (App::RemoteDesktop, true) => "voxl.exe",
            (App::Bridge, false) => "bro",
            (App::RemoteDesktop, false) => "voxl",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            App::Bridge => "--bridge",
            App::RemoteDesktop => "--remote-desktop",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub app: App,
    pub program: PathBuf,
}

impl Target {
    pub fn new(app: App, program: impl Into<PathBuf>) -> Self {
        Self { app, program: program.into() }
    }
}

/// Bro RMM's executable under bro's data folder (`~/.bro`). Updates replace it in place, so its rule lasts.
pub fn remote_desktop_program(bro_dir: &Path) -> PathBuf {
    crate::rmm::package_dir(&bro_dir.join("rmm")).join(App::RemoteDesktop.file_name())
}

/// Whether a bridge bound here can be reached from another device (loopback can't, and needs no rule).
pub fn reachable_bind(bind: &str) -> bool {
    let bind = bind.trim();
    if bind.eq_ignore_ascii_case("localhost") {
        return false;
    }
    bind.parse::<std::net::IpAddr>().map_or(true, |ip| !ip.is_loopback())
}

/// What must be reachable: the bridge's program when it listens beyond this PC, and Bro RMM when remote desktop is on.
pub fn targets(bridge: Option<&Path>, remote_desktop: Option<&Path>) -> Vec<Target> {
    bridge
        .map(|p| Target::new(App::Bridge, p))
        .into_iter()
        .chain(remote_desktop.map(|p| Target::new(App::RemoteDesktop, p)))
        .collect()
}

/// One firewall rule, as much of it as matters here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleInfo {
    pub name: String,
    pub program: String,
    pub inbound: bool,
    pub allow: bool,
    pub enabled: bool,
    pub profiles: i32,
    pub protocol: i32,
    pub local_ports: String,
    pub remote_addresses: String,
    pub interface_types: String,
    pub grouping: String,
}

impl Default for RuleInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            program: String::new(),
            inbound: true,
            allow: true,
            enabled: true,
            profiles: ALL_PROFILES,
            protocol: ANY_PROTOCOL,
            local_ports: "*".into(),
            remote_addresses: "*".into(),
            interface_types: "All".into(),
            grouping: String::new(),
        }
    }
}

/// The firewall's own switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Network types with the firewall on.
    pub enabled_profiles: i32,
    /// Network types set to "Block all incoming connections, including those in the list of allowed apps".
    pub block_all_profiles: i32,
    /// Network types this PC is on now.
    pub current_profiles: i32,
    /// Group policy overrides local rules here, so rules bro adds don't take effect.
    pub group_policy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppReport {
    pub target: Target,
    /// Network types where it isn't allowed in for TCP and UDP.
    pub missing_profiles: i32,
    /// Enabled rules that block it.
    pub blocking: Vec<String>,
}

impl AppReport {
    pub fn open(&self) -> bool {
        self.missing_profiles == 0 && self.blocking.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Phones get through without a prompt on every network type.
    Ready,
    /// Rules need adding or repairing: one UAC prompt.
    NeedsRules,
    /// "Block all incoming connections" is on for these network types; no rule can let a phone in there.
    BlocksAll(i32),
    /// Group policy runs the firewall on this PC; an administrator has to allow bro.
    Managed,
    /// Nothing to do: the firewall is off, or nothing needs to be reachable.
    NotNeeded,
    /// A firewall bro can't change by itself is on (Linux's ufw or firewalld): what to run if a phone can't connect.
    Manual(String),
    /// The firewall couldn't be read.
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub readiness: Readiness,
    pub apps: Vec<AppReport>,
    pub policy: Policy,
}

impl Report {
    pub fn ready(&self) -> bool {
        matches!(self.readiness, Readiness::Ready | Readiness::NotNeeded)
    }

    /// The programs still waiting on a rule.
    pub fn closed(&self) -> Vec<App> {
        self.apps.iter().filter(|a| !a.open()).map(|a| a.target.app).collect()
    }
}

/// "Private and public networks", for messages.
pub fn profile_names(profiles: i32) -> String {
    let names: Vec<&str> = [(PROFILE_DOMAIN, "domain"), (PROFILE_PRIVATE, "private"), (PROFILE_PUBLIC, "public")]
        .into_iter()
        .filter(|(bit, _)| profiles & bit != 0)
        .map(|(_, name)| name)
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => format!("{one} networks"),
        [a, b] => format!("{a} and {b} networks"),
        [a, b, c] => format!("{a}, {b} and {c} networks"),
        _ => unreachable!(),
    }
}

/// Programs compare the way Windows compares them: case-insensitively, `%VARS%` expanded, either slash.
pub fn same_program(rule: &str, program: &Path) -> bool {
    let rule = normalize(&expand_vars(rule));
    !rule.is_empty() && rule == normalize(&program.to_string_lossy())
}

fn normalize(path: &str) -> String {
    let path = path.trim().trim_matches('"').replace('/', "\\");
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    path.to_lowercase()
}

fn expand_vars(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

fn wildcard(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value == "*"
}

/// An enabled inbound allow rule that lets `program` in with `protocol` on network type `profile`, any port, from anywhere.
fn lets_in(rule: &RuleInfo, program: &Path, profile: i32, protocol: i32) -> bool {
    rule.enabled
        && rule.inbound
        && rule.allow
        && rule.profiles & profile != 0
        && (rule.protocol == ANY_PROTOCOL || rule.protocol == protocol)
        && wildcard(&rule.local_ports)
        && wildcard(&rule.remote_addresses)
        && (rule.interface_types.trim().is_empty() || rule.interface_types.eq_ignore_ascii_case("all"))
        && same_program(&rule.program, program)
}

fn blocks(rule: &RuleInfo, program: &Path, profiles: i32) -> bool {
    rule.enabled && rule.inbound && !rule.allow && rule.profiles & profiles != 0 && same_program(&rule.program, program)
}

/// Where each program stands, from the rules and switches as read.
pub fn evaluate(rules: &[RuleInfo], policy: Policy, targets: &[Target]) -> Report {
    // Rules are written for every network type, so a network the PC joins later works too.
    let watched = if policy.enabled_profiles == 0 { 0 } else { ALL_PROFILES };
    let apps: Vec<AppReport> = targets
        .iter()
        .map(|target| {
            let missing_profiles = PROFILES
                .into_iter()
                .filter(|&profile| {
                    watched & profile != 0
                        && ![TCP, UDP].into_iter().all(|protocol| rules.iter().any(|r| lets_in(r, &target.program, profile, protocol)))
                })
                .fold(0, |bits, profile| bits | profile);
            let blocking = if watched == 0 {
                vec![]
            } else {
                rules.iter().filter(|r| blocks(r, &target.program, ALL_PROFILES)).map(|r| r.name.clone()).collect()
            };
            AppReport { target: target.clone(), missing_profiles, blocking }
        })
        .collect();
    let readiness = if targets.is_empty() || policy.enabled_profiles == 0 {
        Readiness::NotNeeded
    } else if policy.group_policy {
        Readiness::Managed
    } else if apps.iter().any(|a| !a.open()) {
        Readiness::NeedsRules
    } else if policy.block_all_profiles & policy.enabled_profiles != 0 {
        Readiness::BlocksAll(policy.block_all_profiles & policy.enabled_profiles)
    } else {
        Readiness::Ready
    };
    Report { readiness, apps, policy }
}

/// What `allow` does to the rules it read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Indexes of rules to switch off: they block one of the programs.
    pub disable: Vec<usize>,
    /// Indexes of bro's own rules to set right again (switched off or edited since), with their program.
    pub repair: Vec<(usize, App)>,
    /// Programs that get a new rule.
    pub add: Vec<Target>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.disable.is_empty() && self.repair.is_empty() && self.add.is_empty()
    }
}

/// The smallest change that opens every target: other software's allow rules are kept, bro's own are reused.
pub fn plan(rules: &[RuleInfo], targets: &[Target]) -> Plan {
    let mut plan = Plan::default();
    for target in targets {
        for (index, rule) in rules.iter().enumerate() {
            if blocks(rule, &target.program, ALL_PROFILES) && !plan.disable.contains(&index) {
                plan.disable.push(index);
            }
        }
        let covered = PROFILES.into_iter().all(|profile| {
            [TCP, UDP].into_iter().all(|protocol| rules.iter().any(|r| lets_in(r, &target.program, profile, protocol)))
        });
        if covered {
            continue;
        }
        let own = rules.iter().position(|r| {
            r.grouping == GROUP && r.inbound && r.allow && same_program(&r.program, &target.program)
        });
        match own {
            Some(index) => plan.repair.push((index, target.app)),
            None => plan.add.push(target.clone()),
        }
    }
    plan
}

/// The rule bro writes for a program.
pub fn rule_for(target: &Target) -> RuleInfo {
    RuleInfo {
        name: target.app.rule_name().into(),
        program: target.program.to_string_lossy().into_owned(),
        grouping: GROUP.into(),
        ..RuleInfo::default()
    }
}

/// `bro firewall allow` arguments for these programs.
pub fn allow_args(targets: &[Target], result: Option<&Path>) -> Vec<String> {
    let mut args = vec!["firewall".to_string(), "allow".to_string()];
    for target in targets {
        args.push(target.app.flag().into());
        args.push(target.program.to_string_lossy().into_owned());
    }
    if let Some(result) = result {
        args.push("--result".into());
        args.push(result.to_string_lossy().into_owned());
    }
    args
}

/// Reads `allow`'s arguments (after `firewall allow`). It runs elevated, so it only accepts bro's own two programs.
pub fn parse_allow_args(args: &[String]) -> anyhow::Result<(Vec<Target>, Option<PathBuf>)> {
    let mut targets = vec![];
    let mut result = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a path"))?;
        let path = PathBuf::from(value);
        match flag.as_str() {
            "--result" => result = Some(path),
            "--bridge" | "--remote-desktop" => {
                let app = if flag == "--bridge" { App::Bridge } else { App::RemoteDesktop };
                anyhow::ensure!(path.is_absolute(), "{} must be a full path", app.file_name());
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                anyhow::ensure!(name.eq_ignore_ascii_case(app.file_name()), "{flag} takes the path of {}", app.file_name());
                if app == App::RemoteDesktop {
                    let folder = path.parent().map(|p| p.to_string_lossy().replace('/', "\\").to_lowercase()).unwrap_or_default();
                    anyhow::ensure!(folder.ends_with(r"packages\protocol-v1"), "voxl.exe must be the Bro RMM package's");
                }
                targets.push(Target { app, program: path });
            }
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    anyhow::ensure!(!targets.is_empty(), "name a program: --bridge <bro.exe> or --remote-desktop <voxl.exe>");
    Ok((targets, result))
}

/// What asking for the rules came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The rules are in place (`report` is read back afterwards).
    Done(Report),
    /// The UAC prompt was declined.
    Declined,
    Failed(String),
}

/// What happened in the elevated `allow`, written for the process that asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowResult {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub repaired: usize,
    #[serde(default)]
    pub unblocked: usize,
}

#[cfg(windows)]
pub use imp::{allow, check, elevated, request};

#[cfg(target_os = "macos")]
pub use mac::{allow, check, elevated, request};
#[cfg(not(any(windows, target_os = "macos")))]
pub use other::{allow, check, elevated, request};

/// The prompt a phone's first connection would wait on, said for this computer.
pub fn prompt_risk() -> &'static str {
    if cfg!(target_os = "macos") {
        "The macOS firewall would ask “accept incoming network connections?” on this Mac before your phone gets in, and you may not be here to answer it."
    } else {
        "Windows would hold your phone's first connection for an Allow prompt on this PC, and you may not be here to answer it."
    }
}

/// What Allow asks of you here.
pub fn asking() -> &'static str {
    if cfg!(target_os = "macos") { "enter your Mac's password once" } else { "approve the one Windows prompt" }
}

/// How to turn off "block all incoming connections" here.
pub fn blocks_all_fix(profiles: i32) -> String {
    if cfg!(target_os = "macos") {
        "bro's entries are in, but the macOS firewall blocks all incoming connections. Turn that off in System Settings → Network → Firewall → Options.".into()
    } else {
        format!(
            "bro's rules are in, but Windows blocks every incoming connection on {}. Turn that off in Windows Security → Firewall & network protection.",
            profile_names(profiles)
        )
    }
}

/// What's missing for one program, for a line under the firewall's status.
pub fn missing_line(app: &AppReport) -> String {
    let mut why = vec![];
    if app.missing_profiles != 0 {
        why.push(if cfg!(windows) { format!("not allowed on {}", profile_names(app.missing_profiles)) } else { "not allowed yet".to_string() });
    }
    if !app.blocking.is_empty() {
        let n = app.blocking.len();
        why.push(if cfg!(windows) {
            format!("blocked by {n} rule{} Windows added when its prompt was dismissed; Allow switches {} off", if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "them" })
        } else {
            "blocked in the firewall's settings; Allow lets it in".to_string()
        });
    }
    why.join("; ")
}

/// What people call this computer's firewall.
pub fn name() -> &'static str {
    if cfg!(windows) {
        "Windows Firewall"
    } else if cfg!(target_os = "macos") {
        "macOS firewall"
    } else {
        "Firewall"
    }
}

/// macOS's Application Firewall, read and set with `socketfilterfw`. Off by default; when it's on, a program
/// without an entry gets "Do you want the application to accept incoming network connections?", the same
/// stuck-at-the-computer prompt as Windows'. Adding entries takes an administrator, asked for with macOS's own
/// password dialog.
#[cfg(any(target_os = "macos", test))]
mod socketfilter {
    /// The firewall is on, from `--getglobalstate` ("Firewall is enabled. (State = 1)").
    pub fn on(out: &str) -> bool {
        let out = out.to_ascii_lowercase();
        if let Some(n) = out.split("state = ").nth(1).and_then(|rest| rest.chars().next()).and_then(|c| c.to_digit(10)) {
            return n > 0;
        }
        out.contains("enabled") && !out.contains("disabled")
    }

    /// "Block all incoming connections" is on, from `--getblockall`.
    pub fn blocks_all(out: &str) -> bool {
        let out = out.to_ascii_lowercase();
        out.contains("enabled") && !out.contains("disabled")
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Entry {
        Permitted,
        Blocked,
        Missing,
    }

    /// One program's entry, from `--getappblocked <path>`.
    pub fn entry(out: &str) -> Entry {
        let out = out.to_ascii_lowercase();
        if out.contains("not part of") || out.contains("not in") || out.contains("no such") || out.trim().is_empty() {
            Entry::Missing
        } else if out.contains("blocked") {
            Entry::Blocked
        } else if out.contains("permitted") || out.contains("allowed") {
            Entry::Permitted
        } else {
            Entry::Missing
        }
    }

    /// A shell word: single-quoted, any single quote spelled out.
    pub fn sh_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', r"'\''"))
    }

    /// An AppleScript string literal.
    pub fn applescript_string(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// The admin command that adds and unblocks each program.
    pub fn allow_script(tool: &str, programs: &[String]) -> String {
        let commands: Vec<String> = programs
            .iter()
            .map(|p| format!("{t} --add {p} >/dev/null; {t} --unblockapp {p} >/dev/null", t = sh_quote(tool), p = sh_quote(p)))
            .collect();
        format!("do shell script {} with administrator privileges", applescript_string(&commands.join("; ")))
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::socketfilter::{self, Entry};
    use super::*;
    use std::process::{Command, Stdio};

    const TOOL: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

    fn run(args: &[&str]) -> Option<String> {
        let out = Command::new(TOOL).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn check(targets: &[Target]) -> Report {
        let Some(state) = run(&["--getglobalstate"]) else {
            return Report { readiness: Readiness::Unknown("socketfilterfw didn't answer".into()), apps: vec![], policy: Policy::default() };
        };
        let on = socketfilter::on(&state);
        let block_all = on && run(&["--getblockall"]).is_some_and(|out| socketfilter::blocks_all(&out));
        let policy = Policy {
            enabled_profiles: if on { ALL_PROFILES } else { 0 },
            block_all_profiles: if block_all { ALL_PROFILES } else { 0 },
            current_profiles: ALL_PROFILES,
            group_policy: false,
        };
        let apps: Vec<AppReport> = targets
            .iter()
            .map(|target| {
                let entry = if on {
                    run(&["--getappblocked", &target.program.to_string_lossy()]).map_or(Entry::Missing, |out| socketfilter::entry(&out))
                } else {
                    Entry::Permitted
                };
                AppReport {
                    target: target.clone(),
                    missing_profiles: if entry == Entry::Missing { ALL_PROFILES } else { 0 },
                    blocking: if entry == Entry::Blocked { vec![name().into()] } else { vec![] },
                }
            })
            .collect();
        let readiness = if targets.is_empty() || !on {
            Readiness::NotNeeded
        } else if apps.iter().any(|a| !a.open()) {
            Readiness::NeedsRules
        } else if block_all {
            Readiness::BlocksAll(ALL_PROFILES)
        } else {
            Readiness::Ready
        };
        Report { readiness, apps, policy }
    }

    pub fn elevated() -> bool {
        // SAFETY: no arguments, no side effects
        unsafe { libc::geteuid() == 0 }
    }

    /// Run as root (`sudo bro firewall allow …`): the entries directly.
    pub fn allow(targets: &[Target]) -> anyhow::Result<AllowResult> {
        for target in targets {
            let path = target.program.to_string_lossy();
            for flag in ["--add", "--unblockapp"] {
                let status = Command::new(TOOL).args([flag, path.as_ref()]).stdout(Stdio::null()).stderr(Stdio::null()).status()?;
                anyhow::ensure!(status.success(), "socketfilterfw {flag} failed for {path}");
            }
        }
        Ok(AllowResult { ok: true, added: targets.len(), ..Default::default() })
    }

    /// macOS's administrator dialog, then the entries (one dialog for every program).
    pub fn request(_exe: &Path, targets: &[Target]) -> Outcome {
        let before = check(targets);
        if before.readiness != Readiness::NeedsRules {
            return Outcome::Done(before);
        }
        if elevated() {
            return match allow(targets) {
                Ok(_) => Outcome::Done(check(targets)),
                Err(error) => Outcome::Failed(error.to_string()),
            };
        }
        let programs: Vec<String> = targets.iter().map(|t| t.program.to_string_lossy().into_owned()).collect();
        let script = socketfilter::allow_script(TOOL, &programs);
        match Command::new("/usr/bin/osascript").args(["-e", &script]).stdin(Stdio::null()).output() {
            Ok(out) if out.status.success() => Outcome::Done(check(targets)),
            // -128: the dialog was cancelled
            Ok(out) if String::from_utf8_lossy(&out.stderr).contains("-128") => Outcome::Declined,
            Ok(out) => Outcome::Failed(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(error) => Outcome::Failed(format!("couldn't ask macOS for permission: {error}")),
        }
    }
}

/// Linux (and anything else): desktop firewalls don't prompt, they just block, and changing them takes sudo. bro
/// says which one is on and what to run if a phone can't get through.
#[cfg(not(any(windows, target_os = "macos")))]
mod other {
    use super::*;

    fn ufw_on() -> bool {
        std::fs::read_to_string("/etc/ufw/ufw.conf")
            .is_ok_and(|conf| conf.lines().any(|l| l.trim().eq_ignore_ascii_case("ENABLED=yes")))
    }

    fn firewalld_on() -> bool {
        std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", "firewalld"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    pub fn check(targets: &[Target]) -> Report {
        let apps = targets.iter().map(|t| AppReport { target: t.clone(), missing_profiles: 0, blocking: vec![] }).collect();
        let readiness = if targets.is_empty() {
            Readiness::NotNeeded
        } else if ufw_on() {
            Readiness::Manual("ufw is on and doesn't ask before blocking. If your phone can't connect, allow the Bridge page's port, for example: sudo ufw allow 10001/tcp. Remote desktop video then goes through the relay or TCP.".into())
        } else if firewalld_on() {
            Readiness::Manual("firewalld is on and doesn't ask before blocking. If your phone can't connect, allow the Bridge page's port, for example: sudo firewall-cmd --add-port=10001/tcp --permanent && sudo firewall-cmd --reload".into())
        } else {
            Readiness::NotNeeded
        };
        Report { readiness, apps, policy: Policy::default() }
    }

    pub fn elevated() -> bool {
        false
    }

    pub fn allow(_targets: &[Target]) -> anyhow::Result<AllowResult> {
        Ok(AllowResult { ok: true, ..Default::default() })
    }

    pub fn request(_exe: &Path, targets: &[Target]) -> Outcome {
        Outcome::Done(check(targets))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::Duration;
    use windows::core::{BSTR, Interface, PCWSTR, HSTRING};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, VARIANT_FALSE, VARIANT_TRUE, WAIT_OBJECT_0, ERROR_CANCELLED};
    use windows::Win32::NetworkManagement::WindowsFirewall::{
        INetFwPolicy2, INetFwRule, INetFwRules, NetFwPolicy2, NetFwRule, NET_FW_ACTION_ALLOW,
        NET_FW_MODIFY_STATE_GP_OVERRIDE, NET_FW_PROFILE2_ALL, NET_FW_PROFILE_TYPE2, NET_FW_RULE_DIR_IN,
    };
    use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_DISPATCH};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Ole::IEnumVARIANT;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SW_HIDE};

    /// COM on a thread of its own, so the caller's apartment doesn't matter.
    fn with_com<T: Send + 'static>(f: impl FnOnce() -> windows::core::Result<T> + Send + 'static) -> anyhow::Result<T> {
        std::thread::spawn(move || unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            let result = f();
            if init.is_ok() {
                CoUninitialize();
            }
            result
        })
        .join()
        .map_err(|_| anyhow::anyhow!("the firewall check crashed"))?
        .map_err(|e| anyhow::anyhow!("Windows Firewall: {}", e.message()))
    }

    fn policy() -> windows::core::Result<INetFwPolicy2> {
        unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
    }

    fn read_policy(policy: &INetFwPolicy2) -> windows::core::Result<Policy> {
        let mut read = Policy::default();
        unsafe {
            read.current_profiles = policy.CurrentProfileTypes()?;
            for profile in PROFILES {
                let kind = NET_FW_PROFILE_TYPE2(profile);
                if policy.get_FirewallEnabled(kind)?.as_bool() {
                    read.enabled_profiles |= profile;
                }
                if policy.get_BlockAllInboundTraffic(kind)?.as_bool() {
                    read.block_all_profiles |= profile;
                }
            }
            read.group_policy = policy.LocalPolicyModifyState()? == NET_FW_MODIFY_STATE_GP_OVERRIDE;
        }
        Ok(read)
    }

    fn info(rule: &INetFwRule) -> windows::core::Result<RuleInfo> {
        unsafe {
            Ok(RuleInfo {
                name: rule.Name()?.to_string(),
                program: rule.ApplicationName().map(|s| s.to_string()).unwrap_or_default(),
                inbound: rule.Direction()? == NET_FW_RULE_DIR_IN,
                allow: rule.Action()? == NET_FW_ACTION_ALLOW,
                enabled: rule.Enabled()?.as_bool(),
                profiles: rule.Profiles()?,
                protocol: rule.Protocol()?,
                local_ports: rule.LocalPorts().map(|s| s.to_string()).unwrap_or_default(),
                remote_addresses: rule.RemoteAddresses().map(|s| s.to_string()).unwrap_or_default(),
                interface_types: rule.InterfaceTypes().map(|s| s.to_string()).unwrap_or_default(),
                grouping: rule.Grouping().map(|s| s.to_string()).unwrap_or_default(),
            })
        }
    }

    /// Every rule with an application, live (changing one changes the firewall).
    fn rules(rules: &INetFwRules) -> windows::core::Result<Vec<(INetFwRule, RuleInfo)>> {
        let mut out = vec![];
        unsafe {
            let items: IEnumVARIANT = rules._NewEnum()?.cast()?;
            loop {
                let mut slot = [VARIANT::default()];
                let mut fetched = 0u32;
                let hr = items.Next(&mut slot, &mut fetched);
                if hr.is_err() || fetched == 0 {
                    break;
                }
                let value = &slot[0].Anonymous.Anonymous;
                let dispatch = if value.vt == VT_DISPATCH { (*value.Anonymous.pdispVal).clone() } else { None };
                let _ = VariantClear(&mut slot[0]);
                let Some(rule) = dispatch.and_then(|d| d.cast::<INetFwRule>().ok()) else { continue };
                // Rules without a program can't be about bro's; skip them quickly.
                let has_program = rule.ApplicationName().map(|s| !s.is_empty()).unwrap_or(false);
                if !has_program {
                    continue;
                }
                if let Ok(read) = info(&rule) {
                    out.push((rule, read));
                }
            }
        }
        Ok(out)
    }

    pub fn check(targets: &[Target]) -> Report {
        let targets_owned = targets.to_vec();
        match with_com(move || {
            let policy = policy()?;
            let read = read_policy(&policy)?;
            let infos: Vec<RuleInfo> = rules(&unsafe { policy.Rules()? })?.into_iter().map(|(_, info)| info).collect();
            Ok(evaluate(&infos, read, &targets_owned))
        }) {
            Ok(report) => report,
            Err(error) => Report {
                readiness: Readiness::Unknown(error.to_string()),
                apps: targets.iter().map(|t| AppReport { target: t.clone(), missing_profiles: 0, blocking: vec![] }).collect(),
                policy: Policy::default(),
            },
        }
    }

    fn write_rule(rule: &INetFwRule, target: &Target) -> windows::core::Result<()> {
        unsafe {
            rule.SetName(&BSTR::from(target.app.rule_name()))?;
            rule.SetDescription(&BSTR::from(target.app.description()))?;
            rule.SetApplicationName(&BSTR::from(target.program.to_string_lossy().as_ref()))?;
            rule.SetGrouping(&BSTR::from(GROUP))?;
            rule.SetDirection(NET_FW_RULE_DIR_IN)?;
            rule.SetAction(NET_FW_ACTION_ALLOW)?;
            // Ports only clear with a protocol that has them; widen in that order.
            rule.SetProtocol(ANY_PROTOCOL)?;
            rule.SetRemoteAddresses(&BSTR::from("*"))?;
            rule.SetInterfaceTypes(&BSTR::from("All"))?;
            rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
            rule.SetEnabled(VARIANT_TRUE)?;
        }
        Ok(())
    }

    /// Adds and repairs the rules (needs administrator rights). Afterwards, read back with [`check`].
    pub fn allow(targets: &[Target]) -> anyhow::Result<AllowResult> {
        let targets = targets.to_vec();
        with_com(move || {
            let policy = policy()?;
            let collection = unsafe { policy.Rules()? };
            let live = rules(&collection)?;
            let infos: Vec<RuleInfo> = live.iter().map(|(_, info)| info.clone()).collect();
            let todo = plan(&infos, &targets);
            for &index in &todo.disable {
                unsafe { live[index].0.SetEnabled(VARIANT_FALSE)? };
            }
            for &(index, app) in &todo.repair {
                let target = targets.iter().find(|t| t.app == app).expect("repair names a target");
                write_rule(&live[index].0, target)?;
            }
            for target in &todo.add {
                let rule: INetFwRule = unsafe { CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)? };
                write_rule(&rule, target)?;
                unsafe { collection.Add(&rule)? };
            }
            Ok(AllowResult { ok: true, error: None, added: todo.add.len(), repaired: todo.repair.len(), unblocked: todo.disable.len() })
        })
    }

    /// Whether this process already has administrator rights.
    pub fn elevated() -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut size = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut size,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && elevation.TokenIsElevated != 0
        }
    }

    /// The foreground window when it's one of this process's, else none.
    fn own_foreground_window() -> HWND {
        unsafe {
            let window = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if !window.is_invalid() && pid == std::process::id() { window } else { HWND::default() }
        }
    }

    /// Windows' own quoting for one argument.
    fn quote(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            return arg.to_string();
        }
        let mut out = String::from("\"");
        let mut slashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                    out.push('"');
                    slashes = 0;
                    continue;
                }
                _ => {}
            }
            if c != '\\' {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                out.push(c);
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }

    /// Adds the rules, asking Windows for administrator rights (one UAC prompt) unless bro already has them.
    /// Blocks until the prompt is answered and the rules are written; run it off the UI thread.
    pub fn request(exe: &Path, targets: &[Target]) -> Outcome {
        // Only missing rules are worth a prompt: with everything allowed, or a firewall that's off, managed or
        // blocking everything, rights wouldn't change the answer.
        let before = check(targets);
        if before.readiness != Readiness::NeedsRules {
            return Outcome::Done(before);
        }
        if elevated() {
            return match allow(targets) {
                Ok(_) => Outcome::Done(check(targets)),
                Err(error) => Outcome::Failed(error.to_string()),
            };
        }
        let result = std::env::temp_dir().join(format!("bro-firewall-{:032x}.json", rand::random::<u128>()));
        let params = allow_args(targets, Some(&result)).iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
        let exe = exe.to_path_buf();
        // The shell wants a single-threaded apartment for ShellExecuteEx; give it a thread of its own.
        let launched = std::thread::spawn(move || -> Result<u32, windows::core::Error> {
            unsafe {
                let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
                let file = HSTRING::from(exe.as_os_str());
                let verb = HSTRING::from("runas");
                let params = HSTRING::from(params);
                let mut info = SHELLEXECUTEINFOW {
                    cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                    fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
                    // bro's own window in front: the consent dialog opens over it rather than blinking in the taskbar
                    hwnd: own_foreground_window(),
                    lpVerb: PCWSTR(verb.as_ptr()),
                    lpFile: PCWSTR(file.as_ptr()),
                    lpParameters: PCWSTR(params.as_ptr()),
                    nShow: SW_HIDE.0,
                    ..Default::default()
                };
                let shown = ShellExecuteExW(&mut info);
                let mut code = 1u32;
                if shown.is_ok() && !info.hProcess.is_invalid() {
                    if WaitForSingleObject(info.hProcess, Duration::from_secs(120).as_millis() as u32) == WAIT_OBJECT_0 {
                        let _ = GetExitCodeProcess(info.hProcess, &mut code);
                    }
                    let _ = CloseHandle(info.hProcess);
                }
                if init.is_ok() {
                    CoUninitialize();
                }
                shown.map(|_| code)
            }
        })
        .join();
        let code = match launched {
            Ok(Ok(code)) => code,
            Ok(Err(error)) => {
                let _ = std::fs::remove_file(&result);
                return if error.code() == ERROR_CANCELLED.to_hresult() {
                    Outcome::Declined
                } else {
                    Outcome::Failed(format!("Windows couldn't start the firewall helper: {}", error.message()))
                };
            }
            Err(_) => return Outcome::Failed("the firewall helper crashed".into()),
        };
        let written = std::fs::read_to_string(&result).ok().and_then(|text| serde_json::from_str::<AllowResult>(&text).ok());
        let _ = std::fs::remove_file(&result);
        match written {
            Some(AllowResult { ok: true, .. }) => Outcome::Done(check(targets)),
            Some(AllowResult { error: Some(error), .. }) => Outcome::Failed(error),
            _ if code == 0 => Outcome::Done(check(targets)),
            _ => Outcome::Failed(format!("the firewall helper stopped with code {code}")),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Reads (never changes) this PC's firewall: `cargo test -p bro-core firewall -- --ignored --nocapture`.
        #[test]
        #[ignore]
        fn reads_this_pcs_firewall() {
            let home = std::env::var("USERPROFILE").unwrap();
            let local = std::env::var("LOCALAPPDATA").unwrap();
            let targets = [
                Target::new(App::Bridge, format!(r"{local}\Programs\bro\bro.exe")),
                Target::new(App::RemoteDesktop, format!(r"{home}\.bro\rmm\packages\protocol-v1\voxl.exe")),
            ];
            let started = std::time::Instant::now();
            let report = check(&targets);
            println!("{:?} in {:?}\npolicy {:?}", report.readiness, started.elapsed(), report.policy);
            for app in &report.apps {
                println!("{:?}: missing {} blocking {:?}", app.target.app, profile_names(app.missing_profiles), app.blocking);
            }
            assert!(!matches!(report.readiness, Readiness::Unknown(_)), "{:?}", report.readiness);
            println!("elevated: {}", elevated());
        }

        /// Without rights, writing fails cleanly and changes nothing. Every property is still set on a fresh rule,
        /// so a bad value would show up here. Run unelevated: `… firewall -- --ignored --nocapture`.
        #[test]
        #[ignore]
        fn allow_without_rights_fails_cleanly_and_changes_nothing() {
            assert!(!elevated(), "run this one without administrator rights");
            let program = std::env::temp_dir().join("bro-firewall-probe").join("bro.exe");
            let targets = [Target::new(App::Bridge, &program)];
            let count = || with_com(|| unsafe { policy()?.Rules()?.Count() }).unwrap();
            let before = count();
            let error = allow(&targets).expect_err("an unelevated write must fail");
            println!("{error}");
            assert!(error.to_string().to_lowercase().contains("denied"), "{error}");
            assert_eq!(count(), before);
            assert_eq!(check(&targets).readiness, Readiness::NeedsRules);
            // A detached rule takes every value bro writes.
            with_com(move || {
                let rule: INetFwRule = unsafe { CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)? };
                write_rule(&rule, &Target::new(App::Bridge, &program))?;
                let read = info(&rule)?;
                assert_eq!(read.protocol, ANY_PROTOCOL);
                assert_eq!(read.profiles & ALL_PROFILES, ALL_PROFILES);
                assert!(read.enabled && read.allow && read.inbound);
                assert_eq!(read.grouping, GROUP);
                assert!(lets_in(&read, &program, PROFILE_PRIVATE, UDP), "{read:?}");
                Ok(())
            })
            .unwrap();
        }

        #[test]
        fn arguments_survive_windows_quoting() {
            assert_eq!(quote("allow"), "allow");
            assert_eq!(quote(r"C:\Users\a b\bro.exe"), r#""C:\Users\a b\bro.exe""#);
            assert_eq!(quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
            assert_eq!(quote(""), r#""""#);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bro() -> PathBuf {
        PathBuf::from(r"C:\Users\me\AppData\Local\Programs\bro\bro.exe")
    }
    fn voxl() -> PathBuf {
        PathBuf::from(r"C:\Users\me\.bro\rmm\packages\protocol-v1\voxl.exe")
    }
    fn on() -> Policy {
        Policy { enabled_profiles: ALL_PROFILES, current_profiles: PROFILE_PRIVATE, ..Policy::default() }
    }
    /// What Windows writes when you click Allow on its prompt: one rule per protocol for the ticked network types.
    fn prompt_rules(program: &Path, profiles: i32, allow: bool) -> Vec<RuleInfo> {
        [TCP, UDP]
            .into_iter()
            .map(|protocol| RuleInfo {
                name: "bro — agentic terminal workspace".into(),
                program: program.to_string_lossy().to_lowercase(),
                allow,
                profiles,
                protocol,
                ..RuleInfo::default()
            })
            .collect()
    }
    fn targets() -> Vec<Target> {
        vec![Target::new(App::Bridge, bro()), Target::new(App::RemoteDesktop, voxl())]
    }

    #[test]
    fn nothing_allowed_needs_a_rule_for_each_program_on_every_network_type() {
        let report = evaluate(&[], on(), &targets());
        assert_eq!(report.readiness, Readiness::NeedsRules);
        assert!(report.apps.iter().all(|a| a.missing_profiles == ALL_PROFILES));
        assert_eq!(plan(&[], &targets()).add, targets());
    }

    #[test]
    fn a_public_only_answer_to_the_prompt_still_prompts_on_a_private_network() {
        // This PC's installed bro.exe had exactly this (2026-11-03).
        let rules = prompt_rules(&bro(), PROFILE_PUBLIC, true);
        let report = evaluate(&rules, on(), &[Target::new(App::Bridge, bro())]);
        assert_eq!(report.apps[0].missing_profiles, PROFILE_DOMAIN | PROFILE_PRIVATE);
        assert_eq!(report.readiness, Readiness::NeedsRules);
        assert_eq!(plan(&rules, &[Target::new(App::Bridge, bro())]).add.len(), 1);
    }

    #[test]
    fn bros_rule_or_an_equivalent_one_is_ready() {
        let ours: Vec<RuleInfo> = targets().iter().map(rule_for).collect();
        assert_eq!(evaluate(&ours, on(), &targets()).readiness, Readiness::Ready);
        assert!(plan(&ours, &targets()).is_empty());
        // Allow-everywhere prompt rules (both protocols, all types) count as well.
        let mut prompt = prompt_rules(&bro(), ALL_PROFILES, true);
        prompt.extend(prompt_rules(&voxl(), ALL_PROFILES, true));
        assert_eq!(evaluate(&prompt, on(), &targets()).readiness, Readiness::Ready);
    }

    #[test]
    fn a_block_rule_wins_and_is_switched_off() {
        // Cancelling the prompt, or leaving public unticked, writes block rules.
        let mut rules: Vec<RuleInfo> = targets().iter().map(rule_for).collect();
        rules.extend(prompt_rules(&voxl(), PROFILE_PUBLIC, false));
        let report = evaluate(&rules, on(), &targets());
        assert_eq!(report.readiness, Readiness::NeedsRules);
        assert_eq!(report.closed(), vec![App::RemoteDesktop]);
        assert_eq!(report.apps[1].blocking.len(), 2);
        let todo = plan(&rules, &targets());
        assert_eq!(todo.disable, vec![2, 3]);
        assert!(todo.add.is_empty() && todo.repair.is_empty());
    }

    #[test]
    fn rules_that_only_partly_open_a_program_dont_count() {
        let program = bro();
        let partial = [
            RuleInfo { local_ports: "10001".into(), ..rule_for(&Target::new(App::Bridge, &program)) },
            RuleInfo { remote_addresses: "LocalSubnet".into(), ..rule_for(&Target::new(App::Bridge, &program)) },
            RuleInfo { protocol: TCP, ..rule_for(&Target::new(App::Bridge, &program)) },
            RuleInfo { inbound: false, ..rule_for(&Target::new(App::Bridge, &program)) },
            RuleInfo { program: r"C:\other\bro.exe".into(), ..rule_for(&Target::new(App::Bridge, &program)) },
        ];
        for rule in partial {
            let report = evaluate(std::slice::from_ref(&rule), on(), &[Target::new(App::Bridge, &program)]);
            assert_eq!(report.readiness, Readiness::NeedsRules, "{rule:?}");
        }
    }

    #[test]
    fn a_switched_off_bro_rule_is_repaired_rather_than_duplicated() {
        let rules = vec![RuleInfo { enabled: false, ..rule_for(&Target::new(App::Bridge, bro())) }];
        let todo = plan(&rules, &[Target::new(App::Bridge, bro())]);
        assert_eq!(todo.repair, vec![(0, App::Bridge)]);
        assert!(todo.add.is_empty());
    }

    #[test]
    fn programs_match_like_windows_matches_them() {
        assert!(same_program(r"c:\users\me\appdata\local\programs\bro\bro.exe", &bro()));
        assert!(same_program(r"C:/Users/me/AppData/Local/Programs/bro/bro.exe", &bro()));
        assert!(same_program(r#""C:\Users\me\AppData\Local\Programs\bro\bro.exe""#, &bro()));
        assert!(same_program(r"\\?\C:\Users\me\AppData\Local\Programs\bro\bro.exe", &bro()));
        assert!(!same_program("", &bro()));
        assert!(!same_program(r"C:\Users\me\AppData\Local\Programs\bro\bro-app.exe", &bro()));
        // SAFETY of the test: a variable only this test reads.
        unsafe { std::env::set_var("BRO_FW_TEST_HOME", r"C:\Users\me") };
        assert!(same_program(r"%BRO_FW_TEST_HOME%\AppData\Local\Programs\bro\bro.exe", &bro()));
        assert!(!same_program(r"%BRO_FW_UNSET_VAR%\bro.exe", &bro()));
    }

    #[test]
    fn the_firewall_being_off_group_policy_and_block_all_are_told_apart() {
        let off = Policy { enabled_profiles: 0, ..on() };
        assert_eq!(evaluate(&[], off, &targets()).readiness, Readiness::NotNeeded);
        let managed = Policy { group_policy: true, ..on() };
        assert_eq!(evaluate(&[], managed, &targets()).readiness, Readiness::Managed);
        let ours: Vec<RuleInfo> = targets().iter().map(rule_for).collect();
        let shields_up = Policy { block_all_profiles: PROFILE_PUBLIC, ..on() };
        assert_eq!(evaluate(&ours, shields_up, &targets()).readiness, Readiness::BlocksAll(PROFILE_PUBLIC));
        assert_eq!(evaluate(&ours, on(), &[]).readiness, Readiness::NotNeeded);
    }

    #[test]
    #[cfg(windows)]
    fn the_elevated_helper_takes_only_bros_programs() {
        let args = allow_args(&targets(), Some(Path::new(r"C:\Temp\result.json")));
        assert_eq!(&args[..2], ["firewall", "allow"]);
        let (parsed, result) = parse_allow_args(&args[2..]).unwrap();
        assert_eq!(parsed, targets());
        assert_eq!(result.as_deref(), Some(Path::new(r"C:\Temp\result.json")));
        for bad in [
            vec!["--bridge".to_string(), r"C:\Windows\System32\cmd.exe".into()],
            vec!["--bridge".to_string(), "bro.exe".into()],
            vec!["--remote-desktop".to_string(), r"C:\Temp\voxl.exe".into()],
            vec!["--bridge".to_string()],
            vec!["--program".to_string(), r"C:\bro.exe".into()],
            vec![],
        ] {
            assert!(parse_allow_args(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_a_bridge_reachable_from_other_devices_needs_a_rule() {
        for bind in ["0.0.0.0", "::", "", "192.168.1.20", "100.101.12.7"] {
            assert!(reachable_bind(bind), "{bind}");
        }
        for bind in ["127.0.0.1", "::1", "localhost", " LOCALHOST "] {
            assert!(!reachable_bind(bind), "{bind}");
        }
        let program = remote_desktop_program(Path::new(r"C:\Users\me\.bro"));
        assert!(program.ends_with(App::RemoteDesktop.file_name()));
        if cfg!(windows) {
            assert_eq!(program, voxl());
            assert!(parse_allow_args(&["--remote-desktop".into(), program.to_string_lossy().into_owned()]).is_ok());
        }
    }

    #[test]
    fn macos_firewall_answers_read_right() {
        use socketfilter::{Entry, allow_script, applescript_string, blocks_all, entry, on, sh_quote};
        assert!(on("Firewall is enabled. (State = 1)"));
        assert!(on("Firewall is enabled. (State = 2)"));
        assert!(!on("Firewall is disabled. (State = 0)"));
        assert!(blocks_all("Firewall has block all state set to enabled."));
        assert!(!blocks_all("Firewall has block all state set to disabled."));
        assert_eq!(entry("The application /Applications/bro.app/Contents/MacOS/bro is permitted"), Entry::Permitted);
        assert_eq!(entry("Incoming connection to the application is blocked"), Entry::Blocked);
        assert_eq!(entry("The application is not part of the firewall"), Entry::Missing);
        assert_eq!(entry(""), Entry::Missing);
        // a path with a space and a quote survives the shell and AppleScript both
        assert_eq!(sh_quote("/Users/me/it's here/voxl"), r"'/Users/me/it'\''s here/voxl'");
        assert_eq!(applescript_string(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
        let script = allow_script("/usr/libexec/ApplicationFirewall/socketfilterfw", &["/Users/me/.bro/rmm/packages/protocol-v1/voxl".into()]);
        assert!(script.starts_with("do shell script \"") && script.ends_with("\" with administrator privileges"), "{script}");
        assert!(script.contains("--add '/Users/me/.bro/rmm/packages/protocol-v1/voxl'") && script.contains("--unblockapp"), "{script}");
    }

    #[test]
    fn profile_names_read_as_a_sentence() {
        assert_eq!(profile_names(PROFILE_PUBLIC), "public networks");
        assert_eq!(profile_names(PROFILE_DOMAIN | PROFILE_PRIVATE), "domain and private networks");
        assert_eq!(profile_names(ALL_PROFILES), "domain, private and public networks");
    }
}
