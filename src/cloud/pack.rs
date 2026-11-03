//! Packing an app folder into the uncompressed tar Quome Cloud builds from.
//! Ported from quome-mac's Go deploy packer (`buildTar`, `ignore.go`).

use crate::errors::QuomeError;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path};
use tempfile::NamedTempFile;

/// Hard cap on the tar size (512 MiB).
pub const MAX_TAR_BYTES: u64 = 512 << 20;

/// A packed source tar in a temp file that is removed on drop.
pub struct PackedSource {
    file: NamedTempFile,
    pub size: u64,
    pub count: usize,
    pub warnings: Vec<String>,
}

impl PackedSource {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

pub fn pack_folder(dir: &Path) -> Result<PackedSource, QuomeError> {
    pack_with_cap(dir, MAX_TAR_BYTES)
}

/// Packs `dir` into a tar with entries capped at `cap` bytes. Paths are relative,
/// forward-slash and sorted; only regular files and in-folder symlinks are emitted.
fn pack_with_cap(dir: &Path, cap: u64) -> Result<PackedSource, QuomeError> {
    let root = fs::canonicalize(dir)
        .map_err(|e| QuomeError::Usage(format!("cannot read {}: {e}", dir.display())))?;
    let set = match fs::read_to_string(root.join(".dockerignore")) {
        Ok(text) => IgnoreSet::parse(&text),
        Err(_) => IgnoreSet::default(),
    };
    let skip = |rel: &str| rel != "Dockerfile" && rel != ".dockerignore" && set.ignored(rel);

    let mut paths = Vec::new();
    walk(&root, "", &set, &skip, false, &mut paths)?;
    paths.sort();

    let tmp = tempfile::Builder::new()
        .prefix("quome-src-")
        .suffix(".tar")
        .tempfile()?;
    let mut builder = tar::Builder::new(CapWriter {
        w: tmp.reopen()?,
        n: 0,
        cap,
    });
    let mut warnings = Vec::new();
    let mut count = 0;
    let mut has_dockerfile = false;
    for rel in &paths {
        let full = root.join(rel);
        let meta = fs::symlink_metadata(&full)?;
        let mut header = tar::Header::new_gnu();
        header.set_metadata_in_mode(&meta, tar::HeaderMode::Deterministic);
        header.set_mtime(0);
        if meta.file_type().is_symlink() {
            let link = fs::read_link(&full)?;
            if !inside_root(&root, &full, &link) {
                warnings.push(format!(
                    "warning: skipping symlink {rel} -> {} (target outside the build context)",
                    link.display()
                ));
                continue;
            }
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder.append_link(&mut header, rel, &link)
        } else if meta.is_file() {
            header.set_size(meta.len());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let exec = meta.permissions().mode() & 0o111 != 0;
                header.set_mode(if exec { 0o755 } else { 0o644 });
            }
            // `take` keeps the tar valid if the file grows after the stat.
            builder.append_data(&mut header, rel, fs::File::open(&full)?.take(meta.len()))
        } else {
            continue;
        }
        .map_err(cap_or_io)?;
        has_dockerfile |= rel == "Dockerfile";
        count += 1;
    }
    if !has_dockerfile {
        return Err(QuomeError::Usage(
            "Quome Cloud builds from a Dockerfile: add a regular Dockerfile (not a link to \
             somewhere outside the folder) at the top of the folder"
                .into(),
        ));
    }
    builder.into_inner().map_err(cap_or_io)?.w.flush()?;
    let size = tmp.as_file().metadata()?.len();
    Ok(PackedSource {
        file: tmp,
        size,
        count,
        warnings,
    })
}

fn cap_or_io(e: io::Error) -> QuomeError {
    if e.to_string().starts_with("source exceeds") {
        QuomeError::Usage(e.to_string())
    } else {
        QuomeError::Io(e)
    }
}

struct CapWriter<W> {
    w: W,
    n: u64,
    cap: u64,
}

impl<W: Write> Write for CapWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.n + buf.len() as u64 > self.cap {
            return Err(io::Error::other(format!(
                "source exceeds {} MiB; add entries to .dockerignore",
                self.cap >> 20
            )));
        }
        self.n += buf.len() as u64;
        self.w.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Collects non-directory entries (symlinks included, never followed) under `dir`.
