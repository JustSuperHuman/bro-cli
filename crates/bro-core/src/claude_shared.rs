//! What every Claude account profile inherits from the machine's own `~/.claude`:
//! its skills and its MCP servers.
//!
//! * **Skills**: `<account>/skills` becomes a link to `~/.claude/skills` (a symlink, or
//!   a junction on Windows when symlinks need elevation), so a skill added or edited
//!   anywhere is the same skill everywhere. An account that already keeps skills of its
//!   own is left as a real directory and gets one link per root skill instead.
//! * **MCP servers**: an account's `.claude.json` also holds its login, so it can't be
//!   linked. The root's user-scope servers (plus the ones it registered for the launch
//!   directory) are written to `<account>/bro-shared/mcp-servers.json` and handed to
//!   Claude Code with `--mcp-config`. Servers the account defines itself win.
use crate::paths;
use crate::util::{home_dir, read_json};
use serde_json::{Map, Value, json};
use std::io;
use std::path::{Path, PathBuf};

/// Skills bro itself writes into a harness home; a `skills` dir holding nothing else
/// is safe to replace with the shared link.
const BRO_SKILLS: [&str; 2] = [crate::skills::BRO_OPEN_SKILL_NAME, "bro-browser"];

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn same_target(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// `dir` is the machine's own `~/.claude`, which has nothing to inherit.
fn is_root(dir: &Path) -> bool {
    let root = paths::claude_local_dir();
    crate::util::same_path(dir, &root) || same_target(dir, &root)
}

#[cfg(windows)]
fn make_link(target: &Path, link: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return Ok(());
    }
    // Symlinks need Developer Mode or elevation; junctions don't.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("mklink /J {} failed", link.display()))) }
}

#[cfg(not(windows))]
fn make_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Removes the link itself, never what it points at.
fn remove_link(link: &Path) -> io::Result<()> {
    if cfg!(windows) { std::fs::remove_dir(link) } else { std::fs::remove_file(link) }
}

/// Points `<account>/skills` at the root `~/.claude/skills`. Idempotent and cheap when
/// already linked; a no-op when the root has no skills.
pub fn share_skills(account: &Path) -> io::Result<()> {
    let root = paths::claude_local_dir().join("skills");
    let link = account.join("skills");
    if !root.is_dir() || same_target(&root, &link) || is_root(account) {
        return Ok(());
    }
    let Ok(meta) = std::fs::symlink_metadata(&link) else {
        std::fs::create_dir_all(account)?;
        return make_link(&root, &link);
    };
    if meta.file_type().is_symlink() {
        // Stale or dangling link (the root moved): re-point it.
        remove_link(&link)?;
        return make_link(&root, &link);
    }
    if !meta.is_dir() {
        return Ok(());
    }
    let own: Vec<_> = std::fs::read_dir(&link)?.flatten().collect();
    if own.iter().all(|e| !is_link(&e.path()) && BRO_SKILLS.iter().any(|s| e.file_name() == *s)) {
        std::fs::remove_dir_all(&link)?;
        return make_link(&root, &link);
    }
    // The account has skills of its own: keep them, link each root skill beside them.
    for entry in own {
        let path = entry.path();
        if is_link(&path) && !path.exists() {
            let _ = remove_link(&path);
        }
    }
    for entry in std::fs::read_dir(&root)?.flatten() {
        let (src, dst) = (entry.path(), link.join(entry.file_name()));
        if src.is_dir() && std::fs::symlink_metadata(&dst).is_err() {
            let _ = make_link(&src, &dst);
        }
    }
    Ok(())
}

/// Drops the links [`share_skills`] made, so deleting a profile can never reach
/// through them into the root's skills.
pub fn unshare_skills(account: &Path) {
    let link = account.join("skills");
    if is_link(&link) {
        let _ = remove_link(&link);
        return;
    }
    for entry in std::fs::read_dir(&link).into_iter().flatten().flatten() {
        if is_link(&entry.path()) {
            let _ = remove_link(&entry.path());
        }
    }
}

/// Claude Code's project keys: forward slashes, compared case-insensitively on Windows.
fn project_key(path: &str) -> String {
    let key = path.replace('\\', "/");
    let key = key.trim_end_matches('/');
    if cfg!(windows) { key.to_lowercase() } else { key.to_string() }
}

fn servers(doc: &Value) -> impl Iterator<Item = (&String, &Value)> {
    doc.get("mcpServers").and_then(Value::as_object).into_iter().flatten()
}

fn project_servers<'a>(doc: &'a Value, cwd: &Path) -> impl Iterator<Item = (&'a String, &'a Value)> {
    let want = project_key(&cwd.to_string_lossy());
    doc.get("projects")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(move |(key, _)| project_key(key) == want)
        .flat_map(|(_, project)| servers(project))
}

/// The root's MCP servers for a session in `cwd`: user scope, then the ones it
/// registered for that directory, minus `skip`.
fn root_servers(cwd: &Path, skip: &dyn Fn(&str) -> bool) -> Map<String, Value> {
    // Claude Code keeps the local login's `.claude.json` in `$HOME`.
    let doc = read_json(&home_dir().join(".claude.json"))
        .or_else(|| read_json(&paths::claude_local_dir().join(".claude.json")))
        .unwrap_or(Value::Null);
    let mut out = Map::new();
    for (name, server) in servers(&doc).chain(project_servers(&doc, cwd)) {
        if server.is_object() && !skip(name) {
            out.insert(name.clone(), server.clone());
        }
    }
    out
}

