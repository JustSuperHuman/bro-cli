//! The optional Bro RMM package. Nothing from Voxl is linked into Bro.
//! Downloads are pinned, staged and checked before the executable can run.
use anyhow::{Context, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const SOURCE: &str = "https://github.com/JustSuperHuman/voxl";
pub const VERSION: &str = "0.1.0";
const CHANNEL: &str = "https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-channel-v1";
/// Windows x64's catalog (the name Bro releases before the other platforms read).
pub const CATALOG_URL: &str =
    "https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-channel-v1/catalog.json";
const RELEASES: &str = "https://github.com/JustSuperHuman/voxl/releases/download/";

/// One computer Bro RMM is built for: its id in the signed catalog, its program and how it's packed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    pub id: &'static str,
    /// For people: "Windows on Arm", "macOS on Apple silicon".
    pub label: &'static str,
    pub executable: &'static str,
    /// "zip" on Windows, "tar.gz" elsewhere: what the system's own tar reads.
    pub archive: &'static str,
}

pub const PLATFORMS: [Platform; 6] = [
    Platform { id: "windows-x86_64", label: "Windows x64", executable: "voxl.exe", archive: "zip" },
    Platform { id: "windows-aarch64", label: "Windows on Arm", executable: "voxl.exe", archive: "zip" },
    Platform { id: "macos-aarch64", label: "macOS on Apple silicon", executable: "voxl", archive: "tar.gz" },
    Platform { id: "macos-x86_64", label: "macOS on Intel", executable: "voxl", archive: "tar.gz" },
    Platform { id: "linux-x86_64", label: "Linux x64", executable: "voxl", archive: "tar.gz" },
    Platform { id: "linux-aarch64", label: "Linux on Arm", executable: "voxl", archive: "tar.gz" },
];

impl Platform {
    /// Where its signed catalog is: Windows x64 keeps the original name, so Bros already out there keep updating.
    pub fn catalog_url(self) -> String {
        if self.id == "windows-x86_64" { CATALOG_URL.into() } else { format!("{CHANNEL}/catalog-{}.json", self.id) }
    }
    fn package_name(self) -> String {
        format!("bro-rmm-{}.{}", self.id, self.archive)
    }
}

/// The package for an OS and CPU as Rust names them (`std::env::consts`).
pub fn platform_for(os: &str, arch: &str) -> Option<Platform> {
    let id = match (os, arch) {
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => return None,
    };
    PLATFORMS.into_iter().find(|p| p.id == id)
}

/// This computer's package.
pub fn platform() -> Option<Platform> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}
pub const CATALOG_PUBLIC_KEY: &str =
    "081579899657111bd51a6318d6da53425bb129d9f7aa95609f0c30102a71ca9d";
const MAX_PACKAGE: u64 = 160 * 1024 * 1024;
const MAX_CATALOG: u64 = 32 * 1024;

#[derive(Serialize, Deserialize)]
struct SignedCatalog {
    payload: String,
    signature: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    #[serde(flatten)]
    manifest: Manifest,
    package_url: String,
    package_sha256: String,
}

fn catalog_for(bytes: &[u8], public_key: &[u8], platform: Platform) -> anyhow::Result<Release> {
    if bytes.len() > MAX_CATALOG as usize {
        bail!("The remote-control update catalog is too large.");
    }
    let envelope: SignedCatalog =
        serde_json::from_slice(bytes).context("The remote-control update catalog is invalid.")?;
    let payload = base64::engine::general_purpose::STANDARD.decode(envelope.payload)?;
    let signature = base64::engine::general_purpose::STANDARD.decode(envelope.signature)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(&payload, &signature)
        .map_err(|_| {
            anyhow::anyhow!(
                "The remote-control update signature is invalid. The installed package was kept."
            )
        })?;
    let release: Release = serde_json::from_slice(&payload)?;
    let manifest = &release.manifest;
    if manifest.schema_version != 1
        || manifest.id != "bro-rmm"
        || manifest.protocol_version != 1
        || manifest.platform != platform.id
        || manifest.executable != platform.executable
        || manifest.version.is_empty()
        || manifest.version.len() > 64
        || !manifest
            .version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        || !valid_hash(&manifest.sha256)
        || !valid_hash(&release.package_sha256)
        || !release.package_url.starts_with(RELEASES)
        || !release.package_url.ends_with(&format!("/{}", platform.package_name()))
    {
        bail!("The remote-control update is incompatible with this Bro.");
    }
    Ok(release)
}

