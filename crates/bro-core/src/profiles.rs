//! A profile is one isolated login directory (v1 semantics, no schema file).
//!
//! * Claude: `~/.claude` ("claude:local") and each `~/.claude-max-pool/accounts/<name>`
//!   (a `CLAUDE_CONFIG_DIR`, v1 `claude-accounts.js`).
//! * Codex: `~/.codex` ("codex:local") and each `~/.bro/codex-profiles/<name>`
//!   (a `CODEX_HOME`, v1 `codex-profiles.js`).
use crate::creds;
use crate::paths;
use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// `~/.claude`
    ClaudeLocal,
    /// `~/.claude-max-pool/accounts/<name>` (a CLAUDE_CONFIG_DIR)
    ClaudeAccount,
    /// `~/.codex`
    CodexLocal,
    /// `~/.bro/codex-profiles/<name>` (a CODEX_HOME)
    CodexProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Stable id: "claude:local", "claude:<name>", "codex:local", "codex:<name>"
    pub id: String,
    pub kind: ProfileKind,
    pub name: String,
    pub dir: std::path::PathBuf,
    pub authenticated: bool,
    /// e.g. "max", "pro", "plus" (Claude subscriptionType / Codex chatgpt_plan_type)
    pub plan: Option<String>,
    /// Claude rateLimitTier, if any
    pub tier: Option<String>,
    /// Dedup identity: Claude `accountUuid:organizationUuid`, Codex account user id
    pub identity: Option<String>,
    pub email: Option<String>,
}

impl Profile {
    pub fn is_claude(&self) -> bool { matches!(self.kind, ProfileKind::ClaudeLocal | ProfileKind::ClaudeAccount) }
    pub fn is_codex(&self) -> bool { matches!(self.kind, ProfileKind::CodexLocal | ProfileKind::CodexProfile) }
    /// The machine's own login (`claude:local` / `codex:local`).
    pub fn is_local(&self) -> bool { matches!(self.kind, ProfileKind::ClaudeLocal | ProfileKind::CodexLocal) }
}

/// Settings copied into a new Codex profile from `~/.codex` (v1 `SEEDED`).
const CODEX_SEEDED: [&str; 6] = ["config.toml", "AGENTS.md", "prompts", "skills", "hooks", "hooks.json"];

