//! Orbit owns realtime audio, credentials and its native character. Bro only discovers a
//! compatible app and hands it the protected external-host descriptor's path. All helpers
//! here are blocking: run them on a worker, never the UI thread. No microphone, hotkey,
//! stock-Orbit installer lives here. Recovery is scoped to the selected managed
//! companion, its original pairing file, and its verified native process lifetime.
use crate::config::OrbitSettings;
use crate::util::{env_path, home_dir};
use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub const FORK_URL: &str = "https://github.com/JustSuperHuman/orbit-voice";
pub const UPSTREAM_URL: &str = "https://github.com/Cookiebubba/orbit-voice";
pub const TESTED_UPSTREAM_REVISION: &str = "f65f437d8df7b5a5b976da6b7bbd79476ebd7232";
pub const MANIFEST_FILE: &str = "orbit-bro-compatibility.json";
pub const PROTOCOL: u32 = 1;
pub const REQUIRED_CAPABILITIES: &[&str] = &[
    "sessions.read",
    "sessions.focus",
    "orchestrator.submit",
    "requests.observe",
    "requests.cancel-queued",
    "avatar.desktop",
    "avatar.attached",
];
pub const RELEASE_UNAVAILABLE: &str = "Automatic Orbit Voice updates are not available for this compatibility preview. Install a tested companion package separately.";
pub const SETUP_GUIDANCE: &str = "Download the Windows companion from https://github.com/JustSuperHuman/orbit-voice/releases/tag/bro-preview-v1.3.0.1, extract it and run install-bro-preview.ps1. Then enable Orbit Voice in bro settings. Developers can also build the integration fork and register its executable and application directory with bro orbit register. The preview uses its own profile and keeps the microphone off until you start voice.";

/// Opaque project identity for voice capture, with no filesystem access on the UI thread.
pub fn project_scope_id(root: &Path) -> Option<String> {
    if !root.is_absolute() { return None; }
    let normalized = root.to_string_lossy().replace('\\', "/");
    let normalized = normalized.strip_prefix("//?/").unwrap_or(&normalized);
    let normalized = if normalized == "/" { "/" } else { normalized.trim_end_matches('/') };
    let normalized = if cfg!(windows) { normalized.to_lowercase() } else { normalized.to_string() };
    Some(crate::coordination::digest_bytes(normalized.as_bytes()))
}

