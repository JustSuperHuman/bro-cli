//! Installed sound packs: `packs/<name>/openpeon.json` (CESP's manifest) and the sounds it lists.
use super::packs_dir;
use anyhow::{Context, bail};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// One sound in a category.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// Relative to the pack (`sounds/PeonYes1.wav`).
    pub file: String,
    /// What it says ("Work, work.").
    pub label: Option<String>,
    pub sha256: Option<String>,
}

/// A pack's manifest.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Manifest {
    pub name: String,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub language: Option<String>,
    pub description: Option<String>,
    /// Category id → its sounds, in the manifest's order.
    pub categories: Vec<(String, Vec<Sound>)>,
}

impl Manifest {
    pub fn title(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    pub fn sounds(&self, cat: &str) -> &[Sound] {
        self.categories.iter().find(|(c, _)| c == cat).map(|(_, s)| s.as_slice()).unwrap_or(&[])
    }

    /// Every file it lists, once each.
    pub fn files(&self) -> Vec<&Sound> {
        let mut seen = std::collections::HashSet::new();
        self.categories.iter().flat_map(|(_, s)| s).filter(|s| seen.insert(s.file.as_str())).collect()
    }

    /// A random sound for `cat`, not `last` (a file name) when there's another.
    pub fn pick(&self, cat: &str, last: &str) -> Option<Sound> {
        let all = self.sounds(cat);
        let fresh: Vec<&Sound> = all.iter().filter(|s| base_name(&s.file) != last).collect();
        let from: Vec<&Sound> = if fresh.is_empty() { all.iter().collect() } else { fresh };
        (!from.is_empty()).then(|| from[rand::random_range(0..from.len())].clone())
    }
}

/// `openpeon.json` as text.
pub fn parse_manifest(text: &str) -> anyhow::Result<Manifest> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).context("not JSON")?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let author = match v.get("author") {
        Some(Value::String(a)) => Some(a.clone()),
        Some(a) => a.get("name").and_then(Value::as_str).map(str::to_string),
        None => None,
    };
    let Some(cats) = v.get("categories").and_then(Value::as_object) else { bail!("no categories") };
    let categories = cats
        .iter()
        .map(|(cat, body)| {
            // `{"sounds": [...]}`, or the list itself
            let list = body.get("sounds").unwrap_or(body).as_array().cloned().unwrap_or_default();
            let sounds = list
                .iter()
                .filter_map(|e| match e {
                    Value::String(f) => Some(Sound { file: f.clone(), label: None, sha256: None }),
                    e => Some(Sound {
                        file: e.get("file")?.as_str()?.to_string(),
                        label: e.get("label").and_then(Value::as_str).map(str::to_string),
                        sha256: e.get("sha256").and_then(Value::as_str).map(str::to_lowercase),
                    }),
                })
                .filter(|s| safe_relative(&s.file))
                .collect();
            (cat.clone(), sounds)
        })
        .collect();
    Ok(Manifest {
        name: s("name").unwrap_or_default(),
        display_name: s("display_name"),
        version: s("version"),
        author,
        license: s("license"),
        language: s("language"),
        description: s("description"),
        categories,
    })
}

/// The manifest in pack folder `dir` (`openpeon.json`, else an older `manifest.json` in the same shape).
pub fn load_manifest(dir: &Path) -> Option<Manifest> {
    ["openpeon.json", "manifest.json"].iter().find_map(|f| {
        let text = std::fs::read_to_string(dir.join(f)).ok()?;
        let mut m = parse_manifest(&text).ok()?;
        if m.name.is_empty() {
            m.name = dir.file_name()?.to_string_lossy().into_owned();
        }
        Some(m)
    })
}

/// A pack name bro will use as a folder: letters, digits, `-`, `_`, `.`, not starting with a dot.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 100 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A path inside a pack: relative, plain names only (no `..`, no root, no drive).
pub(crate) fn safe_relative(file: &str) -> bool {
    let p = Path::new(file);
    !file.is_empty() && !file.contains('\\') && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// The file name of a manifest path.
pub fn base_name(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or(file)
}

/// Where a manifest's `file` is: as written, else in `sounds/` (peon-ping's own fallback). None when it isn't
/// there or would leave the pack.
pub fn sound_path(dir: &Path, file: &str) -> Option<PathBuf> {
    if !safe_relative(file) {
        return None;
    }
    [dir.join(file), dir.join("sounds").join(base_name(file))].into_iter().find(|p| p.is_file())
}

/// An installed pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Option<Manifest>,
    /// Distinct files its manifest lists.
    pub sounds: usize,
}

impl Installed {
    pub fn title(&self) -> &str {
        self.manifest.as_ref().map_or(&self.name, Manifest::title)
    }
    pub fn version(&self) -> Option<&str> {
        self.manifest.as_ref()?.version.as_deref()
    }
    /// Has at least one sound for `cat`.
    pub fn covers(&self, cat: &str) -> bool {
        self.manifest.as_ref().is_some_and(|m| !m.sounds(cat).is_empty())
    }
}

/// The names of the packs installed, sorted.
pub fn installed_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(packs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| valid_name(n))
        .filter(|n| load_manifest(&packs_dir().join(n)).is_some())
        .collect();
    names.sort();
    names
}

/// Every pack installed, by name.
pub fn installed() -> Vec<Installed> {
    installed_names()
        .into_iter()
        .map(|name| {
            let dir = packs_dir().join(&name);
            let manifest = load_manifest(&dir);
            let sounds = manifest.as_ref().map_or(0, |m| m.files().len());
            Installed { name, dir, manifest, sounds }
        })
        .collect()
}

/// Delete pack `name`.
pub fn remove(name: &str) -> anyhow::Result<()> {
    if !valid_name(name) {
        bail!("'{name}' isn't a pack name");
    }
    let dir = packs_dir().join(name);
    if !dir.is_dir() {
        bail!("{name} isn't installed");
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
}
