//! The system's own "open a file" dialog, for a picture (a project's icon). It blocks until you pick or cancel:
//! call it off the window's thread.

use std::path::{Path, PathBuf};

/// The picture formats offered (the ones a project icon can be).
const PATTERNS: &str = "*.svg;*.png;*.ico;*.webp;*.jpg;*.jpeg;*.gif";

pub fn picture(title: &str, start: &Path) -> Option<PathBuf> { choose(title, start, false) }

/// A self-contained T3 or VS Code theme. Parsing and installation also run off the UI thread.
pub fn theme(start: &Path) -> Option<PathBuf> { choose("Import a T3 Code or VS Code theme", start, true) }

/// Ask for a picture, starting in `start`. None when cancelled, or when there's no dialog to ask with.
#[cfg(windows)]
fn choose(title: &str, start: &Path, theme: bool) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    // pairs of (name, patterns), each ending in a nul, the list in another
    let filter: Vec<u16> = if theme { "Themes (JSON, JSONC)\0*.json;*.jsonc\0All files\0*.*\0\0".to_string() }
        else { format!("Pictures (SVG, PNG, ICO, WebP, JPEG, GIF)\0{PATTERNS}\0All files\0*.*\0\0") }.encode_utf16().collect();
    let title = wide(title);
    let start = wide(&start.to_string_lossy());
    let mut file = vec![0u16; 32_768];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrInitialDir: start.as_ptr(),
        lpstrTitle: title.as_ptr(),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: a thread of its own, its apartment set up once; every buffer outlives the call.
    let ok = unsafe {
        CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
        GetOpenFileNameW(&mut ofn)
    };
    if ok == 0 {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&file[..len])))
}

/// Ask for a picture, starting in `start` (AppleScript's chooser).
#[cfg(target_os = "macos")]
fn choose(title: &str, start: &Path, theme: bool) -> Option<PathBuf> {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "POSIX path of (choose file with prompt \"{}\" {} default location (POSIX file \"{}\"))",
        quote(title),
        if theme { "" } else { "of type {\"public.image\", \"com.microsoft.ico\", \"public.svg-image\"}" },
        quote(&start.to_string_lossy())
    );
    let out = std::process::Command::new("osascript").args(["-e", &script]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
}

/// Ask for a picture, starting in `start`: zenity, else kdialog, whichever is installed.
#[cfg(all(unix, not(target_os = "macos")))]
fn choose(title: &str, start: &Path, theme: bool) -> Option<PathBuf> {
    let dir = format!("{}/", start.to_string_lossy());
    let filter = if theme { "*.json *.jsonc".into() } else { PATTERNS.replace(';', " ") };
    let label = if theme { "Themes" } else { "Pictures" };
    let tries: [(&str, Vec<String>); 2] = [
        ("zenity", vec!["--file-selection".into(), format!("--title={title}"), format!("--filename={dir}"), format!("--file-filter={label} | {filter}")]),
        ("kdialog", vec!["--title".into(), title.into(), "--getopenfilename".into(), dir.clone(), format!("{label} ({filter})")]),
    ];
    for (program, args) in tries {
        let Ok(out) = std::process::Command::new(program).args(&args).output() else { continue };
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path));
    }
    None
}
