//! Env files for `bro --env <file|url> [--save]`: reading a dotenv file from disk or a URL,
//! encrypting one behind a password, and saving its variables where new processes find them.
//!
//! An encrypted file is text, so it can be pasted, committed or served like any other:
//!
//! ```text
//! -----BEGIN BRO ENV-----
//! argon2id m=65536 t=3 p=1
//! <base64: 16-byte salt, 24-byte nonce, XChaCha20-Poly1305 ciphertext>
//! -----END BRO ENV-----
//! ```
//!
//! The key comes from the password through Argon2id with the parameters on the second line, which
//! is also the cipher's associated data, so it can't be changed without the file failing to open.
//!
//! Saving writes the user's own environment: `HKCU\Environment` on Windows (what new processes,
//! Task Scheduler tasks and Explorer read), and on macOS and Linux `~/.config/bro/env`, sourced
//! from the shell profiles, plus systemd's `environment.d` on Linux and `launchctl setenv` on macOS
//! for services.
use crate::util::home_dir;
use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A variable and its value, in file order.
pub type Vars = Vec<(String, String)>;

pub const BEGIN: &str = "-----BEGIN BRO ENV-----";
pub const END: &str = "-----END BRO ENV-----";

/// The environment variable a password can come from instead of a prompt (scripts, CI).
pub const PASSWORD_VAR: &str = "BRO_ENV_PASSWORD";

/// Names an env file may not set: they belong to the OS or the login, and a file overriding them
/// (a downloaded one especially) could change which programs run.
const RESERVED: [&str; 15] =
    ["PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "SHELL", "USER", "USERNAME", "LOGNAME"];

fn valid_key(k: &str) -> bool {
    k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Splits `vars` into the ones bro will set and the [`RESERVED`] names it leaves alone.
pub fn usable(vars: Vars) -> (Vars, Vec<String>) {
    let (skip, keep): (Vars, Vars) = vars.into_iter().partition(|(k, _)| RESERVED.iter().any(|r| r.eq_ignore_ascii_case(k)));
    (keep, skip.into_iter().map(|(k, _)| k).collect())
}

/// `key = value` into `vars`, replacing an earlier value for the same key.
pub fn upsert(vars: &mut Vars, key: &str, value: String) {
    match vars.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => vars.push((key.to_string(), value)),
    }
}

// ---- dotenv ----

/// The byte index of the quote that closes a value opened with `q` (a backslash escapes in "…").
fn closing(s: &str, q: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if q == '"' => escaped = true,
            c if c == q => return Some(i),
            _ => {}
        }
    }
    None
}

/// `\n`, `\r`, `\t`, `\"`, `\\` and `\$` in a double-quoted value; any other backslash stays.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(c @ ('"' | '\\' | '$')) => out.push(c),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A dotenv file: `KEY=value`, `export KEY=value`, `# comments`, single quotes (literal), double
/// quotes (with escapes, may span lines) and ` # comments` after an unquoted value.
pub fn parse(text: &str) -> anyhow::Result<Vars> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vars::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        let n = n + 1;
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let l = l.strip_prefix("export ").map(str::trim_start).unwrap_or(l);
        let (key, rest) = l.split_once('=').with_context(|| format!("line {n}: expected KEY=value"))?;
        let key = key.trim();
        if !valid_key(key) {
            bail!("line {n}: '{key}' isn't a variable name");
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let mut body = rest[1..].to_string();
                let end = loop {
                    if let Some(end) = closing(&body, q) {
                        break end;
                    }
                    let Some((_, more)) = lines.next() else { bail!("line {n}: the {q} never closes") };
                    body.push('\n');
                    body.push_str(more);
                };
                let after = body[end + 1..].trim();
                if !after.is_empty() && !after.starts_with('#') {
                    bail!("line {n}: unexpected text after the closing {q}");
                }
                body.truncate(end);
                if q == '"' { unescape(&body) } else { body }
            }
            _ => {
                let cut = [" #", "\t#"].iter().filter_map(|m| rest.find(m)).min().unwrap_or(rest.len());
                rest[..cut].trim_end().to_string()
            }
        };
        upsert(&mut out, key, value);
    }
    Ok(out)
}

