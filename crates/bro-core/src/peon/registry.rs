//! The PeonPing registry (<https://peonping.github.io/registry/index.json>, the packs listed on openpeon.com):
//! every pack's name, author, categories, size and where its files are on GitHub. bro caches the index for a
//! day, previews a pack by fetching one of its sounds, and installs one by fetching its manifest and every sound
//! it lists, each checked against the SHA-256 the registry and the manifest give, into a staging folder that
//! only becomes `packs/<name>` once everything arrived.
use super::pack::{self, Manifest, safe_relative, valid_name};
use crate::util::{atomic_write, env_str};
use anyhow::{Context, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// The index (override `BRO_PEON_REGISTRY`).
pub const REGISTRY_URL: &str = "https://peonping.github.io/registry/index.json";
/// Where pack files are served from (override `BRO_PEON_RAW`).
const RAW_BASE: &str = "https://raw.githubusercontent.com";
/// The cached index is fresh for this long.
pub const CACHE_FOR: Duration = Duration::from_secs(24 * 3600);
/// No file in a pack is bigger than this (the largest listed are a few MB).
const MAX_FILE: u64 = 25 * 1024 * 1024;
/// …nor the whole pack.
const MAX_PACK: u64 = 200 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Author {
    pub name: Option<String>,
    pub github: Option<String>,
}

/// One pack in the index.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct RegistryPack {
    pub name: String,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<Author>,
    /// "official", "verified", "community"
    pub trust_tier: Option<String>,
    pub categories: Vec<String>,
    pub language: Option<String>,
    pub license: Option<String>,
    pub sound_count: Option<u32>,
    pub total_size_bytes: Option<u64>,
    pub source_repo: String,
    pub source_ref: String,
    pub source_path: Option<String>,
    pub manifest_sha256: Option<String>,
    pub tags: Vec<String>,
    /// File names of a couple of its sounds (listed as names, or as `{file, label}`).
    #[serde(deserialize_with = "file_names")]
    pub preview_sounds: Vec<String>,
    /// "gold", "silver", "bronze"
    pub quality: Option<String>,
    pub updated: Option<String>,
}

impl RegistryPack {
    pub fn title(&self) -> &str {
        self.display_name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&self.name)
    }

    pub fn author_name(&self) -> Option<&str> {
        let a = self.author.as_ref()?;
        a.name.as_deref().or(a.github.as_deref())
    }

    /// Its files' folder: `<raw>/<repo>/<ref>/<path>`.
    pub fn base_url(&self) -> anyhow::Result<String> {
        let repo_ok = self.source_repo.split('/').count() == 2 && self.source_repo.split('/').all(|p| !p.is_empty() && p != "." && p != "..");
        if !repo_ok || self.source_ref.is_empty() || self.source_ref.contains("..") {
            bail!("{} has no usable source in the registry", self.name);
        }
        let raw = env_str("BRO_PEON_RAW").unwrap_or_else(|| RAW_BASE.into());
        let mut url = format!("{}/{}/{}", raw.trim_end_matches('/'), self.source_repo, self.source_ref);
        if let Some(p) = self.source_path.as_deref().map(|p| p.trim_matches('/')).filter(|p| !p.is_empty() && *p != ".") {
            if !safe_relative(p) {
                bail!("{} has an odd source path", self.name);
            }
            url = format!("{url}/{p}");
        }
        Ok(url)
    }

    /// Matches every word of `query` in its name, title, author, tags or description.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!(
            "{} {} {} {} {} {}",
            self.name,
            self.title(),
            self.author_name().unwrap_or_default(),
            self.tags.join(" "),
            self.description.as_deref().unwrap_or_default(),
            self.language.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
    }
}

/// `["a.mp3", {"file": "sounds/b.mp3", "label": "…"}]` as file names.
fn file_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let list = Vec::<serde_json::Value>::deserialize(d)?;
    Ok(list
        .iter()
        .filter_map(|v| v.as_str().or_else(|| v.get("file")?.as_str()))
        .map(|f| pack::base_name(f).to_string())
        .collect())
}

