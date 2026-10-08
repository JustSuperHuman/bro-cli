//! Opening a file at a line in your editor: `$BRO_EDITOR`, else `$VISUAL` / `$EDITOR`, else VS Code (`code -g`),
//! else the OS default app. Terminal editors (vim, nvim, nano, hx…) run in a new bro pane beside the session
//! rather than taking over bro's own terminal; GUI editors start detached.

use std::path::{Path, PathBuf};

/// A terminal editor to run in a pane (the plan was made on a worker).
#[derive(Debug, Clone)]
pub struct EditorPane {
    /// the session it was opened from: the editor splits beside it
    pub pane: Option<crate::layout::PaneId>,
    pub cwd: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    /// for the pane's name
    pub file: String,
}

/// How to open it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Start detached (VS Code, Cursor, Zed, Sublime, JetBrains…).
    Gui { program: String, args: Vec<String> },
    /// Run in a bro pane.
    Terminal { program: String, args: Vec<String> },
    /// Hand the file to the OS (its default app for the type).
    System,
}

const TERMINAL: &[&str] = &["vi", "vim", "nvim", "nano", "pico", "micro", "hx", "helix", "kak", "joe", "ne", "mg", "jed", "amp", "ox", "nvi", "edit", "msedit", "vis", "lvim", "kibi"];
const VSCODE: &[&str] = &["code", "code-insiders", "cursor", "windsurf", "codium", "vscodium", "positron", "trae", "kiro"];
const JETBRAINS: &[&str] = &["idea", "idea64", "webstorm", "webstorm64", "rider", "rider64", "pycharm", "pycharm64", "clion", "clion64", "goland", "goland64", "rustrover", "rustrover64", "phpstorm", "studio", "studio64", "fleet"];

/// Split an `$EDITOR` value into words: whitespace-separated, with "double" or 'single' quotes grouping.
pub fn split_command(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The editor's short name: `C:\Program Files\Vim\vim91\gvim.exe` → `gvim`.
fn stem(program: &str) -> String {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(s) = name.strip_suffix(ext) {
            return s.to_string();
        }
    }
    name
}

/// What to run for `path` at `line`/`column`. `env` reads environment variables; `on_path` says whether a
/// program can be found.
pub fn plan(path: &Path, line: Option<u32>, column: Option<u32>, env: &dyn Fn(&str) -> Option<String>, on_path: &dyn Fn(&str) -> bool) -> Plan {
    let p = path.to_string_lossy().into_owned();
    let configured = ["BRO_EDITOR", "VISUAL", "EDITOR"].iter().filter_map(|k| env(k)).map(|v| split_command(&v)).find(|w| !w.is_empty());
    let Some(mut words) = configured.or_else(|| on_path("code").then(|| vec!["code".to_string()])) else { return Plan::System };
    let program = words.remove(0);
    let name = stem(&program);
    let mut args = words;
    let l = line.unwrap_or(1);
    let at = |sep: &str| match (line, column) {
        (Some(l), Some(c)) => format!("{p}{sep}{l}{sep}{c}"),
        (Some(l), None) => format!("{p}{sep}{l}"),
        _ => p.clone(),
    };
    if VSCODE.contains(&name.as_str()) {
        // we don't wait for it to close
        args.retain(|a| a != "--wait" && a != "-w");
        args.extend(["-g".to_string(), at(":")]);
        return Plan::Gui { program, args };
    }
    if JETBRAINS.contains(&name.as_str()) {
        if line.is_some() {
            args.extend(["--line".to_string(), l.to_string()]);
        }
        if let Some(c) = column {
            args.extend(["--column".to_string(), c.to_string()]);
        }
        args.push(p);
        return Plan::Gui { program, args };
    }
    let terminal = TERMINAL.contains(&name.as_str()) || (matches!(name.as_str(), "emacs" | "emacsclient") && args.iter().any(|a| matches!(a.as_str(), "-nw" | "-t" | "--tty")));
    match name.as_str() {
        "subl" | "sublime_text" | "zed" | "zeditor" | "lapce" => args.push(at(":")),
        "notepad++" => args.extend([format!("-n{l}"), p]),
        "kate" | "mate" => {
            if line.is_some() {
                args.extend(["-l".to_string(), l.to_string()]);
            }
            args.push(p);
        }
        "hx" | "helix" | "micro" | "amp" | "ox" => args.push(at(":")),
        "nano" | "pico" => {
            if line.is_some() {
                args.push(match column {
                    Some(c) => format!("+{l},{c}"),
                    None => format!("+{l}"),
                });
            }
            args.push(p);
        }
        "emacs" | "emacsclient" | "kak" => {
            if line.is_some() {
                args.push(match column {
                    Some(c) => format!("+{l}:{c}"),
                    None => format!("+{l}"),
                });
            }
            args.push(p);
        }
        "vi" | "vim" | "nvim" | "gvim" | "mvim" | "lvim" | "nvi" | "vis" | "mg" | "jed" | "ne" | "joe" => {
            if line.is_some() {
                args.push(format!("+{l}"));
            }
            args.push(p);
        }
        _ => args.push(p),
    }
    if terminal { Plan::Terminal { program, args } } else { Plan::Gui { program, args } }
}

