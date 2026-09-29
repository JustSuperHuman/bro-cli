//! Small shared helpers: home dir, atomic writes, lenient JSON, path comparison,
//! JWT payload decoding, time. Everything here is infallible or returns
//! `io::Result`/`Option` — callers decide how loud a failure is.
use serde_json::{Map, Value};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The user's home directory. Reads the environment on every call (so tests can point
/// it at a tempdir): `USERPROFILE` then `HOME` on Windows, `HOME` elsewhere, falling
/// back to the OS answer.
pub fn home_dir() -> PathBuf {
    let vars: &[&str] = if cfg!(windows) { &["USERPROFILE", "HOME"] } else { &["HOME"] };
    for var in vars {
        if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            return PathBuf::from(v);
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Non-empty env var as a path.
pub(crate) fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Non-empty env var as a string.
pub(crate) fn env_str(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// Write `bytes` to `path` atomically: a sibling temp file, then rename over the
/// target. Parent directories are created. On Windows the rename is retried briefly,
/// because another process (v1, an editor, AV) may hold the target open for a moment.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let tmp = parent.join(format!(".{name}.{}.{:08x}.tmp", std::process::id(), rand::random::<u32>()));
    std::fs::write(&tmp, bytes)?;
    let mut last = None;
    for attempt in 0..8 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last.unwrap_or_else(|| io::Error::other("rename failed")))
}

/// Pretty JSON (2-space, like v1's `JSON.stringify(x, null, 2)`) written atomically.
pub fn write_json_pretty(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    atomic_write(path, text.as_bytes())
}

/// Compact JSON written atomically.
pub fn write_json_compact(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let text = serde_json::to_string(value).map_err(io::Error::other)?;
    atomic_write(path, text.as_bytes())
}

/// Parse a JSON file; `None` when missing or malformed. A UTF-8 BOM is tolerated.
pub fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Read at most `max` bytes from the head of a file as (lossy) UTF-8.
pub fn read_head(path: &Path, max: usize) -> String {
    let Ok(file) = std::fs::File::open(path) else { return String::new() };
    let mut buf = Vec::with_capacity(max.min(1 << 20));
    if file.take(max as u64).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Drop everything "commented out" with a leading `#` (v1 `stripHash`): object keys
/// starting with `#`, string array items starting with `#`, object array items whose
/// `id`/`name` starts with `#`, and object items left empty.
pub fn strip_hash(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(strip_hash)
                .filter(|item| match item {
                    Value::Null => false,
                    Value::String(s) => !s.trim_start().starts_with('#'),
                    Value::Object(map) => {
                        if map.is_empty() {
                            return false;
                        }
                        let id = map.get("id").or_else(|| map.get("name"));
                        !matches!(id, Some(Value::String(s)) if s.trim_start().starts_with('#'))
                    }
                    _ => true,
                })
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter().filter(|(k, _)| !k.starts_with('#')).map(|(k, v)| (k.clone(), strip_hash(v))).collect(),
        ),
        other => other.clone(),
    }
}

/// Rebuild `new` so keys that already existed in `old` keep their original position
/// (a hand-edited file stays in the user's order); new keys are appended.
pub(crate) fn merge_ordered(old: &Map<String, Value>, mut new: Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for key in old.keys() {
        if let Some(v) = new.remove(key) {
            out.insert(key.clone(), v);
        }
    }
    for (k, v) in new {
        out.insert(k, v);
    }
    out
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    now_ms() / 1000
}

/// A `SystemTime` as fractional milliseconds (Node's `mtimeMs`).
pub fn system_time_ms(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() * 1000.0,
        Err(_) => 0.0,
    }
}

/// Parse an ISO-8601 / RFC 3339 timestamp to unix seconds.
pub fn parse_iso_secs(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.timestamp())
}

/// Parse an ISO-8601 timestamp to unix milliseconds.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.timestamp_millis())
}

/// Lexically normalise a path: make it absolute (against the current dir), resolve
/// `.`/`..`, strip Windows' `\\?\` verbatim prefix. Does not touch the filesystem.
pub fn normalize_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let stripped = s.strip_prefix(r"\\?\").map(PathBuf::from);
    let p = stripped.as_deref().unwrap_or(p);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf())
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::RootDir | Component::Prefix(_)) | None) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Comparison key for a path: normalised, forward slashes, no trailing slash, and
/// lowercase on Windows (case-insensitive filesystem).
pub fn path_key(p: &Path) -> String {
    let mut s = normalize_path(p).to_string_lossy().replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// Whether two paths name the same location (v1 `samePath`).
pub fn same_path(a: &Path, b: &Path) -> bool {
    !a.as_os_str().is_empty() && !b.as_os_str().is_empty() && path_key(a) == path_key(b)
}

/// `candidate` is strictly inside `root`.
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    let root = path_key(root);
    let cand = path_key(candidate);
    cand.len() > root.len() + 1 && cand.starts_with(&root) && cand.as_bytes()[root.len()] == b'/'
}