fn catalog_key() -> anyhow::Result<Vec<u8>> {
    if !valid_hash(CATALOG_PUBLIC_KEY) {
        bail!("The remote-control release signing key is unavailable.");
    }
    (0..32)
        .map(|i| u8::from_str_radix(&CATALOG_PUBLIC_KEY[i * 2..i * 2 + 2], 16).map_err(Into::into))
        .collect()
}

/// What the update channel said.
enum Fetched {
    Release(Release, Vec<u8>),
    /// No answer: the installed package (if any) is used.
    Offline,
    /// The channel answered that it has no package for this platform yet.
    NotPublished,
}

/// Network failure uses the already verified installed package.
/// An answered but invalid catalog is an error, never a reason to trust different bytes.
fn fetch_catalog(platform: Platform) -> anyhow::Result<Fetched> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let response = match agent
        .get(&platform.catalog_url())
        .header("user-agent", "bro-rmm-installer")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(Fetched::NotPublished),
        Err(_) => return Ok(Fetched::Offline),
    };
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_CATALOG + 1)
        .read_to_end(&mut bytes)?;
    Ok(Fetched::Release(catalog_for(&bytes, &catalog_key()?, platform)?, bytes))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// All viewers are restricted at the gateway and the media transport.
    pub read_only: bool,
    /// The display to share, by index: 0 is the primary (Windows) or main (macOS) display. Linux shares the X11
    /// screen in `$DISPLAY`, all its monitors.
    pub display: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub protocol_version: u32,
    pub platform: String,
    pub executable: String,
    pub sha256: String,
}

/// Bro RMM is built for this computer's system (whether its package is published yet is the channel's to say).
pub fn supported() -> bool {
    platform().is_some()
}

/// How many displays can be shared one at a time. Linux shares its whole X11 screen.
pub fn display_count() -> u32 {
    #[cfg(windows)]
    {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows_sys::Win32::UI::WindowsAndMessaging::SM_CMONITORS,
            )
            .max(1) as u32
        }
    }
    #[cfg(target_os = "macos")]
    {
        mac::display_count()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        1
    }
}

