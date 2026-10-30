//! Co-author lines: Claude Code signs the commits it writes with a `Co-Authored-By: Claude` trailer and its pull
//! requests with a "Generated with Claude Code" line. With the [`KNOB`] knob off (the default, see
//! [`crate::claude_env`]) bro leaves both out: in every launch's `--settings` file, and in each Claude login's own
//! `settings.json` ([`sync`]) so sessions started outside bro follow too. Back on Claude's default, bro takes out
//! exactly what it put in; wording of your own (`"attribution": { "commit": "Made with AI" }`) stays.
use crate::paths;
use crate::util::atomic_write;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The `claude_env` knob: `off` (the default) hides the lines, `default` is Claude's own.
pub const KNOB: &str = "coauthor";

/// The settings that leave the lines out (`attribution` is current Claude Code, `includeCoAuthoredBy` the older
/// switch it replaced; both, so any version honours it).
pub const SETTINGS: &[(&str, &str)] = &[("attribution", r#"{"commit":"","pr":""}"#), ("includeCoAuthoredBy", "false")];

/// Bring the login at `config_dir` in step: `hidden` puts the settings in its `settings.json`, else takes bro's
/// out. Returns whether the file changed. A login that doesn't exist yet is left alone, and so is a
/// `settings.json` that isn't a JSON object (it's yours to fix; Claude would refuse it too).
pub fn sync(config_dir: &Path, hidden: bool) -> anyhow::Result<bool> {
    if !config_dir.is_dir() {
        return Ok(false);
    }
    let path = config_dir.join("settings.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut doc: Value = match text.as_deref().map(|t| t.trim_start_matches('\u{feff}').trim()) {
        Some(t) if !t.is_empty() => serde_json::from_str(t)?,
        _ if hidden => Value::Object(Map::new()),
        _ => return Ok(false),
    };
    let Some(root) = doc.as_object_mut() else { anyhow::bail!("{} isn't a JSON object", path.display()) };
    let changed = if hidden { hide(root) } else { restore(root) };
    if changed {
        atomic_write(&path, format!("{}\n", serde_json::to_string_pretty(&doc)?).as_bytes())?;
    }
    Ok(changed)
}

/// Leave the lines out: empty attribution where none is set, the old switch off. True when `root` changed.
fn hide(root: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    let attribution = root.entry("attribution").or_insert_with(|| {
        changed = true;
        Value::Object(Map::new())
    });
    // something that isn't an object is someone's own setting: keep it
    if let Some(a) = attribution.as_object_mut() {
        for key in ["commit", "pr"] {
            if !a.contains_key(key) {
                a.insert(key.into(), Value::String(String::new()));
                changed = true;
            }
        }
    }
    if root.get("includeCoAuthoredBy") != Some(&Value::Bool(false)) {
        root.insert("includeCoAuthoredBy".into(), Value::Bool(false));
        changed = true;
    }
    changed
}

/// Take out what [`hide`] puts in, and only that. True when `root` changed.
fn restore(root: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    if let Some(a) = root.get_mut("attribution").and_then(Value::as_object_mut) {
        for key in ["commit", "pr"] {
            if a.get(key).and_then(Value::as_str) == Some("") {
                a.remove(key);
                changed = true;
            }
        }
        if a.is_empty() {
            root.remove("attribution");
        }
    }
    if root.get("includeCoAuthoredBy") == Some(&Value::Bool(false)) {
        root.remove("includeCoAuthoredBy");
        changed = true;
    }
    changed
}

/// Every Claude login bro can start: `~/.claude` and each pool account.
pub fn logins() -> Vec<PathBuf> {
    let mut out = vec![paths::claude_local_dir()];
    let mut accounts: Vec<PathBuf> = std::fs::read_dir(paths::claude_accounts_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    accounts.sort();
    out.extend(accounts);
    out
}

/// [`sync`] every login; the ones that couldn't be brought in step, with why. Reads and writes files: run it off
/// the UI thread.
pub fn sync_all(hidden: bool) -> Vec<(PathBuf, String)> {
    logins().into_iter().filter_map(|dir| sync(&dir, hidden).err().map(|e| (dir, e.to_string()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap()
    }

    #[test]
    fn hiding_adds_the_settings_beside_yours_and_restoring_takes_out_only_those() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{"model":"opus","attribution":{"commit":"Made with AI"}}"#).unwrap();
        assert!(sync(dir.path(), true).unwrap());
        assert_eq!(read(dir.path()), json!({ "model": "opus", "attribution": { "commit": "Made with AI", "pr": "" }, "includeCoAuthoredBy": false }));
        // in step already: the file isn't touched
        assert!(!sync(dir.path(), true).unwrap());
        assert!(sync(dir.path(), false).unwrap());
        assert_eq!(read(dir.path()), json!({ "model": "opus", "attribution": { "commit": "Made with AI" } }));
        assert!(!sync(dir.path(), false).unwrap());
    }

    #[test]
    fn a_login_without_settings_gets_them_and_loses_them_whole() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!sync(dir.path(), false).unwrap(), "nothing to take out");
        assert!(!dir.path().join("settings.json").exists());
        assert!(sync(dir.path(), true).unwrap());
        assert_eq!(read(dir.path()), json!({ "attribution": { "commit": "", "pr": "" }, "includeCoAuthoredBy": false }));
        assert!(sync(dir.path(), false).unwrap());
        assert_eq!(read(dir.path()), json!({}));
    }

    #[test]
    fn key_order_survives_and_the_old_switch_turns_off() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "\u{feff}{\"z\":1,\"includeCoAuthoredBy\":true,\"a\":2}").unwrap();
        assert!(sync(dir.path(), true).unwrap());
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        let keys: Vec<_> = read(dir.path()).as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["z", "includeCoAuthoredBy", "a", "attribution"]);
        assert!(text.contains("\"includeCoAuthoredBy\": false"));
    }

    #[test]
    fn missing_logins_and_broken_files_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("not-a-login");
        assert!(!sync(&gone, true).unwrap());
        assert!(!gone.exists(), "no login folder is made");
        std::fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        assert!(sync(dir.path(), true).is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("settings.json")).unwrap(), "{ not json");
        std::fs::write(dir.path().join("settings.json"), "[1]").unwrap();
        assert!(sync(dir.path(), true).is_err());
    }
}
