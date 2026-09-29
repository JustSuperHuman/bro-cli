//! Executable lookup (v1 `proc.js` `which` + `localBinDirs`).
use crate::util::home_dir;
use std::path::{Path, PathBuf};

/// Where bun / npm / native installers drop CLI shims even when they aren't on PATH:
/// `~/.bun/bin`, `~/.local/bin`, `$BUN_INSTALL/bin`, `%APPDATA%\npm`.
pub fn extra_bin_dirs() -> Vec<PathBuf> {
    let home = home_dir();
    let mut dirs = vec![home.join(".bun").join("bin"), home.join(".local").join("bin")];
    if let Some(b) = std::env::var_os("BUN_INSTALL").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(b).join("bin"));
    }
    if let Some(a) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(a).join("npm"));
    }
    dirs
}

fn extensions() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    }
}

/// Resolve an executable on PATH (handles .cmd/.exe shims on Windows).
///
/// PATH is searched first, then the usual shim dirs. A name with a directory part is
/// checked as-is. On Windows each `PATHEXT` extension is tried (and the bare name when
/// it already carries one).
pub fn which(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let exts = extensions();
    let candidates = |dir: &Path| -> Option<PathBuf> {
        let base = dir.join(program);
        let has_ext = Path::new(program).extension().is_some();
        if (has_ext || !cfg!(windows)) && base.is_file() {
            return Some(base);
        }
        exts.iter().filter(|e| !e.is_empty()).map(|e| dir.join(format!("{program}{e}"))).find(|p| p.is_file())
    };
    if program.contains(['/', '\\']) {
        let p = Path::new(program);
        return candidates(p.parent().unwrap_or(Path::new("."))).filter(|_| p.file_name().is_some()).or_else(|| {
            p.is_file().then(|| p.to_path_buf())
        });
    }
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    path_dirs.iter().chain(extra_bin_dirs().iter()).filter(|d| !d.as_os_str().is_empty()).find_map(|d| candidates(d))
}

/// `(program, args)` ready for a PTY: the resolved executable, or on Windows
/// `cmd.exe /c <shim> args…` for `.cmd`/`.bat` shims (which can't be exec'd directly).
/// An unresolvable name is returned as-is so the spawn error names it.
pub fn program_and_args(name: &str, args: Vec<String>) -> (String, Vec<String>) {
    let Some(path) = which(name) else { return (name.to_string(), args) };
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let path_s = path.to_string_lossy().into_owned();
    if cfg!(windows) && (ext == "cmd" || ext == "bat") {
        let mut full = vec!["/c".to_string(), path_s];
        full.extend(args);
        ("cmd.exe".to_string(), full)
    } else {
        (path_s, args)
    }
}
