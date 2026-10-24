//! How a launch gets bro's sound hooks, without writing into any harness's own config:
//!
//! * **Claude Code**: `bro hook peon claude` on its events, in the launch's `--settings` file (Claude merges hooks
//!   from every settings source).
//! * **Codex**: the same hooks as `-c hooks.<Event>=…` overrides, plus `-c hooks.state=…` holding each one's trust
//!   hash, computed as Codex does (sha256 over its normalized identity), so Codex runs them without asking for a
//!   review in `/hooks`. Codex keys session hooks by a synthetic `<session-flags>/config.toml` source.
//! * **Pi and omp**: bro's extension (`bro-peon.ts`) with `-e`, told where bro is through `BRO_PEON_BIN`.
//!
//! A login whose own config already runs peon-ping (its `peon.ps1` / `peon.sh` or an adapter), or bro's own hook
//! from [`take_over_claude`], gets nothing more: the sound would play twice.
use crate::util::{atomic_write, read_json, short_hash};
use crate::{Harness, paths};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The Claude Code events bro hooks (peon-ping's set).
pub const CLAUDE_EVENTS: [&str; 9] = ["SessionStart", "SessionEnd", "Stop", "Notification", "PermissionRequest", "PostToolUseFailure", "PreCompact", "UserPromptSubmit", "SubagentStart"];

/// The Codex events bro hooks, with Codex's key label for each (peon-ping's set: no per-tool hooks, they're noisy).
pub const CODEX_EVENTS: [(&str, &str); 6] = [
    ("SessionStart", "session_start"),
    ("UserPromptSubmit", "user_prompt_submit"),
    ("PermissionRequest", "permission_request"),
    ("PreCompact", "pre_compact"),
    ("SubagentStart", "subagent_start"),
    ("Stop", "stop"),
];

/// Seconds a hook may take (it decides and hands the sound to a player process: a few milliseconds).
const TIMEOUT: u64 = 10;

/// The hook entry for `harness` in Claude Code's exec form (no shell).
fn claude_hook(bro_bin: &Path, harness: &str) -> Value {
    json!({ "type": "command", "command": bro_bin.to_string_lossy(), "args": ["hook", "peon", harness], "timeout": TIMEOUT })
}

/// Adds bro's sound hooks to a Claude launch's `--settings`, next to whatever hooks are there.
pub fn add_claude_hooks(settings: &mut Map<String, Value>, bro_bin: &Path) {
    let hook = claude_hook(bro_bin, "claude");
    let Some(hooks) = settings.entry("hooks").or_insert_with(|| json!({})).as_object_mut() else { return };
    for event in CLAUDE_EVENTS {
        if let Some(list) = hooks.entry(event).or_insert_with(|| json!([])).as_array_mut() {
            list.push(json!({ "hooks": [hook.clone()] }));
        }
    }
}

/// Codex runs a hook's command through the session's shell: PowerShell or `cmd /C` on Windows, `$SHELL -lc`
/// elsewhere. In PowerShell a quoted path followed by arguments is a string, not a call, so on Windows the path
/// goes in bare (its 8.3 short form when it has spaces or symbols), which both Windows shells run; failing that,
/// PowerShell's call operator.
pub fn codex_command(bro_bin: &Path) -> String {
    let bin = bro_bin.to_string_lossy();
    if !cfg!(windows) {
        return format!("'{}' hook peon codex", bin.replace('\'', r"'\''"));
    }
    let bare = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || r"\/:._-~+".contains(c));
    let exe = match () {
        _ if bare(&bin) => bin.into_owned(),
        _ => match short_path(bro_bin).filter(|s| bare(s)) {
            Some(s) => s,
            None => format!("& '{}'", bin.replace('\'', "''")),
        },
    };
    format!("{exe} hook peon codex")
}

/// The 8.3 form of an existing path (`C:\PROGRA~1\bro\bro.exe`), where the volume keeps short names.
#[cfg(windows)]
fn short_path(p: &Path) -> Option<String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated; `buf` holds `buf.len()` u16s and the call writes at most that many.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| std::ffi::OsString::from_wide(&buf[..n]).to_string_lossy().into_owned())
}

#[cfg(not(windows))]
fn short_path(_: &Path) -> Option<String> {
    None
}

/// The source Codex files session-flag hooks under.
fn codex_session_source() -> &'static str {
    if cfg!(windows) { r"C:\<session-flags>\config.toml" } else { "/<session-flags>/config.toml" }
}