// ---- encryption ----

/// Argon2id's cost: memory in KiB, passes, lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kdf {
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

/// What `bro env encrypt` uses: 64 MiB, 3 passes (a fraction of a second on a laptop).
pub const KDF: Kdf = Kdf { m: 65536, t: 3, p: 1 };

impl Kdf {
    fn line(self) -> String {
        format!("argon2id m={} t={} p={}", self.m, self.t, self.p)
    }

    /// The parameters line of a file, refusing costs no real file needs (a hostile file could
    /// otherwise ask for gigabytes).
    fn parse(line: &str) -> anyhow::Result<Kdf> {
        let mut words = line.split_whitespace();
        if words.next() != Some("argon2id") {
            bail!("unknown key derivation '{line}'");
        }
        let (mut m, mut t, mut p) = (None, None, None);
        for w in words {
            let (k, v) = w.split_once('=').with_context(|| format!("bad parameter '{w}'"))?;
            let v: u32 = v.parse().with_context(|| format!("bad parameter '{w}'"))?;
            match k {
                "m" => m = Some(v),
                "t" => t = Some(v),
                "p" => p = Some(v),
                _ => bail!("unknown parameter '{w}'"),
            }
        }
        let kdf = Kdf { m: m.context("no m=")?, t: t.context("no t=")?, p: p.context("no p=")? };
        if kdf.m > 1 << 20 || kdf.t > 20 || kdf.p > 16 {
            bail!("the file asks for more work than bro allows ({line})");
        }
        Ok(kdf)
    }

    fn key(self, password: &str, salt: &[u8]) -> anyhow::Result<[u8; 32]> {
        use argon2::{Algorithm, Argon2, Params, Version};
        let params = Params::new(self.m, self.t, self.p, Some(32)).map_err(|e| anyhow::anyhow!("argon2 parameters: {e}"))?;
        let mut key = [0u8; 32];
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(password.as_bytes(), salt, &mut key).map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
        Ok(key)
    }
}

/// The file starts with [`BEGIN`].
pub fn is_encrypted(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').trim_start().starts_with(BEGIN)
}

/// `plain` behind `password`, as an armored text file.
pub fn encrypt(plain: &str, password: &str) -> anyhow::Result<String> {
    encrypt_with(plain, password, KDF)
}

pub fn encrypt_with(plain: &str, password: &str, kdf: Kdf) -> anyhow::Result<String> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    let salt: [u8; 16] = rand::random();
    let nonce: [u8; 24] = rand::random();
    let line = kdf.line();
    let cipher = XChaCha20Poly1305::new_from_slice(&kdf.key(password, &salt)?).map_err(|e| anyhow::anyhow!("{e}"))?;
    let sealed = cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: plain.as_bytes(), aad: line.as_bytes() }).map_err(|_| anyhow::anyhow!("encryption failed"))?;
    let blob = STANDARD.encode([&salt[..], &nonce[..], &sealed[..]].concat());
    let mut out = format!("{BEGIN}\n{line}\n");
    for chunk in blob.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(END);
    out.push('\n');
    Ok(out)
}

/// The plain text of an armored file. A wrong password and a changed file look the same.
pub fn decrypt(armored: &str, password: &str) -> anyhow::Result<String> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    let mut lines = armored.lines().map(str::trim).skip_while(|l| !l.ends_with(BEGIN)).skip(1).filter(|l| !l.is_empty());
    let line = lines.next().context("the encrypted file is cut short")?;
    let kdf = Kdf::parse(line)?;
    let body: String = lines.take_while(|l| *l != END).collect();
    let blob = STANDARD.decode(body).context("the encrypted file is damaged (bad base64)")?;
    if blob.len() < 16 + 24 + 16 {
        bail!("the encrypted file is cut short");
    }
    let (salt, rest) = blob.split_at(16);
    let (nonce, sealed) = rest.split_at(24);
    let cipher = XChaCha20Poly1305::new_from_slice(&kdf.key(password, salt)?).map_err(|e| anyhow::anyhow!("{e}"))?;
    let plain = cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: line.as_bytes() }).map_err(|_| anyhow::anyhow!("wrong password, or the file was changed"))?;
    String::from_utf8(plain).context("the decrypted file isn't text")
}

