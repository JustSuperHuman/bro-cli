use base64::Engine;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub fn read_preview(cwd: &str, target: &str, requested_line: usize) -> Result<Value, String> {
    if cwd.is_empty()
        || target.is_empty()
        || target.chars().any(char::is_control)
        || target.starts_with("\\\\")
        || target.contains("://")
    {
        return Err("Invalid file path.".into());
    }
    let root = Path::new(cwd)
        .canonicalize()
        .map_err(|_| "The session directory is unavailable.")?;
    let resolved = root
        .join(target)
        .canonicalize()
        .map_err(|_| "File not found or unavailable.")?;
    if !resolved.starts_with(&root) {
        return Err("This file is outside the session's workspace.".into());
    }
    let file = File::open(&resolved).map_err(|_| "File could not be opened.")?;
    let metadata = file
        .metadata()
        .map_err(|_| "File could not be inspected.")?;
    if !metadata.is_file() {
        return Err("This path is not a file.".into());
    }
    if metadata.len() > MAX_BYTES {
        return Err("This file is too large to preview (2 MB limit).".into());
    }
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|_| "File could not be read.")?;
    if data.len() as u64 > MAX_BYTES {
        return Err("This file is too large to preview (2 MB limit).".into());
    }
    let display_path = resolved
        .to_string_lossy()
        .trim_start_matches("\\\\?\\")
        .to_string();
    let extension = resolved
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    };
    if let Some(mime) = mime {
        return Ok(
            json!({ "path": display_path, "kind": "image", "mimeType": mime,
            "data": base64::engine::general_purpose::STANDARD.encode(data) }),
        );
    }
    if data.contains(&0) {
        return Err("This binary file cannot be previewed.".into());
    }
    let text = String::from_utf8(data)
        .map_err(|_| "This file is not UTF-8 text and cannot be previewed.")?;
    let lines: Vec<_> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let line = requested_line.max(1).min(lines.len());
    let start = line.saturating_sub(21);
    let end = (start + 300).min(lines.len());
    let selected = lines[start..end].join("\n");
    let content: String = selected.chars().take(120_000).collect();
    Ok(
        json!({ "path": display_path, "kind": "text", "line": line, "startLine": start + 1,
        "totalLines": lines.len(), "truncated": start > 0 || end < lines.len() || content.len() < selected.len(), "content": content }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_resolves_paths_and_centres_line_references() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file with spaces.rs");
        std::fs::write(
            &file,
            (1..=500)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let preview =
            read_preview(root.path().to_str().unwrap(), "file with spaces.rs", 100).unwrap();
        assert_eq!(preview["line"], 100);
        assert_eq!(preview["startLine"], 80);
        assert_eq!(preview["totalLines"], 500);
        assert_eq!(preview["truncated"], true);
        assert!(preview["content"].as_str().unwrap().contains("line 100"));
        assert!(read_preview(root.path().to_str().unwrap(), file.to_str().unwrap(), 1).is_ok());
        assert!(read_preview(root.path().to_str().unwrap(), ".", 1).is_err());
        assert!(read_preview(root.path().to_str().unwrap(), "missing", 1).is_err());
    }
    #[test]
    fn preview_rejects_files_outside_workspace_and_binary_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let cwd = root.path().to_str().unwrap();
        assert!(
            read_preview(cwd, outside.path().to_str().unwrap(), 1)
                .unwrap_err()
                .contains("outside")
        );
        std::fs::write(root.path().join("binary"), [0u8, 1, 2]).unwrap();
        assert!(
            read_preview(cwd, "binary", 1)
                .unwrap_err()
                .contains("binary")
        );
        std::fs::write(
            root.path().join("large"),
            vec![b'a'; MAX_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            read_preview(cwd, "large", 1)
                .unwrap_err()
                .contains("too large")
        );
    }
}
