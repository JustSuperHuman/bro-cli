//! Agent skills bro installs into the harness homes it launches
//! (`<CLAUDE_CONFIG_DIR>/skills/…`, `<CODEX_HOME>/skills/…`).
//!
//! `bro-open` teaches the agent to show the user a file with `bro open`
//! (docs/file-links.md section 6): the bridge broadcasts it to the TUI, the web
//! client and the phone.
use std::path::{Path, PathBuf};

pub const BRO_OPEN_SKILL_NAME: &str = "bro-open";

pub const BRO_OPEN_SKILL: &str = "---\nname: bro-open\ndescription: Show the user a file (optionally at a line) in bro's file preview on their desktop, web and phone. Use when you want them to look at something specific, e.g. a change to review or the answer to \"where is X\". Only inside bro (BRO_SESSION_ID is set).\n---\n\n\
# Show the user a file\n\n\
You are running inside bro when the `BRO_SESSION_ID` environment variable is set. Then\n\
`bro open` pops the file up in the preview of every bro screen the user has open\n\
(terminal, browser, phone), scrolled to the line:\n\n\
```sh\n\
bro open src/app.rs:42        # file:line (also file:line:col)\n\
bro open README.md            # whole file\n\
bro open src/app.rs:42 --editor   # open it in the user's desktop editor instead\n\
```\n\n\
- Paths are relative to your working directory or the project root; absolute paths inside the project work too.\n\
- Use it sparingly: at most once per answer, for the single most useful file or line. Do not open every file you mention or touched.\n\
- Good moments: after a change you want reviewed, when pointing at where something lives, or when the user asks to see a file.\n\
- If `bro` is not on PATH, use `\"$BRO_BIN\" open …` when `BRO_BIN` is set.\n\
- If `BRO_SESSION_ID` is not set, or the command fails (bro or its bridge isn't running), just mention the path in your reply instead. Do not retry.\n";

/// The skill file under a harness home.
pub fn bro_open_skill_path(home: &Path) -> PathBuf {
    home.join("skills").join(BRO_OPEN_SKILL_NAME).join("SKILL.md")
}

/// Writes (or refreshes) the `bro-open` skill under `home`; atomic and a no-op
/// when unchanged.
pub fn write_bro_open_skill(home: &Path) -> std::io::Result<()> {
    crate::browser::write_if_changed(&bro_open_skill_path(home), BRO_OPEN_SKILL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_is_written_once_and_refreshed() {
        let home = tempfile::tempdir().unwrap();
        write_bro_open_skill(home.path()).unwrap();
        let path = bro_open_skill_path(home.path());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("---\nname: bro-open\n") && body.contains("bro open src/app.rs:42"));
        std::fs::write(&path, "stale").unwrap();
        write_bro_open_skill(home.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), BRO_OPEN_SKILL);
    }
}
