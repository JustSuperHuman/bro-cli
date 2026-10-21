//! What a path is (image, video, audio, PDF) and whether a click may hand it to the OS.

use std::io::Read;
use std::path::Path;

/// What a path is, by extension (case-insensitive), sniffing magic bytes when the extension is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Pdf,
    Other,
}

const IMAGE: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "pjpeg", "pjp", "gif", "webp", "bmp", "dib", "ico", "cur", "tif", "tiff", "svg", "svgz",
    "avif", "heic", "heif", "hif", "jxl", "jxr", "wdp", "hdp", "tga", "dds", "psd", "exr", "hdr", "qoi", "pbm", "pgm", "ppm", "pnm", "pam",
    "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "srw", "pef",
];

/// `.ts` and `.mts` are left out on purpose: in a code tree they are TypeScript far more often
/// than MPEG transport streams, so those go through sniffing instead.
const VIDEO: &[&str] = &[
    "mp4", "m4v", "mov", "qt", "webm", "mkv", "avi", "wmv", "asf", "flv", "f4v", "mpg", "mpeg", "mpe", "m2v", "m2ts", "3gp", "3g2", "ogv",
    "vob", "mxf", "dv", "rm", "rmvb", "divx", "y4m",
];

const AUDIO: &[&str] = &[
    "mp3", "wav", "wave", "m4a", "m4b", "aac", "adts", "flac", "ogg", "oga", "opus", "spx", "wma", "aif", "aiff", "aifc", "caf", "mka", "weba",
    "amr", "ac3", "eac3", "dts", "mp2", "mpa", "au", "snd", "mid", "midi", "ape", "wv", "tta",
];

/// Common text and source extensions: known not to be media, so no disk read to sniff them.
const TEXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonl", "json5", "toml", "yaml", "yml", "ini", "cfg", "conf", "env", "lock",
    "xml", "html", "htm", "css", "scss", "sass", "less", "js", "mjs", "cjs", "jsx", "cts", "tsx", "vue", "svelte", "rs", "go",
    "py", "rb", "php", "java", "kt", "kts", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "fs", "lua", "sh", "bash", "zsh", "fish", "ps1",
    "psm1", "bat", "cmd", "sql", "graphql", "proto", "gradle", "diff", "patch", "gitignore", "gitattributes", "editorconfig",
];

/// Never handed to the OS "open" verb from a click: programs, scripts, installers, and shortcuts or
/// shell files that can point at one. Lowercase, without the dot.
const EXECUTABLE: &[&str] = &[
    // Windows programs and installers
    "exe", "com", "scr", "pif", "cpl", "msc", "msi", "msp", "mst", "msix", "msixbundle", "appx", "appxbundle", "appinstaller", "application",
    "appref-ms", "xbap", "gadget", "dll", "ocx", "sys", "drv",
    // Windows scripts and script hosts
    "bat", "cmd", "ps1", "ps1xml", "ps2", "ps2xml", "psc1", "psc2", "psd1", "psm1", "vb", "vbe", "vbs", "vbscript", "js", "jse", "ws", "wsc",
    "wsf", "wsh", "hta", "sct", "shb", "shs", "msh", "msh1", "msh2", "mshxml", "msh1xml", "msh2xml", "chm", "hlp", "ahk", "au3",
    // Windows shortcuts and shell files that can launch something
    "lnk", "url", "website", "inf", "reg", "scf", "iqy", "settingcontent-ms", "library-ms", "searchconnector-ms", "diagcab", "diagcfg",
    "diagpkg",
    // cross-platform runtimes that run on double-click once installed
    "jar", "jnlp", "py", "pyw", "pyz", "pyzw", "pyc", "pyo", "pl", "rb", "rbw",
    // macOS
    "app", "command", "tool", "terminal", "workflow", "action", "scpt", "scptd", "applescript", "pkg", "mpkg", "osax", "prefpane",
    "webloc", "inetloc", "fileloc",
    // Linux and other Unix
    "sh", "bash", "zsh", "fish", "csh", "tcsh", "ksh", "run", "bin", "out", "elf", "so", "appimage", "desktop", "deb", "rpm", "snap",
    "flatpakref",
];

/// What `path` is. Known extensions decide without touching the disk; anything else (no extension,
/// an unknown one) is sniffed from its first bytes.
pub fn kind_of(path: &Path) -> MediaKind {
    if let Some(ext) = extension(path) {
        let ext = ext.as_str();
        if ext == "pdf" {
            return MediaKind::Pdf;
        }
        for (list, kind) in [(IMAGE, MediaKind::Image), (VIDEO, MediaKind::Video), (AUDIO, MediaKind::Audio)] {
            if list.contains(&ext) {
                return kind;
            }
        }
        if TEXT.contains(&ext) {
            return MediaKind::Other;
        }
    }
    head(path, 1024).map(|bytes| sniff(&bytes)).unwrap_or(MediaKind::Other)
}

