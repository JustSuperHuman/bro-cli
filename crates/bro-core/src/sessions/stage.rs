//! Cross-profile resume: temporarily copy a session's files into another login's
//! directory so the harness can fork it there (v1 `stageSessionForProfile`,
//! `codexSessionEntries` and `stageFiles`). Only what was created here is ever
//! removed — never the source, never the fork the harness writes.
use super::{SessionInfo, is_uuid};
use crate::util::{is_within, same_path};
use crate::{Harness, profiles};
use anyhow::{Context, anyhow, bail};
use std::path::{Path, PathBuf};

/// Per-session artifacts Claude keeps beside the transcript, keyed by session id.
const CLAUDE_AUX_DIRS: [&str; 4] = ["file-history", "session-env", "shell-snapshots", "tasks"];

/// Directory names a cleanup never removes, even when left empty.
const STOP_DIRS: [&str; 6] = ["projects", "sessions", "file-history", "session-env", "shell-snapshots", "tasks"];

fn profile_dir(id: &str) -> anyhow::Result<(profiles::ProfileKind, PathBuf)> {
    let (kind, name) = profiles::parse_id(id).ok_or_else(|| anyhow!("unknown profile id {id:?}"))?;
    Ok((kind, profiles::dir_for(kind, name)))
}

/// Copy a session (and its sidecar files) from its owner profile into `target_profile_id`
/// so it can be resumed there with fork semantics (v1 `stageSessionForProfile` /
/// codex `stageFiles`). Returns the files created so the caller can clean up
/// (see [`cleanup_staged`]). Returns an empty list when the target already owns it.
pub fn stage_for_profile(session: &SessionInfo, target_profile_id: &str) -> anyhow::Result<Vec<PathBuf>> {
    let source_id = session
        .profile_id
        .as_deref()
        .ok_or_else(|| anyhow!("{} sessions have no login profile to stage between", session.harness.label()))?;
    let (source_kind, source_dir) = profile_dir(source_id)?;
    let (target_kind, target_dir) = profile_dir(target_profile_id)?;
    let claude_family = |k: profiles::ProfileKind| matches!(k, profiles::ProfileKind::ClaudeLocal | profiles::ProfileKind::ClaudeAccount);
    if claude_family(source_kind) != claude_family(target_kind) {
        bail!("can't stage a {source_id} session into {target_profile_id}");
    }
    if same_path(&source_dir, &target_dir) {
        return Ok(Vec::new());
    }
    let entries = match session.harness {
        Harness::Claude => claude_entries(session, &source_dir, &target_dir)?,
        Harness::Codex => codex_entries(session, &source_dir, &target_dir)?,
        other => bail!("{} sessions can't be staged across profiles", other.label()),
    };
    stage_files(&target_dir, &entries)
}

fn claude_entries(session: &SessionInfo, source_dir: &Path, target_dir: &Path) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
    let id = &session.id;
    if !is_uuid(id) {
        bail!("Invalid Claude session id: {id}");
    }
    let file = &session.path;
    let project_dir = file.parent().ok_or_else(|| anyhow!("transcript has no parent dir"))?;
    let in_profile = project_dir.parent().is_some_and(|p| same_path(p, &source_dir.join("projects")));
    if file.file_name().is_none_or(|n| n.to_string_lossy() != format!("{id}.jsonl")) || !in_profile || !file.exists() {
        bail!("The source transcript is unavailable or outside its Claude profile: {}", file.display());
    }
    let project_name = project_dir.file_name().ok_or_else(|| anyhow!("bad project dir"))?;
    let target_project = target_dir.join("projects").join(project_name);
    let mut entries = vec![
        (file.clone(), target_project.join(format!("{id}.jsonl"))),
        (project_dir.join(id), target_project.join(id)),
    ];
    entries.extend(CLAUDE_AUX_DIRS.iter().map(|aux| (source_dir.join(aux).join(id), target_dir.join(aux).join(id))));
    Ok(entries)
}

fn codex_entries(session: &SessionInfo, source_dir: &Path, target_dir: &Path) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
    let id = &session.id;
    if !is_uuid(id) {
        bail!("Invalid Codex session id: {id}");
    }
    let root = source_dir.join("sessions");
    let rel = crate::util::normalize_path(&session.path)
        .strip_prefix(crate::util::normalize_path(&root))
        .map(Path::to_path_buf)
        .ok()
        .filter(|_| is_within(&root, &session.path))
        .filter(|_| session.path.to_string_lossy().contains(id.as_str()))
        .ok_or_else(|| anyhow!("The source rollout is unavailable or outside its Codex profile: {}", session.path.display()))?;
    Ok(vec![(session.path.clone(), target_dir.join("sessions").join(rel))])
}