#[derive(Deserialize)]
struct Index {
    #[serde(default)]
    packs: Vec<serde_json::Value>,
}

/// The index from its JSON, valid names only, sorted by title.
pub fn parse_index(text: &str) -> anyhow::Result<Vec<RegistryPack>> {
    let idx: Index = serde_json::from_str(text.trim_start_matches('\u{feff}')).context("the registry isn't the JSON bro expects")?;
    // a pack in a shape bro doesn't read is left out, not the whole registry
    let mut packs: Vec<RegistryPack> = idx.packs.into_iter().filter_map(|p| serde_json::from_value::<RegistryPack>(p).ok()).filter(|p| valid_name(&p.name)).collect();
    packs.sort_by_key(|p| p.title().to_lowercase());
    Ok(packs)
}

fn cache_path() -> PathBuf {
    crate::paths::bro_dir().join("cache").join("peon-registry.json")
}

/// The cached index and how old it is; None when there's none.
pub fn cached() -> Option<(Vec<RegistryPack>, Duration)> {
    let path = cache_path();
    let age = path.metadata().ok()?.modified().ok()?.elapsed().unwrap_or_default();
    let packs = parse_index(&std::fs::read_to_string(path).ok()?).ok()?;
    Some((packs, age))
}

/// The index: the cache while it's fresh (unless `force`), else fetched (and cached). A failed fetch falls back
/// to an old cache.
pub fn load(force: bool) -> anyhow::Result<Vec<RegistryPack>> {
    let cache = cached();
    if !force && let Some((packs, age)) = &cache
        && *age < CACHE_FOR
    {
        return Ok(packs.clone());
    }
    let url = env_str("BRO_PEON_REGISTRY").unwrap_or_else(|| REGISTRY_URL.into());
    let fetched = fetch(&url, MAX_FILE).and_then(|bytes| {
        let text = String::from_utf8(bytes).context("the registry isn't text")?;
        let packs = parse_index(&text)?;
        let _ = atomic_write(&cache_path(), text.as_bytes());
        Ok(packs)
    });
    match (fetched, cache) {
        (Ok(p), _) => Ok(p),
        (Err(_), Some((packs, _))) => Ok(packs),
        (Err(e), None) => Err(e),
    }
}

/// `GET url`, at most `max` bytes.
fn fetch(url: &str, max: u64) -> anyhow::Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).http_status_as_error(false).build().into();
    let resp = agent.get(url).header("user-agent", "bro").call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        bail!("GET {url}: HTTP {status}");
    }
    let mut out = vec![];
    resp.into_body().into_reader().take(max + 1).read_to_end(&mut out).with_context(|| format!("downloading {url}"))?;
    if out.len() as u64 > max {
        bail!("{url} is bigger than bro expects of a sound pack file");
    }
    Ok(out)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `bytes` match `want` (hex, any case); no `want` passes.
fn check(bytes: &[u8], want: Option<&str>, what: &str) -> anyhow::Result<()> {
    match want.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) if !w.eq_ignore_ascii_case(&sha256_hex(bytes)) => bail!("{what} doesn't match its SHA-256: not installed"),
        _ => Ok(()),
    }
}

/// The pack's manifest from its source, checked against the registry's hash.
fn fetch_manifest(p: &RegistryPack) -> anyhow::Result<(Vec<u8>, Manifest)> {
    let base = p.base_url()?;
    let bytes = fetch(&format!("{base}/openpeon.json"), MAX_FILE)?;
    check(&bytes, p.manifest_sha256.as_deref(), &format!("{}'s manifest", p.name))?;
    let manifest = pack::parse_manifest(std::str::from_utf8(&bytes).context("the manifest isn't text")?)?;
    Ok((bytes, manifest))
}

