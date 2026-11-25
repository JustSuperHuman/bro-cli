//! JustBrowseMCP (github.com/JustSuperHuman/mcp-chrome-but-good): your own signed-in Chrome, Edge or Brave as an
//! MCP server (`justbrowse`, `http://127.0.0.1:12306/mcp`), each agent session in tabs of its own.
//!
//! Bro's Quick Install downloads, verifies and extracts the extension and native bridge into
//! `%LOCALAPPDATA%\JustBrowseMCP` (macOS `~/Library/Application Support/JustBrowseMCP`, Linux
//! `~/.local/share/justbrowse-mcp`), registers the native host and supplies the complete `justbrowse-browser`
//! skill and session-scoped MCP config to Claude Code and Codex. Loading the unpacked extension is the one step
//! left to you: browsers only take one by hand.
//!
//! A bridge registered some other way (a source checkout, the upstream installer, hangwin/mcp-chrome) counts
//! too: [`installed`] follows the browser's native-host manifest to it, so its sessions are wired the same way.
use crate::util::{env_path, home_dir, read_json, same_path};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use anyhow::{Context, bail};

pub const REPO: &str = "JustSuperHuman/mcp-chrome-but-good";
/// The released extension's ID (its manifest key fixes it wherever it's unpacked).
pub const EXTENSION_ID: &str = "dgpjcgoidnfedcpaijeelljbppcfhcll";
/// The MCP server's name and its Streamable HTTP endpoint.
pub const SERVER_NAME: &str = "justbrowse";
pub const MCP_URL: &str = "http://127.0.0.1:12306/mcp";

/// Where the installer puts it (override `JUSTBROWSE_DIR`).
pub fn install_dir() -> PathBuf {
    if let Some(dir) = env_path("JUSTBROWSE_DIR") {
        return dir;
    }
    if cfg!(windows) {
        env_path("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local")).join("JustBrowseMCP")
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library").join("Application Support").join("JustBrowseMCP")
    } else {
        env_path("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local").join("share")).join("justbrowse-mcp")
    }
}

/// The unpacked extension to load in the browser.
pub fn extension_dir() -> PathBuf {
    install_dir().join("extension")
}

/// The shell command that installs (or updates) it: the newest release, checked against its SHA-256.
pub fn installer_command() -> String {
    if cfg!(windows) {
        format!("irm https://raw.githubusercontent.com/{REPO}/master/install.ps1 | iex")
    } else {
        format!("curl -fsSL https://raw.githubusercontent.com/{REPO}/master/install.sh | bash")
    }
}

/// The native-messaging host the extension starts (JustBrowseMCP keeps hangwin/mcp-chrome's name).
pub const HOST_NAME: &str = "com.chromemcp.nativehost";

/// An installed JustBrowseMCP: bro's Quick Install, or a bridge the browser was pointed at some other way.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub version: Option<String>,
    /// [`install_dir`] for Quick Install; otherwise the root of the checkout the bridge runs from (the
    /// folder holding its `skills/`), or the bridge's own folder when it ships no skill.
    pub dir: PathBuf,
    /// The `justbrowse-browser` skill it ships, if any.
    pub skill: Option<PathBuf>,
}

impl Installed {
    /// Bro's own Quick Install, rather than a bridge registered some other way.
    pub fn quick_install(&self) -> bool {
        same_path(&self.dir, &install_dir())
    }
}

/// What's installed: Quick Install first, then any bridge registered with the browser. File reads only,
/// cheap enough for every launch; no network.
pub fn installed() -> Option<Installed> {
    quick_installed().or_else(registered)
}

fn shipped_skill(root: &Path) -> Option<PathBuf> {
    Some(root.join("skills").join(crate::tool_skills::BROWSER)).filter(|s| s.join("SKILL.md").is_file())
}

fn quick_installed() -> Option<Installed> {
    let dir = install_dir();
    if !dir.join("extension/manifest.json").is_file() || !dir.join("bridge/dist/cli.js").is_file() {
        return None;
    }
    let version = read_json(&dir.join("installed.json")).and_then(|v| v.get("version")?.as_str().map(str::to_string));
    Some(Installed { version, skill: shipped_skill(&dir), dir })
}