/// Ignored directories are pruned unless a negation could re-include something inside.
fn walk(
    dir: &Path,
    prefix: &str,
    set: &IgnoreSet,
    skip: &dyn Fn(&str) -> bool,
    in_ignored: bool,
    out: &mut Vec<String>,
) -> Result<(), QuomeError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            // A name that is not UTF-8 cannot be matched against the patterns. Inside an
            // ignored directory (only reached when a negation might re-include something)
            // it is left out; anywhere else it would be packed, so refuse.
            if in_ignored {
                continue;
            }
            return Err(QuomeError::Usage(format!(
                "file name is not valid UTF-8: {}",
                entry.path().display()
            )));
        };
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = entry.file_type()?.is_dir();
        if skip(&rel) && (!is_dir || !set.has_negation()) {
            continue;
        }
        if is_dir {
            let ignored = in_ignored || skip(&rel);
            walk(&entry.path(), &rel, set, skip, ignored, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Whether the symlink at `full` (under `root`) resolves to a path within `root`.
fn inside_root(root: &Path, full: &Path, target: &Path) -> bool {
    if target.has_root() || target.is_absolute() {
        return false;
    }
    // Lexical check relative to the root.
    let Ok(parent_rel) = full.parent().unwrap_or(root).strip_prefix(root) else {
        return false;
    };
    let mut stack: Vec<&std::ffi::OsStr> = parent_rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n),
            _ => None,
        })
        .collect();
    for c in target.components() {
        match c {
            Component::Normal(n) => stack.push(n),
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return false;
                }
            }
            Component::CurDir => {}
            _ => return false,
        }
    }
    // Resolved check when the target exists (a dangling link passes the lexical check).
    if let Ok(real) = fs::canonicalize(full) {
        match fs::canonicalize(root) {
            Ok(real_root) => return real.starts_with(real_root),
            Err(_) => return false,
        }
    }
    true
}

/// A `.dockerignore` matcher: patterns are relative to the context root, `*` and `?`
/// stay within one path segment, `**` crosses segments, a leading `!` re-includes, the
/// last matching rule wins, and a match on a directory covers its contents.
#[derive(Default)]
struct IgnoreSet {
    rules: Vec<Rule>,
}

struct Rule {
    tokens: Vec<Tok>,
    negate: bool,
}

enum Tok {
    Lit(char),
    /// `?`: one character other than `/`.
    One,
    /// `*`: any run of characters other than `/`.
    Star,
    /// `**`: any run of characters.
    Any,
    /// `**/`: zero or more whole directories.
    Dirs,
    Class {
        negated: bool,
        items: Vec<(char, char)>,
    },
}

impl IgnoreSet {
    fn parse(text: &str) -> IgnoreSet {
        let mut rules = Vec::new();
        for line in text.lines() {
            let mut line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut negate = false;
            if let Some(rest) = line.strip_prefix('!') {
                negate = true;
                line = rest.trim();
            }
            let line = line.strip_prefix('/').unwrap_or(line);
            let line = line.strip_suffix('/').unwrap_or(line);
            // Improves on the Go packer: patterns are path-cleaned the way Docker does
            // (`./x`, `a/../x`, `a//b`, `x/.`), where Go left them as written and so
            // never matched the file the user meant to exclude.
            let Some(line) = clean_pattern(line) else {
                continue;
            };
            if let Some(tokens) = compile(&line) {
                rules.push(Rule { tokens, negate });
            }
        }
        IgnoreSet { rules }
    }

    /// Whether any rule re-includes, which forbids pruning ignored directories.
    fn has_negation(&self) -> bool {
        self.rules.iter().any(|r| r.negate)
    }

    /// Whether the slash-separated relative path is excluded.
    fn ignored(&self, rel: &str) -> bool {
        let mut prefixes = Vec::new();
        for (i, c) in rel.char_indices() {
            if c == '/' {
                prefixes.push(&rel[..i]);
            }
        }
        prefixes.push(rel);
        let mut out = false;
        for r in &self.rules {
            if prefixes.iter().any(|p| matches(&r.tokens, p)) {
                out = !r.negate;
            }
        }
        out
    }
}

/// Lexically cleans a pattern; `None` when nothing is left or it escapes the root.
fn clean_pattern(p: &str) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            s => stack.push(s),
        }
    }
    if stack.is_empty() {
        None
    } else {
        Some(stack.join("/"))
    }
}