// ---- reading ----

/// `http://` is only for this machine: anything else could be read or changed on the way.
fn allowed_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or_default().to_string() };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

/// What a folder URL serves: `<url>/bro.env`.
pub const DEFAULT_NAME: &str = "bro.env";

/// The URLs to try, in order. A URL naming a file (its last path segment has an extension) is
/// fetched as it is. One naming a folder or a site (`https://m.justgains.com`, `…/team/`) is
/// looked in for `bro.env`, then `bro.env.enc`, then fetched as given (a gist's `…/raw`).
pub fn candidates(url: &str) -> Vec<String> {
    let (base, query) = match url.find(['?', '#']) {
        Some(i) => url.split_at(i),
        None => (url, ""),
    };
    let after_scheme = base.split_once("://").map_or(base, |(_, rest)| rest);
    let path = after_scheme.split_once('/').map_or("", |(_, p)| p);
    let last = path.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return vec![url.to_string()];
    }
    let dir = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
    let mut out = vec![format!("{dir}{DEFAULT_NAME}{query}"), format!("{dir}{DEFAULT_NAME}.enc{query}")];
    out.push(url.to_string());
    out
}

pub fn is_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

/// The text of a file, or of a URL (https, or http on this machine). Blocking.
pub fn read_source(source: &str) -> anyhow::Result<String> {
    if is_url(source) {
        if !allowed_url(source) {
            bail!("{source}: use https (plain http only works for localhost)");
        }
        let mut tried = vec![];
        for url in candidates(source) {
            let (status, body) = crate::http::get_text(&url, Duration::from_secs(30))?;
            let page = body.trim_start().get(..15).is_some_and(|h| h.to_ascii_lowercase().starts_with("<!doctype") || h.to_ascii_lowercase().starts_with("<html"));
            match status {
                200..300 if !page => return Ok(body),
                // not there (a site answers a missing file with its own page): the next place a folder URL may keep it
                200..300 | 404 | 410 => tried.push(url),
                _ => bail!("{url}: HTTP {status}"),
            }
        }
        bail!("no env file at {} (missing, or a web page)", tried.join(", "));
    }
    let bytes = std::fs::read(source).with_context(|| format!("reading {source}"))?;
    decode_text(&bytes).with_context(|| format!("{source} isn't text"))
}

/// UTF-8, or the UTF-16 Windows PowerShell 5 writes with `>`.
fn decode_text(bytes: &[u8]) -> anyhow::Result<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        return Ok(String::from_utf16(&units)?);
    }
    Ok(String::from_utf8(bytes.to_vec())?)
}

// ---- saving ----

/// `$XDG_CONFIG_HOME`, else `~/.config`.
fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config"))
}

/// Where `--save` keeps the variables on macOS and Linux (sourced from the shell profiles).
pub fn saved_file() -> PathBuf {
    config_dir().join("bro").join("env")
}

const HOOK_MARK: &str = "# bro: variables saved with `bro --env … --save`";

fn sh_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', r"'\''"))
}

fn sh_unquote(v: &str) -> Option<String> {
    let inner = v.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace(r"'\''", "'"))
}

/// The variables in a file bro wrote (`export K='v'` lines).
fn read_saved(text: &str) -> Vars {
    text.lines().filter_map(|l| l.strip_prefix("export ")?.split_once('=')).filter_map(|(k, v)| Some((k.to_string(), sh_unquote(v)?))).collect()
}