/// Where the bridge's `register` command writes the per-user host manifest. On Windows the browsers find
/// it through the registry (Edge and Brave share Chrome's); elsewhere each browser looks in its own folder.
fn host_manifests() -> Vec<PathBuf> {
    let home = home_dir();
    let browsers: Vec<PathBuf> = if cfg!(windows) {
        let roaming = env_path("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
        ["Google/Chrome", "Chromium", "Microsoft/Edge", "BraveSoftware/Brave-Browser"].iter().map(|b| roaming.join(b)).collect()
    } else if cfg!(target_os = "macos") {
        let support = home.join("Library").join("Application Support");
        ["Google/Chrome", "Chromium", "Microsoft Edge", "BraveSoftware/Brave-Browser"].iter().map(|b| support.join(b)).collect()
    } else {
        let config = home.join(".config");
        ["google-chrome", "chromium", "microsoft-edge", "BraveSoftware/Brave-Browser"].iter().map(|b| config.join(b)).collect()
    };
    browsers.into_iter().map(|b| b.join("NativeMessagingHosts").join(format!("{HOST_NAME}.json"))).collect()
}

/// A bridge registered outside Quick Install, found through its host manifest. A manifest whose
/// host script is gone is a leftover registration and doesn't count.
fn registered() -> Option<Installed> {
    host_manifests().into_iter().find_map(|manifest| {
        let host = PathBuf::from(read_json(&manifest)?.get("path")?.as_str()?);
        let host = if host.is_relative() { manifest.parent()?.join(host) } else { host };
        if !host.is_file() {
            return None;
        }
        // run_host.{bat,sh} lives in the bridge's dist/, beside its package.json.
        let bridge = host.parent()?.parent()?;
        let version = read_json(&bridge.join("package.json")).and_then(|v| v.get("version")?.as_str().map(str::to_string));
        // A checkout keeps the skill at its root, a few folders above the bridge.
        let root = host.ancestors().skip(1).take(5).find(|d| shipped_skill(d).is_some());
        Some(Installed { version, skill: root.and_then(shipped_skill), dir: root.unwrap_or(bridge).to_path_buf() })
    })
}

/// The folder Quick Install's "Open extension folder" shows: the extracted extension, or the checkout
/// a bridge registered some other way runs from.
pub fn shown_folder() -> PathBuf {
    match installed() {
        Some(found) if !found.quick_install() => found.dir,
        _ => extension_dir(),
    }
}

/// The one manual browser step, kept visible in Quick Install even after installation.
pub fn after_install() -> String {
    let wired = "Bro manages its skill and connects new Claude and Codex sessions automatically.";
    if let Some(found) = installed().filter(|f| !f.quick_install()) {
        return format!(
            "Your browser already runs the JustBrowse bridge from:\n{}\nKeep the extension you loaded from there, then open it and click Connect.\n{wired}\nInstalling from here registers Bro's release copy in place of that bridge.",
            found.dir.display()
        );
    }
    format!(
        "1. Open chrome://extensions or edge://extensions.\n2. Turn on Developer mode, choose Load unpacked, and select:\n{}\n3. Open the JustBrowseMCP extension and click Connect. After an update, click Reload first.\n{wired}",
        extension_dir().display()
    )
}

/// Local readiness is separate from installation: an installed bridge can have no browser attached.
pub fn connected() -> Result<bool, String> {
    let reply = crate::http::get_json("http://127.0.0.1:12306/ping", &[], Duration::from_secs(2)).map_err(|_| "The JustBrowse bridge is not responding. Open the extension and click Connect.".to_string())?;
    match reply.body.get("extensionConnected").and_then(|v| v.as_bool()) {
        Some(on) if reply.ok() || reply.status == 503 => Ok(on),
        _ => Err("The local bridge did not report its browser connection. Reload JustBrowseMCP and try again.".into()),
    }
}

/// A launch-scoped MCP configuration: no global settings are overwritten and pooled logins work too.
pub fn mcp_config(home: &Path) -> std::io::Result<PathBuf> {
    let file = home.join("bro-browser/justbrowse-mcp.json");
    let body = serde_json::json!({"mcpServers": {SERVER_NAME: {"type": "http", "url": MCP_URL,
        "headers": {"x-bro-session-id": "${BRO_SESSION_ID}"}}}});
    crate::util::write_json_pretty(&file, &body)?;
    Ok(file)
}

fn run(program: &str, args: Vec<String>) -> anyhow::Result<()> {
    let (program, args) = crate::launch::program_and_args(program, args);
    let status = Command::new(&program).args(args).status().with_context(|| format!("starting {program}"))?;
    if !status.success() {
        bail!("{program} failed ({status}); disconnect the extension before retrying an update");
    }
    Ok(())
}

fn safe_entry(entry: &str) -> bool {
    !entry.is_empty() && !entry.starts_with('/') && !entry.contains(['\\', ':']) && !entry.split('/').any(|p| p == "..")
}

fn asset<'a>(release: &'a serde_json::Value, matches: impl Fn(&str) -> bool) -> anyhow::Result<(&'a str, &'a str)> {
    release.get("assets").and_then(|a| a.as_array()).into_iter().flatten().find_map(|a| {
        let name = a.get("name")?.as_str()?;
        (safe_entry(name) && !name.contains('/') && matches(name)).then_some((name, a.get("browser_download_url")?.as_str()?))
    }).context("the JustBrowse release is missing its archive or SHA256SUMS")
}