/// True for files that must never be handed to the OS "open" verb from a click (exe, msi, bat,
/// cmd, ps1, scr, com, lnk, vbs, js, jar, app, sh...). An extensionless file counts when it starts
/// like a program (PE, ELF, Mach-O, or a `#!` script).
pub fn is_executable(path: &Path) -> bool {
    match extension(path) {
        Some(ext) => EXECUTABLE.contains(&ext.as_str()),
        None => head(path, 4).is_some_and(|b| looks_like_program(&b)),
    }
}

/// The lowercase extension, ignoring the trailing dots and spaces Windows strips when it opens a
/// path ("setup.exe." runs setup.exe).
fn extension(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let name = name.trim_end_matches(['.', ' ']);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() || name.starts_with('.')).then(|| ext.to_ascii_lowercase()).filter(|e| !e.is_empty())
}

fn head(path: &Path, n: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(n);
    std::fs::File::open(path).ok()?.take(n as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn looks_like_program(b: &[u8]) -> bool {
    b.starts_with(b"MZ")
        || b.starts_with(b"\x7fELF")
        || b.starts_with(b"#!")
        || b.starts_with(&[0xfe, 0xed, 0xfa, 0xce])
        || b.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
        || b.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || b.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
}

/// The kind a file's first bytes announce.
pub(crate) fn sniff(b: &[u8]) -> MediaKind {
    let at = |off: usize, magic: &[u8]| b.len() >= off + magic.len() && &b[off..off + magic.len()] == magic;
    if at(0, b"%PDF-") {
        return MediaKind::Pdf;
    }
    let image = at(0, b"\x89PNG\r\n\x1a\n")
        || at(0, &[0xff, 0xd8, 0xff])
        || at(0, b"GIF87a")
        || at(0, b"GIF89a")
        || (at(0, b"RIFF") && at(8, b"WEBP"))
        || (at(0, b"BM") && b.len() >= 18 && matches!(u32::from_le_bytes([b[14], b[15], b[16], b[17]]), 12 | 40 | 52 | 56 | 64 | 108 | 124))
        || at(0, &[0, 0, 1, 0])
        || at(0, &[0, 0, 2, 0])
        || at(0, b"II*\0")
        || at(0, b"MM\0*")
        || at(0, &[0xff, 0x0a])
        || at(0, b"\0\0\0\x0cJXL ")
        || at(0, b"8BPS")
        || at(0, b"qoif");
    if image {
        return MediaKind::Image;
    }
    if at(4, b"ftyp") {
        let brand = b.get(8..12).unwrap_or_default();
        return match brand {
            b"avif" | b"avis" | b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => MediaKind::Image,
            b"M4A " | b"M4B " | b"M4P " | b"F4A " | b"F4B " => MediaKind::Audio,
            _ => MediaKind::Video,
        };
    }
    if at(0, b"RIFF") {
        return if at(8, b"WAVE") { MediaKind::Audio } else if at(8, b"AVI ") { MediaKind::Video } else { MediaKind::Other };
    }
    if at(0, &[0x1a, 0x45, 0xdf, 0xa3]) || at(0, b"FLV") || at(0, &[0, 0, 1, 0xba]) || at(0, &[0, 0, 1, 0xb3]) {
        return MediaKind::Video;
    }
    // MPEG transport stream: 188-byte packets (or 192 with a timecode prefix), each starting 0x47.
    if (at(0, &[0x47]) && at(188, &[0x47])) || (at(4, &[0x47]) && at(196, &[0x47])) {
        return MediaKind::Video;
    }
    // ASF holds WMV and WMA alike; a video player handles both.
    if at(0, &[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11]) {
        return MediaKind::Video;
    }
    if at(0, b"OggS") {
        return if contains(b, b"\x80theora") { MediaKind::Video } else { MediaKind::Audio };
    }
    if at(0, b"ID3")
        || at(0, b"fLaC")
        || at(0, b"MThd")
        || at(0, b"#!AMR")
        || at(0, b".snd")
        || at(0, b"MAC ")
        || at(0, b"wvpk")
        || (at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")))
        || (b.len() >= 2 && b[0] == 0xff && (b[1] & 0xe0) == 0xe0 && (b[1] & 0x06) != 0)
    {
        return MediaKind::Audio;
    }
    if is_svg(b) {
        return MediaKind::Image;
    }
    MediaKind::Other
}

/// An XML document whose root is `<svg`, allowing a BOM, an XML declaration, comments and a doctype first.
pub(crate) fn is_svg(b: &[u8]) -> bool {
    let b = b.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(b);
    let text = String::from_utf8_lossy(&b[..b.len().min(1024)]);
    let t = text.trim_start();
    (t.starts_with('<')) && (t.starts_with("<svg") || ((t.starts_with("<?xml") || t.starts_with("<!")) && t.contains("<svg")))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
