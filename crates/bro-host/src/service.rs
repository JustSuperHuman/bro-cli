//! Starting the session server when you sign in, registered for your user only (no admin): a startup app on
//! Windows (`HKCU\…\Run`, listed in Task Manager › Startup apps), a LaunchAgent on macOS, a systemd user unit on
//! Linux. A server for the usual bro folder then stays running when idle instead of stopping (see [`resident`]),
//! whether sign-in or bro started it.
//!
//! Deliberately not a system service: one would run outside your desktop (no browser for a login, no windows,
//! no credential store for the sessions it starts), and a reboot ends every session anyway, so there is nothing
//! for it to do before you sign in.

use std::path::Path;

/// Whether bro starts its session server when you sign in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Startup {
    Off,
    /// Registered: `command` runs when you sign in.
    On { command: String },
    /// Registered, but turned off outside bro (Task Manager › Startup apps, `systemctl --user disable`).
    Disabled { command: String },
}

impl Startup {
    pub fn on(&self) -> bool {
        matches!(self, Startup::On { .. })
    }

    /// What runs at sign-in, when registered.
    pub fn command(&self) -> Option<&str> {
        match self {
            Startup::Off => None,
            Startup::On { command } | Startup::Disabled { command } => Some(command),
        }
    }
}

/// What's registered now. Quick (a registry value or a file or two).
pub fn startup() -> Startup {
    os::startup()
}

/// Run `program args` when you sign in, replacing what was registered (and turning it back on if it was turned
/// off outside bro). It takes effect at the next sign-in; it doesn't start anything now.
pub fn install(program: &Path, args: &[&str]) -> anyhow::Result<()> {
    os::install(program, args)
}

/// Stop starting it at sign-in. A server already running keeps running (and stops once idle, as usual).
pub fn uninstall() -> anyhow::Result<()> {
    os::uninstall()
}

