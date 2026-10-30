//! Optional Bun preload for Claude Code's periodic agent status summaries.
//! The script is bundled, versioned by content, and only enabled in Bro's Claude launches.
use std::path::{Path, PathBuf};

pub const KNOB: &str = "skip_agent_summaries";
pub const ENABLED: &str = "BRO_CLAUDE_SKIP_AGENT_SUMMARIES";
pub const CREDIT: &str = "Original post by Flamerix AI";
pub const SOURCE_URL: &str = "https://gist.github.com/flamerix-ai/c3905705cc4205e5de3011fa2e74efce";
const PRELOAD: &str = include_str!("agent_summaries/preload.cjs");
const NOTICE: &str = include_str!("agent_summaries/NOTICE");

/// Keep each build's script in place for running sessions and their future child processes.
pub fn install() -> std::io::Result<PathBuf> {
    let dir = std::path::absolute(crate::paths::bro_dir())?.join("claude-preloads");
    let path = dir.join(format!("no-agent-summary-{}.cjs", crate::util::short_hash(PRELOAD, 12)));
    crate::browser::write_if_changed(&dir.join("NOTICE"), NOTICE)?;
    crate::browser::write_if_changed(&path, PRELOAD)?;
    Ok(path)
}

/// Use `=` so inherited options also work in Bun subcommands. Compiled Claude Code's Bun parser
/// treats quotes literally and can split paths at spaces when multiple preloads are present.
pub fn bun_options(existing: &str, preload: &Path) -> std::io::Result<String> {
    let path = option_path(preload)?;
    let path = path.as_str();
    // A child Bro can inherit the same preload in either supported form. Loading it again is
    // harmless, but keeping the environment idempotent avoids growing it on nested launches.
    for flag in ["--preload=", "--preload "] {
        for (at, flag) in existing.match_indices(flag) {
            if at > 0 && !existing[..at].ends_with(char::is_whitespace) {
                continue;
            }
            let rest = &existing[at + flag.len()..];
            if let Some(tail) = rest.strip_prefix(path)
                && (tail.is_empty() || tail.starts_with(char::is_whitespace))
            {
                return Ok(existing.into());
            }
        }
    }
    let gap = if existing.is_empty() || existing.ends_with(char::is_whitespace) { "" } else { " " };
    Ok(format!("{existing}{gap}--preload={path}"))
}

fn option_path(preload: &Path) -> std::io::Result<String> {
    #[cfg(windows)]
    let short;
    #[cfg(windows)]
    let preload = if preload.as_os_str().to_string_lossy().chars().any(char::is_whitespace) {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
        let parent = preload.parent().ok_or_else(|| std::io::Error::other("Claude preload has no parent directory"))?;
        let name = preload.file_name().ok_or_else(|| std::io::Error::other("Claude preload has no filename"))?;
        let wide: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: wide is NUL-terminated; the first call asks for the output buffer length.
        let size = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
        if size == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0u16; size as usize];
        // SAFETY: both buffers have the lengths passed to Windows and do not overlap.
        let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), size) };
        if written == 0 || written >= size {
            return Err(std::io::Error::other("could not resolve a short Claude preload path"));
        }
        // Bun resolves the filename against a directory listing, which contains long names only.
        // An 8.3 filename fails that lookup; shorten the directory and keep Bro's real filename.
        short = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written as usize])).join(name);
        short.as_path()
    } else {
        preload
    };
    let path = preload.to_str().ok_or_else(|| std::io::Error::other("Claude preload path is not UTF-8"))?;
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    let path: &str = path.as_ref();
    if path.chars().any(char::is_whitespace) {
        return Err(std::io::Error::other("Bun needs a Claude preload path without whitespace"));
    }
    Ok(path.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_bun_options_without_duplicating_preloads() {
        let path = Path::new("/home/user/.bro/no-agent-summary.cjs");
        let once = bun_options("--smol --preload=/other/hook.cjs", path).unwrap();
        assert_eq!(once, "--smol --preload=/other/hook.cjs --preload=/home/user/.bro/no-agent-summary.cjs");
        assert_eq!(bun_options(&once, path).unwrap(), once);
        let spaced = "--preload /home/user/.bro/no-agent-summary.cjs --smol";
        assert_eq!(bun_options(spaced, path).unwrap(), spaced);
        let prefix = "--preload=/home/user/.bro/no-agent-summary.cjs.old";
        assert!(bun_options(prefix, path).unwrap().ends_with(" --preload=/home/user/.bro/no-agent-summary.cjs"));
    }

    #[test]
    fn paths_with_spaces_use_windows_short_names_or_fail_without_breaking_launch() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Someone Else");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("preload.cjs");
        std::fs::write(&file, PRELOAD).unwrap();
        match bun_options("--smol", &file) {
            Ok(options) => {
                assert!(cfg!(windows), "only Windows can resolve a short name");
                let path = options.strip_prefix("--smol --preload=").unwrap();
                assert!(!path.chars().any(char::is_whitespace));
                assert!(path.ends_with("/preload.cjs"), "keep the long filename for Bun's resolver");
                assert_eq!(std::fs::read_to_string(path).unwrap(), PRELOAD);
            }
            Err(_) => assert!(file.is_file(), "no usable short path: caller leaves the launch alone"),
        }
    }
}
