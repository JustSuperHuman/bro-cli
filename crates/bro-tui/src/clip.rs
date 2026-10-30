//! The system clipboard (ported from z4-oriel): copy text out of bro (selections), and grab an image or copied
//! files from it (ctrl+v pastes text; alt+v pastes an image into Claude Code / Codex as a file path). On Windows the
//! desktop app also copies pictures and files onto it ([`put_picture`], [`put_files`]).
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
/// The desktop app's own clipboard (set once at its start): when there is one, copies go straight to it instead
/// of through the terminal (OSC 52) and the clipboard tools.
pub static NATIVE: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

pub fn copy(text: &str) {
    if cfg!(test) {
        return;
    }
    if let Some(native) = NATIVE.get() {
        native(text);
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
        use clipboard_win::{Getter, formats};
        let _clipboard = open_clipboard()?;
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

/// Pixels read from the clipboard: straight-alpha RGBA8.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{})", self.w, self.h)
    }
}

/// Thumbnails of pictures just pasted, made while their pixels were at hand: the window shows one the moment the
/// paste lands instead of reading the file again. Taken once; the last few kept.
static PREVIEWS: std::sync::Mutex<Vec<(PathBuf, Picture)>> = std::sync::Mutex::new(Vec::new());

/// The thumbnail made for `path` as it was pasted, if there is one (once).
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn take_preview(path: &std::path::Path) -> Option<Picture> {
    let mut p = PREVIEWS.lock().ok()?;
    let i = p.iter().position(|(q, _)| q == path)?;
    Some(p.remove(i).1)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn keep_preview(path: &std::path::Path, picture: &Picture) {
    if let Ok(mut p) = PREVIEWS.lock() {
        p.push((path.to_path_buf(), shrink(picture, 256)));
        let n = p.len();
        if n > 6 {
            p.drain(..n - 6);
        }
    }
}

/// `pic` fitted inside `max` × `max`, each pixel the average of those it covers.
#[cfg_attr(not(windows), allow(dead_code))]
fn shrink(pic: &Picture, max: u32) -> Picture {
    let long = pic.w.max(pic.h).max(1);
    if long <= max {
        return pic.clone();
    }
    let (w, h) = (((pic.w as u64 * max as u64) / long as u64).max(1) as u32, ((pic.h as u64 * max as u64) / long as u64).max(1) as u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let (y0, y1) = (y * pic.h / h, ((y + 1) * pic.h / h).max(y * pic.h / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * pic.w / w, ((x + 1) * pic.w / w).max(x * pic.w / w + 1));
            let mut sum = [0u64; 4];
            for sy in y0..y1 {
                let row = (sy * pic.w) as usize * 4;
                for sx in x0..x1 {
                    let i = row + sx as usize * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += pic.px[i + c] as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                px[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    Picture { w, h, px }
}

/// If the clipboard holds an image, save it as a PNG under `~/.bro/paste` and return its path; if it holds
/// copied files, return their paths. None = nothing image-like. On Windows the clipboard's own PNG or bitmap is
/// read directly (a few ms, decoded once), Windows PowerShell only for a bitmap bro can't read; elsewhere the
/// clipboard tools. Blocking: call it from a background thread.
pub fn grab_image() -> Option<Vec<String>> {
    let dir = crate::util::bro_dir().join("paste");
    let _ = std::fs::create_dir_all(&dir);
    let file: PathBuf = dir.join(format!("paste-{}.png", crate::util::now_ms()));
    #[cfg(windows)]
    {
        // Browsers can publish a correct PNG alongside a synthesized CF_BITMAP
        // that WinForms reads as an all-black 1-bit image. Keep the original PNG.
        if let Some((bytes, picture)) = grab_png() {
            std::fs::write(&file, bytes).ok()?;
            keep_preview(&file, &picture);
            return Some(vec![file.to_string_lossy().into_owned()]);
        }
        // a screenshot (Print Screen, most apps): the bitmap itself, as a PNG
        if let Some(picture) = grab_dib() {
            write_png(&file, &picture)?;
            keep_preview(&file, &picture);
            return Some(vec![file.to_string_lossy().into_owned()]);
        }
        // copied files
        {
            use clipboard_win::{Getter, formats};
            let files = open_clipboard().and_then(|_c| {
                let mut paths: Vec<String> = vec![];
                formats::FileList.read_clipboard(&mut paths).ok()?;
                (!paths.is_empty()).then_some(paths)
            });
            if files.is_some() {
                return files;
            }
        }
        // nothing image-like: no need to ask anything slower
        if !has_bitmap() {
            return None;
        }
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

/// The clipboard's PNG (as a browser or a screenshot tool puts it there) and its pixels: decoding it once both
/// checks it and gives the thumbnail.
#[cfg(windows)]
fn grab_png() -> Option<(Vec<u8>, Picture)> {
    use clipboard_win::raw;
    let candidates = {
        let _clipboard = open_clipboard()?;
        ["PNG", "image/png"].into_iter().filter_map(|name| {
            let format = raw::register_format(name)?.get();
            let size = raw::size(format)?.get();
            if size > 256 * 1024 * 1024 { return None; }
            let mut bytes = vec![0; size];
            let copied = raw::get(format, &mut bytes).ok()?;
            bytes.truncate(copied);
            Some(bytes)
        }).collect::<Vec<_>>()
    }; // Release the clipboard before decoding or writing anything.
    candidates.into_iter().find_map(|bytes| decode_png(&bytes).map(|p| (bytes, p)))
}

/// The clipboard's bitmap (CF_DIB), read directly.
#[cfg(windows)]
fn grab_dib() -> Option<Picture> {
    use clipboard_win::raw;
    const CF_DIB: u32 = 8;
    let bytes = {
        let _clipboard = open_clipboard()?;
        let size = raw::size(CF_DIB)?.get();
        if size > 512 * 1024 * 1024 {
            return None;
        }
        let mut bytes = vec![0; size];
        let copied = raw::get(CF_DIB, &mut bytes).ok()?;
        bytes.truncate(copied);
        bytes
    };
    dib_picture(&bytes)
}

/// The clipboard has a bitmap of some kind (one bro may not read itself).
#[cfg(windows)]
fn has_bitmap() -> bool {
    use clipboard_win::raw;
    const FORMATS: [u32; 3] = [2, 8, 17]; // CF_BITMAP, CF_DIB, CF_DIBV5
    open_clipboard().is_some_and(|_c| FORMATS.iter().any(|f| raw::is_format_avail(*f)))
}

/// Open the clipboard, waiting a moment while someone else has it: Windows' clipboard history and other watchers
/// read every copy the moment it's made, a few milliseconds each. Blocking (up to a fifth of a second): call it
/// from a background thread.
#[cfg(windows)]
fn open_clipboard() -> Option<clipboard_win::Clipboard> {
    for _ in 0..50 {
        if let Ok(clipboard) = clipboard_win::Clipboard::new() {
            return Some(clipboard);
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    None
}

/// A PNG's pixels as RGBA8 (None: it isn't one, or it's broken or too big).
#[cfg(any(windows, test))]
fn decode_png(bytes: &[u8]) -> Option<Picture> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_limits(png::Limits { bytes: 256 * 1024 * 1024 });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let size = reader.output_buffer_size().filter(|size| *size <= 256 * 1024 * 1024)?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = buf.get(y * info.line_size..y * info.line_size + w as usize * channels)?;
        for p in row.chunks_exact(channels) {
            px.extend_from_slice(&match channels {
                1 => [p[0], p[0], p[0], 255],
                2 => [p[0], p[0], p[0], p[1]],
                3 => [p[0], p[1], p[2], 255],
                _ => [p[0], p[1], p[2], p[3]],
            });
        }
    }
    Some(Picture { w, h, px })
}

/// A device-independent bitmap (the clipboard's CF_DIB: a BITMAPINFOHEADER, its colour masks, then the rows) as
/// RGBA: the 24- and 32-bit ones Windows makes for screenshots. None for any other kind.
#[cfg(any(windows, test))]
fn dib_picture(dib: &[u8]) -> Option<Picture> {
    let u32_at = |i: usize| dib.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let header = u32_at(0)? as usize;
    let (w, h) = (u32_at(4)? as i32, u32_at(8)? as i32);
    let bits = dib.get(14..16).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
    let compression = u32_at(16)?;
    let colours = u32_at(32)? as usize;
    if header < 40 || w <= 0 || h == 0 || w > 30_000 || h.unsigned_abs() > 30_000 || !matches!(bits, 24 | 32) {
        return None;
    }
    // BI_RGB: no masks; BI_BITFIELDS / BI_ALPHABITFIELDS: masks in a V4/V5 header, else right after it
    let (masks, data) = match compression {
        0 => (None, header),
        3 | 6 if header >= 52 => (Some([u32_at(40)?, u32_at(44)?, u32_at(48)?, if header >= 56 { u32_at(52)? } else { 0 }]), header),
        3 => (Some([u32_at(40)?, u32_at(44)?, u32_at(48)?, 0]), header + 12),
        6 => (Some([u32_at(40)?, u32_at(44)?, u32_at(48)?, u32_at(52)?]), header + 16),
        _ => return None,
    };
    let (w, rows, top_down) = (w as usize, h.unsigned_abs() as usize, h < 0);
    let stride = (w * bits as usize).div_ceil(32) * 4;
    let start = data + colours * 4;
    let pixels = dib.get(start..start + stride * rows)?;
    let channel = |v: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 0;
        }
        let (shift, width) = (mask.trailing_zeros(), mask.count_ones());
        let c = (v & mask) >> shift;
        if width >= 8 { (c >> (width - 8)) as u8 } else { (c * 255 / ((1 << width) - 1)) as u8 }
    };
    let mut px = vec![0u8; w * rows * 4];
    let mut alpha = false;
    for y in 0..rows {
        let src = &pixels[(if top_down { y } else { rows - 1 - y }) * stride..][..stride];
        for x in 0..w {
            let o = (y * w + x) * 4;
            let p = match (bits, masks) {
                (24, _) => [src[x * 3 + 2], src[x * 3 + 1], src[x * 3], 255],
                (_, None) => [src[x * 4 + 2], src[x * 4 + 1], src[x * 4], src[x * 4 + 3]],
                (_, Some(m)) => {
                    let v = u32::from_le_bytes([src[x * 4], src[x * 4 + 1], src[x * 4 + 2], src[x * 4 + 3]]);
                    [channel(v, m[0]), channel(v, m[1]), channel(v, m[2]), if m[3] == 0 { 255 } else { channel(v, m[3]) }]
                }
            };
            alpha |= bits == 32 && masks.is_none_or(|m| m[3] != 0) && p[3] != 0;
            px[o..o + 4].copy_from_slice(&p);
        }
    }
    // a 32-bit bitmap whose fourth byte is always 0 doesn't use it: it's opaque
    if bits == 32 && masks.is_none_or(|m| m[3] != 0) && !alpha {
        px.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
    Some(Picture { w: w as u32, h: rows as u32, px })
}

/// Save `pic` as a PNG (fast compression: it's read again in moments).
#[cfg(windows)]
fn write_png(path: &std::path::Path, pic: &Picture) -> Option<()> {
    let file = std::fs::File::create(path).ok()?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), pic.w, pic.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header().ok()?.write_image_data(&pic.px).ok()
}

/// Put a picture on the clipboard as a browser's "copy image" does: its PNG (`png`, else one made from the pixels),
/// which keeps transparency for the apps that read it, then a bitmap (CF_DIB) for the rest; Windows makes the other
/// bitmap kinds from that. Blocking: call it from a background thread.
#[cfg(windows)]
pub fn put_picture(pic: &Picture, png: Option<Vec<u8>>) -> Result<(), String> {
    use clipboard_win::raw;
    const CF_DIB: u32 = 8;
    if pic.w == 0 || pic.h == 0 {
        return Err("the picture is empty".into());
    }
    // both made before the clipboard opens: it's held only while they're handed over
    let png = png.or_else(|| encode_png(pic)).ok_or("couldn't make a PNG of the picture")?;
    let dib = dib(pic);
    let _clipboard = open_clipboard().ok_or(BUSY)?;
    raw::empty().map_err(not_put)?;
    // the PNG first: apps take the first kind they can read
    if let Some(format) = raw::register_format("PNG") {
        raw::set_without_clear(format.get(), &png).map_err(not_put)?;
    }
    raw::set_without_clear(CF_DIB, &dib).map_err(not_put)
}

/// Put files on the clipboard as Explorer copies them (CF_HDROP, marked to be copied rather than moved): a folder,
/// a chat or a mail pastes the files themselves. Blocking: call it from a background thread.
#[cfg(windows)]
pub fn put_files(paths: &[String]) -> Result<(), String> {
    use clipboard_win::{options, raw};
    const DROPEFFECT_COPY: u32 = 1;
    let _clipboard = open_clipboard().ok_or(BUSY)?;
    raw::empty().map_err(not_put)?;
    raw::set_file_list_with(paths, options::NoClear).map_err(not_put)?;
    if let Some(format) = raw::register_format("Preferred DropEffect") {
        raw::set_without_clear(format.get(), &DROPEFFECT_COPY.to_le_bytes()).map_err(not_put)?;
    }
    Ok(())
}

#[cfg(windows)]
const BUSY: &str = "another app is using the clipboard; try again";

#[cfg(windows)]
fn not_put(e: clipboard_win::ErrorCode) -> String {
    format!("couldn't put it on the clipboard ({e})")
}

/// `pic` as PNG bytes (fast compression: it's pasted in moments).
#[cfg(any(windows, test))]
fn encode_png(pic: &Picture) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, pic.w, pic.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&pic.px).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// `pic` as a device-independent bitmap, as browsers copy pictures: a BITMAPINFOHEADER, then the rows bottom-up as
/// 32-bit BGRA, the alpha in the fourth byte.
#[cfg(any(windows, test))]
fn dib(pic: &Picture) -> Vec<u8> {
    let row = pic.w as usize * 4;
    let mut out = Vec::with_capacity(40 + pic.px.len());
    // size, width, height (positive: bottom-up)
    for v in [40, pic.w, pic.h] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // one plane, 32 bits
    out.extend_from_slice(&[1, 0, 32, 0]);
    // BI_RGB, the pixels' bytes, no resolution (apps assume their own), no palette
    for v in [0, pic.px.len() as u32, 0, 0, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for line in pic.px.chunks_exact(row.max(4)).rev() {
        for [r, g, b, a] in line.as_chunks::<4>().0 {
            out.extend_from_slice(&[*b, *g, *r, *a]);
        }
    }
    out
}

/// How paths are pasted: quoted when they contain spaces (like dragging a file into a terminal).
pub fn paste_form(paths: &[String]) -> String {
    paths.iter().map(|p| if p.contains(' ') { format!("\"{p}\"") } else { p.clone() }).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_png_accepts_rgba_and_rejects_broken_data() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 0, 10, 0, 0, 255, 20, 128]).unwrap();
        }
        let p = super::decode_png(&bytes).expect("a picture");
        assert_eq!((p.w, p.h, p.px.clone()), (2, 1, vec![255, 0, 10, 0, 0, 255, 20, 128]), "alpha kept as it was");
        assert!(super::decode_png(&bytes[..33]).is_none());
        assert!(super::decode_png(b"not an image").is_none());
    }

    #[test]
    fn a_clipboard_bitmap_reads_as_the_picture_it_is() {
        // 2x2, 24-bit, bottom-up: the bottom row comes first in the data, each row padded to 4 bytes
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 2;
        dib[8] = 2;
        dib[12] = 1;
        dib[14] = 24;
        // bottom row: blue, green; top row: red, white (BGR order)
        dib.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0]);
        dib.extend_from_slice(&[0, 0, 255, 255, 255, 255, 0, 0]);
        let p = super::dib_picture(&dib).expect("a picture");
        assert_eq!((p.w, p.h), (2, 2));
        assert_eq!(&p.px[..8], &[255, 0, 0, 255, 255, 255, 255, 255], "the top row first: red, white");
        assert_eq!(&p.px[8..], &[0, 0, 255, 255, 0, 255, 0, 255]);
        // 32-bit with nothing in the fourth byte: opaque, not invisible
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 1;
        dib[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        dib[12] = 1;
        dib[14] = 32;
        dib.extend_from_slice(&[10, 20, 30, 0]);
        let p = super::dib_picture(&dib).expect("a picture");
        assert_eq!(p.px, [30, 20, 10, 255]);
        dib[14] = 8;
        assert!(super::dib_picture(&dib).is_none(), "palette bitmaps go the slow way");
    }

    #[test]
    fn a_copied_picture_reads_back_as_itself() {
        // 2x2 with transparency: red, half-see-through green / blue, invisible
        let pic = super::Picture { w: 2, h: 2, px: vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 9, 9, 9, 0] };
        let dib = super::dib(&pic);
        assert_eq!(dib.len(), 40 + 16);
        assert_eq!(&dib[40..44], &[255, 0, 0, 255], "bottom row first, blue-green-red");
        assert_eq!(super::dib_picture(&dib), Some(pic.clone()), "the bitmap is the picture, alpha and all");
        let png = super::encode_png(&pic).expect("a PNG");
        assert_eq!(super::decode_png(&png), Some(pic));
    }

    #[test]
    fn a_pasted_picture_has_a_thumbnail_waiting() {
        let big = super::Picture { w: 512, h: 256, px: vec![200; 512 * 256 * 4] };
        let small = super::shrink(&big, 256);
        assert_eq!((small.w, small.h), (256, 128));
        assert!(small.px.iter().all(|v| *v == 200));
        let path = std::path::Path::new("/paste-test.png");
        super::keep_preview(path, &big);
        assert_eq!(super::take_preview(path).map(|p| (p.w, p.h)), Some((256, 128)));
        assert!(super::take_preview(path).is_none(), "taken once");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads the current clipboard PNG and briefly creates its paste file; does not change the clipboard"]
    fn live_image_paste_preserves_original_png_bytes() {
        let (original, _) = super::grab_png().expect("copy an image with a PNG clipboard format first");
        let paths = super::grab_image().expect("image paste failed");
        assert_eq!(paths.len(), 1);
        let saved = std::fs::read(&paths[0]).unwrap();
        std::fs::remove_file(&paths[0]).unwrap();
        assert_eq!(saved, original, "image paste must preserve the original PNG, including alpha");
    }

    /// What `read` finds on the clipboard once a new copy has settled: for a few milliseconds after it's made,
    /// Windows (its clipboard history) is still looking at it, and a read can come back empty.
    #[cfg(windows)]
    fn settled<T>(read: impl Fn() -> Option<T>) -> Option<T> {
        (0..50).find_map(|_| read().or_else(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            None
        }))
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "puts a picture, then a file, on the real clipboard; puts back a picture that was there before"]
    fn live_copy_puts_pictures_and_files_on_the_clipboard() {
        use clipboard_win::{Getter, formats, raw};
        let before = super::grab_png();
        let pic = super::Picture { w: 3, h: 2, px: (0..24).map(|i| i * 10).collect() };
        let png = super::encode_png(&pic).unwrap();
        super::put_picture(&pic, Some(png.clone())).expect("copy the picture");
        let (got, read) = settled(super::grab_png).expect("a PNG on the clipboard");
        assert_eq!(got, png, "the PNG as it was given");
        assert_eq!(read, pic);
        assert_eq!(settled(super::grab_dib), Some(pic), "and a bitmap of the same pixels");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clip one.mp4");
        std::fs::write(&file, b"not really a video").unwrap();
        let name = file.to_string_lossy().to_string();
        super::put_files(std::slice::from_ref(&name)).expect("copy the file");
        let (paths, effect, bitmap) = settled(|| {
            let _clipboard = super::open_clipboard()?;
            let mut paths: Vec<String> = vec![];
            formats::FileList.read_clipboard(&mut paths).ok()?;
            let mut effect = vec![0; 4];
            raw::get(raw::register_format("Preferred DropEffect")?.get(), &mut effect).ok()?;
            Some((paths, effect, raw::is_format_avail(8)))
        })
        .expect("the file on the clipboard");
        assert_eq!(paths, vec![name]);
        assert_eq!(effect, 1u32.to_le_bytes(), "pasted as a copy, not a move");
        assert!(!bitmap, "the picture before it is gone");
        if let Some((png, pic)) = before {
            super::put_picture(&pic, Some(png)).unwrap();
        }
    }

    #[test]
    fn base64_and_paste_form() {
        assert_eq!(super::base64(b"hi"), "aGk=");
        assert_eq!(super::base64(b"abc"), "YWJj");
        assert_eq!(super::paste_form(&["a b.png".into(), "c.png".into()]), "\"a b.png\" c.png");
    }
}