/// v1 `stageFiles`: copy each existing source to its target inside `target_root`.
/// Missing sources are skipped; an existing target is an error, never overwritten.
/// On failure everything created so far is removed again.
fn stage_files(target_root: &Path, entries: &[(PathBuf, PathBuf)]) -> anyhow::Result<Vec<PathBuf>> {
    let mut created: Vec<PathBuf> = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        for (source, target) in entries {
            if !source.exists() {
                continue;
            }
            if !is_within(target_root, target) {
                bail!("Refusing to copy a session artifact outside the destination profile.");
            }
            if target.exists() {
                bail!("The destination profile already has session artifact: {}", target.display());
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
            }
            created.push(target.clone());
            crate::profiles::copy_tree(source, target).with_context(|| format!("copying {}", source.display()))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(created),
        Err(e) => {
            cleanup_staged(&created);
            Err(e)
        }
    }
}

/// Remove files/dirs returned by [`stage_for_profile`] (or `CommandSpec::cleanup`),
/// then any directories the staging left empty — stopping at a login's `projects`/
/// `sessions`/aux roots. Best-effort; never touches anything else.
pub fn cleanup_staged(paths: &[PathBuf]) {
    for p in paths.iter().rev() {
        let _ = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    }
    for p in paths.iter().rev() {
        let mut dir = p.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.is_empty() || STOP_DIRS.contains(&name.as_str()) || std::fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths;
    use crate::util::test_env::sandbox;
    use std::time::SystemTime;

    fn info(harness: Harness, profile: &str, id: &str, path: PathBuf) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            harness,
            profile_id: Some(profile.into()),
            path,
            cwd: None,
            project: None,
            title: "t".into(),
            branch: None,
            modified: SystemTime::now(),
            size: 1,
        }
    }

    #[test]
    fn stages_claude_transcript_and_sidecars() {
        let _sb = sandbox();
        let id = "11111111-1111-4111-8111-111111111111";
        let src = paths::claude_accounts_dir().join("a");
        let dst = paths::claude_accounts_dir().join("b");
        let transcript = src.join("projects").join("F--p").join(format!("{id}.jsonl"));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "{}").unwrap();
        std::fs::create_dir_all(src.join("projects").join("F--p").join(id)).unwrap();
        std::fs::write(src.join("projects").join("F--p").join(id).join("x"), "1").unwrap();
        std::fs::create_dir_all(src.join("file-history").join(id)).unwrap();
        std::fs::create_dir_all(dst.join("projects")).unwrap();

        let s = info(Harness::Claude, "claude:a", id, transcript.clone());
        let staged = stage_for_profile(&s, "claude:b").unwrap();
        assert_eq!(staged.len(), 3);
        assert!(dst.join("projects").join("F--p").join(format!("{id}.jsonl")).exists());
        assert!(dst.join("projects").join("F--p").join(id).join("x").exists());
        assert!(dst.join("file-history").join(id).exists());
        // Second stage refuses to overwrite.
        assert!(stage_for_profile(&s, "claude:b").is_err());
        cleanup_staged(&staged);
        assert!(!dst.join("projects").join("F--p").exists());
        assert!(dst.join("projects").exists());
        assert!(transcript.exists());
        // Same profile → nothing to do.
        assert!(stage_for_profile(&s, "claude:a").unwrap().is_empty());
        // Wrong family / bad id.
        assert!(stage_for_profile(&s, "codex:local").is_err());
        let bad = info(Harness::Claude, "claude:a", "../../x", transcript);
        assert!(stage_for_profile(&bad, "claude:b").is_err());
    }

    #[test]
    fn stages_codex_rollout() {
        let sb = sandbox();
        let id = "01a0ed53-d984-7ad0-badc-5b7024210c1a";
        let rel = PathBuf::from("2026").join("09").join("29").join(format!("rollout-x-{id}.jsonl"));
        let src = sb.home().join(".codex").join("sessions").join(&rel);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "{}").unwrap();
        let s = info(Harness::Codex, "codex:local", id, src.clone());
        let staged = stage_for_profile(&s, "codex:smol").unwrap();
        let target = paths::codex_profiles_dir().join("smol").join("sessions").join(&rel);
        assert_eq!(staged, vec![target.clone()]);
        assert!(target.exists());
        cleanup_staged(&staged);
        assert!(!target.exists());
        assert!(!paths::codex_profiles_dir().join("smol").join("sessions").join("2026").exists());
        assert!(src.exists());
    }
}