/// Native process lifetime, not a PID/name scan or a claim supplied by the companion.
/// Unsupported/unreadable identities fail closed. Run on the lifecycle worker, not the UI.
pub fn managed_process_identity(pid: u32) -> Option<String> {
    if pid == 0 { return None; }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{FILETIME, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
        if raw.is_null() { return None; }
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_TIMEOUT { return None; }
        let mut created: FILETIME = unsafe { std::mem::zeroed() };
        let mut exited: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
        let mut user: FILETIME = unsafe { std::mem::zeroed() };
        if unsafe { GetProcessTimes(process.as_raw_handle(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 { return None; }
        let birth = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        Some(format!("windows:{birth:016x}"))
    }
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, fields) = stat.rsplit_once(") ")?;
        let fields = fields.split_whitespace().collect::<Vec<_>>();
        if fields.first().is_some_and(|state| matches!(*state, "Z" | "X")) { return None; }
        let birth = fields.get(19)?.parse::<u64>().ok()?;
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        let boot = boot.trim();
        if boot.len() != 36 || !boot.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') { return None; }
        Some(format!("linux:{boot}:{birth}"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    { let _ = pid; None } // The current managed Electron preview is Windows-only.
}

/// Supported native targets, not a promise that a compatible artifact has been published.
pub fn platform() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("win32-x64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("darwin-arm64")
    } else {
        None
    }
}

pub fn supported() -> bool {
    platform().is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
    BroPreview,
}

/// Parse the version itself: GitHub's prerelease flag is not reliable for upstream betas.
/// Numeric identifiers obey semver's leading-zero rule; build metadata is not a channel.
pub fn version_channel(version: &str) -> anyhow::Result<Channel> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let (version, build) = version
        .split_once('+')
        .map_or((version, None), |(v, b)| (v, Some(b)));
    let identifiers = |s: &str, numeric_zero_rule: bool| {
        !s.is_empty()
            && s.split('.').all(|part| {
                !part.is_empty()
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !(numeric_zero_rule
                        && part.len() > 1
                        && part.starts_with('0')
                        && part.bytes().all(|b| b.is_ascii_digit()))
            })
    };
    if build.is_some_and(|b| !identifiers(b, false)) {
        bail!("invalid Orbit semver build metadata");
    }
    let (core, pre) = version
        .split_once('-')
        .map_or((version, None), |(v, p)| (v, Some(p)));
    let components: Vec<_> = core.split('.').collect();
    if components.len() != 3
        || components.iter().any(|p| {
            p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
        })
    {
        bail!("invalid Orbit semver version");
    }
    match pre {
        None => Ok(Channel::Stable),
        Some(p) if identifiers(p, true) && p.starts_with("bro-preview.") => Ok(Channel::BroPreview),
        Some(p) if identifiers(p, true) && p.starts_with("beta.") => Ok(Channel::Beta),
        Some(_) => bail!("unsupported Orbit prerelease channel"),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub repository: String,
    pub revision: String,
    pub upstream_revision: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolRange {
    pub min: u32,
    pub max: u32,
}

/// Generated by Orbit's `npm run compatibility:manifest`, not a hand-written ready marker.
/// Hashes validate the explicitly selected source preview, not a publisher signature or
/// the behavior of an already-running process. Pairing needs the authenticated handshake.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityManifest {
    pub schema: u32,
    pub version: String,
    pub channel: String,
    pub platform: String,
    pub source: Source,
    pub protocol: ProtocolRange,
    pub capabilities: Vec<String>,
    pub realtime_providers: Vec<String>,
    pub app_id: String,
    pub profile: String,
    pub updates_disabled: bool,
    pub files: BTreeMap<String, String>,
}

fn hex_digest(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn checked_file(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    if !safe_relative(relative) {
        bail!("Orbit's manifest contains an unsafe file path");
    }
    let file = root
        .join(relative)
        .canonicalize()
        .context("Orbit's preview is missing a manifest file")?;
    if !file.starts_with(root) || !file.is_file() {
        bail!("Orbit's manifest file escapes the application directory");
    }
    Ok(file)
}

fn source_files(root: &Path, dir: &str, files: &mut Vec<String>) -> anyhow::Result<()> {
    let directory = root.join(dir);
    if !directory.is_dir() {
        bail!("Orbit's preview is missing {dir}");
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("Orbit's preview has a non-UTF-8 file name"))?;
        let relative = format!("{dir}/{name}");
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("Orbit's preview source files must not be symbolic links");
        }
        if kind.is_dir() {
            source_files(root, &relative, files)?;
        } else if kind.is_file() {
            files.push(relative);
        }
    }
    Ok(())
}

/// Validate real payload hashes, provenance, protocol, provider and preview isolation.
/// There is deliberately no equivalent acceptance path for unsigned stock/beta releases.
pub fn validate_preview(
    application_dir: &Path,
    expected_platform: &str,
) -> anyhow::Result<CompatibilityManifest> {
    if !matches!(expected_platform, "win32-x64" | "darwin-arm64") {
        bail!("Orbit Voice supports Windows x64 and native Apple-silicon macOS only");
    }
    let root = application_dir
        .canonicalize()
        .context("Orbit's application directory does not exist")?;
    let manifest_path = root.join(MANIFEST_FILE);
    if std::fs::metadata(&manifest_path)
        .context("This Orbit build has no bro compatibility manifest")?
        .len()
        > 4 * 1024 * 1024
    {
        bail!("Orbit's compatibility manifest is too large");
    }
    let manifest: CompatibilityManifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)
        .context("Orbit's compatibility manifest is invalid")?;
    if manifest.schema != 1 || manifest.platform != expected_platform {
        bail!("Orbit's compatibility manifest has an unsupported schema or platform");
    }
    if version_channel(&manifest.version)? != Channel::BroPreview
        || manifest.channel != "bro-preview"
    {
        bail!(
            "This is not the isolated bro-preview channel; stock Orbit releases are incompatible"
        );
    }
    if manifest.source.repository != FORK_URL
        || !hex_digest(&manifest.source.revision, 40)
        || manifest.source.upstream_revision != TESTED_UPSTREAM_REVISION
    {
        bail!("Orbit's preview source provenance does not match the tested integration");
    }
    if manifest.protocol.min == 0
        || manifest.protocol.min > PROTOCOL
        || manifest.protocol.max < PROTOCOL
        || manifest.protocol.min > manifest.protocol.max
    {
        bail!("Orbit's preview does not support external-host protocol {PROTOCOL}");
    }
    if REQUIRED_CAPABILITIES
        .iter()
        .any(|cap| !manifest.capabilities.iter().any(|c| c == cap))
    {
        bail!("Orbit's preview is missing required bro session or native-character capabilities");
    }
    if !manifest.realtime_providers.iter().any(|p| p == "openai") {
        bail!(
            "Orbit's preview does not advertise OpenAI Realtime; record/transcribe/playback is not compatible"
        );
    }
    if manifest.app_id != "dev.justsuperhuman.orbit.bro-preview"
        || manifest.profile != "OrbitBroPreview"
        || !manifest.updates_disabled
    {
        bail!(
            "Orbit's preview must use its isolated app identity and profile with upstream updates disabled"
        );
    }
    // Reject both stale hashes and unlisted injected source files. Include all built UI assets
    // in public, not only a pair of recognizable entry points or a marker file.
    let mut expected_files = vec!["package.json".to_string()];
    for dir in ["electron", "src", "public"] {
        source_files(&root, dir, &mut expected_files)?;
    }
    for file in &expected_files {
        if !manifest.files.contains_key(file) {
            bail!("Orbit's compatibility manifest does not cover {file}");
        }
    }
    for (relative, expected) in &manifest.files {
        if !hex_digest(expected, 64) {
            bail!("Orbit's manifest has an invalid SHA-256 checksum");
        }
        let file = checked_file(&root, relative)?;
        let actual = crate::self_update::sha256_file(&file)?;
        if !actual.eq_ignore_ascii_case(expected) {
            bail!(
                "Orbit's preview checksum failed for {relative}; rebuild and regenerate its compatibility manifest"
            );
        }
    }
    for required in ["electron/main.js", "src/server.js"] {
        if !manifest.files.contains_key(required) {
            bail!("Orbit's compatibility manifest does not cover {required}");
        }
    }
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(checked_file(&root, "package.json")?)?)?;
    if package.get("version").and_then(|v| v.as_str()) != Some(manifest.version.as_str()) {
        bail!("Orbit's package version does not match its compatibility manifest");
    }
    Ok(manifest)
}

/// A validated, explicitly installed fork. Registration only changes these two paths;
/// it never imports provider credentials or rewrites the user's other preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewInstallation {
    pub executable: PathBuf,
    pub application_dir: PathBuf,
    pub version: String,
}

impl PreviewInstallation {
    pub fn apply(&self, settings: &mut OrbitSettings) {
        settings.executable = Some(self.executable.clone());
        settings.application_dir = Some(self.application_dir.clone());
    }
}

/// Blocking validation for the live settings owner. Do not run on the UI thread.
/// No process launch or process scan is needed to register a payload.
pub fn validate_installation(settings: &OrbitSettings) -> anyhow::Result<PreviewInstallation> {
    for path in [&settings.executable, &settings.application_dir]
        .into_iter()
        .flatten()
    {
        if !path.is_absolute() {
            bail!("Installed Orbit preview paths must be absolute");
        }
    }
    let executable = settings
        .executable
        .as_ref()
        .context("Set orbit_voice.executable to the installed fork's Electron executable first")?
        .canonicalize()
        .context("The installed Orbit executable is missing")?;
    if !executable.is_file() {
        bail!("The installed Orbit executable must be a file");
    }
    let application_dir = settings
        .application_dir
        .as_ref()
        .context("Set orbit_voice.application_dir to the built fork's application folder first")?
        .canonicalize()
        .context("The installed Orbit application directory is missing")?;
    let manifest = validate_preview(
        &application_dir,
        platform().context("Orbit Voice is unsupported on this platform")?,
    )?;
    Ok(PreviewInstallation {
        executable: crate::util::normalize_path(&executable),
        application_dir: crate::util::normalize_path(&application_dir),
        version: manifest.version,
    })
}

#[derive(serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewRegistration {
    schema: u32,
    executable: PathBuf,
    application_dir: PathBuf,
}