/// Download, verify and extract a release ourselves so the upstream installer cannot overwrite a
/// personal skill or change global agent settings. Only the bridge's documented registration command
/// runs; downloaded installer scripts are never executed. Blocking; call from the install terminal.
pub fn install(mut say: impl FnMut(&str)) -> anyhow::Result<()> {
    let node = crate::launch::which("node").context("JustBrowseMCP needs Node.js 20+: run `bro install node` first")?;
    let version = Command::new(node).arg("--version").output().context("checking Node.js")?;
    let major = String::from_utf8_lossy(&version.stdout).trim().trim_start_matches('v').split('.').next().and_then(|v| v.parse::<u32>().ok());
    if !version.status.success() || !major.is_some_and(|v| v >= 20) {
        bail!("JustBrowseMCP needs Node.js 20 or newer: run `bro install node --force`");
    }
    if crate::launch::which("npm").is_none() {
        bail!("JustBrowseMCP needs npm: run `bro install node` first");
    }
    let tag = crate::util::env_str("JUSTBROWSE_VERSION");
    let api = match tag {
        Some(tag) => format!("https://api.github.com/repos/{REPO}/releases/tags/{}", url::form_urlencoded::byte_serialize(tag.as_bytes()).collect::<String>()),
        None => format!("https://api.github.com/repos/{REPO}/releases/latest"),
    };
    let release = crate::http::get_json(&api, &[("user-agent", "bro")], Duration::from_secs(30))?;
    if !release.ok() {
        bail!("GitHub answered HTTP {}", release.status);
    }
    let suffix = if cfg!(windows) { ".zip" } else { ".tar.gz" };
    let (name, download) = asset(&release.body, |n| n.starts_with("justbrowse-") && n.ends_with(suffix))?;
    let (_, sums_url) = asset(&release.body, |n| n == "SHA256SUMS")?;
    let tmp = std::env::temp_dir().join(format!("bro-justbrowse-{}-{:x}", std::process::id(), rand::random::<u64>()));
    std::fs::create_dir(&tmp)?;
    let result = (|| {
        // Downloads and extraction each get their own fresh directory.
        let downloads = tmp.join("downloads");
        let unpacked = tmp.join("unpacked");
        std::fs::create_dir(&downloads)?;
        std::fs::create_dir(&unpacked)?;
        say("Downloading JustBrowseMCP and checking SHA256SUMS…");
        let archive = downloads.join(name);
        let sums = downloads.join("SHA256SUMS");
        crate::http::download(download, &archive, Duration::from_secs(300))?;
        crate::http::download(sums_url, &sums, Duration::from_secs(30))?;
        let expected = crate::self_update::sum_for(&std::fs::read_to_string(sums)?, name).context("SHA256SUMS has no matching archive")?;
        if expected.len() != 64 || crate::self_update::sha256_file(&archive)? != expected {
            bail!("JustBrowseMCP's archive failed its SHA-256 check; nothing was installed");
        }
        let listed = Command::new(crate::self_update::tar()).arg("-tf").arg(&archive).output()?;
        if !listed.status.success() || !String::from_utf8_lossy(&listed.stdout).lines().all(safe_entry) {
            bail!("the JustBrowse archive contains an invalid path");
        }
        say("Extracting the unpacked browser extension…");
        let status = Command::new(crate::self_update::tar()).arg("-xf").arg(&archive).arg("-C").arg(&unpacked).status()?;
        if !status.success() {
            bail!("extracting JustBrowseMCP failed ({status})");
        }
        let bundle = std::fs::read_dir(&unpacked)?.filter_map(Result::ok).map(|e| e.path()).find(|p| p.join("bridge/package.json").is_file()).context("the archive has no JustBrowse release bundle")?;
        let info = read_json(&bundle.join("release.json")).context("the bundle has no release.json")?;
        if !bundle.join("extension/manifest.json").is_file() || !bundle.join("bridge/dist/cli.js").is_file() || !bundle.join("skills/justbrowse-browser/SKILL.md").is_file() {
            bail!("the JustBrowse release bundle is incomplete");
        }
        let root = install_dir();
        std::fs::create_dir_all(&root)?;
        let bridge = root.join("bridge");
        crate::skill_repos::mirror(&bundle.join("bridge"), &bridge, true)?;
        say("Installing the native bridge and registering Chrome, Edge and Brave…");
        run("npm", vec!["install".into(), "--prefix".into(), bridge.to_string_lossy().into_owned(), "--omit=dev".into(), "--install-links".into(), "--no-audit".into(), "--no-fund".into()])?;
        run("node", vec![bridge.join("dist/cli.js").to_string_lossy().into_owned(), "register".into(), "--browser".into(), "all".into()])?;
        // Older bridges register Chrome's manifest but omit the Chromium-family registry aliases.
        #[cfg(windows)]
        for browser in ["Microsoft\\Edge", "BraveSoftware\\Brave-Browser"] {
            let manifest = env_path("APPDATA").context("APPDATA is not set")?.join(format!("Google/Chrome/NativeMessagingHosts/{HOST_NAME}.json"));
            run("reg", vec!["add".into(), format!("HKCU\\Software\\{browser}\\NativeMessagingHosts\\com.chromemcp.nativehost"), "/ve".into(), "/t".into(), "REG_SZ".into(), "/d".into(), manifest.to_string_lossy().into_owned(), "/f".into()])?;
        }
        crate::skill_repos::mirror(&bundle.join("extension"), &extension_dir(), true)?;
        crate::skill_repos::mirror(&bundle.join("skills"), &root.join("skills"), true)?;
        crate::util::write_json_pretty(&root.join("installed.json"), &info)?;
        say("Keeping justbrowse-browser in Bro's managed skills…");
        crate::tool_skills::refresh_browser().map_err(anyhow::Error::msg)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn installed_reads_the_installers_record() {
        let sb = sandbox();
        // SAFETY: the sandbox holds the env lock
        unsafe { std::env::set_var("JUSTBROWSE_DIR", sb.home().join("jb")) };
        assert_eq!(installed(), None);
        std::fs::create_dir_all(install_dir()).unwrap();
        std::fs::write(install_dir().join("installed.json"), r#"{"version":"1.1.0","extensionId":"dgpjcgoidnfedcpaijeelljbppcfhcll"}"#).unwrap();
        assert_eq!(installed(), None, "a stale installer record does not count as an extracted extension");
        crate::util::atomic_write(&extension_dir().join("manifest.json"), b"{}").unwrap();
        crate::util::atomic_write(&install_dir().join("bridge/dist/cli.js"), b"// bridge").unwrap();
        assert_eq!(installed(), Some(Installed { version: Some("1.1.0".into()), dir: install_dir(), skill: None }));
        assert!(installed().unwrap().quick_install());
        let config = mcp_config(&sb.home().join("profile")).unwrap();
        assert_eq!(read_json(&config).unwrap()["mcpServers"][SERVER_NAME]["url"], MCP_URL);
        assert!(["../escape", "/absolute", "C:/drive", "dir/../escape", "dir\\\\file"].into_iter().all(|p| !safe_entry(p)));
        assert!(safe_entry("justbrowse-1.1.0/extension/manifest.json"));
        assert!(after_install().contains("Load unpacked") && extension_dir().ends_with("extension"));
        assert!(installer_command().contains(REPO));
    }

    /// A checkout laid out like JustBrowseMCP's repo, registered the way its bridge's `register` does it.
    fn register_checkout(root: &Path, host: &Path) {
        crate::util::atomic_write(&root.join("skills/justbrowse-browser/SKILL.md"), b"---\nname: justbrowse-browser\n---\nfrom the checkout").unwrap();
        crate::util::atomic_write(&root.join("app/native-server/package.json"), br#"{"name":"justbrowse-bridge","version":"1.1.2"}"#).unwrap();
        crate::util::atomic_write(&root.join("app/native-server/dist/run_host.bat"), b"@echo off").unwrap();
        let manifest = serde_json::json!({"name": HOST_NAME, "path": host, "type": "stdio"});
        crate::util::write_json_pretty(&host_manifests()[0], &manifest).unwrap();
    }

    #[test]
    fn a_bridge_registered_outside_quick_install_counts() {
        let sb = sandbox();
        let root = sb.home().join("src/JustBrowseMCP");
        let host = root.join("app/native-server/dist/run_host.bat");
        register_checkout(&root, &host);
        let found = installed().expect("a registered checkout is installed");
        assert_eq!(found, Installed { version: Some("1.1.2".into()), dir: root.clone(), skill: Some(root.join("skills/justbrowse-browser")) });
        assert!(!found.quick_install());
        assert_eq!(shown_folder(), root);
        assert!(after_install().contains(&root.display().to_string()) && after_install().contains("in place of that bridge"));

        // The upstream bridge from npm ships no skill: its own package folder stands in for the root.
        let npm = sb.home().join("npm/node_modules/mcp-chrome-bridge");
        crate::util::atomic_write(&npm.join("package.json"), br#"{"version":"1.0.31"}"#).unwrap();
        crate::util::atomic_write(&npm.join("dist/run_host.bat"), b"@echo off").unwrap();
        crate::util::write_json_pretty(&host_manifests()[0], &serde_json::json!({"path": npm.join("dist/run_host.bat")})).unwrap();
        assert_eq!(installed(), Some(Installed { version: Some("1.0.31".into()), dir: npm.clone(), skill: None }));

        // A registration whose host script was deleted is a leftover.
        std::fs::remove_file(npm.join("dist/run_host.bat")).unwrap();
        assert_eq!(installed(), None);

        // Quick Install wins over another registration.
        register_checkout(&root, &host);
        crate::util::atomic_write(&extension_dir().join("manifest.json"), b"{}").unwrap();
        crate::util::atomic_write(&install_dir().join("bridge/dist/cli.js"), b"// bridge").unwrap();
        assert!(installed().unwrap().quick_install());
        assert_eq!(shown_folder(), extension_dir());
    }

    #[test]
    fn the_browser_skill_comes_from_the_checkout_it_runs() {
        let sb = sandbox();
        let root = sb.home().join("src/JustBrowseMCP");
        register_checkout(&root, &root.join("app/native-server/dist/run_host.bat"));
        let codex = crate::paths::codex_local_dir();
        std::fs::create_dir_all(&codex).unwrap();
        crate::tool_skills::refresh_browser().expect("no network needed for a shipped skill");
        let copy = codex.join("skills/justbrowse-browser");
        assert_eq!(std::fs::read_to_string(copy.join("SKILL.md")).unwrap(), "---\nname: justbrowse-browser\n---\nfrom the checkout");
        assert!(copy.join(".bro-managed").is_file());
    }
}
