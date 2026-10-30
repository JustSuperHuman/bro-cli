//! Optional managed workspaces. Provisioning is explicit; merge and deletion remain separate decisions.
use crate::coordination_evidence::git_output;
use bro_core::coordination::digest_bytes;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const GIT_TIME: Duration = Duration::from_secs(30);
const RECORD_LIMIT: u64 = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateRequest {
    pub task_id: String,
    pub repository: PathBuf,
    pub branch: String,
    pub base: String,
    pub integration_owner: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeRecord {
    pub task_id: String,
    pub path: PathBuf,
    pub repository: PathBuf,
    pub common_dir: PathBuf,
    pub branch: String,
    pub base_commit: String,
    pub integration_owner: String,
    pub created_at: i64,
    /// Git mutations/integration share this key across all worktrees of the repository.
    pub resource_key: String,
    /// Independent edits may use this key while integration retains common-repository ownership.
    pub workspace_resource_key: String,
    /// A durable provisioning intent is saved before Git runs. An uncertain creation is never retried blindly.
    pub ready: bool,
}

fn id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 200
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err("Worktree task and owner identities must be bounded coordination IDs.".into());
    }
    Ok(())
}
fn text(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.contains(['\0', '\r', '\n']) {
        return Err("Worktree branch/base must be a bounded explicit Git reference.".into());
    }
    Ok(())
}
fn link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn safe_dir(base: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut dir = base.to_path_buf();
    for segment in Path::new(relative) {
        dir.push(segment);
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(format!(
                    "Create managed worktree directory {}: {e}",
                    dir.display()
                ));
            }
        }
        let metadata = fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || link(&metadata) {
            return Err(format!(
                "Managed worktree directory is not a regular owned directory: {}",
                dir.display()
            ));
        }
    }
    Ok(dir)
}
fn roots(data_root: &Path) -> Result<(PathBuf, PathBuf, File), String> {
    fs::create_dir_all(data_root).map_err(|e| e.to_string())?;
    let base = fs::canonicalize(data_root).map_err(|e| e.to_string())?;
    let workspaces = safe_dir(&base, "coordination/workspaces")?;
    let records = safe_dir(&base, "coordination/worktree-records")?;
    let lock = records.join("authority.lock");
    if fs::symlink_metadata(&lock).is_ok_and(|m| link(&m)) {
        return Err("Managed worktree lock is a symlink/reparse point.".into());
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock)
        .map_err(|e| e.to_string())?;
    lock.lock()
        .map_err(|e| format!("Lock managed worktrees: {e}"))?;
    Ok((workspaces, records, lock))
}
fn key(prefix: &str, path: &Path) -> String {
    let normalized = bro_core::util::normalize_path(path)
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    let normalized = normalized.replace('\\', "/").to_ascii_lowercase();
    format!("{prefix}:{normalized}")
}
fn record_path(records: &Path, task_id: &str) -> PathBuf {
    records.join(format!("{}.json", digest_bytes(task_id.as_bytes())))
}
fn read_record(path: &Path) -> Result<WorktreeRecord, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("Read worktree ownership {}: {e}", path.display()))?;
    if !metadata.is_file() || link(&metadata) || metadata.len() > RECORD_LIMIT {
        return Err("Worktree provenance file is invalid, linked, or oversized.".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("Invalid worktree provenance: {e}"))
}
fn save(path: &Path, record: &WorktreeRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err("Worktree provenance exceeds its size limit.".into());
    }
    let tmp = path.with_extension(format!("{:016x}.pending", rand::random::<u64>()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| format!("Commit worktree provenance: {e}"))?;
    #[cfg(unix)]
    {
        File::open(path.parent().unwrap())
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn output_string(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes)
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .map_err(|_| "Git returned a non-UTF-8 repository identity.".into())
}
fn root(repository: &Path) -> Result<PathBuf, String> {
    let repository =
        fs::canonicalize(repository).map_err(|e| format!("Resolve worktree repository: {e}"))?;
    let root = output_string(git_output(
        &repository,
        &["rev-parse", "--show-toplevel"],
        GIT_TIME,
    )?)?;
    fs::canonicalize(root).map_err(|e| e.to_string())
}
fn common(repository: &Path) -> Result<PathBuf, String> {
    let common = PathBuf::from(output_string(git_output(
        repository,
        &["rev-parse", "--git-common-dir"],
        GIT_TIME,
    )?)?);
    let common = if common.is_absolute() {
        common
    } else {
        repository.join(common)
    };
    fs::canonicalize(common).map_err(|e| e.to_string())
}

pub(crate) fn inspect(record: &WorktreeRecord) -> Result<(), String> {
    if !record.ready {
        return Err("Worktree creation is uncertain; inspect its branch/path and reconcile its saved provisioning intent before reuse.".into());
    }
    let metadata =
        fs::symlink_metadata(&record.path).map_err(|e| format!("Inspect managed worktree: {e}"))?;
    if link(&metadata) || !metadata.is_dir() {
        return Err("Managed worktree path was replaced or is not a directory.".into());
    }
    if root(&record.path)? != fs::canonicalize(&record.path).map_err(|e| e.to_string())?
        || common(&record.path)? != record.common_dir
    {
        return Err("Managed worktree no longer belongs to its recorded repository.".into());
    }
    let branch = output_string(git_output(
        &record.path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        GIT_TIME,
    )?)?;
    if branch != record.branch {
        return Err("Managed worktree branch differs from its immutable provenance; do not silently rebind it.".into());
    }
    if record.resource_key != key("repo", &record.common_dir)
        || record.workspace_resource_key != key("workspace", &record.path)
    {
        return Err("Managed worktree resource identity differs from its provenance.".into());
    }
    Ok(())
}

pub(crate) fn load(data_root: &Path, task_id: &str) -> Result<WorktreeRecord, String> {
    id(task_id)?;
    let (workspaces, records, _guard) = roots(data_root)?;
    let record = read_record(&record_path(&records, task_id))?;
    let expected = workspaces.join(digest_bytes(task_id.as_bytes()));
    if record.task_id != task_id || record.path != expected {
        return Err("Worktree task/path provenance does not match its managed location.".into());
    }
    inspect(&record)?;
    Ok(record)
}

/// Explicit recovery after a provisioning crash. This observes an existing checkout and never
/// retries Git creation, alters the user's branch, or deletes an uncertain path.
pub(crate) fn reconcile(data_root: &Path, task_id: &str) -> Result<WorktreeRecord, String> {
    id(task_id)?;
    let (workspaces, records, _guard) = roots(data_root)?;
    let path = record_path(&records, task_id);
    let mut record = read_record(&path)?;
    if record.task_id != task_id || record.path != workspaces.join(digest_bytes(task_id.as_bytes()))
    {
        return Err("Worktree task/path provenance does not match its managed location.".into());
    }
    record.ready = true;
    inspect(&record)?;
    git_output(
        &record.path,
        &["merge-base", "--is-ancestor", &record.base_commit, "HEAD"],
        GIT_TIME,
    )?;
    save(&path, &record)?;
    Ok(record)
}

pub(crate) fn create(data_root: &Path, request: &CreateRequest) -> Result<WorktreeRecord, String> {
    id(&request.task_id)?;
    id(&request.integration_owner)?;
    text(&request.branch)?;
    text(&request.base)?;
    let repository = root(&request.repository)?;
    git_output(
        &repository,
        &["check-ref-format", "--branch", &request.branch],
        GIT_TIME,
    )?;
    let base = format!("{}^{{commit}}", request.base);
    let base_commit = output_string(git_output(
        &repository,
        &["rev-parse", "--verify", "--end-of-options", &base],
        GIT_TIME,
    )?)?;
    if !matches!(base_commit.len(), 40 | 64) || !base_commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Git base is not a single observed commit identity.".into());
    }
    let common_dir = common(&repository)?;
    let (workspaces, records, _guard) = roots(data_root)?;
    let path = workspaces.join(digest_bytes(request.task_id.as_bytes()));
    let record_file = record_path(&records, &request.task_id);
    if record_file.exists() {
        let record = read_record(&record_file)?;
        if record.task_id != request.task_id
            || record.path != path
            || record.repository != repository
            || record.common_dir != common_dir
            || record.branch != request.branch
            || record.base_commit != base_commit
            || record.integration_owner != request.integration_owner
        {
            return Err("Task worktree already has different immutable branch/base/repository/owner provenance.".into());
        }
        inspect(&record)?;
        return Ok(record);
    }
    if fs::symlink_metadata(&path).is_ok() {
        return Err("Managed worktree target already exists without matching task provenance; no files were changed.".into());
    }
    let branch_ref = format!("refs/heads/{}", request.branch);
    let refs = output_string(git_output(
        &repository,
        &["for-each-ref", "--format=%(refname)", &branch_ref],
        GIT_TIME,
    )?)?;
    if refs.lines().any(|line| line == branch_ref) {
        return Err("The requested branch already exists; choose a new task-owned branch.".into());
    }
    let mut record = WorktreeRecord {
        task_id: request.task_id.clone(),
        path: path.clone(),
        repository: repository.clone(),
        common_dir: common_dir.clone(),
        branch: request.branch.clone(),
        base_commit,
        integration_owner: request.integration_owner.clone(),
        created_at: bro_core::util::now_secs(),
        resource_key: key("repo", &common_dir),
        workspace_resource_key: key("workspace", &path),
        ready: false,
    };
    save(&record_file, &record)?;
    // Normalize external Git argv only; canonical paths remain the identity and containment boundary.
    let git_path = bro_core::util::normalize_path(&path);
    let path_arg = git_path
        .to_str()
        .ok_or("Managed worktree path is not UTF-8")?;
    if let Err(error) = git_output(
        &repository,
        &[
            "worktree",
            "add",
            "-b",
            &request.branch,
            "--",
            path_arg,
            &record.base_commit,
        ],
        GIT_TIME,
    ) {
        return Err(format!(
            "Worktree provisioning did not complete: {error}. Its immutable provisioning record was preserved; no branch/path cleanup or retry was attempted."
        ));
    }
    record.ready = true;
    inspect(&record)?;
    save(&record_file, &record)?;
    Ok(record)
}