/// Codex's trust hash for one command hook (no matcher, synchronous, `timeout` seconds): `sha256:` over the
/// compact JSON of `{event_name, hooks: [handler]}` with every key sorted.
pub fn codex_trust_hash(label: &str, command: &str, timeout: u64) -> String {
    // keys written in sorted order (serde_json keeps insertion order in this crate)
    let mut handler = Map::new();
    handler.insert("async".into(), Value::Bool(false));
    handler.insert("command".into(), Value::String(command.into()));
    handler.insert("timeout".into(), json!(timeout));
    handler.insert("type".into(), Value::String("command".into()));
    let mut identity = Map::new();
    identity.insert("event_name".into(), Value::String(label.into()));
    identity.insert("hooks".into(), Value::Array(vec![Value::Object(handler)]));
    let text = serde_json::to_string(&Value::Object(identity)).unwrap_or_default();
    use sha2::Digest;
    let hex: String = sha2::Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// TOML-quote a string (a JSON string is a TOML basic string).
fn toml_str(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("\"{v}\""))
}

/// `-c` overrides that hook Codex's events to bro and trust them.
pub fn codex_args(bro_bin: &Path) -> Vec<String> {
    let command = codex_command(bro_bin);
    let mut args = vec![];
    let mut state = vec![];
    for (event, label) in CODEX_EVENTS {
        args.push("-c".into());
        args.push(format!("hooks.{event}=[{{hooks=[{{type=\"command\",command={},timeout={TIMEOUT}}}]}}]", toml_str(&command)));
        let key = format!("{}:{label}:0:0", codex_session_source());
        state.push(format!("{}={{trusted_hash={}}}", toml_str(&key), toml_str(&codex_trust_hash(label, &command, TIMEOUT))));
    }
    args.push("-c".into());
    args.push(format!("hooks.state={{{}}}", state.join(",")));
    args
}

const EXTENSION: &str = include_str!("bro-peon.ts");

/// bro's Pi / omp extension, written once per version to `~/.bro/peon-ext/bro-peon-<hash>.ts`.
pub fn extension_file() -> std::io::Result<PathBuf> {
    let path = paths::bro_dir().join("peon-ext").join(format!("bro-peon-{}.ts", short_hash(EXTENSION, 10)));
    if !path.is_file() {
        atomic_write(&path, EXTENSION.as_bytes())?;
    }
    Ok(path)
}

/// A launch's extra arguments and environment.
pub type Additions = (Vec<String>, Vec<(String, String)>);

/// What a Pi or omp launch adds: `-e <extension>` and the env telling it where bro is.
pub fn pi_launch(harness: Harness, bro_bin: &Path) -> std::io::Result<Additions> {
    let ext = extension_file()?;
    let args = vec!["-e".to_string(), ext.to_string_lossy().into_owned()];
    let env = vec![("BRO_PEON_BIN".to_string(), bro_bin.to_string_lossy().into_owned()), ("BRO_PEON_HARNESS".to_string(), harness.label().to_string())];
    Ok((args, env))
}

/// A hook command that runs peon-ping's own scripts.
fn is_peon_ping(command: &str) -> bool {
    let c = command.to_lowercase().replace('\\', "/");
    ["peon-ping/", "peon.ps1", "peon.sh", "hook-handle-use"].iter().any(|n| c.contains(n))
}

/// A hook entry that runs bro's own (from [`take_over_claude`]).
fn is_bro_hook(h: &Value) -> bool {
    let args: Vec<&str> = h.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    args.starts_with(&["hook", "peon"]) || h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(" hook peon "))
}

/// Who already plays sounds for a login.
#[derive(Debug, Clone, PartialEq)]
pub enum Own {
    /// Nobody: bro's launch hooks do.
    None,
    /// peon-ping's own hooks, in this file.
    PeonPing(PathBuf),
    /// bro's hook, put there by [`take_over_claude`]: it plays outside bro too.
    Bro(PathBuf),
}

impl Own {
    /// A launch must not add its own hooks.
    pub fn covered(&self) -> bool {
        !matches!(self, Own::None)
    }
}

/// Every command hook in a Claude settings document, as (event, the handler).
fn claude_handlers(doc: &Value) -> Vec<(&str, &Value)> {
    let Some(hooks) = doc.get("hooks").and_then(Value::as_object) else { return vec![] };
    hooks
        .iter()
        .flat_map(|(event, groups)| groups.as_array().into_iter().flatten().flat_map(move |g| g.get("hooks").and_then(Value::as_array).into_iter().flatten().map(move |h| (event.as_str(), h))))
        .collect()
}