/// Writes `text` through a temp file and a rename, readable by its owner only.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// `vars` merged into the saved file under `config`, and the hook that sources it added to the
/// shell profiles under `home` (`~/.profile` always; `~/.bashrc` when it exists; `~/.zshenv` when it
/// exists or zsh is the shell; fish's `conf.d` when fish is set up). Returns what changed.
pub fn save_shell(home: &Path, config: &Path, vars: &Vars, zsh: bool) -> anyhow::Result<Vec<PathBuf>> {
    let file = config.join("bro").join("env");
    let mut all = std::fs::read_to_string(&file).map(|t| read_saved(&t)).unwrap_or_default();
    for (k, v) in vars {
        upsert(&mut all, k, v.clone());
    }
    let mut text = String::from("# Written by bro (`bro --env <file> --save`). Edits are kept; bro updates the lines it set.\n");
    for (k, v) in &all {
        text.push_str(&format!("export {k}={}\n", sh_quote(v)));
    }
    write_private(&file, &text).with_context(|| format!("writing {}", file.display()))?;
    let mut changed = vec![file.clone()];

    let shown = match file.strip_prefix(home) {
        Ok(rel) => format!("$HOME/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => file.display().to_string(),
    };
    let hook = format!("\n{HOOK_MARK}\n[ -f \"{shown}\" ] && . \"{shown}\"\n");
    let mut profiles = vec![home.join(".profile")];
    if home.join(".bashrc").is_file() {
        profiles.push(home.join(".bashrc"));
    }
    if zsh || home.join(".zshenv").is_file() {
        profiles.push(home.join(".zshenv"));
    }
    for p in profiles {
        let old = std::fs::read_to_string(&p).unwrap_or_default();
        if !old.contains(HOOK_MARK) {
            crate::util::atomic_write(&p, format!("{old}{hook}").as_bytes()).with_context(|| format!("writing {}", p.display()))?;
            changed.push(p);
        }
    }
    if config.join("fish").is_dir() {
        let fish = config.join("fish").join("conf.d").join("bro-env.fish");
        let mut text = String::from("# Written by bro (`bro --env <file> --save`).\n");
        for (k, v) in &all {
            text.push_str(&format!("set -gx {k} '{}'\n", v.replace('\\', r"\\").replace('\'', r"\'")));
        }
        write_private(&fish, &text).with_context(|| format!("writing {}", fish.display()))?;
        changed.push(fish);
    }
    Ok(changed)
}

/// A value systemd's `environment.d` takes as it is (it expands `$` and treats quotes and spaces).
fn plain_value(v: &str) -> bool {
    v.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:+=@,%~".contains(c))
}

/// `vars` merged into `<config>/environment.d/60-bro.conf`, read by the systemd user manager when
/// it starts (services, and desktop sessions). Values it would mangle are left out and returned.
pub fn save_environment_d(config: &Path, vars: &Vars) -> anyhow::Result<(PathBuf, Vec<String>)> {
    let file = config.join("environment.d").join("60-bro.conf");
    let mut all: Vars = std::fs::read_to_string(&file)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut left_out = vec![];
    for (k, v) in vars {
        if plain_value(v) {
            upsert(&mut all, k, v.clone());
        } else {
            left_out.push(k.clone());
        }
    }
    let mut text = String::from("# Written by bro (`bro --env <file> --save`).\n");
    for (k, v) in &all {
        text.push_str(&format!("{k}={v}\n"));
    }
    write_private(&file, &text).with_context(|| format!("writing {}", file.display()))?;
    Ok((file, left_out))
}

/// Saves `vars` (already [`usable`]) for new processes, and says where. Values with line breaks
/// can't be saved. Blocking.
pub fn save(vars: &Vars) -> anyhow::Result<Vec<String>> {
    if let Some((k, _)) = vars.iter().find(|(_, v)| v.contains(['\n', '\r', '\0'])) {
        bail!("{k} has a line break in its value, which can't be saved");
    }
    save_os(vars)
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn save_os(vars: &Vars) -> anyhow::Result<Vec<String>> {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegOpenKeyExW, RegSetValueExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
    let sub = wide("Environment");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: plain registry calls with NUL-terminated UTF-16 buffers that outlive them.
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key) };
    if rc != 0 {
        bail!("couldn't open HKCU\\Environment (error {rc})");
    }
    let mut failed = None;
    for (k, v) in vars {
        let (name, data) = (wide(k), wide(v));
        // SAFETY: as above; the data length is in bytes, NUL included.
        let rc = unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        if rc != 0 {
            failed = Some(format!("couldn't save {k} (error {rc})"));
            break;
        }
    }
    // SAFETY: the key opened above.
    unsafe { RegCloseKey(key) };
    if let Some(e) = failed {
        bail!(e);
    }
    // Explorer (and what it starts from now on) re-reads the environment
    let mut result = 0usize;
    // SAFETY: a broadcast with a NUL-terminated string that outlives the call.
    unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, sub.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
    Ok(vec![r"HKCU\Environment (your user's environment variables)".into()])
}