/// Glob to tokens; `None` when the pattern would be an invalid expression (the rule is dropped).
fn compile(p: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = p.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '*' => {
                if c.get(i + 1) == Some(&'*') {
                    i += 1;
                    if c.get(i + 1) == Some(&'/') {
                        i += 1;
                        toks.push(Tok::Dirs);
                    } else {
                        toks.push(Tok::Any);
                    }
                } else {
                    toks.push(Tok::Star);
                }
            }
            '?' => toks.push(Tok::One),
            '[' => match c[i + 1..].iter().position(|&x| x == ']') {
                Some(end) if end > 0 => {
                    let mut class = &c[i + 1..i + 1 + end];
                    i += end + 1;
                    let negated = matches!(class[0], '!' | '^');
                    if negated {
                        class = &class[1..];
                        if class.is_empty() {
                            return None;
                        }
                    }
                    let mut items = Vec::new();
                    let mut k = 0;
                    while k < class.len() {
                        if k + 2 < class.len() && class[k + 1] == '-' {
                            if class[k] > class[k + 2] {
                                return None;
                            }
                            items.push((class[k], class[k + 2]));
                            k += 3;
                        } else {
                            items.push((class[k], class[k]));
                            k += 1;
                        }
                    }
                    toks.push(Tok::Class { negated, items });
                }
                _ => toks.push(Tok::Lit('[')),
            },
            '\\' => {
                if i + 1 < c.len() {
                    i += 1;
                    toks.push(Tok::Lit(c[i]));
                }
            }
            ch => toks.push(Tok::Lit(ch)),
        }
        i += 1;
    }
    Some(toks)
}

fn matches(toks: &[Tok], s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    m(toks, &chars)
}

