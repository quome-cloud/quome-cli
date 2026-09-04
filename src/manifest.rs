//! Directory → deploy manifest. Port of the Python CLI's manifest.py
//! (quome-fastapi monorepo): same junk filtering as the browser drop
//! pipeline plus filesystem-only exclusions (.git, node_modules).
//! Meaningful dotfiles (.well-known/) are kept. The root-index check
//! mirrors the server's validate_static_upload so a bad upload fails
//! before any bytes move.

use std::path::{Path, PathBuf};

use crate::errors::{QuomeError, Result};

pub const MAX_FILES: usize = 5000;

const JUNK_NAMES: &[&str] = &["__MACOSX", ".DS_Store", "Thumbs.db"];
const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// Forward-slash path relative to the site root.
    pub path: String,
    pub size: u64,
    pub local: PathBuf,
}

/// mimetype guessing is table-driven: pin the modern web types a wrong
/// guess would hurt, fall back to a small extension map, then
/// octet-stream.
pub fn content_type_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "html" | "htm" => "text/html",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

pub fn build_manifest(root: &Path) -> Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    walk(root, root, &mut entries)?;
    if entries.len() > MAX_FILES {
        return Err(QuomeError::ApiError(format!(
            "{} files exceeds the {} file limit",
            entries.len(),
            MAX_FILES
        )));
    }
    if !entries.iter().any(|e| e.path == "index.html") {
        return Err(QuomeError::ApiError(
            "no index.html at the site root — deploy your build output directory".into(),
        ));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<ManifestEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if JUNK_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walk stays under root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(ManifestEntry {
                path: rel,
                size: meta.len(),
                local: path,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn site(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in files {
            let p = dir.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        dir
    }

    #[test]
    fn requires_root_index_html() {
        let dir = site(&["about.html"]);
        let err = build_manifest(dir.path()).unwrap_err();
        assert!(err.to_string().contains("index.html"), "{err}");
    }

    #[test]
    fn walks_and_filters_junk() {
        let dir = site(&[
            "index.html",
            "assets/app.js",
            ".well-known/security.txt",
            ".DS_Store",
            "__MACOSX/resource",
            ".git/HEAD",
            "node_modules/pkg/index.js",
        ]);
        let entries = build_manifest(dir.path()).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"index.html"));
        assert!(paths.contains(&"assets/app.js"));
        assert!(paths.contains(&".well-known/security.txt"));
        assert_eq!(paths.len(), 3, "junk leaked: {paths:?}");
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for("a/app.js"), "text/javascript");
        assert_eq!(content_type_for("s.svg"), "image/svg+xml");
        assert_eq!(content_type_for("f.woff2"), "font/woff2");
        assert_eq!(content_type_for("x.html"), "text/html");
        assert_eq!(content_type_for("unknown.zzz"), "application/octet-stream");
    }
}