/// `<account>/bro-shared/mcp-servers.json`
pub fn shared_mcp_path(account: &Path) -> PathBuf {
    account.join("bro-shared").join("mcp-servers.json")
}

/// Writes the root's MCP servers for a session of `account` in `cwd` and returns the
/// file to pass as `--mcp-config`; None when there is nothing to share.
pub fn write_shared_mcp(account: &Path, cwd: &Path) -> io::Result<Option<PathBuf>> {
    if is_root(account) {
        return Ok(None);
    }
    let own = read_json(&account.join(".claude.json")).unwrap_or(Value::Null);
    let own: Vec<&String> = servers(&own).chain(project_servers(&own, cwd)).map(|(name, _)| name).collect();
    let shared = root_servers(cwd, &|name| own.iter().any(|n| *n == name));
    if shared.is_empty() {
        return Ok(None);
    }
    let path = shared_mcp_path(account);
    let body = serde_json::to_string_pretty(&json!({ "mcpServers": shared })).map_err(io::Error::other)?;
    crate::browser::write_if_changed(&path, &body)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("SKILL.md"), name).unwrap();
    }

    #[test]
    fn account_skills_link_to_the_root() {
        let _sb = sandbox();
        let root = paths::claude_local_dir().join("skills");
        let account = paths::claude_accounts_dir().join("work");
        std::fs::create_dir_all(&account).unwrap();
        // No root skills yet: nothing to link.
        share_skills(&account).unwrap();
        assert!(!account.join("skills").exists());

        skill(&root, "alpha");
        // A dir holding only bro's own skills is replaced by the link.
        skill(&account.join("skills"), "bro-open");
        share_skills(&account).unwrap();
        share_skills(&account).unwrap();
        assert!(is_link(&account.join("skills")));
        assert!(account.join("skills").join("alpha").join("SKILL.md").is_file());
        // Shared both ways.
        skill(&account.join("skills"), "beta");
        assert!(root.join("beta").join("SKILL.md").is_file());

        // Removing the profile leaves the root's skills alone.
        crate::profiles::remove("claude:work").unwrap();
        assert!(!account.exists());
        assert!(root.join("alpha").join("SKILL.md").is_file());
        assert!(root.join("beta").join("SKILL.md").is_file());
    }

    #[test]
    fn an_accounts_own_skills_are_kept() {
        let _sb = sandbox();
        let root = paths::claude_local_dir().join("skills");
        let account = paths::claude_accounts_dir().join("work");
        skill(&root, "alpha");
        skill(&root, "gone");
        skill(&account.join("skills"), "mine");
        skill(&account.join("skills"), "alpha");
        share_skills(&account).unwrap();
        let own = account.join("skills");
        assert!(!is_link(&own));
        assert_eq!(std::fs::read_to_string(own.join("mine").join("SKILL.md")).unwrap(), "mine");
        assert!(!is_link(&own.join("alpha")), "the account's own copy wins");
        assert!(is_link(&own.join("gone")));
        // A root skill that went away takes its link with it.
        std::fs::remove_dir_all(root.join("gone")).unwrap();
        share_skills(&account).unwrap();
        assert!(std::fs::symlink_metadata(own.join("gone")).is_err());

        unshare_skills(&account);
        std::fs::remove_dir_all(&account).unwrap();
        assert!(root.join("alpha").join("SKILL.md").is_file());
    }

    #[test]
    fn root_mcp_servers_are_written_for_the_account() {
        let sb = sandbox();
        let account = paths::claude_accounts_dir().join("work");
        std::fs::create_dir_all(&account).unwrap();
        let cwd = sb.home().join("proj");
        assert!(write_shared_mcp(&account, &cwd).unwrap().is_none());

        let key = cwd.to_string_lossy().replace('\\', "/");
        let root = json!({
            "oauthAccount": { "emailAddress": "a@b.c" },
            "mcpServers": { "user": { "type": "http", "url": "http://u" }, "both": { "type": "http", "url": "http://root" } },
            "projects": {
                key: { "mcpServers": { "local": { "type": "stdio", "command": "x" } } },
                "/elsewhere": { "mcpServers": { "other": { "type": "stdio", "command": "y" } } }
            }
        });
        std::fs::write(sb.home().join(".claude.json"), root.to_string()).unwrap();
        std::fs::write(account.join(".claude.json"), json!({ "mcpServers": { "both": { "type": "http", "url": "http://own" } } }).to_string()).unwrap();

        let path = write_shared_mcp(&account, &cwd).unwrap().unwrap();
        assert_eq!(path, shared_mcp_path(&account));
        let doc = read_json(&path).unwrap();
        let names: Vec<&String> = doc["mcpServers"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["user", "local"]);
        assert_eq!(doc.as_object().unwrap().len(), 1, "nothing but servers leaves the root file");
        // The local login never shares with itself.
        assert!(write_shared_mcp(&paths::claude_local_dir(), &cwd).unwrap().is_none());
    }
}