fn m(toks: &[Tok], s: &[char]) -> bool {
    let Some((t, rest)) = toks.split_first() else {
        return s.is_empty();
    };
    match t {
        Tok::Lit(c) => s.first() == Some(c) && m(rest, &s[1..]),
        Tok::One => s.first().is_some_and(|&c| c != '/') && m(rest, &s[1..]),
        Tok::Class { negated, items } => {
            s.first()
                .is_some_and(|&c| items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated)
                && m(rest, &s[1..])
        }
        Tok::Star => (0..=s.len()).any(|n| s[..n].iter().all(|&c| c != '/') && m(rest, &s[n..])),
        Tok::Any => (0..=s.len()).any(|n| m(rest, &s[n..])),
        Tok::Dirs => {
            m(rest, s)
                || s.iter()
                    .enumerate()
                    .any(|(i, &c)| c == '/' && m(rest, &s[i + 1..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Read;

    fn check_ignored(set: &IgnoreSet, cases: &[(&str, bool)]) {
        for (p, want) in cases {
            assert_eq!(set.ignored(p), *want, "ignored({p:?})");
        }
    }

    #[test]
    fn test_ignore_golden() {
        let set = IgnoreSet::parse(
            "\n# comment\nnode_modules\n*.log\n/build\n**/secret.txt\ndocs/**/*.md\n!docs/keep.md\ntmp/\na?c\n",
        );
        check_ignored(
            &set,
            &[
                ("node_modules", true),
                ("node_modules/x/y.js", true),
                ("src/node_modules", false),
                ("app.log", true),
                ("sub/app.log", false),
                ("build/out", true),
                ("src/build", false),
                ("secret.txt", true),
                ("a/b/secret.txt", true),
                ("docs/x/y/z.md", true),
                ("docs/keep.md", false),
                ("docs/guide.md", true),
                ("tmp", true),
                ("tmp/f", true),
                ("abc", true),
                ("ac", false),
                ("main.go", false),
                ("docs/readme.txt", false),
            ],
        );
    }

    #[test]
    fn test_ignore_char_classes_and_reinclude() {
        let set = IgnoreSet::parse("node_modules\n!node_modules/keep\nfile[0-9].txt\nx[!a]y\n");
        check_ignored(
            &set,
            &[
                ("node_modules/a.js", true),
                ("node_modules/keep", false),
                ("node_modules/keep/a.js", false),
                ("node_modules/other/a.js", true),
                ("file3.txt", true),
                ("fileX.txt", false),
                ("xby", true),
                ("xay", false),
            ],
        );
    }

    fn write_tree(root: &Path, files: &[(&str, &str)]) {
        for (p, c) in files {
            let full = root.join(p);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, c).unwrap();
        }
    }

    /// Entry names in the tar's own order plus symlink targets, read back from the tar.
    fn entries(src: &PackedSource) -> (Vec<String>, BTreeMap<String, String>) {
        let mut bytes = Vec::new();
        fs::File::open(src.path())
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes.len() as u64, src.size);
        let mut names = Vec::new();
        let mut links = BTreeMap::new();
        let mut ar = tar::Archive::new(&bytes[..]);
        for e in ar.entries().unwrap() {
            let e = e.unwrap();
            let name = e.path().unwrap().to_string_lossy().into_owned();
            if e.header().entry_type().is_symlink() {
                links.insert(
                    name.clone(),
                    e.link_name()
                        .unwrap()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            names.push(name);
        }
        (names, links)
    }

    #[test]
    fn test_build_tar_honors_dockerignore() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[
                ("Dockerfile", "FROM x"),
                ("main.go", "x"),
                ("node_modules/a/b.js", "x"),
                ("src/a.go", "x"),
                ("src/debug.log", "x"),
                (".env", "x"),
                (".dockerignore", "node_modules\n**/*.log\n.env\n"),
            ],
        );
        let src = pack_folder(dir.path()).unwrap();
        let (names, _) = entries(&src);
        assert_eq!(names.join(","), ".dockerignore,Dockerfile,main.go,src/a.go");
        assert_eq!(src.count, 4);
    }

    #[test]
    fn test_build_tar_keeps_dockerfile_and_reincludes() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[
                ("Dockerfile", "x"),
                ("a.txt", "x"),
                ("node_modules/keep/k.js", "x"),
                ("node_modules/o.js", "x"),
                (
                    ".dockerignore",
                    "Dockerfile\n.dockerignore\nnode_modules\n!node_modules/keep\n",
                ),
            ],
        );
        let src = pack_folder(dir.path()).unwrap();
        let (names, _) = entries(&src);
        assert_eq!(
            names.join(","),
            ".dockerignore,Dockerfile,a.txt,node_modules/keep/k.js"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_build_tar_skips_escaping_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[("Dockerfile", "x"), ("a.txt", "x"), ("sub/b.txt", "x")],
        );
        for (name, target) in [
            ("ok", "a.txt"),
            ("ok2", "sub/b.txt"),
            ("abs", "/etc/passwd"),
            ("up", "../outside"),
            ("sub/up2", "../../x"),
        ] {
            std::os::unix::fs::symlink(target, dir.path().join(name)).unwrap();
        }
        let src = pack_folder(dir.path()).unwrap();
        let (_, links) = entries(&src);
        assert_eq!(links.len(), 2, "links = {links:?}");
        assert_eq!(links["ok"], "a.txt");
        assert_eq!(links["ok2"], "sub/b.txt");
        let warn = src.warnings.join("\n");
        for n in ["abs", "up", "sub/up2"] {
            assert!(
                warn.contains(&format!("{n} ->")),
                "no warning for {n}: {warn}"
            );
        }
    }

    #[test]
    fn missing_dockerfile_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), &[("main.py", "x")]);
        let err = pack_folder(dir.path()).err().expect("must fail");
        assert!(err
            .to_string()
            .contains("Quome Cloud builds from a Dockerfile"));
    }

    #[test]
    fn oversized_source_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[("Dockerfile", "x"), ("big", &"y".repeat(4096))],
        );
        let err = pack_with_cap(dir.path(), 2048).err().expect("must fail");
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn temp_file_is_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), &[("Dockerfile", "x")]);
        let src = pack_folder(dir.path()).unwrap();
        let p = src.path().to_path_buf();
        assert!(p.exists());
        drop(src);
        assert!(!p.exists());
    }

    #[test]
    fn entries_are_sorted_in_the_tar_itself() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[
                ("zeta", "x"),
                ("Dockerfile", "x"),
                ("b", "x"),
                ("sub/z", "x"),
                ("a", "x"),
                ("C", "x"),
                ("sub/a", "x"),
            ],
        );
        let (names, _) = entries(&pack_folder(dir.path()).unwrap());
        assert_eq!(
            names,
            ["C", "Dockerfile", "a", "b", "sub/a", "sub/z", "zeta"]
        );
    }

    // APFS refuses non-UTF-8 names, so this can only run where they can be created.
    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_name_in_ignored_dir_is_skipped_elsewhere_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[
                ("Dockerfile", "x"),
                (".dockerignore", "node_modules\n!node_modules/ok.js\n"),
                ("node_modules/ok.js", "x"),
            ],
        );
        let bad = OsStr::from_bytes(b"bad\xff.js");
        fs::write(dir.path().join("node_modules").join(bad), "x").unwrap();
        let (names, _) = entries(&pack_folder(dir.path()).unwrap());
        assert_eq!(names, [".dockerignore", "Dockerfile", "node_modules/ok.js"]);

        fs::write(dir.path().join(bad), "x").unwrap();
        let err = pack_folder(dir.path()).err().expect("must fail");
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }

    #[test]
    fn ignore_patterns_are_path_cleaned() {
        // Improves on the Go packer, whose patterns are not cleaned and so never match.
        let set = IgnoreSet::parse("./a.env\nb/../c.env\nd//e.pem\n/./f\ng/.\nh/./i/\n");
        check_ignored(
            &set,
            &[
                ("a.env", true),
                ("c.env", true),
                ("d/e.pem", true),
                ("f", true),
                ("g", true),
                ("g/x", true),
                ("h/i/x", true),
                ("b/c.env", false),
                ("other", false),
            ],
        );
        let set = IgnoreSet::parse("../outside\n");
        check_ignored(&set, &[("outside", false)]);
    }

    #[cfg(unix)]
    #[test]
    fn dockerfile_that_is_an_escaping_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("Dockerfile")).unwrap();
        let err = pack_folder(dir.path()).err().expect("must fail");
        assert!(
            err.to_string()
                .contains("Quome Cloud builds from a Dockerfile"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn headers_are_normalised() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_tree(
            dir.path(),
            &[("Dockerfile", "x"), ("run.sh", "x"), ("secret", "x")],
        );
        let mode = |n: &str, m: u32| {
            fs::set_permissions(dir.path().join(n), fs::Permissions::from_mode(m)).unwrap()
        };
        mode("run.sh", 0o4755);
        mode("secret", 0o600);
        std::os::unix::fs::symlink("Dockerfile", dir.path().join("link")).unwrap();
        let src = pack_folder(dir.path()).unwrap();
        let mut ar = tar::Archive::new(fs::File::open(src.path()).unwrap());
        let mut seen = 0;
        for e in ar.entries().unwrap() {
            let e = e.unwrap();
            let h = e.header();
            let name = e.path().unwrap().to_string_lossy().into_owned();
            assert_eq!((h.uid().unwrap(), h.gid().unwrap()), (0, 0), "{name}");
            assert_eq!(h.mtime().unwrap(), 0, "{name}");
            assert!(h.username_bytes().is_none_or(|u| u.is_empty()), "{name}");
            assert!(h.groupname_bytes().is_none_or(|u| u.is_empty()), "{name}");
            let want = match name.as_str() {
                "run.sh" => Some(0o755),
                "Dockerfile" | "secret" => Some(0o644),
                _ => None,
            };
            if let Some(w) = want {
                assert_eq!(h.mode().unwrap(), w, "{name}");
            }
            assert_eq!(h.mode().unwrap() & 0o7000, 0, "{name}");
            seen += 1;
        }
        assert_eq!(seen, 4);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_chain_resolving_outside_is_skipped() {
        let outside = tempfile::tempdir().unwrap();
        write_tree(outside.path(), &[("secret", "x")]);
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), &[("Dockerfile", "x"), ("sub/f", "x")]);
        // Lexically inside, but sub/inner points outside, so hop and dirlink escape.
        std::os::unix::fs::symlink(outside.path(), dir.path().join("sub/inner")).unwrap();
        std::os::unix::fs::symlink("sub/inner", dir.path().join("hop")).unwrap();
        std::os::unix::fs::symlink("sub/inner/secret", dir.path().join("file")).unwrap();
        let src = pack_folder(dir.path()).unwrap();
        let (names, links) = entries(&src);
        assert!(links.is_empty(), "{links:?}");
        assert_eq!(names, ["Dockerfile", "sub/f"]);
        assert_eq!(src.warnings.len(), 3, "{:?}", src.warnings);
    }

    #[cfg(unix)]
    #[test]
    fn folder_reached_through_a_symlink_is_packed() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), &[("app/Dockerfile", "x"), ("app/a", "x")]);
        std::os::unix::fs::symlink(dir.path().join("app"), dir.path().join("alias")).unwrap();
        std::os::unix::fs::symlink("a", dir.path().join("app/rel")).unwrap();
        let (names, links) = entries(&pack_folder(&dir.path().join("alias")).unwrap());
        assert_eq!(names, ["Dockerfile", "a", "rel"]);
        assert_eq!(links["rel"], "a");
    }

    #[test]
    fn cap_writer_refuses_before_writing_past_the_cap() {
        let mut w = CapWriter {
            w: Vec::new(),
            n: 0,
            cap: 250,
        };
        w.write_all(&[0; 100]).unwrap();
        w.write_all(&[0; 100]).unwrap();
        assert!(w.write_all(&[0; 100]).is_err());
        assert_eq!(w.w.len(), 200);
    }

    #[test]
    fn many_small_files_over_the_cap_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), &[("Dockerfile", "x")]);
        for i in 0..40 {
            fs::write(dir.path().join(format!("f{i:02}")), "z").unwrap();
        }
        // Every file fits on its own; only the running total passes the cap.
        let err = pack_with_cap(dir.path(), 8192).err().expect("must fail");
        assert!(err.to_string().contains("source exceeds"), "{err}");
        assert!(pack_with_cap(dir.path(), 1 << 20).is_ok());
    }
}
