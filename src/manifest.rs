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
        return Err(QuomeError::Usage(format!(
            "{} files exceeds the {} file limit",
            entries.len(),
            MAX_FILES
        )));
    }
    if !entries.iter().any(|e| e.path == "index.html") {
        return Err(QuomeError::Usage(format!(
            "no index.html at the site root — deploy your build output directory{}",
            nested_index_hint(root, &entries)
        )));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// When there's no root `index.html` but exactly one first-level subdirectory
/// has one, point at it — the Python original did this by finding the
/// shallowest nested `index.html` anywhere in the tree; this mirrors the
/// common case (a single build-output subdir) without over-guessing when
/// several subdirs qualify.
fn nested_index_hint(root: &Path, entries: &[ManifestEntry]) -> String {
    let mut candidates: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.path.strip_suffix("/index.html"))
        .filter(|dir| !dir.contains('/'))
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [dir] => format!(
            " — did you mean to deploy {}/? (e.g. `quome deploy {}`)",
            dir,
            root.join(dir).display()
        ),
        _ => String::new(),
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<ManifestEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if JUNK_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        // DirEntry::metadata() does NOT follow symlinks (unlike fs::metadata),
        // so a symlinked directory correctly reads as a symlink here — walking
        // into it would risk a cycle. A symlinked FILE, however, should behave
        // like the Python original (which used Path.rglob + is_file, both of
        // which follow symlinks): read it with fs::metadata so it's included.
        let entry_meta = entry.metadata()?;
        if entry_meta.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, out)?;
            continue;
        }
        let meta = if entry_meta.is_symlink() {
            match std::fs::metadata(&path) {
                Ok(m) => m,
                // Broken symlink — nothing to upload.
                Err(_) => continue,
            }
        } else {
            entry_meta
        };
        if meta.is_file() {
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

    #[test]
    fn suggests_the_nested_index_when_exactly_one_subdir_has_one() {
        let dir = site(&["dist/index.html", "dist/assets/app.js"]);
        let err = build_manifest(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("did you mean to deploy dist/?"),
            "{err}"
        );
    }

    #[test]
    fn no_hint_when_multiple_subdirs_have_index_html() {
        let dir = site(&["dist/index.html", "build/index.html"]);
        let err = build_manifest(dir.path()).unwrap_err();
        assert!(!err.to_string().contains("did you mean"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinked_files() {
        let dir = site(&["index.html", "real.txt"]);
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("alias.txt"))
            .unwrap();
        let entries = build_manifest(dir.path()).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"real.txt"), "{paths:?}");
        assert!(paths.contains(&"alias.txt"), "symlink dropped: {paths:?}");
    }
}
