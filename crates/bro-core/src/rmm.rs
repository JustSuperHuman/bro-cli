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
pub const CATALOG_URL: &str =
    "https://github.com/JustSuperHuman/voxl/releases/download/bro-rmm-channel-v1/catalog.json";
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

fn catalog(bytes: &[u8], public_key: &[u8]) -> anyhow::Result<Release> {
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
        || manifest.platform != "windows-x86_64"
        || manifest.executable != "voxl.exe"
        || manifest.version.is_empty()
        || manifest.version.len() > 64
        || !manifest
            .version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        || !valid_hash(&manifest.sha256)
        || !valid_hash(&release.package_sha256)
        || !release
            .package_url
            .starts_with("https://github.com/JustSuperHuman/voxl/releases/download/")
        || !release.package_url.ends_with("/bro-rmm-windows-x86_64.zip")
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

/// Network failure uses the already verified installed package.
/// An answered but invalid catalog is an error, never a reason to trust different bytes.
fn fetch_catalog() -> anyhow::Result<Option<(Release, Vec<u8>)>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let Ok(response) = agent
        .get(CATALOG_URL)
        .header("user-agent", "bro-rmm-installer")
        .call()
    else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_CATALOG + 1)
        .read_to_end(&mut bytes)?;
    Ok(Some((catalog(&bytes, &catalog_key()?)?, bytes)))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    /// All viewers are restricted at the gateway and the media transport.
    pub read_only: bool,
    /// Voxl's Windows display index; 0 is the primary display.
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

pub fn supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

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
    #[cfg(not(windows))]
    {
        1
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
    let file =
        File::open(dir.join("manifest.json")).context("Remote control is not installed yet.")?;
    let manifest: Manifest = serde_json::from_reader(file.take(16 * 1024))
        .context("The remote-control package manifest is invalid.")?;
    let mut bytes = Vec::new();
    File::open(dir.join("catalog.json"))
        .context("The installed remote-control signature is missing. Choose Repair download.")?
        .take(MAX_CATALOG + 1)
        .read_to_end(&mut bytes)?;
    let release = catalog(&bytes, key)?;
    if manifest.schema_version != 1
        || manifest.id != "bro-rmm"
        || manifest.version != release.manifest.version
        || release.manifest.sha256 != manifest.sha256
        || manifest.protocol_version != 1
        || manifest.platform != "windows-x86_64"
        || manifest.executable != "voxl.exe"
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
pub fn install(root: &Path) -> anyhow::Result<Manifest> {
    if !supported() {
        bail!("Remote control is currently available on Windows x64.");
    }
    let _lock = lock(root, "install.lock")?;
    let current = installed(root).ok();
    let update = fetch_catalog()?;
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
        let archive = staging.join("package.zip");
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
        let (_, manifest) = validate(&unpacked)?;
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
            catalog(&signed, key.public_key().as_ref())
                .unwrap()
                .manifest
                .version,
            "0.2.0"
        );
        let mut envelope: SignedCatalog = serde_json::from_slice(&signed).unwrap();
        envelope.payload = base64::engine::general_purpose::STANDARD.encode(b"altered bytes");
        assert!(
            catalog(
                &serde_json::to_vec(&envelope).unwrap(),
                key.public_key().as_ref()
            )
            .is_err()
        );
        let mut incompatible = payload;
        incompatible["protocolVersion"] = serde_json::json!(2);
        assert!(catalog(&sign(&incompatible), key.public_key().as_ref()).is_err());
        incompatible["protocolVersion"] = serde_json::json!(1);
        incompatible["packageUrl"] =
            serde_json::json!("https://other.example/bro-rmm-windows-x86_64.zip");
        assert!(catalog(&sign(&incompatible), key.public_key().as_ref()).is_err());
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
        assert!(validate_with_key(&dir, key.public_key().as_ref()).is_ok());
        std::fs::write(dir.join("voxl.exe"), b"modified").unwrap();
        assert!(validate_with_key(&dir, key.public_key().as_ref()).is_err());
        manifest.executable = "../voxl.exe".into();
        save(&manifest);
        assert!(validate_with_key(&dir, key.public_key().as_ref()).is_err());
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