pub(crate) fn list(data_root: &Path) -> Result<Vec<WorktreeRecord>, String> {
    let (workspaces, records, _guard) = roots(data_root)?;
    let mut out = Vec::new();
    for entry in fs::read_dir(&records).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().is_none_or(|e| e != "json") {
            continue;
        }
        if out.len() >= 512 {
            return Err("Managed worktree registry exceeds its 512-record listing limit; no truncated list was returned.".into());
        }
        let record = read_record(&entry.path())?;
        if record.path != workspaces.join(digest_bytes(record.task_id.as_bytes())) {
            return Err("Worktree registry contains an escaped task path.".into());
        }
        out.push(record);
    }
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    fn git(root: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "--quiet"]);
        fs::write(repo.path().join("tracked.txt"), "base").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(
            repo.path(),
            &[
                "-c",
                "user.name=Bro test",
                "-c",
                "user.email=bro@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        );
        (repo, tempfile::tempdir().unwrap())
    }
    fn request(repo: &Path) -> CreateRequest {
        CreateRequest {
            task_id: "task-one".into(),
            repository: repo.into(),
            branch: "bro/task-one".into(),
            base: "HEAD".into(),
            integration_owner: "integrator".into(),
        }
    }

    #[test]
    fn creates_owned_isolated_worktree_and_preserves_dirty_user_tree() {
        let (repo, data) = fixture();
        fs::write(repo.path().join("tracked.txt"), "dirty user work").unwrap();
        fs::write(repo.path().join("untracked.txt"), "keep me").unwrap();
        let record = create(data.path(), &request(repo.path())).unwrap();
        assert!(
            record.ready
                && record
                    .path
                    .starts_with(fs::canonicalize(data.path()).unwrap())
        );
        assert_eq!(
            fs::read_to_string(record.path.join("tracked.txt")).unwrap(),
            "base"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "dirty user work"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("untracked.txt")).unwrap(),
            "keep me"
        );
        assert_eq!(
            create(data.path(), &request(repo.path())).unwrap(),
            record,
            "repeated creation reuses provenance"
        );
        assert_eq!(load(data.path(), "task-one").unwrap(), record);
        assert_eq!(list(data.path()).unwrap().len(), 1);
        let mut other = request(repo.path());
        other.task_id = "task-two".into();
        other.branch = "bro/task-two".into();
        let second = create(data.path(), &other).unwrap();
        assert_eq!(record.resource_key, second.resource_key);
        assert_ne!(record.workspace_resource_key, second.workspace_resource_key);
    }

    #[test]
    fn refuses_rebinding_branch_collisions_and_unowned_existing_paths() {
        let (repo, data) = fixture();
        let req = request(repo.path());
        let first = create(data.path(), &req).unwrap();
        let mut changed = req.clone();
        changed.integration_owner = "other-owner".into();
        assert!(create(data.path(), &changed).is_err());
        changed = req.clone();
        changed.branch = "bro/other".into();
        assert!(create(data.path(), &changed).is_err());
        let mut collision = req.clone();
        collision.task_id = "another-task".into();
        assert!(
            create(data.path(), &collision)
                .unwrap_err()
                .contains("branch already exists")
        );
        let mut unowned = req;
        unowned.task_id = "unowned".into();
        unowned.branch = "bro/unowned".into();
        let target = first.path.parent().unwrap().join(digest_bytes(b"unowned"));
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "user data").unwrap();
        assert!(create(data.path(), &unowned).is_err());
        assert_eq!(
            fs::read_to_string(target.join("keep")).unwrap(),
            "user data"
        );
    }

    #[test]
    fn rejects_escaped_identity_and_detects_branch_provenance_change() {
        let (repo, data) = fixture();
        let mut req = request(repo.path());
        req.task_id = "../escape".into();
        assert!(create(data.path(), &req).is_err());
        req = request(repo.path());
        req.branch = "--detach".into();
        assert!(create(data.path(), &req).is_err());
        let record = create(data.path(), &request(repo.path())).unwrap();
        git(&record.path, &["checkout", "--quiet", "-b", "unexpected"]);
        assert!(inspect(&record).unwrap_err().contains("branch differs"));
    }
}