/// The server for bro folder `dir` stays running when idle: bro starts it when you sign in. Only for the usual
/// folder (`~/.bro`), the one sign-in starts it for; a bro pointed elsewhere (`BRO_DIR`, as tests are) never keeps
/// a server around.
pub fn resident(dir: &Path) -> bool {
    same_dir(dir, &bro_core::util::home_dir().join(".bro")) && startup().on()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The program and its arguments as one line, for showing (and for Windows' Run key, which takes exactly this).
fn command_line(program: &Path, args: &[&str]) -> String {
    std::iter::once(format!("\"{}\"", program.display())).chain(args.iter().map(|a| a.to_string())).collect::<Vec<_>>().join(" ")
}

#[cfg(windows)]
mod os {
    use super::{Startup, command_line};
    use anyhow::bail;
    use std::path::Path;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW};

    /// Where the entry lives, under `HKCU`.
    pub(super) struct Keys {
        pub run: String,
        /// Where Task Manager keeps the startup apps it turned off.
        pub approved: String,
    }

    fn keys() -> Keys {
        let at = r"Software\Microsoft\Windows\CurrentVersion";
        Keys { run: format!(r"{at}\Run"), approved: format!(r"{at}\Explorer\StartupApproved\Run") }
    }

    const NAME: &str = "bro session server";
    const ERROR_FILE_NOT_FOUND: u32 = 2;

    pub fn startup() -> Startup {
        startup_at(&keys())
    }

    pub fn install(program: &Path, args: &[&str]) -> anyhow::Result<()> {
        install_at(&keys(), program, args)
    }

    pub fn uninstall() -> anyhow::Result<()> {
        uninstall_at(&keys())
    }

    pub(super) fn startup_at(keys: &Keys) -> Startup {
        let Some(bytes) = get(&keys.run, RRF_RT_REG_SZ) else { return Startup::Off };
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        let command = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
        // an odd first byte is "turned off"
        match get(&keys.approved, RRF_RT_REG_BINARY) {
            Some(b) if b.first().is_some_and(|x| x & 1 == 1) => Startup::Disabled { command },
            _ => Startup::On { command },
        }
    }

    pub(super) fn install_at(keys: &Keys, program: &Path, args: &[&str]) -> anyhow::Result<()> {
        let data: Vec<u8> = wide(&command_line(program, args)).iter().flat_map(|u| u.to_le_bytes()).collect();
        let rc = set(&keys.run, REG_SZ, &data);
        if rc != 0 {
            bail!("couldn't add bro to your startup apps (error {rc})");
        }
        // on again, if Task Manager had turned it off
        delete(&keys.approved);
        Ok(())
    }

    pub(super) fn uninstall_at(keys: &Keys) -> anyhow::Result<()> {
        delete(&keys.approved);
        match delete(&keys.run) {
            0 | ERROR_FILE_NOT_FOUND => Ok(()),
            rc => bail!("couldn't remove bro from your startup apps (error {rc})"),
        }
    }

    /// Set our value under `HKCU\sub`, creating the key.
    pub(super) fn set(sub: &str, kind: u32, data: &[u8]) -> u32 {
        let (sub, name) = (wide(sub), wide(NAME));
        // SAFETY: NUL-terminated names and a buffer of `data.len()` bytes, all outliving the call.
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr(), kind, data.as_ptr().cast(), data.len() as u32) }
    }

    /// Our value under `HKCU\sub`, as raw bytes.
    fn get(sub: &str, kind: u32) -> Option<Vec<u8>> {
        let (sub, name) = (wide(sub), wide(NAME));
        let mut len = 0u32;
        // SAFETY: a size query (no buffer) with NUL-terminated names that outlive the call.
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr(), kind, std::ptr::null_mut(), std::ptr::null_mut(), &mut len) };
        if rc != 0 {
            return None;
        }
        let mut data = vec![0u8; len as usize];
        // SAFETY: the buffer is `len` bytes, as the call is told.
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr(), kind, std::ptr::null_mut(), data.as_mut_ptr().cast(), &mut len) };
        (rc == 0).then(|| {
            data.truncate(len as usize);
            data
        })
    }

    fn delete(sub: &str) -> u32 {
        let (sub, name) = (wide(sub), wide(NAME));
        // SAFETY: NUL-terminated names that outlive the call.
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr()) }
    }

    pub(super) fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::{Startup, launchd};
    use std::path::Path;

    pub fn startup() -> Startup {
        let home = bro_core::util::home_dir();
        match std::fs::read_to_string(launchd::file(&home)).ok().as_deref().and_then(launchd::command) {
            Some(command) => Startup::On { command },
            None => Startup::Off,
        }
    }

    pub fn install(program: &Path, args: &[&str]) -> anyhow::Result<()> {
        // launchd reads it when you next sign in
        bro_core::util::atomic_write(&launchd::file(&bro_core::util::home_dir()), launchd::render(program, args).as_bytes())?;
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        // left loaded until you sign out: unloading it now would stop a server it started, and its sessions
        super::remove(&launchd::file(&bro_core::util::home_dir()))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod os {
    use super::{Startup, systemd};
    use anyhow::bail;
    use std::path::Path;

    pub fn startup() -> Startup {
        let dir = systemd::dir(&bro_core::util::home_dir());
        let Some(command) = std::fs::read_to_string(dir.join(systemd::UNIT)).ok().as_deref().and_then(systemd::command) else { return Startup::Off };
        // enabled is the link `systemctl --user enable` makes
        match dir.join(systemd::WANTS).join(systemd::UNIT).symlink_metadata() {
            Ok(_) => Startup::On { command },
            Err(_) => Startup::Disabled { command },
        }
    }

    pub fn install(program: &Path, args: &[&str]) -> anyhow::Result<()> {
        if !Path::new("/run/systemd/system").exists() {
            bail!("starting at sign-in needs systemd, and this system doesn't run it");
        }
        // the user manager reads these when you next sign in
        let dir = systemd::dir(&bro_core::util::home_dir());
        std::fs::create_dir_all(dir.join(systemd::WANTS))?;
        bro_core::util::atomic_write(&dir.join(systemd::UNIT), systemd::render(program, args).as_bytes())?;
        let link = dir.join(systemd::WANTS).join(systemd::UNIT);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(Path::new("..").join(systemd::UNIT), &link)?;
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        let dir = systemd::dir(&bro_core::util::home_dir());
        super::remove(&dir.join(systemd::WANTS).join(systemd::UNIT))?;
        super::remove(&dir.join(systemd::UNIT))
    }
}

#[cfg(not(any(windows, unix)))]
mod os {
    use super::Startup;
    use std::path::Path;

    pub fn startup() -> Startup {
        Startup::Off
    }

    pub fn install(_: &Path, _: &[&str]) -> anyhow::Result<()> {
        anyhow::bail!("bro can't start at sign-in on this system")
    }

    pub fn uninstall() -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn remove(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(anyhow::anyhow!("couldn't remove {}: {e}", path.display())),
        _ => Ok(()),
    }
}

/// macOS: a LaunchAgent that runs once at sign-in (no KeepAlive: a server that stops, or finds one already
/// running, stays stopped).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod launchd {
    use std::path::{Path, PathBuf};

    pub const LABEL: &str = "com.justsuperhuman.bro.host";

    pub fn file(home: &Path) -> PathBuf {
        home.join("Library").join("LaunchAgents").join(format!("{LABEL}.plist"))
    }

    pub fn render(program: &Path, args: &[&str]) -> String {
        let strings: String = std::iter::once(program.to_string_lossy().into_owned())
            .chain(args.iter().map(|a| a.to_string()))
            .map(|s| format!("        <string>{}</string>\n", escape(&s)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{LABEL}</string>\n    <key>ProgramArguments</key>\n    <array>\n{strings}    </array>\n    <key>RunAtLoad</key>\n    <true/>\n</dict>\n</plist>\n"
        )
    }

    /// The command a plist we wrote runs, as one line.
    pub fn command(plist: &str) -> Option<String> {
        let rest = &plist[plist.find("<key>ProgramArguments</key>")?..];
        let mut array = &rest[rest.find("<array>")?..rest.find("</array>")?];
        let mut words = vec![];
        while let Some(i) = array.find("<string>") {
            array = &array[i + "<string>".len()..];
            let end = array.find("</string>")?;
            words.push(unescape(&array[..end]));
            array = &array[end..];
        }
        let (program, args) = words.split_first()?;
        Some(super::command_line(Path::new(program), &args.iter().map(String::as_str).collect::<Vec<_>>()))
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    fn unescape(s: &str) -> String {
        s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    }
}

/// Linux: a systemd user unit, enabled for `default.target` (started with your first session, SSH included).
#[cfg_attr(any(not(unix), target_os = "macos"), allow(dead_code))]
mod systemd {
    use std::path::{Path, PathBuf};

    pub const UNIT: &str = "bro-host.service";
    pub const WANTS: &str = "default.target.wants";

    /// `~/.config/systemd/user` (or under `XDG_CONFIG_HOME`).
    pub fn dir(home: &Path) -> PathBuf {
        let config = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        config.join("systemd").join("user")
    }

    pub fn render(program: &Path, args: &[&str]) -> String {
        let exec: Vec<String> = std::iter::once(program.to_string_lossy().into_owned()).chain(args.iter().map(|a| a.to_string())).map(|w| quote(&w)).collect();
        format!(
            "[Unit]\nDescription=bro's session server (keeps your bro sessions running while bro is closed)\n\n[Service]\nExecStart={}\n\n[Install]\nWantedBy=default.target\n",
            exec.join(" ")
        )
    }

    /// What a unit's `ExecStart=` runs, as written.
    pub fn command(unit: &str) -> Option<String> {
        unit.lines().find_map(|l| l.strip_prefix("ExecStart=")).map(|c| c.replace("%%", "%"))
    }

    /// A word for `ExecStart=`: quoted, with systemd's specifier `%` doubled.
    fn quote(word: &str) -> String {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_quotes_the_program() {
        assert_eq!(command_line(Path::new(r"C:\Users\a b\Programs\bro\bro-app.exe"), &["--host"]), r#""C:\Users\a b\Programs\bro\bro-app.exe" --host"#);
    }

    #[test]
    fn a_launch_agent_reads_back_what_it_runs() {
        let plist = launchd::render(Path::new("/Applications/Bro & co.app/Contents/MacOS/bro"), &["host"]);
        assert!(plist.contains("<string>com.justsuperhuman.bro.host</string>"), "{plist}");
        assert!(plist.contains("&amp;"), "{plist}");
        assert_eq!(launchd::command(&plist).as_deref(), Some(r#""/Applications/Bro & co.app/Contents/MacOS/bro" host"#));
        assert_eq!(launchd::file(Path::new("/Users/me")), Path::new("/Users/me/Library/LaunchAgents/com.justsuperhuman.bro.host.plist"));
    }

    #[test]
    fn a_systemd_unit_quotes_its_command() {
        let unit = systemd::render(Path::new("/home/me/my bin/bro"), &["host"]);
        assert!(unit.contains("ExecStart=\"/home/me/my bin/bro\" \"host\"\n"), "{unit}");
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
        assert_eq!(systemd::command(&systemd::render(Path::new("/opt/100%/bro"), &["host"])).as_deref(), Some("\"/opt/100%/bro\" \"host\""));
    }

    #[cfg(windows)]
    #[test]
    fn a_startup_app_is_added_turned_off_outside_bro_and_removed() {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_BINARY, RegDeleteTreeW};
        // a scratch key of the test's own, not your startup apps
        let base = format!(r"Software\bro-test-{}", uuid::Uuid::new_v4().simple());
        let keys = os::Keys { run: format!(r"{base}\Run"), approved: format!(r"{base}\Approved") };
        let program = Path::new(r"C:\Users\me\AppData\Local\Programs\bro\bro-app.exe");
        assert_eq!(os::startup_at(&keys), Startup::Off);
        os::install_at(&keys, program, &["--host"]).unwrap();
        let command = format!(r#""{}" --host"#, program.display());
        assert_eq!(os::startup_at(&keys), Startup::On { command: command.clone() });
        // what Task Manager writes when you turn a startup app off
        assert_eq!(os::set(&keys.approved, REG_BINARY, &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), 0);
        assert_eq!(os::startup_at(&keys), Startup::Disabled { command });
        os::install_at(&keys, program, &["--host"]).unwrap();
        assert!(os::startup_at(&keys).on(), "installing again turns it back on");
        os::uninstall_at(&keys).unwrap();
        assert_eq!(os::startup_at(&keys), Startup::Off);
        os::uninstall_at(&keys).unwrap();
        let base = os::wide(&base);
        // SAFETY: a NUL-terminated name that outlives the call.
        assert_eq!(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, base.as_ptr()) }, 0);
    }

    #[test]
    fn only_the_usual_folder_keeps_a_server() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!resident(dir.path()), "a folder of its own never stays");
    }
}
