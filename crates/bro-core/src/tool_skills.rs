//! Skills attached to installed browser/desktop tools. Bro owns one complete cached tree per skill,
//! refreshes it off the UI thread, and fans it out to agent homes (including new profiles at launch).
//! Personal skills and links of the same name are never adopted or overwritten.
use crate::{agent_desktop, justbrowse, managed_skill, skill_repos};
use std::path::{Path, PathBuf};

pub const BROWSER: &str = "justbrowse-browser";

pub fn cache_dir(name: &str) -> PathBuf {
    crate::paths::bro_dir().join("tool-skills").join(name)
}

/// Publish a shipped skill tree to Bro's cache and all existing agent homes. Blocking, local files only.
pub fn cache(name: &str, source: &Path) -> Result<(), String> {
    if !source.join("SKILL.md").is_file() {
        return Err(format!("{} has no SKILL.md", source.display()));
    }
    let dest = cache_dir(name);
    skill_repos::mirror(source, &dest, true).map_err(|e| format!("caching {name}: {e}"))?;
    publish(name)
}

/// Fan out an existing cache without downloading anything (also used with auto-sync off).
pub fn publish(name: &str) -> Result<(), String> {
    for home in crate::justimagine::skill_homes() {
        managed_skill::write_tree(&home, name, &cache_dir(name)).map_err(|e| format!("writing {name} into {}: {e}", home.display()))?;
    }
    Ok(())
}

/// No network at launch. A tool that's absent does not add a stale skill to a new home.
pub fn write_cached(home: &Path) {
    if justbrowse::installed().is_some() {
        let _ = managed_skill::write_tree(home, BROWSER, &cache_dir(BROWSER));
    }
    write_desktop_cached(home);
}

/// CLI-based desktop automation also works in harnesses that do not expose browser MCP tools.
pub fn write_desktop_cached(home: &Path) {
    if agent_desktop::installed().is_some() {
        for name in agent_desktop::skill_names() {
            let _ = managed_skill::write_tree(home, name, &cache_dir(name));
        }
    }
}

/// Refresh the browser skill from its release bundle (older installs fall back to GitHub).
/// No global agent configuration edits.
pub fn refresh_browser() -> Result<(), String> {
    let shipped = justbrowse::install_dir().join("skills").join(BROWSER);
    if shipped.join("SKILL.md").is_file() {
        return cache(BROWSER, &shipped);
    }
    // Earlier upstream installers copied only into the local agent homes. Bring those installs
    // into Bro's managed layout without adopting or changing the user's existing copies.
    let listed = skill_repos::sync_at(justbrowse::REPO, &crate::paths::bro_dir().join("tool-sources/justbrowse"))?;
    let found = listed.found.iter().find(|f| f.name == BROWSER).ok_or("JustBrowse's repository has no browser skill")?;
    cache(BROWSER, &found.dir)
}

/// Pull the desktop tool's complete skill trees, not just SKILL.md. Other platforms, FFI and the optional
/// model-provider helper remain out of the agents' way. Blocking; needs Git and the network.
pub fn refresh_desktop() -> Result<(), String> {
    let listed = skill_repos::sync_at(agent_desktop::REPO, &crate::paths::bro_dir().join("tool-sources/agent-desktop"))?;
    for name in agent_desktop::skill_names() {
        let found = listed.found.iter().find(|f| f.name == name).ok_or_else(|| format!("{} has no {name} skill", agent_desktop::REPO))?;
        cache(name, &found.dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{atomic_write, test_env::sandbox};

    #[test]
    fn complete_trees_refresh_but_personal_and_other_repo_copies_stay() {
        let sb = sandbox();
        let source = sb.home().join("source");
        atomic_write(&source.join("SKILL.md"), b"---\nname: example\n---\nv1").unwrap();
        atomic_write(&source.join("references").join("guide.md"), b"the whole guide").unwrap();
        atomic_write(&source.join("assets").join("image.bin"), &[0, 255, 42]).unwrap();
        let home = sb.home().join(".claude");
        std::fs::create_dir_all(&home).unwrap();
        cache("example", &source).unwrap();
        let copy = home.join("skills/example");
        assert!(copy.join("references/guide.md").is_file());
        assert_eq!(std::fs::read(copy.join("assets/image.bin")).unwrap(), [0, 255, 42]);
        assert_eq!(skill_repos::owner(&copy).as_deref(), Some(""));
        std::fs::remove_dir_all(source.join("assets")).unwrap();
        atomic_write(&source.join("SKILL.md"), b"v2").unwrap();
        cache("example", &source).unwrap();
        assert!(!copy.join("assets").exists());
        assert_eq!(std::fs::read_to_string(copy.join("SKILL.md")).unwrap(), "v2");

        let personal = sb.home().join("personal");
        atomic_write(&personal.join("skills/example/SKILL.md"), b"mine").unwrap();
        assert!(!managed_skill::write_tree(&personal, "example", &source).unwrap());
        assert_eq!(std::fs::read_to_string(personal.join("skills/example/SKILL.md")).unwrap(), "mine");
        atomic_write(&personal.join("skills/example/.bro-managed"), b"written by bro from other/repo; keep").unwrap();
        assert!(!managed_skill::write_tree(&personal, "example", &source).unwrap());
        let new_home = sb.home().join("new-profile");
        assert!(managed_skill::write_tree(&new_home, "example", &cache_dir("example")).unwrap());
        assert!(new_home.join("skills/example/references/guide.md").is_file());
    }
}
