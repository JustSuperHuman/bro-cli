//! The system clipboard (ported from z4-oriel): copy text out of bro (selections), and grab an image or copied
//! files from it (ctrl+v pastes text; alt+v pastes an image into Claude Code / Codex as a file path).
//!
//! Copy goes two ways at once: OSC 52 (the terminal sets the clipboard; works over SSH too) and the OS tool as a
//! backup (clip.exe with UTF-16LE, wl-copy, xclip, xsel).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn cmd(prog: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(prog);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // no console window
    }
    c
}

/// Put text on the clipboard. Never blocks the UI: the OS tool runs on its own thread.
pub fn copy(text: &str) {
    if cfg!(test) {
        return;
    }
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
    let text = text.to_string();
    std::thread::spawn(move || {
        if cfg!(windows) {
            // clip.exe reads UTF-16LE with a BOM correctly (plain UTF-8 mangles anything non-ASCII)
            if let Ok(mut ch) = cmd("clip.exe").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                if let Some(mut si) = ch.stdin.take() {
                    let mut bytes = vec![0xFF, 0xFE];
                    for u in text.encode_utf16() {
                        bytes.extend_from_slice(&u.to_le_bytes());
                    }
                    let _ = si.write_all(&bytes);
                }
                let _ = ch.wait();
            }
        } else {
            for (prog, args) in [("wl-copy", vec![]), ("xclip", vec!["-selection", "clipboard"]), ("xsel", vec!["--clipboard", "--input"])] {
                if crate::util::which(prog).is_none() {
                    continue;
                }
                if let Ok(mut ch) = cmd(prog).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                    if let Some(mut si) = ch.stdin.take() {
                        let _ = si.write_all(text.as_bytes());
                    }
                    let _ = ch.wait();
                    break;
                }
            }
        }
    });
}

/// Read text or copied file paths. Run on a worker so clipboard contention never blocks input.
pub fn grab_text() -> Option<String> {
    #[cfg(windows)]
    {
        use clipboard_win::{Clipboard, Getter, formats};
        let _clipboard = Clipboard::new_attempts(10).ok()?;
        let mut text = String::new();
        if formats::Unicode.read_clipboard(&mut text).is_ok() && !text.is_empty() {
            return Some(text);
        }
        let mut paths: Vec<String> = vec![];
        formats::FileList.read_clipboard(&mut paths).ok()?;
        (!paths.is_empty()).then(|| paste_form(&paths))
    }
    #[cfg(not(windows))]
    {
        let helpers: &[(&str, &[&str])] = &[
            ("pbpaste", &[]),
            ("wl-paste", &["--no-newline", "--type", "text"]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
        ];
        let (prog, args) = helpers.iter().find(|(prog, _)| crate::util::which(prog).is_some())?;
        let out = cmd(prog).args(*args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        (!text.is_empty()).then_some(text)
    }
}

/// If the clipboard holds an image, save it as a PNG under `~/.bro/paste` and return its path; if it holds
/// copied files, return their paths. None = nothing image-like. Blocking (~0.3–1 s on Windows): call it from a
/// background thread.
pub fn grab_image() -> Option<Vec<String>> {
    let dir = crate::util::bro_dir().join("paste");
    let _ = std::fs::create_dir_all(&dir);
    let file: PathBuf = dir.join(format!("paste-{}.png", crate::util::now_ms()));
    #[cfg(windows)]
    {
        // Windows PowerShell 5.1 is STA by default (the clipboard needs STA); give it its own module path so a
        // pwsh-7 parent's PSModulePath doesn't break it.
        let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let ps = format!(r"{sysroot}\System32\WindowsPowerShell\v1.0\powershell.exe");
        let target = file.to_string_lossy().replace('\'', "''");
        let script = format!(
            r#"Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$img = [System.Windows.Forms.Clipboard]::GetImage()
if ($img) {{ $img.Save('{target}', [System.Drawing.Imaging.ImageFormat]::Png); 'IMG'; exit }}
$files = [System.Windows.Forms.Clipboard]::GetFileDropList()
foreach ($f in $files) {{ 'FILE ' + $f }}"#
        );
        let out = cmd(&ps)
            .env("PSModulePath", format!(r"{sysroot}\System32\WindowsPowerShell\v1.0\Modules"))
            .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", &script])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        if text.lines().any(|l| l.trim() == "IMG") && file.exists() {
            return Some(vec![file.to_string_lossy().to_string()]);
        }
        let files: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("FILE ")).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if files.is_empty() { None } else { Some(files) }
    }
    #[cfg(not(windows))]
    {
        if crate::util::which("wl-paste").is_some() {
            let types = cmd("wl-paste").arg("--list-types").output().ok()?;
            let types = String::from_utf8_lossy(&types.stdout).to_string();
            if types.lines().any(|t| t.trim() == "image/png") {
                let png = cmd("wl-paste").args(["--type", "image/png"]).output().ok()?;
                if !png.stdout.is_empty() && std::fs::write(&file, &png.stdout).is_ok() {
                    return Some(vec![file.to_string_lossy().to_string()]);
                }
            }
            if types.lines().any(|t| t.trim() == "text/uri-list") {
                let l = cmd("wl-paste").args(["--type", "text/uri-list"]).output().ok()?;
                let files: Vec<String> = String::from_utf8_lossy(&l.stdout).lines().filter_map(|u| u.trim().strip_prefix("file://")).map(String::from).collect();
                if !files.is_empty() {
                    return Some(files);
                }
            }
            return None;
        }
        if crate::util::which("xclip").is_some() {
            let t = cmd("xclip").args(["-selection", "clipboard", "-t", "TARGETS", "-o"]).output().ok()?;
            if String::from_utf8_lossy(&t.stdout).lines().any(|l| l.trim() == "image/png") {
                let png = cmd("xclip").args(["-selection", "clipboard", "-t", "image/png", "-o"]).output().ok()?;
                if !png.stdout.is_empty() && std::fs::write(&file, &png.stdout).is_ok() {
                    return Some(vec![file.to_string_lossy().to_string()]);
                }
            }
        }
        None
    }
}

/// How paths are pasted: quoted when they contain spaces (like dragging a file into a terminal).
pub fn paste_form(paths: &[String]) -> String {
    paths.iter().map(|p| if p.contains(' ') { format!("\"{p}\"") } else { p.clone() }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_and_paste_form() {
        assert_eq!(super::base64(b"hi"), "aGk=");
        assert_eq!(super::base64(b"abc"), "YWJj");
        assert_eq!(super::paste_form(&["a b.png".into(), "c.png".into()]), "\"a b.png\" c.png");
    }
}