/// Who plays sounds for the Claude login in `config_dir` (its `settings.json`).
pub fn claude_own(config_dir: &Path) -> Own {
    let path = config_dir.join("settings.json");
    let Some(doc) = read_json(&path) else { return Own::None };
    let handlers = claude_handlers(&doc);
    if handlers.iter().any(|(_, h)| h.get("command").and_then(Value::as_str).is_some_and(is_peon_ping)) {
        Own::PeonPing(path)
    } else if handlers.iter().any(|(_, h)| is_bro_hook(h)) {
        Own::Bro(path)
    } else {
        Own::None
    }
}

/// Who plays sounds for the Codex home `home`: peon-ping in its `config.toml` (inline hooks, or the legacy
/// `notify` adapter) or `hooks.json`.
pub fn codex_own(home: &Path) -> Own {
    ["hooks.json", "config.toml"]
        .iter()
        .map(|f| home.join(f))
        .find(|p| std::fs::read_to_string(p).is_ok_and(|t| is_peon_ping(&t)))
        .map_or(Own::None, Own::PeonPing)
}

/// peon-ping's own extension for Pi or omp, installed where it loads it by itself.
pub fn pi_own(harness: Harness) -> Own {
    let dir = match harness {
        Harness::Omp => paths::omp_agent_dir(),
        _ => paths::pi_agent_dir(),
    }
    .join("extensions");
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("peon-ping")))
        .map_or(Own::None, Own::PeonPing)
}

/// Who plays sounds for a launch of `harness` in the login at `dir` (Claude's config dir, Codex's home).
pub fn own(harness: Harness, dir: &Path) -> Own {
    match harness {
        Harness::Claude => claude_own(dir),
        Harness::Codex => codex_own(dir),
        h => pi_own(h),
    }
}

/// One login, as Sounds lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub harness: Harness,
    /// "local", or the account's / profile's name
    pub name: String,
    pub dir: PathBuf,
    pub own: Own,
}

fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

/// Every login bro can start, and who plays its sounds: Claude's own and each pool account, Codex's own and each
/// profile, Pi, omp. Reads their config files; run it off the UI thread.
pub fn logins() -> Vec<Login> {
    let mut out = vec![];
    let claude = std::iter::once(("local".to_string(), paths::claude_local_dir())).chain(subdirs(&paths::claude_accounts_dir()));
    out.extend(claude.map(|(name, dir)| Login { harness: Harness::Claude, own: claude_own(&dir), name, dir }));
    let codex = std::iter::once(("local".to_string(), paths::codex_local_dir())).chain(subdirs(&paths::codex_profiles_dir()));
    out.extend(codex.map(|(name, dir)| Login { harness: Harness::Codex, own: codex_own(&dir), name, dir }));
    for (h, dir) in [(Harness::Pi, paths::pi_agent_dir()), (Harness::Omp, paths::omp_agent_dir())] {
        out.push(Login { harness: h, name: "local".into(), own: pi_own(h), dir });
    }
    out
}

/// The copy [`take_over_claude`] keeps of a settings file it changed.
pub fn backup_path(settings: &Path) -> PathBuf {
    let name = settings.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "settings.json".into());
    settings.with_file_name(format!("{name}.before-bro-peon"))
}

/// Swap peon-ping's hooks in the Claude login at `config_dir` for bro's: sounds then play the same in sessions
/// started outside bro (MP3 packs and the volume included on Windows), and bro's launches add nothing more. The
/// file as it was is kept beside it ([`backup_path`], once). Returns how many of peon-ping's handlers went.
pub fn take_over_claude(config_dir: &Path, bro_bin: &Path) -> anyhow::Result<usize> {
    let path = config_dir.join("settings.json");
    let text = std::fs::read_to_string(&path)?;
    let mut doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    let mut removed = 0;
    if let Some(hooks) = doc.get_mut("hooks").and_then(Value::as_object_mut) {
        for groups in hooks.values_mut() {
            let Some(groups) = groups.as_array_mut() else { continue };
            for g in groups.iter_mut() {
                if let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = list.len();
                    list.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(is_peon_ping) && !is_bro_hook(h));
                    removed += before - list.len();
                }
            }
            groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|l| !l.is_empty()));
        }
        hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    }
    let Some(root) = doc.as_object_mut() else { anyhow::bail!("{} isn't a JSON object", path.display()) };
    add_claude_hooks(root, bro_bin);
    let backup = backup_path(&path);
    if !backup.exists() {
        atomic_write(&backup, text.as_bytes())?;
    }
    atomic_write(&path, serde_json::to_string_pretty(&doc)?.as_bytes())?;
    Ok(removed)
}
