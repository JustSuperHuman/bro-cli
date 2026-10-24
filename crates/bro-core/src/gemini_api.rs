//! Gemini API key profiles, model discovery, and usage recorded by Bro's proxy.
use crate::{catalogue::ModelRow, config::Config, profiles::Profile};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
pub const STUDIO: &str = "https://aistudio.google.com/usage";

/// Existing keys stay in their original location. Never copy an environment key to disk.
pub fn detected_key(cfg: &Config) -> Option<String> {
    cfg.key_for("gemini-api", Some("GEMINI_API_KEY"))
        .or_else(|| cfg.key_for("google", Some("GOOGLE_API_KEY")))
        .or_else(|| cfg.key_for("gemini", None))
}

pub fn key(profile: &Profile) -> Option<String> {
    if profile.name == "local" {
        return detected_key(&Config::load().ok()?);
    }
    let stored = crate::util::read_json(&profile.dir.join("api-key.json"))?;
    if stored.get("source").and_then(Value::as_str) == Some("detected") {
        return detected_key(&Config::load().ok()?);
    }
    stored
        .get("key")?
        .as_str()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
}

pub fn save_detected_key(dir: &Path) -> anyhow::Result<()> {
    crate::util::write_json_pretty(&dir.join("api-key.json"), &json!({"source": "detected"}))?;
    Ok(())
}

/// Save only the secret, with the same permission restrictions as other Bro credentials.
pub fn save_key(dir: &Path, key: &str) -> anyhow::Result<()> {
    let key = key.trim();
    if key.is_empty() || key.chars().any(char::is_control) {
        bail!("Paste a Gemini API key.");
    }
    let path = dir.join("api-key.json");
    crate::util::write_json_pretty(&path, &json!({"key": key}))?;
    crate::creds::restrict_permissions(&path);
    Ok(())
}

/// Validate credentials without generating content or charging for a test prompt.
/// Keys travel in a header, never in a URL or error message.
pub fn models(key: &str) -> anyhow::Result<Vec<ModelRow>> {
    let mut url =
        url::Url::parse("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000")?;
    let mut out = vec![];
    for _ in 0..10 {
        let r = crate::http::get_json(
            url.as_str(),
            &[("x-goog-api-key", key)],
            Duration::from_secs(12),
        )
        .map_err(|_| {
            anyhow::anyhow!("Could not reach Gemini. Check your connection and try again.")
        })?;
        if !r.ok() {
            bail!(
                "{}",
                match r.status {
                    400 | 401 => "Gemini did not accept this API key. Check the key in AI Studio.",
                    403 =>
                        "This key cannot access Gemini models. Check its API restrictions in AI Studio.",
                    429 => "Gemini is rate limiting key checks. Try again shortly.",
                    _ => "Gemini could not check this key. Try again shortly.",
                }
            );
        }
        out.extend(parse_models(&r.body));
        let Some(next) = r
            .body
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            break;
        };
        url.query_pairs_mut()
            .clear()
            .append_pair("pageSize", "1000")
            .append_pair("pageToken", next);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    if out.is_empty() {
        bail!("This key has no Gemini text models available.");
    }
    Ok(out)
}

fn parse_models(body: &Value) -> Vec<ModelRow> {
    body.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("name")?.as_str()?.strip_prefix("models/")?;
            let generates = m
                .get("supportedGenerationMethods")?
                .as_array()?
                .iter()
                .any(|v| v == "generateContent");
            // These models generate media or embeddings and cannot run a coding session.
            if !generates
                || !id.starts_with("gemini-")
                || ["image", "tts", "audio", "robotics", "computer-use"]
                    .iter()
                    .any(|s| id.contains(s))
            {
                return None;
            }
            Some(ModelRow {
                id: id.into(),
                name: m
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .into(),
                context: m.get("inputTokenLimit").and_then(Value::as_u64),
                pricing: None,
                reasoning: true,
                created: None,
            })
        })
        .collect()
}

/// A local day's totals. Missing token metadata stays visible as incomplete coverage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub requests: u64,
    pub errors: u64,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub metered: u64,
}

impl Totals {
    fn add(&mut self, other: &Self) {
        self.requests += other.requests;
        self.errors += other.errors;
        self.input += other.input;
        self.output += other.output;
        self.cached += other.cached;
        self.metered += other.metered;
    }
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cached
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiUsage {
    pub today: Totals,
    pub month: Totals,
}

type Ledger = BTreeMap<String, BTreeMap<String, Totals>>;

fn ledger_path() -> std::path::PathBuf {
    crate::paths::bro_dir().join("gemini-usage.json")
}

pub fn usage(id: &str) -> anyhow::Result<ApiUsage> {
    usage_at(&ledger_path(), id, chrono::Local::now().date_naive())
}

fn usage_at(path: &Path, id: &str, today: chrono::NaiveDate) -> anyhow::Result<ApiUsage> {
    let ledger = read_ledger(path)?;
    let mut out = ApiUsage::default();
    let day = today.to_string();
    let month = today.format("%Y-%m").to_string();
    if let Some(days) = ledger.get(id) {
        out.today = days.get(&day).cloned().unwrap_or_default();
        for (_, total) in days.iter().filter(|(d, _)| d.starts_with(&month)) {
            out.month.add(total);
        }
    }
    Ok(out)
}

fn read_ledger(path: &Path) -> anyhow::Result<Ledger> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Could not read Gemini usage history."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
        Err(e) => Err(e).context("Could not read Gemini usage history."),
    }
}