/// Decode a JWT's payload (no signature check — only used to read claims we issued
/// ourselves via the user's own login).
pub fn jwt_payload(token: &str) -> Option<Value> {
    use base64::Engine;
    let part = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Short lowercase hex digest of `input` (first `len` chars of SHA-256).
pub fn short_hash(input: &str, len: usize) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..len.min(hex.len())].to_string()
}

/// String field of a JSON object.
pub(crate) fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Number (or numeric string) as f64.
pub(crate) fn num_of(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n: &f64| n.is_finite())
}

/// A test-only env/home sandbox. Env vars are process-global, so every test that
/// touches them holds [`test_env::lock`] for its whole duration.
#[cfg(test)]
pub(crate) mod test_env {
    use parking_lot::{Mutex, MutexGuard};
    use std::path::{Path, PathBuf};

    static LOCK: Mutex<()> = Mutex::new(());

    /// Every override bro-core reads; cleared or pointed into the sandbox.
    const VARS: &[&str] = &[
        "BRO_DIR",
        "CLAUDE_POOL_DIR",
        "BRO_CODEX_PROFILES_DIR",
        "HOME",
        "USERPROFILE",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "BRO_STATE_PATH",
        "BRO_USAGE_HISTORY",
        "BRO_CLAUDE_BROWSER_DIR",
        "BRO_MODELS_URL",
        "PATH",
        "APPDATA",
        "BUN_INSTALL",
    ];

    /// Holds the global env lock and a tempdir acting as `$HOME`; restores the
    /// environment on drop.
    pub struct Sandbox {
        pub dir: tempfile::TempDir,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _guard: MutexGuard<'static, ()>,
    }

    impl Sandbox {
        pub fn home(&self) -> &Path {
            self.dir.path()
        }
        pub fn bro(&self) -> PathBuf {
            self.dir.path().join(".bro")
        }
    }

    /// Take the lock and point every root at a fresh tempdir.
    pub fn sandbox() -> Sandbox {
        let guard = LOCK.lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let saved = VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
        let home = dir.path();
        // SAFETY: all env mutation in tests happens while holding LOCK.
        unsafe {
            for v in VARS {
                std::env::remove_var(v);
            }
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            std::env::set_var("BRO_DIR", home.join(".bro"));
            std::env::set_var("CLAUDE_POOL_DIR", home.join(".claude-max-pool"));
            std::env::set_var("BRO_CODEX_PROFILES_DIR", home.join(".bro").join("codex-profiles"));
            std::env::set_var("BRO_MODELS_URL", "http://127.0.0.1:9/never");
            // Executables resolve only inside the sandbox (tests drop fake shims here).
            std::env::set_var("PATH", home.join("bin"));
        }
        Sandbox { dir, saved, _guard: guard }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            // SAFETY: still holding LOCK.
            unsafe {
                for (k, v) in &self.saved {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_hash_rules() {
        let v = json!({"#a": 1, "b": ["#x", "y", {"id": "#z"}, {"#id": "q"}, {"id": "k"}]});
        assert_eq!(strip_hash(&v), json!({"b": ["y", {"id": "k"}]}));
    }

    #[test]
    fn path_helpers() {
        let root = if cfg!(windows) { Path::new(r"C:\a\b") } else { Path::new("/a/b") };
        let inner = root.join("c").join("d");
        assert!(is_within(root, &inner));
        assert!(!is_within(root, root));
        assert!(same_path(&root.join("c").join(".."), root));
        if cfg!(windows) {
            assert!(same_path(Path::new(r"c:\A\B\"), root));
            assert_eq!(path_key(Path::new(r"F:\Foo\Bar\")), "f:/foo/bar");
        }
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub").join("x.json");
        atomic_write(&f, b"1").unwrap();
        atomic_write(&f, b"2").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "2");
        assert_eq!(std::fs::read_dir(f.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn jwt_decode() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":5}"#);
        assert_eq!(jwt_payload(&format!("h.{payload}.s")).unwrap()["exp"], 5);
        assert!(jwt_payload("garbage").is_none());
    }
}
