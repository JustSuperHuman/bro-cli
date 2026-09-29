//! Embeds the Just Terminal web client (`web/`, a copy of
//! `tools/terminal-web/dist/client`) into the crate as a static asset table.
//!
//! Generates `$OUT_DIR/embedded_client.rs` containing
//! `static EMBEDDED_CLIENT_ASSETS: &[EmbeddedClientAsset]`. When `web/` is
//! missing or empty the table is empty: the build still succeeds and the host
//! answers 404 for every non-API path.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let client = manifest.join("web");
    println!("cargo:rerun-if-changed=web");
    println!("cargo:rerun-if-changed=build.rs");

    let mut files = Vec::new();
    collect_files(&client, &mut files);
    files.sort();

    let mut generated =
        String::from("static EMBEDDED_CLIENT_ASSETS: &[EmbeddedClientAsset] = &[\n");
    for file in files {
        let Ok(relative) = file.strip_prefix(&client) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.starts_with('.') || relative.contains("/.") {
            // Dotfiles (e.g. .gitkeep) are not part of the client.
            continue;
        }
        println!("cargo:rerun-if-changed={}", file.display());
        let absolute = file
            .canonicalize()
            .unwrap_or_else(|_| file.clone())
            .to_string_lossy()
            .into_owned();
        generated.push_str(&format!(
            "    EmbeddedClientAsset {{ path: {relative:?}, content_type: {:?}, bytes: include_bytes!({absolute:?}) }},\n",
            content_type(&file)
        ));
    }
    generated.push_str("];\n");

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("embedded_client.rs");
    fs::write(output, generated).expect("could not generate the embedded web client table");
}
