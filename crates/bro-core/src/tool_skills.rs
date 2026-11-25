//! Skills attached to installed browser/desktop tools. Bro owns one complete cached tree per skill,
//! refreshes it off the UI thread, and fans it out to agent homes (including new profiles at launch).
//! Personal skills and links of the same name are never adopted or overwritten.
use crate::util::{home_dir, same_path};
use crate::{agent_desktop, justbrowse, managed_skill, paths, skill_repos};
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
    let mut homes = crate::justimagine::skill_homes();
    // `~/.agents` before the Codex homes, which only need a copy while it has none.
    homes.sort_by_key(|h| is_codex_home(h));
    for home in homes {
        write_home(&home, name).map_err(|e| format!("writing {name} into {}: {e}", home.display()))?;
    }
    Ok(())
}

fn is_codex_home(home: &Path) -> bool {
    same_path(home, &paths::codex_local_dir()) || home.parent().is_some_and(|p| same_path(p, &paths::codex_profiles_dir()))
}

/// Codex reads `~/.agents/skills` whatever its CODEX_HOME, so with a copy there one in a Codex home
/// lists the skill twice. Bro's own copy steps aside; a personal one stays. A Codex `skills` folder
/// linked to `~/.agents/skills` is that same folder and is written as usual.
fn write_home(home: &Path, name: &str) -> std::io::Result<bool> {
    let agents = home_dir().join(".agents").join("skills");
    let canonical = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    if is_codex_home(home) && agents.join(name).join("SKILL.md").is_file() && !same_path(&canonical(home.join("skills")), &canonical(agents)) {
        managed_skill::remove(home, name);
        return Ok(false);
    }
    managed_skill::write_tree(home, name, &cache_dir(name))
}

/// No network at launch. A tool that's absent does not add a stale skill to a new home.
pub fn write_cached(home: &Path) {
    let active = crate::automation::Active::load();
    write_configured(home, &active);
}

pub fn write_configured(home: &Path, active: &crate::automation::Active) {
    if active.browser {
        let _ = write_home(home, BROWSER);
    } else {
        managed_skill::remove(home, BROWSER);
    }
    if active.desktop.is_some() {
        let _ = write_home(home, crate::cua::SKILL);
    } else {
        managed_skill::remove(home, crate::cua::SKILL);
    }
    write_desktop_cached(home);
}

/// CLI-based desktop automation also works in harnesses that do not expose browser MCP tools.
pub fn write_desktop_cached(home: &Path) {
    if agent_desktop::installed().is_some() {
        for name in agent_desktop::skill_names() {
            let _ = write_home(home, name);
        }
    }
}

/// Refresh the browser skill from the install itself: Quick Install's release bundle or the checkout
/// the registered bridge runs from. Installs that ship none fall back to GitHub (needs Git and the
/// network). No global agent configuration edits.
pub fn refresh_browser() -> Result<(), String> {
    if let Some(shipped) = justbrowse::installed().and_then(|found| found.skill) {
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

    #[test]
    fn codex_homes_lean_on_the_shared_agents_folder() {
        let sb = sandbox();
        let source = sb.home().join("source");
        atomic_write(&source.join("SKILL.md"), b"---\nname: example\n---\n").unwrap();
        let (claude, codex, profile) = (paths::claude_local_dir(), paths::codex_local_dir(), paths::codex_profiles_dir().join("smol"));
        for home in [&claude, &codex, &profile] {
            std::fs::create_dir_all(home).unwrap();
        }
        // Without `~/.agents/skills` every home gets its own copy.
        cache("example", &source).unwrap();
        assert!([&claude, &codex, &profile].iter().all(|h| h.join("skills/example/.bro-managed").is_file()));

        // With it, Codex reads that one copy: bro's own copies in Codex homes step aside, Claude keeps its.
        std::fs::create_dir_all(sb.home().join(".agents/skills")).unwrap();
        atomic_write(&profile.join("skills/mine/SKILL.md"), b"personal").unwrap();
        cache("example", &source).unwrap();
        assert!(sb.home().join(".agents/skills/example/.bro-managed").is_file());
        assert!(claude.join("skills/example/.bro-managed").is_file());
        assert!(!codex.join("skills/example").exists() && !profile.join("skills/example").exists());
        write_home(&profile, "example").unwrap();
        assert!(!profile.join("skills/example").exists(), "a launch doesn't add it back");
        assert!(profile.join("skills/mine/SKILL.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_skills_folder_linked_to_agents_is_written_not_emptied() {
        let sb = sandbox();
        let source = sb.home().join("source");
        atomic_write(&source.join("SKILL.md"), b"---\nname: example\n---\n").unwrap();
        let agents = sb.home().join(".agents/skills");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::create_dir_all(paths::codex_local_dir()).unwrap();
        std::os::unix::fs::symlink(&agents, paths::codex_local_dir().join("skills")).unwrap();
        cache("example", &source).unwrap();
        cache("example", &source).unwrap();
        assert!(agents.join("example/SKILL.md").is_file());
    }
}