#[cfg(not(windows))]
fn save_os(vars: &Vars) -> anyhow::Result<Vec<String>> {
    use std::process::{Command, Stdio};
    let home = home_dir();
    let config = config_dir();
    let zsh = std::env::var("SHELL").is_ok_and(|s| s.ends_with("zsh"));
    let mut places: Vec<String> = save_shell(&home, &config, vars, zsh)?.iter().map(|p| p.display().to_string()).collect();
    let names: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
    if cfg!(target_os = "macos") {
        // launchd hands these to apps and agents started from now on (until you log out)
        let ok = vars.iter().all(|(k, v)| Command::new("launchctl").args(["setenv", k, v]).stdin(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()));
        if ok {
            places.push("launchd (apps and services started from now on)".into());
        }
    } else if Path::new("/run/systemd/system").exists() {
        let (file, left_out) = save_environment_d(&config, vars)?;
        places.push(file.display().to_string());
        if !left_out.is_empty() {
            places.push(format!("({} left out of environment.d: its syntax would change them)", left_out.join(", ")));
        }
        // the running user manager learns them now; the values come from bro's own environment,
        // so they never appear on a command line
        let mut cmd = Command::new("systemctl");
        cmd.args(["--user", "import-environment"]).args(&names).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        if cmd.status().is_ok_and(|s| s.success()) {
            places.push("the systemd user manager (services started from now on)".into());
        }
    }
    Ok(places)
}

// ---- exporting ----

/// Variables the OS, the login, the terminal or a bro session set, which an export leaves out
/// unless asked for everything (matched without case: Windows names are case-insensitive).
const NOISE: [&str; 63] = [
    "PWD", "OLDPWD", "SHLVL", "_", "TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "TERM_SESSION_ID", "COLORTERM", "COLORFGBG", "DISPLAY", "WAYLAND_DISPLAY",
    "MAIL", "HOSTNAME", "HOSTTYPE", "MACHTYPE", "OSTYPE", "LANG", "LANGUAGE", "LS_COLORS", "LSCOLORS", "LESSOPEN", "LESSCLOSE", "MOTD_SHOWN", "TMPDIR",
    "SECURITYSESSIONID", "COMMAND_MODE", "__CF_USER_TEXT_ENCODING", "__CFBUNDLEIDENTIFIER", "ONEDRIVE", "ONEDRIVECONSUMER", "ONEDRIVECOMMERCIAL",
    "CHOCOLATEYLASTPATHUPDATE", "WSLENV", "WT_SESSION", "WT_PROFILE_ID", "PROMPT", "PSMODULEPATH", "POWERSHELL_DISTRIBUTION_CHANNEL", "SYSTEMDRIVE",
    "PROGRAMDATA", "PROGRAMFILES", "PROGRAMW6432", "COMMONPROGRAMFILES", "COMMONPROGRAMW6432", "ALLUSERSPROFILE", "PUBLIC", "COMPUTERNAME", "USERDOMAIN",
    "USERDOMAIN_ROAMINGPROFILE", "HOMEDRIVE", "HOMEPATH", "LOGONSERVER", "SESSIONNAME", "NUMBER_OF_PROCESSORS", "OS", "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER", "PROCESSOR_LEVEL", "PROCESSOR_REVISION", "DRIVERDATA", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT",
];

const NOISE_PREFIXES: [&str; 14] = ["XDG_", "SSH_", "DBUS_", "LC_", "GPG_", "VSCODE_", "BRO_SESSION", "GNOME_", "KDE_", "ITERM_", "KITTY_", "WEZTERM_", "WINDOWID", "CONEMU"];