/// What keeps the desktop from being shared from this session, said for the person at it (None: nothing).
/// Linux: voxl captures an X11 display, so it needs `$DISPLAY`, and on a Wayland desktop it would only see the
/// X11 windows. macOS: Screen Recording, which only someone at the Mac can allow.
pub fn session_problem() -> Option<String> {
    session_problem_with(&|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

fn session_problem_with(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if cfg!(target_os = "linux") {
        return linux_session_problem(env);
    }
    // Without it voxl streams only the wallpaper, and its own prompt would wait on a Mac nobody's at.
    if mac_permissions().is_some_and(|p| !p.screen) {
        return Some("This Mac hasn't allowed Screen Recording for Bro yet. Someone at the Mac needs to allow it in Remote Control settings.".into());
    }
    None
}

/// voxl captures an X11 display: it needs `$DISPLAY`, and under Wayland it would see only the X11 windows.
fn linux_session_problem(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let wayland = env("XDG_SESSION_TYPE").is_some_and(|t| t.eq_ignore_ascii_case("wayland")) || env("WAYLAND_DISPLAY").is_some();
    if wayland {
        return Some("Remote desktop on Linux shares an X11 session. This desktop runs Wayland, so sign in to an Xorg session (on Ubuntu, “Ubuntu on Xorg”) to use it.".into());
    }
    if env("DISPLAY").is_none() {
        return Some("Remote desktop on Linux needs a graphical X11 session. Start Bro from your desktop rather than over SSH or a service.".into());
    }
    None
}

/// What `bro remote-desktop-permissions` prints: a fresh process's answer, since a running app keeps seeing
/// "not allowed" until it relaunches.
pub fn permissions_line(p: MacPermissions) -> String {
    format!("screen={} accessibility={}", p.screen as u8, p.accessibility as u8)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_permissions(line: &str) -> Option<MacPermissions> {
    let mut found = (None, None);
    for part in line.split_whitespace() {
        match part.split_once('=') {
            Some(("screen", v)) => found.0 = Some(v == "1"),
            Some(("accessibility", v)) => found.1 = Some(v == "1"),
            _ => {}
        }
    }
    Some(MacPermissions { screen: found.0?, accessibility: found.1? })
}

/// What a settings screen can say while it draws: [`session_problem`] without anything slow (Linux's session
/// type from the environment; a Mac's permissions have their own row).
pub fn session_note() -> Option<String> {
    if cfg!(target_os = "linux") {
        linux_session_problem(&|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
    } else {
        None
    }
}

/// Who has to be at the computer for what, for the settings page.
pub fn at_the_computer() -> &'static str {
    if cfg!(windows) {
        "Windows lock screens and UAC prompts need someone at the computer. Bro RMM runs in your Windows session."
    } else if cfg!(target_os = "macos") {
        "The Mac's lock screen and password prompts need someone at the Mac. Bro RMM runs in your macOS session."
    } else {
        "The lock screen needs someone at the computer. Bro RMM shares the X11 session you're signed in to."
    }
}

/// What the lock screen and other protected screens mean here, for "the stream didn't start".
pub fn locked_hint() -> &'static str {
    if cfg!(windows) {
        "The desktop stream did not start. Unlock Windows and try again."
    } else if cfg!(target_os = "macos") {
        "The desktop stream did not start. Unlock the Mac, check Screen Recording for Bro in Remote Control settings, and try again."
    } else {
        "The desktop stream did not start. Unlock the desktop and try again."
    }
}

/// The privacy permissions macOS asks for before an app may see the screen and move the pointer. They belong to
/// the app that starts Bro RMM (Bro), so Bro asks for them while you're at the Mac, before a phone connects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacPermissions {
    /// Screen Recording: without it the phone sees only the wallpaper.
    pub screen: bool,
    /// Accessibility: without it the phone can watch but its mouse and keys do nothing.
    pub accessibility: bool,
}

/// None off macOS. Asked of a fresh Bro process (same app, so the same permissions), else of this one.
pub fn mac_permissions() -> Option<MacPermissions> {
    #[cfg(target_os = "macos")]
    {
        let fresh = std::env::current_exe().ok().and_then(|exe| {
            let out = Command::new(exe).arg("remote-desktop-permissions").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
            parse_permissions(String::from_utf8_lossy(&out.stdout).trim())
        });
        Some(fresh.unwrap_or_else(mac::permissions))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// This process's own answer (what `bro remote-desktop-permissions` prints). None off macOS.
pub fn mac_permissions_here() -> Option<MacPermissions> {
    #[cfg(target_os = "macos")]
    {
        Some(mac::permissions())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Ask macOS for what's missing: its own prompts for Screen Recording and Accessibility (each adds Bro to its list
/// in System Settings), then the settings page of the first that's still off. No-op elsewhere.
pub fn request_mac_permissions() -> Option<MacPermissions> {
    #[cfg(target_os = "macos")]
    {
        Some(mac::request())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::MacPermissions;
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(value: *const c_void);
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
    }

    pub fn display_count() -> u32 {
        let mut count = 0u32;
        // SAFETY: a null list asks only for the count.
        let status = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
        if status == 0 { count.max(1) } else { 1 }
    }

    pub fn permissions() -> MacPermissions {
        // SAFETY: both are side-effect-free checks.
        unsafe { MacPermissions { screen: CGPreflightScreenCaptureAccess(), accessibility: AXIsProcessTrusted() } }
    }

    pub fn request() -> MacPermissions {
        let before = permissions();
        // SAFETY: CoreFoundation objects made and released here; the prompt option is a CFBoolean.
        unsafe {
            if !before.screen {
                CGRequestScreenCaptureAccess();
            }
            if !before.accessibility {
                let keys = [kAXTrustedCheckOptionPrompt];
                let values = [kCFBooleanTrue];
                let options = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
                    &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
                );
                AXIsProcessTrustedWithOptions(options);
                if !options.is_null() {
                    CFRelease(options);
                }
            }
        }
        let after = super::mac_permissions().unwrap_or_else(permissions);
        // macOS shows each prompt once per app; after that only System Settings can change it.
        let page = if !after.screen {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        } else if !after.accessibility {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        } else {
            None
        };
        if let Some(page) = page.filter(|_| before == after) {
            let _ = std::process::Command::new("open").arg(page).spawn();
        }
        after
    }
}

pub fn package_dir(root: &Path) -> PathBuf {
    root.join("packages").join("protocol-v1")
}

pub fn installed(root: &Path) -> anyhow::Result<(PathBuf, Manifest)> {
    validate(&package_dir(root))
}

fn validate(dir: &Path) -> anyhow::Result<(PathBuf, Manifest)> {
    validate_with_key(dir, &catalog_key()?)
}

fn validate_with_key(dir: &Path, key: &[u8]) -> anyhow::Result<(PathBuf, Manifest)> {
    let Some(platform) = platform() else { bail!("Remote desktop isn't available for this computer's system.") };
    validate_for(dir, key, platform)
}

fn validate_for(dir: &Path, key: &[u8], platform: Platform) -> anyhow::Result<(PathBuf, Manifest)> {
    let file =
        File::open(dir.join("manifest.json")).context("Remote control is not installed yet.")?;
    let manifest: Manifest = serde_json::from_reader(file.take(16 * 1024))
        .context("The remote-control package manifest is invalid.")?;
    let mut bytes = Vec::new();
    // Every platform's package keeps its signed catalog under this one name.
    File::open(dir.join("catalog.json"))
        .context("The installed remote-control signature is missing. Choose Repair download.")?
        .take(MAX_CATALOG + 1)
        .read_to_end(&mut bytes)?;
    let release = catalog_for(&bytes, key, platform)?;
    if manifest.schema_version != 1
        || manifest.id != "bro-rmm"
        || manifest.version != release.manifest.version
        || release.manifest.sha256 != manifest.sha256
        || manifest.protocol_version != 1
        || manifest.platform != platform.id
        || manifest.executable != platform.executable
        || !valid_hash(&manifest.sha256)
    {
        bail!("This remote-control package is incompatible with Bro.");
    }
    let executable = dir.join(&manifest.executable);
    if std::fs::symlink_metadata(&executable)?
        .file_type()
        .is_symlink()
        || crate::self_update::sha256_file(&executable)? != manifest.sha256.to_ascii_lowercase()
    {
        bail!("The remote-control executable failed its SHA-256 check. Choose Repair download.");
    }
    Ok((executable, manifest))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The lock file is permanent: unlinking a locked file could give another owner a new inode.
pub fn lock(root: &Path, name: &str) -> anyhow::Result<File> {
    std::fs::create_dir_all(root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(name))?;
    file.try_lock()
        .map_err(|_| anyhow::anyhow!("Another Bro window is using remote control."))?;
    Ok(file)
}

/// Blocking. Called only by an explicitly enabled installation worker.
/// A newer Bro RMM than the installed one, if the channel has one: its signed catalog is fetched and checked, and
/// nothing is downloaded or replaced (that's [`install`]'s), so a running stream carries on while it looks.
pub fn update_available(root: &Path) -> anyhow::Result<Option<String>> {
    let Some(platform) = platform() else { bail!("Remote desktop isn't available for this computer's system.") };
    let current = installed(root).ok().map(|(_, manifest)| manifest.version);
    match fetch_catalog(platform)? {
        Fetched::Release(release, _) => {
            let latest = release.manifest.version;
            Ok(current.is_none_or(|v| crate::justtools::is_newer(&latest, &v)).then_some(latest))
        }
        Fetched::Offline => bail!("Couldn't reach Bro RMM's release channel. Check your connection and try again."),
        Fetched::NotPublished => Ok(None),
    }
}

pub fn install(root: &Path) -> anyhow::Result<Manifest> {
    let Some(platform) = platform() else {
        bail!("Remote desktop isn't available for this computer's system ({} on {}).", std::env::consts::OS, std::env::consts::ARCH);
    };
    let _lock = lock(root, "install.lock")?;
    let current = installed(root).ok();
    let fetched = fetch_catalog(platform)?;
    let update = match fetched {
        Fetched::Release(release, bytes) => Some((release, bytes)),
        Fetched::Offline => None,
        // nothing to update to: what's installed stays
        Fetched::NotPublished if current.is_some() => None,
        Fetched::NotPublished => bail!(
            "Bro RMM for {} isn't published yet. Bro looks again when you choose Repair download.",
            platform.label
        ),
    };
    if let Some((_, manifest)) = &current {
        if update.as_ref().is_none_or(|(release, _)| {
            release.manifest.version == manifest.version
                || !crate::justtools::is_newer(&release.manifest.version, &manifest.version)
        }) {
            return Ok(manifest.clone());
        }
    }
    let (release, _) = update.as_ref().context("Could not reach the remote-control package channel. Check your connection and choose Repair download.")?;
    let (package_url, package_sha256) = (
        release.package_url.as_str(),
        release.package_sha256.as_str(),
    );
    let staging = root.join(format!(".download-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(&staging)?;
    let result = (|| {
        let archive = staging.join(format!("package.{}", platform.archive));
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .build()
            .into();
        let response = agent.get(package_url).header("user-agent", "bro-rmm-installer").call()
            .context("Could not download remote control. Check your connection and choose Repair download.")?;
        let mut reader = response.into_body().into_reader().take(MAX_PACKAGE + 1);
        let mut output = File::create(&archive)?;
        let size = std::io::copy(&mut reader, &mut output)?;
        output.flush()?;
        drop(output);
        if size > MAX_PACKAGE || crate::self_update::sha256_file(&archive)? != package_sha256 {
            bail!("The remote-control download failed its SHA-256 check. Choose Repair download.");
        }
        let tar = archive_tool();
        let listing = hidden(Command::new(&tar).arg("-tf").arg(&archive)).output()?;
        if !listing.status.success() {
            bail!("Could not read the remote-control package.");
        }
        let members = std::str::from_utf8(&listing.stdout)?;
        if members.lines().count() > 4096 {
            bail!("The remote-control package contains too many files.");
        }
        if !members.lines().all(safe_member) {
            bail!("The remote-control package contains an unsafe path.");
        }
        // Reject links as well as path traversal before extracting anything.
        let kinds = hidden(Command::new(&tar).arg("-tvf").arg(&archive)).output()?;
        if !kinds.status.success()
            || !String::from_utf8_lossy(&kinds.stdout)
                .lines()
                .all(|line| line.starts_with('-') || line.starts_with('d'))
        {
            bail!("The remote-control package contains a link or a special file.");
        }
        let unpacked = staging.join("unpacked");
        std::fs::create_dir(&unpacked)?;
        let status = hidden(
            Command::new(tar)
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(&unpacked),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
        if !status.success() {
            bail!("Could not unpack remote control.");
        }
        if let Some((_, bytes)) = &update {
            std::fs::write(unpacked.join("catalog.json"), bytes)?;
        }
        let (executable, manifest) = validate(&unpacked)?;
        // tar keeps the mode, but a package packed on Windows wouldn't have one: voxl must be runnable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        let destination = package_dir(root);
        std::fs::create_dir_all(destination.parent().unwrap())?;
        let previous = staging.join("previous");
        if destination.exists() {
            std::fs::rename(&destination, &previous)?;
        }
        if let Err(error) = std::fs::rename(&unpacked, &destination) {
            if previous.exists() {
                let _ = std::fs::rename(&previous, &destination);
            }
            return Err(error.into());
        }
        Ok(manifest)
    })();
    // The random staging directory was created above inside this package root.
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn archive_tool() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32")
            .join("tar.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("tar")
    }
}

fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn safe_member(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        && !name.split('/').any(|part| part == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: Platform = PLATFORMS[0];

    #[test]
    fn signed_updates_reject_tampering_and_incompatible_packages() {
        use ring::signature::KeyPair;
        let random = ring::rand::SystemRandom::new();
        let key = ring::signature::Ed25519KeyPair::from_pkcs8(
            ring::signature::Ed25519KeyPair::generate_pkcs8(&random)
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let payload = serde_json::json!({"schemaVersion":1,"id":"bro-rmm","version":"0.2.0","protocolVersion":1,
            "platform":"windows-x86_64","executable":"voxl.exe","sha256":"a".repeat(64),
            "packageUrl":"https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-v0.2.0/bro-rmm-windows-x86_64.zip","packageSha256":"b".repeat(64)});
        let sign = |payload: &serde_json::Value| {
            let bytes = serde_json::to_vec(payload).unwrap();
            serde_json::to_vec(&SignedCatalog {
                payload: base64::engine::general_purpose::STANDARD.encode(&bytes),
                signature: base64::engine::general_purpose::STANDARD
                    .encode(key.sign(&bytes).as_ref()),
            })
            .unwrap()
        };
        let signed = sign(&payload);
        assert_eq!(
            catalog_for(&signed, key.public_key().as_ref(), WIN)
                .unwrap()
                .manifest
                .version,
            "0.2.0"
        );
        let mut envelope: SignedCatalog = serde_json::from_slice(&signed).unwrap();
        envelope.payload = base64::engine::general_purpose::STANDARD.encode(b"altered bytes");
        assert!(
            catalog_for(&serde_json::to_vec(&envelope).unwrap(), key.public_key().as_ref(), WIN)
            .is_err()
        );
        let mut incompatible = payload;
        incompatible["protocolVersion"] = serde_json::json!(2);
        assert!(catalog_for(&sign(&incompatible), key.public_key().as_ref(), WIN).is_err());
        incompatible["protocolVersion"] = serde_json::json!(1);
        incompatible["packageUrl"] =
            serde_json::json!("https://other.example/bro-rmm-windows-x86_64.zip");
        assert!(catalog_for(&sign(&incompatible), key.public_key().as_ref(), WIN).is_err());
    }

    #[test]
    fn archive_paths_cannot_escape_or_name_windows_streams() {
        for name in [
            "../voxl.exe",
            "/voxl.exe",
            "C:/voxl.exe",
            "x/../../file",
            "x\\voxl.exe",
            "voxl.exe:stream",
        ] {
            assert!(!safe_member(name), "{name}");
        }
        for name in ["voxl.exe", "./manifest.json", "licenses/GPL-2.0.txt"] {
            assert!(safe_member(name));
        }
    }

    #[test]
    fn verifies_manifest_identity_and_actual_executable_bytes() {
        use ring::signature::KeyPair;
        let root = tempfile::tempdir().unwrap();
        let dir = package_dir(root.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("voxl.exe"), b"test executable").unwrap();
        let mut manifest = Manifest {
            schema_version: 1,
            id: "bro-rmm".into(),
            version: VERSION.into(),
            protocol_version: 1,
            platform: "windows-x86_64".into(),
            executable: "voxl.exe".into(),
            sha256: crate::self_update::sha256_file(&dir.join("voxl.exe")).unwrap(),
        };
        let save = |manifest: &Manifest| {
            std::fs::write(
                dir.join("manifest.json"),
                serde_json::to_vec(manifest).unwrap(),
            )
            .unwrap()
        };
        save(&manifest);
        assert!(
            installed(root.path()).is_err(),
            "an unsigned cached package must be refused"
        );
        let random = ring::rand::SystemRandom::new();
        let key = ring::signature::Ed25519KeyPair::from_pkcs8(
            ring::signature::Ed25519KeyPair::generate_pkcs8(&random)
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let mut release = serde_json::to_value(&manifest).unwrap();
        release["packageUrl"] = serde_json::json!(
            "https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-v0.1.0/bro-rmm-windows-x86_64.zip"
        );
        release["packageSha256"] = serde_json::json!("a".repeat(64));
        let payload = serde_json::to_vec(&release).unwrap();
        let envelope = SignedCatalog {
            payload: base64::engine::general_purpose::STANDARD.encode(&payload),
            signature: base64::engine::general_purpose::STANDARD
                .encode(key.sign(&payload).as_ref()),
        };
        std::fs::write(
            dir.join("catalog.json"),
            serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
        assert!(validate_for(&dir, key.public_key().as_ref(), WIN).is_ok());
        std::fs::write(dir.join("voxl.exe"), b"modified").unwrap();
        assert!(validate_for(&dir, key.public_key().as_ref(), WIN).is_err());
        manifest.executable = "../voxl.exe".into();
        save(&manifest);
        assert!(validate_for(&dir, key.public_key().as_ref(), WIN).is_err());
    }

    fn signed(payload: &serde_json::Value) -> (Vec<u8>, Vec<u8>) {
        use ring::signature::KeyPair;
        let random = ring::rand::SystemRandom::new();
        let key = ring::signature::Ed25519KeyPair::from_pkcs8(ring::signature::Ed25519KeyPair::generate_pkcs8(&random).unwrap().as_ref()).unwrap();
        let bytes = serde_json::to_vec(payload).unwrap();
        let envelope = serde_json::to_vec(&SignedCatalog {
            payload: base64::engine::general_purpose::STANDARD.encode(&bytes),
            signature: base64::engine::general_purpose::STANDARD.encode(key.sign(&bytes).as_ref()),
        })
        .unwrap();
        (envelope, key.public_key().as_ref().to_vec())
    }

    fn release_for(p: Platform, url_platform: &str, archive: &str, executable: &str) -> serde_json::Value {
        serde_json::json!({"schemaVersion":1,"id":"bro-rmm","version":"0.3.0","protocolVersion":1,
            "platform":p.id,"executable":executable,"sha256":"a".repeat(64),
            "packageUrl":format!("https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-v0.3.0/bro-rmm-{url_platform}.{archive}"),
            "packageSha256":"b".repeat(64)})
    }

    #[test]
    fn every_desktop_system_has_its_package_catalog_and_program() {
        let id = |os, arch| platform_for(os, arch).map(|p| p.id);
        assert_eq!(id("windows", "x86_64"), Some("windows-x86_64"));
        assert_eq!(id("windows", "aarch64"), Some("windows-aarch64"));
        assert_eq!(id("macos", "aarch64"), Some("macos-aarch64"));
        assert_eq!(id("macos", "x86_64"), Some("macos-x86_64"));
        assert_eq!(id("linux", "x86_64"), Some("linux-x86_64"));
        assert_eq!(id("linux", "aarch64"), Some("linux-aarch64"));
        assert_eq!(id("freebsd", "x86_64"), None);
        assert!(supported(), "this machine's own system has a package");
        // Windows x64 keeps the catalog name Bros already released read
        assert_eq!(WIN.catalog_url(), CATALOG_URL);
        let mac = platform_for("macos", "aarch64").unwrap();
        assert_eq!(mac.catalog_url(), "https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-channel-v1/catalog-macos-aarch64.json");
        assert_eq!(mac.package_name(), "bro-rmm-macos-aarch64.tar.gz");
        assert_eq!(WIN.package_name(), "bro-rmm-windows-x86_64.zip");
        for p in PLATFORMS {
            assert_eq!(p.executable == "voxl.exe", p.id.starts_with("windows"), "{p:?}");
            assert_eq!(p.archive == "zip", p.id.starts_with("windows"), "{p:?}");
        }
    }

    #[test]
    fn a_catalog_only_installs_on_the_system_it_was_signed_for() {
        let mac = platform_for("macos", "aarch64").unwrap();
        let linux = platform_for("linux", "x86_64").unwrap();
        let (bytes, key) = signed(&release_for(mac, "macos-aarch64", "tar.gz", "voxl"));
        assert_eq!(catalog_for(&bytes, &key, mac).unwrap().manifest.platform, "macos-aarch64");
        // another system's package, even signed, is refused
        assert!(catalog_for(&bytes, &key, linux).is_err());
        assert!(catalog_for(&bytes, &key, WIN).is_err());
        // the right platform with the wrong program, archive or download
        for wrong in [
            release_for(mac, "macos-aarch64", "tar.gz", "voxl.exe"),
            release_for(mac, "macos-aarch64", "zip", "voxl"),
            release_for(mac, "linux-x86_64", "tar.gz", "voxl"),
        ] {
            let (bytes, key) = signed(&wrong);
            assert!(catalog_for(&bytes, &key, mac).is_err(), "{wrong}");
        }
        let (bytes, key) = signed(&release_for(linux, "linux-x86_64", "tar.gz", "voxl"));
        assert!(catalog_for(&bytes, &key, linux).is_ok());
    }

    #[test]
    fn linux_says_why_its_desktop_cant_be_shared() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
        assert!(linux_session_problem(&env(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "x11")])).is_none());
        let wayland = linux_session_problem(&env(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "wayland")])).unwrap();
        assert!(wayland.contains("Xorg"), "{wayland}");
        assert!(linux_session_problem(&env(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")])).is_some());
        let headless = linux_session_problem(&env(&[])).unwrap();
        assert!(headless.contains("SSH"), "{headless}");
    }

    #[test]
    fn a_fresh_process_reports_the_macs_permissions_in_one_line() {
        let both = MacPermissions { screen: true, accessibility: false };
        assert_eq!(parse_permissions(&permissions_line(both)), Some(both));
        assert_eq!(parse_permissions("screen=0 accessibility=1"), Some(MacPermissions { screen: false, accessibility: true }));
        assert_eq!(parse_permissions("unknown argument"), None);
        assert_eq!(parse_permissions("screen=1"), None);
    }

    #[test]
    fn control_lock_is_exclusive_and_released_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let first = lock(root.path(), "control.lock").unwrap();
        assert!(lock(root.path(), "control.lock").is_err());
        drop(first);
        assert!(lock(root.path(), "control.lock").is_ok());
    }

    #[test]
    fn old_settings_do_not_grant_desktop_access() {
        let settings: crate::config::Settings = toml::from_str("theme = 'graphite'").unwrap();
        assert!(!settings.mobile_companion.enabled);
    }
}