fn registration_path() -> PathBuf {
    crate::paths::bro_dir().join("orbit/installed-preview.json")
}

/// Installer handoff, separate from v2.toml so an already queued whole-settings
/// save cannot erase the selection before the live owner gets to reload it.
/// This writes no settings and launches no process.
pub fn register_installed_preview(settings: &OrbitSettings) -> anyhow::Result<PreviewInstallation> {
    let preview = validate_installation(settings)?;
    let registration = PreviewRegistration {
        schema: 1,
        executable: preview.executable.clone(),
        application_dir: preview.application_dir.clone(),
    };
    crate::util::atomic_write(
        &registration_path(),
        &serde_json::to_vec_pretty(&registration)?,
    )?;
    Ok(preview)
}

/// Reload only the installed-preview selection. The caller merges it into current
/// live settings, never into a potentially stale full settings document.
pub fn installed_preview() -> anyhow::Result<PreviewInstallation> {
    let file = registration_path();
    match std::fs::metadata(&file) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > 8192 {
                bail!("The installed Orbit preview registration is invalid");
            }
            let registration: PreviewRegistration = serde_json::from_slice(&std::fs::read(&file)?)
                .context("The installed Orbit preview registration is invalid")?;
            if registration.schema != 1 {
                bail!("The installed Orbit preview registration schema is unsupported");
            }
            validate_installation(&OrbitSettings {
                executable: Some(registration.executable),
                application_dir: Some(registration.application_dir),
                ..Default::default()
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_installation(&crate::config::Settings::load().orbit_voice)
        }
        Err(error) => Err(error).context("Reading the installed Orbit preview registration"),
    }
}

#[derive(Debug, Clone, Default)]
pub struct Detection {
    /// A local executable exists; does not imply compatibility or microphone readiness.
    pub installed: bool,
    /// A process was observed, independently of installation and compatibility.
    pub running: bool,
    /// The selected local payload passed validation; runtime pairing is a separate state.
    pub compatible: bool,
    pub version: Option<String>,
    pub error: Option<String>,
    pub executable: Option<PathBuf>,
    pub application_dir: Option<PathBuf>,
    /// Observation errors never imply that no instance is running.
    pub process_error: Option<String>,
    pub instances: Vec<RunningInstance>,
}

impl Detection {
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
    pub fn summary(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if let Some(error) = &self.process_error {
            return format!(
                "Local preview validation finished, but running-instance status is unknown. {error}"
            );
        }
        match (self.installed, self.running, self.compatible) {
            (_, true, false) => "Orbit is running, but bro compatibility has not been verified. Stock Orbit cannot connect.".into(),
            (true, true, true) => "Compatible Orbit preview is installed; an Orbit process is running. Connection and microphone readiness are checked separately.".into(),
            (true, false, true) => "Compatible Orbit preview is ready to open. Configure OpenAI Realtime and microphone access in Orbit.".into(),
            (true, _, false) => "Orbit is installed, but this build is not compatible with bro.".into(),
            _ => RELEASE_UNAVAILABLE.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunningInstance {
    pub pid: u32,
    pub executable: Option<PathBuf>,
    /// Only true when the observed executable (and Electron application) matches the
    /// validated preview. This is not a substitute for the authenticated protocol handshake.
    pub selected_preview: bool,
    /// Native launch metadata only, never credentials or arbitrary command lines.
    pub managed_descriptor: Option<PathBuf>,
    pub process_identity: Option<String>,
}

fn default_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(windows) {
        if let Some(local) = env_path("LOCALAPPDATA") {
            for dir in [
                "Programs/Orbit Voice Bro Preview",
                "Programs/orbit-voice",
                "Programs/Orbit",
                "Programs/Orbit Voice",
            ] {
                for exe in [
                    "Orbit Voice Bro Preview.exe",
                    "Orbit.exe",
                    "Orbit Voice.exe",
                    "orbit-voice.exe",
                ] {
                    paths.push(local.join(dir).join(exe));
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        for base in [
            home_dir().join("Applications"),
            PathBuf::from("/Applications"),
        ] {
            for app in [
                "Orbit Voice Bro Preview.app",
                "Orbit.app",
                "Orbit Voice.app",
            ] {
                paths.push(base.join(app).join("Contents/MacOS/Orbit"));
            }
        }
    }
    paths
}

fn detect_local(settings: &OrbitSettings) -> Detection {
    let mut found = Detection::default();
    if !supported() {
        found.error = Some("Orbit Voice supports Windows x64 and native Apple-silicon macOS only. No legacy voice fallback is started.".into());
    }
    let application_dir = settings
        .application_dir
        .clone()
        .or_else(|| env_path("BRO_ORBIT_APPLICATION_DIR"));
    // Explicit config wins. Never silently fall back to stock if a chosen preview is missing.
    let explicit = settings
        .executable
        .clone()
        .or_else(|| env_path("BRO_ORBIT_EXECUTABLE"));
    found.executable = if explicit.is_some() || application_dir.is_some() {
        explicit
    } else {
        default_candidates().into_iter().find(|p| p.is_file())
    };
    found.application_dir = application_dir;
    found.installed = found.executable.as_ref().is_some_and(|p| p.is_file());
    if !found.installed {
        found.error.get_or_insert_with(|| if found.executable.is_some() || found.application_dir.is_some() {
            "The selected Orbit preview executable is missing. Set both the Electron executable and application directory for a local preview.".into()
        } else { RELEASE_UNAVAILABLE.into() });
        return found;
    }
    let executable = found.executable.as_ref().unwrap();
    let root = found
        .application_dir
        .as_deref()
        .or_else(|| executable.parent())
        .unwrap_or(Path::new("."));
    match validate_preview(root, platform().unwrap_or("unsupported")) {
        Ok(manifest) => {
            found.version = Some(manifest.version);
            // Until a verified packaged artifact exists, only explicitly selected source previews
            // may launch. A file beside an arbitrary stock executable cannot make it compatible.
            if found.application_dir.is_none() {
                found.error.get_or_insert_with(|| "Packaged Orbit compatibility releases are not available. Select an Electron executable and the built preview's application directory.".into());
            } else {
                found.compatible = supported();
            }
        }
        Err(error) => {
            found.error.get_or_insert_with(|| {
                format!("This Orbit build cannot connect to bro: {error}. {RELEASE_UNAVAILABLE}")
            });
        }
    }
    found
}

/// Read-only detection. No network, microphone, credentials, startup entry, kill or relaunch.
pub fn detect(settings: &OrbitSettings) -> Detection {
    let mut found = detect_local(settings);
    match running_instances(&found) {
        Ok(instances) => {
            found.running = !instances.is_empty();
            found.instances = instances;
        }
        Err(error) => {
            found.process_error = Some(format!(
                "Could not inspect running Orbit instances: {error}"
            ))
        }
    }
    found
}

fn quiet(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW; never a kill-on-close job.
    }
    command
}

#[cfg(windows)]
fn powershell() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe")
}

#[cfg(windows)]
fn running_instances(found: &Detection) -> anyhow::Result<Vec<RunningInstance>> {
    // The fixed script emits identities and a match bit, never command lines or tokens.
    // A generic Electron process only counts when its application-directory argument matches.
    const SCRIPT: &str = include_str!("orbit_voice/processes.ps1");
    let mut command = Command::new(powershell());
    quiet(&mut command).args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]);
    command.stdout(Stdio::piped());
    command.env(
        "BRO_ORBIT_SCAN_EXE",
        found
            .executable
            .as_ref()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| crate::util::normalize_path(&p))
            .unwrap_or_default(),
    );
    command.env(
        "BRO_ORBIT_SCAN_APP",
        found
            .application_dir
            .as_ref()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| crate::util::normalize_path(&p))
            .unwrap_or_default(),
    );
    let output = command
        .output()
        .context("process inspection is unavailable")?;
    if !output.status.success() {
        bail!("Windows process inspection failed");
    }
    parse_processes(&output.stdout, found.compatible)
}

