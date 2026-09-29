//! Recent launch combos, shown at the top of the launcher. Persisted in `~/.bro/v2-recents.json` (atomic
//! writes, on a background thread).

use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::launch::Permission;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One remembered launch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub harness: Harness,
    pub profile_id: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub permission: Permission,
    #[serde(default)]
    pub browser: BrowserMode,
    /// unix seconds
    pub at: i64,
}

impl Recent {
    /// Same combo (ignores time).
    pub fn same(&self, o: &Recent) -> bool {
        self.harness == o.harness && self.profile_id == o.profile_id && self.provider_id == o.provider_id && self.model == o.model && self.cwd == o.cwd
    }
}

/// How many are kept.
pub const KEEP: usize = 20;

pub fn path() -> PathBuf {
    crate::util::bro_dir().join("v2-recents.json")
}

/// Load (missing or broken file = empty).
pub fn load() -> Vec<Recent> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Put `r` first (dropping an equal older entry), cap the list.
pub fn push(list: &mut Vec<Recent>, r: Recent) {
    list.retain(|x| !x.same(&r));
    list.insert(0, r);
    list.truncate(KEEP);
}

/// Save on a background thread.
pub fn save(list: Vec<Recent>) {
    std::thread::spawn(move || {
        if let Ok(s) = serde_json::to_vec_pretty(&list) {
            let _ = crate::util::atomic_write(&path(), &s);
        }
    });
}

/// Directories you've launched in, newest first, deduplicated.
pub fn dirs(list: &[Recent]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for r in list {
        if !out.contains(&r.cwd) {
            out.push(r.cwd.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(model: &str, at: i64) -> Recent {
        Recent { harness: Harness::Claude, profile_id: Some("claude:work".into()), provider_id: None, model: Some(model.into()), cwd: PathBuf::from("/x"), permission: Permission::Default, browser: BrowserMode::Off, at }
    }

    #[test]
    fn push_dedups_and_caps() {
        let mut l = vec![];
        for i in 0..30 {
            push(&mut l, r(&format!("m{i}"), i));
        }
        assert_eq!(l.len(), KEEP);
        assert_eq!(l[0].model.as_deref(), Some("m29"));
        push(&mut l, r("m20", 99));
        assert_eq!(l[0].at, 99);
        assert_eq!(l.iter().filter(|x| x.model.as_deref() == Some("m20")).count(), 1);
        assert_eq!(dirs(&l), vec![PathBuf::from("/x")]);
        let json = serde_json::to_string(&l).unwrap();
        let back: Vec<Recent> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
    }
}