/// Install `p` into the packs folder (over an older copy). `progress(done, total)` after each file.
pub fn install(p: &RegistryPack, mut progress: impl FnMut(usize, usize)) -> anyhow::Result<PathBuf> {
    if !valid_name(&p.name) {
        bail!("'{}' isn't a pack name", p.name);
    }
    let base = p.base_url()?;
    let (manifest_bytes, manifest) = fetch_manifest(p)?;
    let files = manifest.files();
    if files.is_empty() {
        bail!("{} lists no sounds", p.name);
    }
    let packs = super::packs_dir();
    std::fs::create_dir_all(&packs).with_context(|| format!("creating {}", packs.display()))?;
    let staging = packs.join(format!(".installing-{}-{:08x}", p.name, rand::random::<u32>()));
    let result = (|| -> anyhow::Result<()> {
        let mut total = 0u64;
        progress(0, files.len());
        for (i, s) in files.iter().enumerate() {
            let bytes = fetch(&format!("{base}/{}", s.file), MAX_FILE)?;
            total += bytes.len() as u64;
            if total > MAX_PACK {
                bail!("{} is bigger than bro expects of a sound pack", p.name);
            }
            check(&bytes, s.sha256.as_deref(), &s.file)?;
            let to = staging.join(&s.file);
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&to, &bytes).with_context(|| format!("writing {}", to.display()))?;
            progress(i + 1, files.len());
        }
        std::fs::write(staging.join("openpeon.json"), &manifest_bytes)?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let dest = packs.join(&p.name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).with_context(|| format!("replacing {}", dest.display()))?;
    }
    std::fs::rename(&staging, &dest).with_context(|| format!("moving the pack into {}", dest.display()))?;
    Ok(dest)
}

fn preview_dir(name: &str) -> PathBuf {
    crate::paths::bro_dir().join("cache").join("peon-preview").join(name)
}

/// One of `p`'s sounds on disk to listen to before installing: its `n`th preview sound (wrapping), fetched once
/// into bro's cache. An installed pack plays from its own folder.
pub fn preview(p: &RegistryPack, n: usize) -> anyhow::Result<PathBuf> {
    if !valid_name(&p.name) {
        bail!("'{}' isn't a pack name", p.name);
    }
    let installed = super::packs_dir().join(&p.name);
    let dir = preview_dir(&p.name);
    let manifest = match pack::load_manifest(&installed).or_else(|| pack::load_manifest(&dir)) {
        Some(m) => m,
        None => {
            let (bytes, m) = fetch_manifest(p)?;
            atomic_write(&dir.join("openpeon.json"), &bytes)?;
            m
        }
    };
    let files = manifest.files();
    // the registry's picks, else whatever the manifest lists
    let wanted: Vec<&pack::Sound> = p.preview_sounds.iter().filter_map(|f| files.iter().find(|s| pack::base_name(&s.file) == f.as_str()).copied()).collect();
    let pool = if wanted.is_empty() { files } else { wanted };
    let Some(sound) = pool.get(n % pool.len().max(1)) else { bail!("{} lists no sounds", p.name) };
    if let Some(path) = pack::sound_path(&installed, &sound.file) {
        return Ok(path);
    }
    let to = dir.join(pack::base_name(&sound.file));
    if !to.is_file() {
        let bytes = fetch(&format!("{}/{}", p.base_url()?, sound.file), MAX_FILE)?;
        check(&bytes, sound.sha256.as_deref(), &sound.file)?;
        atomic_write(&to, &bytes)?;
    }
    Ok(to)
}

/// Is `installed` older than what the registry lists (by version)?
pub fn outdated(installed: Option<&str>, listed: Option<&str>) -> bool {
    let parse = |v: &str| v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    match (installed, listed) {
        (Some(a), Some(b)) => parse(a) < parse(b),
        _ => false,
    }
}

/// Bytes as "440 KB" / "3.1 MB".
pub fn size_label(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b => format!("{} KB", (b / 1024).max(1)),
    }
}