/// [`plan`] for this process's environment and PATH.
pub fn plan_here(path: &Path, line: Option<u32>, column: Option<u32>) -> Plan {
    plan(path, line, column, &|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()), &|p| crate::util::which(p).is_some())
}

/// Start a GUI editor detached (worker threads only: resolving and spawning touch the disk).
pub fn spawn_gui(program: &str, args: &[String]) -> Result<(), String> {
    let resolved: PathBuf = crate::util::which(program).unwrap_or_else(|| PathBuf::from(program));
    let mut cmd = std::process::Command::new(&resolved);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // code.cmd and friends: no console window flashing up
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("couldn't start {program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with(vars: &[(&str, &str)], path_has: &[&str], line: Option<u32>, column: Option<u32>) -> Plan {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let path_has: Vec<String> = path_has.iter().map(|s| s.to_string()).collect();
        plan(Path::new("/repo/src/app.rs"), line, column, &|k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()), &|p| path_has.iter().any(|x| x == p))
    }

    #[test]
    fn editors_get_their_own_line_syntax() {
        assert_eq!(plan_with(&[("EDITOR", "code --wait")], &[], Some(42), Some(3)), Plan::Gui { program: "code".into(), args: vec!["-g".into(), "/repo/src/app.rs:42:3".into()] });
        assert_eq!(plan_with(&[("EDITOR", "nvim")], &[], Some(42), None), Plan::Terminal { program: "nvim".into(), args: vec!["+42".into(), "/repo/src/app.rs".into()] });
        assert_eq!(plan_with(&[("EDITOR", "hx")], &[], Some(7), Some(2)), Plan::Terminal { program: "hx".into(), args: vec!["/repo/src/app.rs:7:2".into()] });
        assert_eq!(plan_with(&[("EDITOR", "nano")], &[], Some(7), Some(2)), Plan::Terminal { program: "nano".into(), args: vec!["+7,2".into(), "/repo/src/app.rs".into()] });
        assert_eq!(plan_with(&[("EDITOR", "emacs -nw")], &[], Some(7), None), Plan::Terminal { program: "emacs".into(), args: vec!["-nw".into(), "+7".into(), "/repo/src/app.rs".into()] });
        assert_eq!(plan_with(&[("EDITOR", "emacs")], &[], Some(7), None), Plan::Gui { program: "emacs".into(), args: vec!["+7".into(), "/repo/src/app.rs".into()] });
        assert_eq!(plan_with(&[("EDITOR", "idea64.exe")], &[], Some(9), None), Plan::Gui { program: "idea64.exe".into(), args: vec!["--line".into(), "9".into(), "/repo/src/app.rs".into()] });
        assert_eq!(plan_with(&[("EDITOR", "zed")], &[], None, None), Plan::Gui { program: "zed".into(), args: vec!["/repo/src/app.rs".into()] });
    }

    #[test]
    fn bro_editor_wins_then_visual_then_editor_then_vscode_then_the_system() {
        assert!(matches!(plan_with(&[("BRO_EDITOR", "subl"), ("VISUAL", "vim"), ("EDITOR", "nano")], &["code"], None, None), Plan::Gui { ref program, .. } if program == "subl"));
        assert!(matches!(plan_with(&[("VISUAL", "vim"), ("EDITOR", "nano")], &[], None, None), Plan::Terminal { ref program, .. } if program == "vim"));
        assert!(matches!(plan_with(&[], &["code"], Some(1), None), Plan::Gui { ref program, .. } if program == "code"));
        assert_eq!(plan_with(&[], &[], Some(1), None), Plan::System);
        assert_eq!(plan_with(&[("EDITOR", "   ")], &[], None, None), Plan::System);
    }

    #[test]
    fn quoted_editor_paths_keep_their_spaces() {
        assert_eq!(split_command(r#""C:\Program Files\Microsoft VS Code\bin\code.cmd" --wait"#), vec![r"C:\Program Files\Microsoft VS Code\bin\code.cmd", "--wait"]);
        assert_eq!(split_command("  nvim   -u NONE "), vec!["nvim", "-u", "NONE"]);
        assert!(matches!(
            plan_with(&[("EDITOR", r#""C:\Program Files\Microsoft VS Code\bin\code.cmd" --wait"#)], &[], Some(2), None),
            Plan::Gui { ref args, .. } if args == &vec!["-g".to_string(), "/repo/src/app.rs:2".to_string()]
        ));
    }
}