fn sorted_subdirs(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Build a profile from its kind and directory (reads its credential files).
pub fn describe(kind: ProfileKind, name: &str, dir: PathBuf) -> Profile {
    let (prefix, label) = match kind {
        ProfileKind::ClaudeLocal => ("claude", "local"),
        ProfileKind::CodexLocal => ("codex", "local"),
        ProfileKind::ClaudeAccount => ("claude", name),
        ProfileKind::CodexProfile => ("codex", name),
    };
    let mut p = Profile {
        id: format!("{prefix}:{label}"),
        kind,
        name: label.to_string(),
        dir,
        authenticated: false,
        plan: None,
        tier: None,
        identity: None,
        email: None,
    };
    if p.is_claude() {
        let login = creds::claude_login(&p.dir);
        p.authenticated = login.authenticated;
        p.plan = login.plan;
        p.tier = login.tier;
        p.identity = login.identity;
        p.email = login.email;
    } else {
        let login = creds::codex_login(&p.dir);
        p.authenticated = login.authenticated;
        p.plan = login.plan;
        p.identity = login.identity;
        p.email = login.email;
    }
    p
}

/// Claude profiles only: local first, then pool accounts alphabetically.
pub fn list_claude() -> Vec<Profile> {
    let mut out = vec![describe(ProfileKind::ClaudeLocal, "local", paths::claude_local_dir())];
    let root = paths::claude_accounts_dir();
    out.extend(sorted_subdirs(&root).into_iter().map(|n| {
        let dir = root.join(&n);
        describe(ProfileKind::ClaudeAccount, &n, dir)
    }));
    out
}

/// Codex profiles only: local first, then bro's codex profiles alphabetically.
pub fn list_codex() -> Vec<Profile> {
    let mut out = vec![describe(ProfileKind::CodexLocal, "local", paths::codex_local_dir())];
    let root = paths::codex_profiles_dir();
    out.extend(sorted_subdirs(&root).into_iter().map(|n| {
        let dir = root.join(&n);
        describe(ProfileKind::CodexProfile, &n, dir)
    }));
    out
}

/// All profiles, local first then alphabetical. Cheap (reads small JSON files).
pub fn list() -> Vec<Profile> {
    let mut all = list_claude();
    all.extend(list_codex());
    all
}

/// Split "claude:work" into its family and name.
pub fn parse_id(id: &str) -> Option<(ProfileKind, &str)> {
    let (family, name) = id.split_once(':')?;
    match (family, name) {
        ("claude", "local") => Some((ProfileKind::ClaudeLocal, name)),
        ("codex", "local") => Some((ProfileKind::CodexLocal, name)),
        ("claude", n) if valid_name(n) => Some((ProfileKind::ClaudeAccount, n)),
        ("codex", n) if valid_name(n) => Some((ProfileKind::CodexProfile, n)),
        _ => None,
    }
}

/// The directory a profile id maps to, whether or not it exists.
pub fn dir_for(kind: ProfileKind, name: &str) -> PathBuf {
    match kind {
        ProfileKind::ClaudeLocal => paths::claude_local_dir(),
        ProfileKind::CodexLocal => paths::codex_local_dir(),
        ProfileKind::ClaudeAccount => paths::claude_accounts_dir().join(name),
        ProfileKind::CodexProfile => paths::codex_profiles_dir().join(name),
    }
}

/// A profile by id. Local profiles always resolve; named ones only if their dir exists.
pub fn get(id: &str) -> Option<Profile> {
    let (kind, name) = parse_id(id)?;
    let dir = dir_for(kind, name);
    let named = matches!(kind, ProfileKind::ClaudeAccount | ProfileKind::CodexProfile);
    if named && !dir.is_dir() {
        return None;
    }
    Some(describe(kind, name, dir))
}

/// Create an empty Claude account dir / seeded Codex profile (v1 `seedProfile`).
pub fn create(kind: ProfileKind, name: &str) -> anyhow::Result<Profile> {
    let name = name.trim();
    if !valid_name(name) {
        bail!("Invalid profile name: {name:?} (letters, digits, dot, dash and underscore only)");
    }
    let dir = dir_for(kind, name);
    match kind {
        ProfileKind::ClaudeLocal | ProfileKind::CodexLocal => bail!("the local profile always exists"),
        ProfileKind::ClaudeAccount => {
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        ProfileKind::CodexProfile => {
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let from = paths::codex_local_dir();
            for item in CODEX_SEEDED {
                let (src, dst) = (from.join(item), dir.join(item));
                if src.exists() && !dst.exists() {
                    // Settings are a convenience — a profile without them still works.
                    let _ = copy_tree(&src, &dst);
                }
            }
        }
    }
    Ok(describe(kind, name, dir))
}

/// Delete a named profile's directory. Local logins are never removed.
pub fn remove(id: &str) -> anyhow::Result<()> {
    let (kind, name) = parse_id(id).ok_or_else(|| anyhow!("unknown profile id {id:?}"))?;
    if matches!(kind, ProfileKind::ClaudeLocal | ProfileKind::CodexLocal) {
        bail!("refusing to remove the machine's own login ({id})");
    }
    let dir = dir_for(kind, name);
    if !dir.exists() {
        bail!("no such profile: {id}");
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
}

/// `^[A-Za-z0-9._-]+$`
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Command that performs an interactive login for this profile inside a PTY
/// (claude with CLAUDE_CONFIG_DIR then `/login`; `codex login` with CODEX_HOME).
pub fn login_command(profile: &Profile) -> crate::launch::CommandSpec {
    let cwd = crate::util::home_dir();
    if profile.is_claude() {
        let (program, args) = crate::launch::program_and_args("claude", Vec::new());
        let mut spec = crate::launch::CommandSpec {
            program,
            args,
            cwd,
            label: format!("login · claude · {}", profile.name),
            env_remove: ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"].map(String::from).to_vec(),
            ..Default::default()
        };
        if profile.kind == ProfileKind::ClaudeLocal {
            spec.env_remove.push("CLAUDE_CONFIG_DIR".into());
        } else {
            spec.env.push(("CLAUDE_CONFIG_DIR".into(), profile.dir.to_string_lossy().into_owned()));
        }
        spec
    } else {
        let (program, args) = crate::launch::program_and_args("codex", vec!["login".into()]);
        crate::launch::CommandSpec {
            program,
            args,
            cwd,
            label: format!("login · codex · {}", profile.name),
            env: vec![("CODEX_HOME".into(), profile.dir.to_string_lossy().into_owned())],
            ..Default::default()
        }
    }
}

/// Recursive copy (files and directories).
pub(crate) fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn lists_local_and_accounts_in_order() {
        let sb = sandbox();
        let accounts = paths::claude_accounts_dir();
        write(&accounts.join("zeta").join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a","subscriptionType":"max","rateLimitTier":"t20"}}"#);
        write(&accounts.join("Alpha").join(".credentials.json"), "{ broken json");
        std::fs::create_dir_all(accounts.join("beta")).unwrap();
        write(&accounts.join("beta").join(".claude.json"), r#"{"oauthAccount":{"accountUuid":"u1","organizationUuid":"o1","emailAddress":"b@x"}}"#);
        write(&paths::codex_profiles_dir().join("smol").join("config.toml"), "");
        write(&sb.home().join(".claude").join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"L"}}"#);

        let all = list();
        let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["claude:local", "claude:Alpha", "claude:beta", "claude:zeta", "codex:local", "codex:smol"]);
        assert!(all[0].authenticated);
        assert!(!all[1].authenticated);
        assert_eq!(all[2].identity.as_deref(), Some("u1:o1"));
        assert_eq!(all[2].email.as_deref(), Some("b@x"));
        assert!(all[3].authenticated);
        assert_eq!(all[3].plan.as_deref(), Some("max"));
        assert_eq!(all[3].tier.as_deref(), Some("t20"));
        assert!(!all[5].authenticated);
        assert!(get("claude:zeta").unwrap().authenticated);
        assert!(get("claude:nope").is_none());
        assert!(get("codex:local").is_some());
        assert!(get("bogus").is_none());
    }

    #[test]
    fn create_seed_and_remove() {
        let sb = sandbox();
        write(&sb.home().join(".codex").join("config.toml"), "model = \"x\"");
        write(&sb.home().join(".codex").join("prompts").join("a.md"), "hi");
        write(&sb.home().join(".codex").join("auth.json"), "{}");
        let p = create(ProfileKind::CodexProfile, "work").unwrap();
        assert_eq!(p.id, "codex:work");
        assert!(p.dir.join("config.toml").exists());
        assert!(p.dir.join("prompts").join("a.md").exists());
        assert!(!p.dir.join("auth.json").exists());
        let c = create(ProfileKind::ClaudeAccount, "c1").unwrap();
        assert!(c.dir.is_dir());
        assert!(create(ProfileKind::ClaudeAccount, "../x").is_err());
        assert!(create(ProfileKind::ClaudeLocal, "x").is_err());
        remove("codex:work").unwrap();
        assert!(!p.dir.exists());
        assert!(remove("claude:local").is_err());
    }

    #[test]
    fn names() {
        assert!(valid_name("a.b-c_1"));
        assert!(!valid_name(""));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a b"));
    }

    #[test]
    fn login_commands() {
        let _sb = sandbox();
        let acct = describe(ProfileKind::ClaudeAccount, "w", paths::claude_accounts_dir().join("w"));
        let spec = login_command(&acct);
        assert!(spec.env.iter().any(|(k, v)| k == "CLAUDE_CONFIG_DIR" && v.ends_with('w')));
        let local = describe(ProfileKind::ClaudeLocal, "local", paths::claude_local_dir());
        assert!(login_command(&local).env_remove.contains(&"CLAUDE_CONFIG_DIR".to_string()));
        let cx = describe(ProfileKind::CodexProfile, "s", paths::codex_profiles_dir().join("s"));
        let spec = login_command(&cx);
        assert_eq!(spec.args.last().map(String::as_str), Some("login"));
        assert!(spec.env.iter().any(|(k, _)| k == "CODEX_HOME"));
    }
}