/// Set by the OS, the login or the terminal rather than by you.
pub fn is_noise(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    RESERVED.contains(&k.as_str()) || NOISE.contains(&k.as_str()) || NOISE_PREFIXES.iter().any(|p| k.starts_with(p))
}

/// What an export writes: names [`parse`] can read, sorted, and (unless `all`) without [`is_noise`].
pub fn export_filter(vars: Vars, all: bool) -> Vars {
    let mut vars: Vars = vars.into_iter().filter(|(k, _)| valid_key(k) && (all || !is_noise(k))).collect();
    vars.sort_by_key(|a| a.0.to_ascii_lowercase());
    vars
}

/// A dotenv file [`parse`] reads back unchanged: plain values as they are, the rest double-quoted.
pub fn to_dotenv(vars: &Vars, header: &str) -> String {
    let mut out = String::new();
    for line in header.lines() {
        out.push_str(&format!("# {line}\n"));
    }
    for (k, v) in vars {
        if !v.is_empty() && plain_value(v) {
            out.push_str(&format!("{k}={v}\n"));
        } else {
            let quoted = v.replace('\\', r"\\").replace('"', "\\\"").replace('$', r"\$").replace('\n', r"\n").replace('\r', r"\r").replace('\t', r"\t");
            out.push_str(&format!("{k}=\"{quoted}\"\n"));
        }
    }
    out
}

/// Where [`system_vars`] reads from, for messages.
pub fn system_source() -> &'static str {
    if cfg!(windows) { r"your user environment variables (HKCU\Environment)" } else { "this shell's environment" }
}

/// Your own variables: on Windows the user environment variables in `HKCU\Environment` (what
/// System Properties lists as "User variables", `%…%` expanded); elsewhere this process's
/// environment, which your shell profiles built. Blocking.
pub fn system_vars() -> anyhow::Result<Vars> {
    system_vars_os()
}

#[cfg(windows)]
fn system_vars_os() -> anyhow::Result<Vars> {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW};
    const ERROR_MORE_DATA: u32 = 234;
    const ERROR_NO_MORE_ITEMS: u32 = 259;
    let sub = wide("Environment");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: a registry open with a NUL-terminated name that outlives the call.
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) };
    if rc != 0 {
        bail!("couldn't open HKCU\\Environment (error {rc})");
    }
    let mut vars = Vars::new();
    let (mut name, mut data) = (vec![0u16; 16384], vec![0u8; 64 * 1024]);
    let mut i = 0u32;
    let result = loop {
        let (mut name_len, mut data_len, mut kind) = (name.len() as u32, data.len() as u32, 0u32);
        // SAFETY: the buffers and their lengths match; the key is open.
        let rc = unsafe { RegEnumValueW(key, i, name.as_mut_ptr(), &mut name_len, std::ptr::null_mut(), &mut kind, data.as_mut_ptr(), &mut data_len) };
        match rc {
            0 => {}
            ERROR_NO_MORE_ITEMS => break Ok(()),
            ERROR_MORE_DATA => {
                data.resize(data_len as usize + 2, 0);
                continue;
            }
            rc => break Err(anyhow::anyhow!("reading HKCU\\Environment (error {rc})")),
        }
        i += 1;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            continue;
        }
        let units: Vec<u16> = data[..data_len as usize].as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        let value = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
        let value = if kind == REG_EXPAND_SZ { crate::quick_install::expand_vars(&value) } else { value };
        vars.push((String::from_utf16_lossy(&name[..name_len as usize]), value));
    };
    // SAFETY: the key opened above.
    unsafe { RegCloseKey(key) };
    result.map(|()| vars)
}