#[cfg(windows)]
fn parse_processes(bytes: &[u8], compatible: bool) -> anyhow::Result<Vec<RunningInstance>> {
    #[derive(Deserialize)]
    struct Process {
        pid: u32,
        executable: Option<PathBuf>,
        selected: bool,
        #[serde(default)]
        managed_descriptor: Option<PathBuf>,
        #[serde(default)]
        process_identity: Option<String>,
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let values = match value {
        serde_json::Value::Array(v) => v,
        serde_json::Value::Null => Vec::new(),
        v => vec![v],
    };
    values
        .into_iter()
        .map(|v| {
            let p: Process = serde_json::from_value(v)?;
            Ok(RunningInstance {
                pid: p.pid,
                executable: p.executable,
                selected_preview: p.selected && compatible,
                managed_descriptor: (p.selected && compatible).then_some(p.managed_descriptor).flatten(),
                process_identity: (p.selected && compatible).then_some(p.process_identity).flatten(),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn running_instances(found: &Detection) -> anyhow::Result<Vec<RunningInstance>> {
    let out = Command::new("/bin/ps")
        .args(["-axo", "pid=,comm=,args="])
        .output()?;
    if !out.status.success() {
        bail!("macOS process inspection failed");
    }
    let selected = found
        .executable
        .as_ref()
        .and_then(|p| p.canonicalize().ok());
    let app = found
        .application_dir
        .as_ref()
        .and_then(|p| p.canonicalize().ok());
    let mut instances = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut fields = line.trim().splitn(2, char::is_whitespace);
        let Some(pid) = fields.next().and_then(|p| p.parse().ok()) else {
            continue;
        };
        let rest = fields.next().unwrap_or("").trim();
        // comm paths may contain spaces; identify the explicitly selected full path first.
        let same = selected
            .as_ref()
            .is_some_and(|p| rest.starts_with(p.to_string_lossy().as_ref()));
        let named = rest.contains("/Orbit.app/Contents/MacOS/")
            || rest.contains("/Orbit Voice.app/Contents/MacOS/")
            || rest.contains("/Orbit Voice Bro Preview.app/Contents/MacOS/");
        let app_matches = app
            .as_ref()
            .is_some_and(|p| rest.contains(p.to_string_lossy().as_ref()));
        if named || (same && app_matches) {
            instances.push(RunningInstance {
                pid,
                executable: same.then(|| selected.clone()).flatten(),
                selected_preview: found.compatible && same && app_matches,
                managed_descriptor: None,
                process_identity: None,
            });
        }
    }
    Ok(instances)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn running_instances(_found: &Detection) -> anyhow::Result<Vec<RunningInstance>> {
    Ok(Vec::new())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchOutcome {
    /// PID of the launch request, which may exit after forwarding to Orbit's single instance.
    pub pid: u32,
    pub reused: bool,
}

fn launch_arguments(
    found: &Detection,
    descriptor: Option<&Path>,
    settings: bool,
) -> anyhow::Result<Vec<OsString>> {
    if !found.compatible {
        bail!("{}", found.summary());
    }
    if let Some(error) = &found.process_error {
        bail!(
            "{error}. Open the compatible preview yourself before connecting; bro will not start a second voice runtime blindly."
        );
    }
    if found.instances.iter().any(|p| !p.selected_preview) {
        bail!(
            "Another Orbit instance is running. Quit or switch it yourself before opening the compatible preview; bro will not kill or replace it."
        );
    }
    let mut args = Vec::new();
    if let Some(app) = &found.application_dir {
        args.push(app.canonicalize()?.into_os_string());
    }
    if let Some(path) = descriptor {
        if !path.is_absolute() || !path.is_file() {
            bail!("Orbit's external-host descriptor must be an existing absolute file path");
        }
        args.push("--external-host-file".into());
        args.push(path.as_os_str().to_owned());
    }
    if settings {
        args.push("--open-settings".into());
    }
    Ok(args)
}

fn start(
    found: &Detection,
    descriptor: Option<&Path>,
    settings: bool,
) -> anyhow::Result<LaunchOutcome> {
    start_mode(found, descriptor, settings, false)
}

fn start_mode(found: &Detection, descriptor: Option<&Path>, settings: bool, managed: bool) -> anyhow::Result<LaunchOutcome> {
    let mut args = launch_arguments(found, descriptor, settings)?;
    if managed { args.push("--bro-managed".into()); }
    let executable = found
        .executable
        .as_ref()
        .context("Orbit's compatible executable was not found")?
        .canonicalize()?;
    let mut command = Command::new(executable);
    quiet(&mut command).args(args);
    // Inherited development overrides must not redirect pairing to another host or hijack Electron.
    command
        .env_remove("ORBIT_EXTERNAL_HOST_FILE")
        .env_remove("ELECTRON_RUN_AS_NODE")
        .env_remove("NODE_OPTIONS");
    let mut child = command
        .spawn()
        .context("opening the compatible Orbit preview")?;
    let pid = child.id();
    // Reap only; never attach an owner/job that kills an independent desktop app on drop.
    let _ = std::thread::Builder::new()
        .name("orbit-launch-request".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(LaunchOutcome {
        pid,
        reused: found.instances.iter().any(|p| p.selected_preview),
    })
}

/// Explicit user action only. Orbit's second-instance handler receives the descriptor path
/// when already open. Callers own consent/connection state and must not restart after Quit.
pub fn launch(settings: &OrbitSettings, descriptor: &Path) -> anyhow::Result<LaunchOutcome> {
    if !settings.enabled {
        bail!(
            "Orbit Voice is disabled. Enable it explicitly before connecting; legacy voice settings do not grant consent."
        );
    }
    start(&detect(settings), Some(descriptor), false)
}

/// Validate managed support and identify the actual selected payload, off the UI thread.
pub fn managed_build_id(settings: &OrbitSettings) -> anyhow::Result<String> {
    let preview = validate_installation(settings)?;
    let path = preview.application_dir.join(MANIFEST_FILE);
    let manifest: CompatibilityManifest = serde_json::from_slice(&std::fs::read(&path)?)?;
    if !manifest.capabilities.iter().any(|c| c == "voice.lifecycle.v1") {
        bail!("The selected Orbit build needs an update for background controls. Existing runtimes are not restarted automatically.");
    }
    crate::self_update::sha256_file(&path)
}

/// The actual managed process, including when no new launch request was needed.
#[derive(Clone, Debug)]
pub struct ManagedLaunch {
    pub pid: u32,
    pub build_id: String,
    pub process_identity: String,
    pub reused: bool,
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = crate::util::normalize_path(left);
    let right = crate::util::normalize_path(right);
    if cfg!(windows) { left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy()) } else { left == right }
}

fn managed_instance<'a>(found: &'a Detection, descriptor: &Path) -> anyhow::Result<Option<&'a RunningInstance>> {
    if !found.compatible { bail!("{}", found.summary()); }
    if let Some(error) = &found.process_error { bail!("{error}"); }
    if !descriptor.is_absolute() || !descriptor.is_file() {
        bail!("Orbit's external-host descriptor must be an existing absolute file path");
    }
    let mut matches = found.instances.iter().filter(|p| p.selected_preview
        && p.managed_descriptor.as_ref().is_some_and(|path| same_path(path, descriptor))
        && p.process_identity.is_some());
    let instance = matches.next();
    if matches.next().is_some() { bail!("More than one Orbit companion is paired with this workspace. Retry after they finish closing."); }
    Ok(instance)
}

/// Enabled integration only. Reconnect the selected preview at its original
/// pairing path; its authenticated heartbeat must still prove its build and lifetime.
pub fn launch_managed(settings: &OrbitSettings, descriptor: &Path) -> anyhow::Result<ManagedLaunch> {
    if !settings.enabled { bail!("Orbit Voice is disabled"); }
    let build = managed_build_id(settings)?;
    let found = detect(settings);
    if let Some(instance) = managed_instance(&found, descriptor)? {
        let identity = instance.process_identity.as_ref().unwrap();
        if managed_process_identity(instance.pid).as_ref() != Some(identity) {
            bail!("Orbit finished closing during connection. Try connecting again.");
        }
        return Ok(ManagedLaunch { pid: instance.pid, build_id: build, process_identity: identity.clone(), reused: true });
    }
    if found.instances.iter().any(|p| p.selected_preview) {
        bail!("The selected Orbit preview is open with another connection. Close that preview before connecting it to this workspace.");
    }
    let launched = start_mode(&found, Some(descriptor), false, true)?;
    let identity = managed_process_identity(launched.pid).context("Orbit exited before its connection could be verified")?;
    Ok(ManagedLaunch { pid: launched.pid, build_id: build, process_identity: identity, reused: false })
}

/// Recover a nonresponsive companion only after the lifecycle owner has waited
/// for its heartbeat. Never terminate by process name or follow a changed pairing.
pub fn restart_managed(settings: &OrbitSettings, descriptor: &Path, previous: &ManagedLaunch, mut still_needed: impl FnMut() -> bool) -> anyhow::Result<Option<ManagedLaunch>> {
    if !settings.enabled { bail!("Orbit Voice is disabled"); }
    managed_build_id(settings)?; // An invalid update must not close a running companion.
    let found = detect(settings);
    if !still_needed() { return Ok(None); } // Discovery can overlap a late heartbeat or disable/quit.
    if let Some(instance) = managed_instance(&found, descriptor)? {
        if instance.pid != previous.pid || instance.process_identity.as_ref() != Some(&previous.process_identity) {
            bail!("Orbit changed while reconnecting. Try connecting again.");
        }
        stop_managed_process(previous.pid, &previous.process_identity)?;
    } else if managed_process_identity(previous.pid).as_ref() == Some(&previous.process_identity) {
        bail!("Orbit's original connection could not be verified for recovery");
    }
    if !still_needed() { return Ok(None); }
    launch_managed(settings, descriptor).map(Some)
}

#[cfg(windows)]
fn stop_managed_process(pid: u32, identity: &str) -> anyhow::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{FILETIME, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    if raw.is_null() { return Err(std::io::Error::last_os_error()).context("Opening the unresponsive Orbit companion"); }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } == WAIT_OBJECT_0 { return Ok(()); }
    let mut created: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    if unsafe { GetProcessTimes(process.as_raw_handle(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Verifying the Orbit companion's process lifetime");
    }
    let birth = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if identity != format!("windows:{birth:016x}") { bail!("Orbit's process lifetime changed during recovery"); }
    if unsafe { TerminateProcess(process.as_raw_handle(), 0) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Restarting the unresponsive Orbit companion");
    }
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) } != WAIT_OBJECT_0 { bail!("Orbit did not finish closing during recovery"); }
    Ok(())
}

#[cfg(not(windows))]
fn stop_managed_process(_pid: u32, _identity: &str) -> anyhow::Result<()> {
    bail!("Automatic recovery of an unresponsive Orbit companion is unavailable on this platform")
}

/// Open Orbit's own provider/microphone/hotkey settings. Never start recording in bro.
pub fn open_settings(settings: &OrbitSettings) -> anyhow::Result<()> {
    start(&detect(settings), None, true).map(|_| ())
}

/// Repository links are fixed constants; never shell-interpolate configured paths or tokens.
pub fn open_repository(upstream: bool) -> anyhow::Result<()> {
    let url = if upstream { UPSTREAM_URL } else { FORK_URL };
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let mut child = quiet(&mut command)
        .spawn()
        .context("opening the Orbit repository")?;
    let _ = std::thread::Builder::new()
        .name("orbit-repository".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

/// No compatible published, checksum-verified artifact is known. Do not query upstream's
/// latest endpoint or invent a fork asset URL; it would install the wrong application.
pub fn check_release() -> anyhow::Result<()> {
    bail!("{RELEASE_UNAVAILABLE} {SETUP_GUIDANCE}")
}
pub fn install() -> anyhow::Result<()> {
    check_release()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{atomic_write, test_env::sandbox};
    use serde_json::json;

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn managed_identity_comes_from_a_live_native_process_not_a_pid_claim() {
        let identity = managed_process_identity(std::process::id()).unwrap();
        assert_eq!(managed_process_identity(std::process::id()).as_ref(), Some(&identity));
        assert!(managed_process_identity(0).is_none());
        assert!(managed_process_identity(u32::MAX).is_none());
    }

    #[test]
    fn managed_recovery_requires_the_selected_preview_and_original_pairing() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = root.path().join("connection.json");
        atomic_write(&descriptor, b"{}").unwrap();
        let own = RunningInstance {
            pid: std::process::id(), executable: None, selected_preview: true,
            managed_descriptor: Some(descriptor.clone()), process_identity: Some("native-lifetime".into()),
        };
        let mut found = Detection { compatible: true, instances: vec![own.clone()], ..Default::default() };
        assert_eq!(managed_instance(&found, &descriptor).unwrap().unwrap().pid, own.pid);
        found.instances[0].managed_descriptor = Some(root.path().join("another-workspace.json"));
        assert!(managed_instance(&found, &descriptor).unwrap().is_none());
        found.instances[0] = own.clone();
        found.instances[0].selected_preview = false;
        assert!(managed_instance(&found, &descriptor).unwrap().is_none());
        found.instances[0] = own.clone();
        found.instances[0].process_identity = None;
        assert!(managed_instance(&found, &descriptor).unwrap().is_none());
        found.instances[0] = own.clone();
        found.instances.push(own);
        assert!(managed_instance(&found, &descriptor).is_err(), "ambiguous ownership must not pick a process to terminate");
        found.instances.pop();
        found.process_error = Some("inspection failed".into());
        assert!(managed_instance(&found, &descriptor).is_err());
        assert!(managed_instance(&found, Path::new("relative.json")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_discovery_matches_native_arguments_and_ignores_electron_helpers() {
        let script = r#"
function Get-CimInstance {
    @(
        [pscustomobject]@{Name='electron.exe';ProcessId=41;ExecutablePath='C:\preview\electron.exe';CommandLine='"C:\preview\electron.exe" "\\?\F:\Orbit preview" --external-host-file "C:\bro data\orbit\connection.json" --bro-managed'},
        [pscustomobject]@{Name='electron.exe';ProcessId=42;ExecutablePath='C:\preview\electron.exe';CommandLine='"C:\preview\electron.exe" --type=gpu-process --user-data-dir="F:\Orbit preview"'},
        [pscustomobject]@{Name='Orbit Helper.exe';ProcessId=43;ExecutablePath='C:\preview\electron.exe';CommandLine='"C:\preview\electron.exe" --type=renderer'},
        [pscustomobject]@{Name='Orbit.exe';ProcessId=44;ExecutablePath='C:\stock\Orbit.exe';CommandLine='"C:\stock\Orbit.exe"'},
        [pscustomobject]@{Name='electron.exe';ProcessId=45;ExecutablePath='C:\preview\electron.exe';CommandLine='"C:\preview\electron.exe" "F:\another app" --label "F:\Orbit preview --bro-managed --external-host-file C:\wrong.json"'},
        [pscustomobject]@{Name='electron.exe';ProcessId=46;ExecutablePath='C:\preview\electron.exe';CommandLine='"C:\preview\electron.exe" "F:\Orbit preview" --label "--bro-managed --external-host-file C:\wrong.json"'}
    )
}
function Get-Process { [pscustomobject]@{Path='C:\preview\electron.exe';StartTime=[DateTime]::UtcNow} }
"#;
        let mut command = Command::new(powershell());
        quiet(&mut command).args(["-NoProfile", "-NonInteractive", "-Command", &format!("{script}\n{}", include_str!("orbit_voice/processes.ps1"))]);
        command.env("BRO_ORBIT_SCAN_EXE", "C:\\preview\\electron.exe").env("BRO_ORBIT_SCAN_APP", "F:\\Orbit preview");
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let instances = parse_processes(&output.stdout, true).unwrap();
        assert_eq!(instances.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![41, 44, 46]);
        assert_eq!(instances[0].managed_descriptor.as_deref(), Some(Path::new("C:\\bro data\\orbit\\connection.json")));
        assert!(instances[0].process_identity.as_deref().unwrap().starts_with("windows:"));
        assert!(!instances[1].selected_preview);
        assert!(instances[2].selected_preview && instances[2].managed_descriptor.is_none());
        let unvalidated = parse_processes(&output.stdout, false).unwrap();
        assert!(unvalidated.iter().all(|p| p.managed_descriptor.is_none() && p.process_identity.is_none()));
    }

    #[cfg(windows)]
    #[test]
    fn recovery_terminates_only_the_verified_child_lifetime() {
        let mut command = Command::new(powershell());
        quiet(&mut command).args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 60"]);
        let mut child = command.spawn().unwrap();
        let identity = managed_process_identity(child.id()).unwrap();
        assert!(stop_managed_process(child.id(), "windows:wrong-lifetime").is_err());
        assert!(child.try_wait().unwrap().is_none(), "a mismatched lifetime cannot authorize termination");
        stop_managed_process(child.id(), &identity).unwrap();
        assert!(child.wait().unwrap().success());
        assert!(managed_process_identity(child.id()).is_none());
    }

    // Extra roots/overrides aren't in the shared sandbox yet. Save/restore them while its
    // global env lock is held, without modifying another worker's shared util.rs changes.
    struct OrbitEnv(Vec<(&'static str, Option<OsString>)>);
    impl OrbitEnv {
        fn isolated(home: &Path) -> Self {
            let vars = [
                "LOCALAPPDATA",
                "APPDATA",
                "XDG_DATA_HOME",
                "XDG_CONFIG_HOME",
                "BRO_ORBIT_EXECUTABLE",
                "BRO_ORBIT_APPLICATION_DIR",
                "ORBIT_USER_DATA_DIR",
                "ORBIT_EXTERNAL_HOST_FILE",
            ];
            let saved = vars.iter().map(|&v| (v, std::env::var_os(v))).collect();
            unsafe {
                for var in vars {
                    std::env::remove_var(var);
                }
                for var in [
                    "LOCALAPPDATA",
                    "APPDATA",
                    "XDG_DATA_HOME",
                    "XDG_CONFIG_HOME",
                    "ORBIT_USER_DATA_DIR",
                ] {
                    std::env::set_var(var, home.join(var));
                }
            }
            Self(saved)
        }
    }
    impl Drop for OrbitEnv {
        fn drop(&mut self) {
            unsafe {
                for (var, value) in &self.0 {
                    match value {
                        Some(v) => std::env::set_var(var, v),
                        None => std::env::remove_var(var),
                    }
                }
            }
        }
    }

    fn preview(home: &Path) -> OrbitSettings {
        let app = home.join("Orbit preview with spaces");
        let executable = home.join(if cfg!(windows) {
            "electron.exe"
        } else {
            "electron"
        });
        atomic_write(&executable, b"test fixture, never executed").unwrap();
        let files = [
            (
                "package.json",
                br#"{"version":"1.3.0-bro-preview.1"}"#.as_slice(),
            ),
            ("electron/main.js", b"// fixture"),
            ("src/server.js", b"// fixture"),
            ("public/app.js", b"// fixture"),
        ];
        let mut sums = BTreeMap::new();
        for (path, bytes) in files {
            atomic_write(&app.join(path), bytes).unwrap();
            sums.insert(
                path,
                crate::self_update::sha256_file(&app.join(path)).unwrap(),
            );
        }
        let manifest = json!({"schema":1,"version":"1.3.0-bro-preview.1","channel":"bro-preview","platform":platform().unwrap_or("win32-x64"),"source":{"repository":FORK_URL,"revision":"1234567890123456789012345678901234567890","upstreamRevision":TESTED_UPSTREAM_REVISION},"protocol":{"min":1,"max":1},"capabilities":REQUIRED_CAPABILITIES,"realtimeProviders":["openai"],"appId":"dev.justsuperhuman.orbit.bro-preview","profile":"OrbitBroPreview","updatesDisabled":true,"files":sums});
        atomic_write(
            &app.join(MANIFEST_FILE),
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        OrbitSettings {
            enabled: true,
            executable: Some(executable),
            application_dir: Some(app),
            ..Default::default()
        }
    }

    fn rewrite_manifest(settings: &OrbitSettings, change: impl FnOnce(&mut serde_json::Value)) {
        let path = settings
            .application_dir
            .as_ref()
            .unwrap()
            .join(MANIFEST_FILE);
        let mut value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        change(&mut value);
        atomic_write(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn installed_preview_registration_validates_payload_and_merges_only_paths() {
        let sb = sandbox();
        let _env = OrbitEnv::isolated(sb.home());
        let selected = preview(sb.home());
        let mut disk = crate::config::Settings::default();
        disk.orbit_voice = selected.clone();
        disk.save().unwrap();
        if supported() {
            let installation = installed_preview().unwrap();
            let mut current = OrbitSettings {
                enabled: false,
                mode: "attached".into(),
                ..Default::default()
            };
            installation.apply(&mut current);
            assert!(!current.enabled);
            assert_eq!(current.mode, "attached");
            assert_eq!(current.executable, selected.executable);
            assert_eq!(current.application_dir, selected.application_dir);
            register_installed_preview(&selected).unwrap();
            // An older already-queued settings save may run before the UI reload.
            crate::config::Settings::default().save().unwrap();
            assert_eq!(
                installed_preview().unwrap(),
                installation,
                "the independent installer handoff survives old whole-settings writes"
            );
            atomic_write(
                &selected
                    .application_dir
                    .as_ref()
                    .unwrap()
                    .join("src/server.js"),
                b"// stale manifest",
            )
            .unwrap();
            assert!(
                installed_preview().is_err(),
                "a stale or replaced payload must not register"
            );
        } else {
            assert!(installed_preview().is_err());
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected real, locally built Orbit preview"]
    fn actual_generated_preview_manifest() {
        let application_dir =
            PathBuf::from(std::env::var_os("BRO_ORBIT_TEST_PREVIEW_DIR").expect(
                "set BRO_ORBIT_TEST_PREVIEW_DIR to the built preview application directory",
            ));
        let sb = sandbox();
        let _env = OrbitEnv::isolated(sb.home());
        let manifest = validate_preview(
            &application_dir,
            platform().expect("supported native target"),
        )
        .unwrap();
        assert!(
            manifest
                .realtime_providers
                .iter()
                .any(|provider| provider == "openai")
        );
    }

    #[cfg(windows)]
    #[test]
    fn observed_processes_do_not_assert_compatibility_or_expose_command_lines() {
        let processes = br#"[{"pid":12,"executable":"C:\\Orbit\\Orbit.exe","selected":false},{"pid":42,"executable":"C:\\preview\\electron.exe","selected":true}]"#;
        let stock = parse_processes(processes, false).unwrap();
        assert_eq!(stock.len(), 2);
        assert!(stock.iter().all(|p| !p.selected_preview));
        let preview = parse_processes(processes, true).unwrap();
        assert!(!preview[0].selected_preview && preview[1].selected_preview);
        assert!(parse_processes(b"", true).unwrap().is_empty());
        assert!(parse_processes(b"null", true).unwrap().is_empty());
        let unknown = Detection {
            compatible: true,
            installed: true,
            process_error: Some("inspection failed".into()),
            ..Default::default()
        };
        assert!(unknown.summary().contains("unknown"));
        assert!(!unknown.running);
    }

    #[test]
    fn semver_channels_do_not_trust_github_prerelease_flags() {
        assert_eq!(version_channel("v1.3.0-beta.4").unwrap(), Channel::Beta);
        assert_eq!(
            version_channel("1.3.0-bro-preview.1+build.42").unwrap(),
            Channel::BroPreview
        );
        assert_eq!(version_channel("1.3.0+beta.4").unwrap(), Channel::Stable);
        for invalid in [
            "1.3",
            "01.3.0",
            "1.3.0-beta.01",
            "1.3.0-bro-preview.",
            "1.3.0-preview.1",
            "1.3.0+",
            "1.3.0-beta.4+foo+bar",
        ] {
            assert!(version_channel(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn real_manifest_checks_payload_source_provider_protocol_and_isolation() {
        let sb = sandbox();
        let _env = OrbitEnv::isolated(sb.home());
        let settings = preview(sb.home());
        let app = settings.application_dir.as_ref().unwrap();
        let expected = platform().unwrap_or("win32-x64");
        assert_eq!(
            validate_preview(app, expected).unwrap().source.repository,
            FORK_URL
        );
        for (key, value) in [
            ("channel", json!("stable")),
            ("version", json!("1.3.0-beta.4")),
            ("realtimeProviders", json!(["record-stt-playback"])),
            ("updatesDisabled", json!(false)),
            ("profile", json!("Orbit")),
            ("platform", json!("linux-x64")),
            ("capabilities", json!(["sessions.read"])),
            ("protocol", json!({"min":2,"max":2})),
        ] {
            let original = std::fs::read(app.join(MANIFEST_FILE)).unwrap();
            rewrite_manifest(&settings, |m| m[key] = value);
            assert!(validate_preview(app, expected).is_err(), "{key}");
            atomic_write(&app.join(MANIFEST_FILE), &original).unwrap();
        }
        rewrite_manifest(&settings, |m| {
            m["source"]["repository"] = json!(UPSTREAM_URL)
        });
        assert!(validate_preview(app, expected).is_err());
    }

    #[test]
    fn markers_tampering_unlisted_source_and_traversal_are_not_compatibility() {
        let sb = sandbox();
        let _env = OrbitEnv::isolated(sb.home());
        let settings = preview(sb.home());
        let app = settings.application_dir.as_ref().unwrap();
        let expected = platform().unwrap_or("win32-x64");
        atomic_write(
            &app.join("src/server.js"),
            b"// replaced after validation manifest",
        )
        .unwrap();
        assert!(validate_preview(app, expected).is_err());
        let settings = preview(sb.home());
        atomic_write(&app.join("src/unlisted.js"), b"// injected").unwrap();
        assert!(validate_preview(app, expected).is_err());
        std::fs::remove_file(app.join("src/unlisted.js")).unwrap();
        rewrite_manifest(&settings, |m| {
            m["files"]["../outside"] = json!("0".repeat(64));
        });
        assert!(validate_preview(app, expected).is_err());
        for path in ["../x", "/x", "C:/x", "src/../x", "src\\x", "./x", "src//x"] {
            assert!(!safe_relative(path));
        }
        assert!(safe_relative("public/assets/app.js"));
    }

    #[test]
    fn detection_is_opt_in_explicit_and_separate_from_running() {
        let sb = sandbox();
        let _env = OrbitEnv::isolated(sb.home());
        let settings = preview(sb.home());
        let found = detect_local(&settings);
        assert!(found.installed && !found.running);
        assert_eq!(found.compatible, supported());
        assert_eq!(
            found.version(),
            supported().then_some("1.3.0-bro-preview.1")
        );
        let mut missing = settings.clone();
        missing.executable = Some(sb.home().join("missing.exe"));
        assert!(!detect_local(&missing).installed);
        unsafe {
            std::env::set_var(
                "BRO_ORBIT_EXECUTABLE",
                settings.executable.as_ref().unwrap(),
            );
            std::env::set_var(
                "BRO_ORBIT_APPLICATION_DIR",
                settings.application_dir.as_ref().unwrap(),
            );
        }
        assert_eq!(
            detect_local(&OrbitSettings::default()).compatible,
            supported()
        );
        assert!(
            !detect_local(&missing).installed,
            "explicit config never falls back to environment or stock"
        );
        let disabled = OrbitSettings {
            enabled: false,
            ..settings
        };
        assert!(
            launch(&disabled, &sb.home().join("unused"))
                .unwrap_err()
                .to_string()
                .contains("disabled")
        );
    }

    #[test]
    fn descriptor_path_not_token_is_passed_and_foreign_instances_are_left_alone() {
        let sb = sandbox();
        let _env = OrbitEnv::isolated(sb.home());
        let settings = preview(sb.home());
        let mut found = detect_local(&settings);
        if !supported() {
            return;
        }
        let descriptor = sb.bro().join("orbit/connection.json");
        atomic_write(
            &descriptor,
            br#"{"token":"must-never-appear-in-arguments"}"#,
        )
        .unwrap();
        let args = launch_arguments(&found, Some(&descriptor), false).unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(
            args[0],
            settings
                .application_dir
                .as_ref()
                .unwrap()
                .canonicalize()
                .unwrap()
                .as_os_str()
        );
        assert_eq!(args[1], "--external-host-file");
        assert_eq!(args[2], descriptor.as_os_str());
        assert!(!format!("{args:?}").contains("must-never"));
        assert!(launch_arguments(&found, Some(Path::new("relative.json")), false).is_err());
        assert_eq!(
            launch_arguments(&found, None, true)
                .unwrap()
                .last()
                .unwrap(),
            "--open-settings"
        );
        found.instances.push(RunningInstance {
            pid: 42,
            executable: None,
            selected_preview: false,
            managed_descriptor: None,
            process_identity: None,
        });
        assert!(
            launch_arguments(&found, Some(&descriptor), false)
                .unwrap_err()
                .to_string()
                .contains("will not kill")
        );
        found.instances[0].selected_preview = true;
        assert!(
            launch_arguments(&found, Some(&descriptor), false).is_ok(),
            "forward to the compatible single instance without terminating it"
        );
        found.process_error = Some("inspection unavailable".into());
        assert!(launch_arguments(&found, Some(&descriptor), false).is_err());
        assert!(
            check_release()
                .unwrap_err()
                .to_string()
                .contains("not available")
        );
        assert!(install().is_err());
    }
}
