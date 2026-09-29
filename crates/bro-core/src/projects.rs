//! Project identity used to group the sidebar.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProjectKey {
    /// git root if inside a repo (walk up for `.git`), else the cwd itself.
    /// Absolute, no trailing separator.
    pub root: PathBuf,
    /// Normalized `root` for equality/hash (lowercase + forward slashes on Windows).
    pub key: String,
    /// Display name: last path component (drive letter for roots, e.g. "F:").
    pub name: String,
}

/// Cached per path — cheap enough to call per frame.
pub fn project_for(cwd: &Path) -> ProjectKey { todo!() }

#[allow(unused_imports)]
use PathBuf as _;