/// Called once per completed proxy request. No prompts, keys, or responses enter the ledger.
pub fn record(
    id: &str,
    at_ms: i64,
    status: u16,
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
) -> anyhow::Result<()> {
    record_at(&ledger_path(), id, at_ms, status, input, output, cached)
}

fn record_at(
    path: &Path,
    id: &str,
    at_ms: i64,
    status: u16,
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
) -> anyhow::Result<()> {
    use chrono::TimeZone;
    let lock = crate::creds::dir_lock(path);
    let _guard = lock.lock();
    let date = chrono::Local
        .timestamp_millis_opt(at_ms)
        .single()
        .context("Invalid usage timestamp")?
        .date_naive();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    file_lock.lock()?;
    let mut ledger = read_ledger(path)?;
    let total = ledger
        .entry(id.into())
        .or_default()
        .entry(date.to_string())
        .or_default();
    total.add(&Totals {
        requests: 1,
        errors: u64::from(!(200..300).contains(&status)),
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        cached: cached.unwrap_or(0),
        metered: u64::from(input.is_some() && output.is_some()),
    });
    let cutoff = (date - chrono::Duration::days(62)).to_string();
    for days in ledger.values_mut() {
        days.retain(|d, _| d >= &cutoff);
    }
    crate::util::write_json_pretty(path, &ledger)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_keys_become_profiles_without_copying_environment_secrets() {
        let _sandbox = crate::util::test_env::sandbox();
        assert!(!crate::profiles::list().iter().any(Profile::is_gemini));
        unsafe {
            std::env::set_var("GOOGLE_API_KEY", "fake-google-key");
        }
        let local = crate::profiles::list()
            .into_iter()
            .find(Profile::is_gemini)
            .unwrap();
        assert_eq!(local.id, "gemini:local");
        assert!(local.is_local() && local.authenticated);
        assert!(!local.is_claude() && !local.is_codex());
        assert!(crate::profiles::remove(&local.id).is_err());
        let work =
            crate::profiles::create(crate::profiles::ProfileKind::GeminiApi, "work").unwrap();
        save_detected_key(&work.dir).unwrap();
        assert_eq!(key(&work).as_deref(), Some("fake-google-key"));
        assert!(
            !std::fs::read_to_string(work.dir.join("api-key.json"))
                .unwrap()
                .contains("fake-google-key")
        );
        unsafe {
            std::env::set_var("GOOGLE_API_KEY", "rotated-google-key");
        }
        assert_eq!(key(&work).as_deref(), Some("rotated-google-key"));
        save_key(&work.dir, "separate-work-key").unwrap();
        assert_eq!(key(&work).as_deref(), Some("separate-work-key"));
        crate::profiles::remove(&work.id).unwrap();
        assert!(crate::profiles::get(&work.id).is_none());
        assert!(crate::profiles::parse_id("gemini:../escape").is_none());
    }

    #[test]
    fn ledger_rolls_over_local_days_and_calendar_months() {
        use chrono::TimeZone;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let stamp = |month, day| {
            chrono::Local
                .with_ymd_and_hms(2026, month, day, 12, 0, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        };
        record_at(
            &path,
            "gemini:work",
            stamp(9, 30),
            200,
            Some(10),
            Some(1),
            None,
        )
        .unwrap();
        record_at(
            &path,
            "gemini:work",
            stamp(10, 1),
            200,
            Some(20),
            Some(2),
            None,
        )
        .unwrap();
        record_at(
            &path,
            "gemini:work",
            stamp(10, 2),
            200,
            Some(30),
            Some(3),
            None,
        )
        .unwrap();
        let u = usage_at(
            &path,
            "gemini:work",
            chrono::NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
        )
        .unwrap();
        assert_eq!(u.today.tokens(), 33);
        assert_eq!(u.month.tokens(), 55);
        assert_eq!(u.month.requests, 2);
    }

    #[test]
    fn model_discovery_excludes_media_and_embedding_models() {
        let models = parse_models(&json!({"models": [
            {"name":"models/gemini-text", "displayName":"Text", "inputTokenLimit":1000, "supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-image", "supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-embedding", "supportedGenerationMethods":["embedContent"]}
        ]}));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-text");
        assert_eq!(models[0].context, Some(1000));
    }

    #[test]
    fn ledger_survives_restart_and_separates_profiles_and_missing_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let now = crate::util::now_ms();
        record_at(
            &path,
            "gemini:work",
            now,
            200,
            Some(100),
            Some(20),
            Some(10),
        )
        .unwrap();
        record_at(&path, "gemini:work", now, 429, None, None, None).unwrap();
        record_at(&path, "gemini:other", now, 200, Some(999), Some(1), None).unwrap();
        let u = usage_at(&path, "gemini:work", chrono::Local::now().date_naive()).unwrap();
        assert_eq!(u.today.requests, 2);
        assert_eq!(u.today.errors, 1);
        assert_eq!(u.today.metered, 1);
        assert_eq!(u.month.tokens(), 130);
        std::fs::write(&path, b"invalid").unwrap();
        assert!(record_at(&path, "gemini:work", now, 200, None, None, None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
    }
}