#[cfg(not(windows))]
fn system_vars_os() -> anyhow::Result<Vars> {
    Ok(std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_what_you_set_and_reads_back_the_same() {
        let raw: Vars = [
            ("OPENROUTER_API_KEY", "sk-or-v1-abc"),
            ("Path", r"C:\x"),
            ("OneDrive", r"C:\Users\me\OneDrive"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
            ("ODD", "a \"b\" $c\\d\nnext\ttab"),
            ("EMPTY", ""),
            ("apple", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let vars = export_filter(raw.clone(), false);
        let names: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["apple", "EMPTY", "ODD", "OPENROUTER_API_KEY"]);
        assert_eq!(export_filter(raw, true).len(), 7, "--all keeps everything a dotenv file can name");
        let text = to_dotenv(&vars, "exported by bro\nfrom a test");
        assert!(text.starts_with("# exported by bro\n# from a test\n") && text.contains("OPENROUTER_API_KEY=sk-or-v1-abc\n") && text.contains("EMPTY=\"\"\n"), "{text}");
        assert_eq!(parse(&text).unwrap(), vars);
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("A=1\r\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(parse(&decode_text(&utf16).unwrap()).unwrap(), vec![("A".to_string(), "1".to_string())]);
    }

    const CHEAP: Kdf = Kdf { m: 64, t: 1, p: 1 };

    #[test]
    fn parses_dotenv() {
        let text = "\u{feff}# keys\nOPENROUTER_API_KEY=sk-or-v1-abc\r\nexport ELEVENLABS_API_KEY = 'x y # not a comment'\n\
            QUOTED=\"line\\none \\\"two\\\" \\$HOME\" # trailing\nPLAIN=value # comment\nEMPTY=\nMULTI=\"a\nb\"\nOPENROUTER_API_KEY=sk-or-v1-new\n";
        let vars = parse(text).unwrap();
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("OPENROUTER_API_KEY"), Some("sk-or-v1-new"), "a later line wins");
        assert_eq!(get("ELEVENLABS_API_KEY"), Some("x y # not a comment"));
        assert_eq!(get("QUOTED"), Some("line\none \"two\" $HOME"));
        assert_eq!(get("PLAIN"), Some("value"));
        assert_eq!(get("EMPTY"), Some(""));
        assert_eq!(get("MULTI"), Some("a\nb"));
        assert_eq!(vars[0].0, "OPENROUTER_API_KEY", "file order is kept");
        assert!(parse("NOEQUALS\n").unwrap_err().to_string().contains("line 1"));
        assert!(parse("1BAD=x").is_err() && parse("A=\"open").is_err() && parse("A='x' y").is_err());
    }

    #[test]
    fn reserved_names_are_left_alone() {
        let (keep, skip) = usable(vec![("PATH".into(), "/evil".into()), ("OPENROUTER_API_KEY".into(), "k".into()), ("home".into(), "x".into())]);
        assert_eq!(keep, vec![("OPENROUTER_API_KEY".to_string(), "k".to_string())]);
        assert_eq!(skip, vec!["PATH".to_string(), "home".to_string()]);
    }

    #[test]
    fn encrypts_and_decrypts_with_the_password_only() {
        let plain = "OPENROUTER_API_KEY=sk-or-v1-abc\n";
        let armored = encrypt_with(plain, "correct horse", CHEAP).unwrap();
        assert!(is_encrypted(&armored) && armored.contains("argon2id m=64 t=1 p=1") && !armored.contains("sk-or"));
        assert!(armored.lines().all(|l| l.len() <= 64 || l == BEGIN || l == END));
        assert_eq!(decrypt(&armored, "correct horse").unwrap(), plain);
        assert!(decrypt(&armored, "wrong").unwrap_err().to_string().contains("wrong password"));
        // the parameters are bound to the ciphertext
        let tampered = armored.replace("t=1", "t=2");
        assert!(decrypt(&tampered, "correct horse").is_err());
        // a CRLF copy with text around it still opens
        let pasted = format!("here you go:\r\n{}\r\n", armored.replace('\n', "\r\n"));
        assert_eq!(decrypt(&pasted, "correct horse").unwrap(), plain);
        assert!(!is_encrypted(plain));
        assert!(Kdf::parse("argon2id m=99999999 t=1 p=1").is_err());
        assert_eq!(Kdf::parse(&KDF.line()).unwrap(), KDF);
    }

    #[test]
    fn urls_must_be_https_unless_local() {
        assert!(allowed_url("https://gist.githubusercontent.com/x/raw/keys.env.enc"));
        assert!(allowed_url("http://localhost:8080/keys.env") && allowed_url("http://127.0.0.1/k") && allowed_url("http://[::1]:9/k"));
        assert!(!allowed_url("http://example.com/keys.env") && !allowed_url("http://localhost.evil.com/k") && !allowed_url("http://127.0.0.1@evil.com/k"));
        assert!(is_url("https://x") && !is_url("./keys.env"));
        assert_eq!(candidates("https://m.justgains.com"), ["https://m.justgains.com/bro.env", "https://m.justgains.com/bro.env.enc", "https://m.justgains.com"]);
        assert_eq!(candidates("https://x.com/team/")[0], "https://x.com/team/bro.env");
        assert_eq!(candidates("https://x.com/team?t=1")[..2], ["https://x.com/team/bro.env?t=1".to_string(), "https://x.com/team/bro.env.enc?t=1".into()]);
        assert_eq!(candidates("https://x.com/keys.env.enc"), ["https://x.com/keys.env.enc"]);
        assert_eq!(candidates("https://gist.githubusercontent.com/u/1/raw").last().unwrap(), "https://gist.githubusercontent.com/u/1/raw");
        assert_eq!(candidates("http://127.0.0.1:8765")[0], "http://127.0.0.1:8765/bro.env", "a port isn't an extension");
    }

    #[test]
    fn saves_for_shells_merging_with_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let (home, config) = (dir.path().to_path_buf(), dir.path().join(".config"));
        std::fs::write(home.join(".bashrc"), "alias ll='ls -l'\n").unwrap();
        std::fs::create_dir_all(config.join("fish")).unwrap();
        let first = vec![("OPENROUTER_API_KEY".to_string(), "sk-1".to_string()), ("QUOTE".to_string(), "it's \"x\" $y".to_string())];
        let changed = save_shell(&home, &config, &first, true).unwrap();
        assert_eq!(changed.len(), 5, "{changed:?}");
        let env = std::fs::read_to_string(config.join("bro").join("env")).unwrap();
        assert!(env.contains("export OPENROUTER_API_KEY='sk-1'") && env.contains(r#"export QUOTE='it'\''s "x" $y'"#), "{env}");
        assert_eq!(read_saved(&env), first);
        let bashrc = std::fs::read_to_string(home.join(".bashrc")).unwrap();
        assert!(bashrc.starts_with("alias ll") && bashrc.contains(r#"[ -f "$HOME/.config/bro/env" ] && . "$HOME/.config/bro/env""#), "{bashrc}");
        assert!(std::fs::read_to_string(config.join("fish").join("conf.d").join("bro-env.fish")).unwrap().contains(r"set -gx QUOTE 'it\'s"));
        // again: the key is replaced, the other kept, and the profiles aren't hooked twice
        let changed = save_shell(&home, &config, &vec![("OPENROUTER_API_KEY".to_string(), "sk-2".to_string())], true).unwrap();
        assert_eq!(changed.len(), 2, "only the saved file and fish's: {changed:?}");
        let env = std::fs::read_to_string(config.join("bro").join("env")).unwrap();
        assert!(env.contains("'sk-2'") && !env.contains("'sk-1'") && env.contains("QUOTE="));
        assert_eq!(std::fs::read_to_string(home.join(".profile")).unwrap().matches(HOOK_MARK).count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(config.join("bro").join("env")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn saves_plain_values_for_systemd() {
        let dir = tempfile::tempdir().unwrap();
        let vars = vec![("OPENROUTER_API_KEY".to_string(), "sk-or-v1-abc".to_string()), ("ODD".to_string(), "a b$c".to_string())];
        let (file, left_out) = save_environment_d(dir.path(), &vars).unwrap();
        assert_eq!(left_out, vec!["ODD".to_string()]);
        let (_, _) = save_environment_d(dir.path(), &vec![("OTHER".to_string(), "1".to_string())]).unwrap();
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains("OPENROUTER_API_KEY=sk-or-v1-abc\n") && text.contains("OTHER=1\n") && !text.contains("ODD"), "{text}");
        assert!(save(&vec![("X".to_string(), "a\nb".to_string())]).is_err());
    }
}
