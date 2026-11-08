//! Which local folders the MCP tools may read (`sync_folder`, `deploy_app`).
//!
//! A folder must lie inside one of the allowed roots: the roots the MCP
//! client declared, or, when it declared none, the directory the server was
//! started in. The home folder itself, anything containing it, and the usual
//! credential folders are refused even inside a root. Paths are canonicalised
//! (symlinks and `..` resolved) before any check.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::{service::Peer, RoleServer};

/// Credential locations under the home folder, refused wherever they appear.
const SENSITIVE: &[&str] = &[
    ".ssh",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker",
    ".gnupg",
    ".netrc",
    ".git-credentials",
    ".quome",
    "Library/Keychains",
];

/// How long to wait for the client's answer to `roots/list`.
const ROOTS_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct LocalScope {
    /// The directory the server was started in: the root when the client gives none.
    start_dir: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl LocalScope {
    pub fn new(start_dir: Option<PathBuf>, home: Option<PathBuf>) -> LocalScope {
        LocalScope { start_dir, home }
    }

    /// The scope of a server started now: the current directory and the user's home.
    pub fn at_start() -> LocalScope {
        LocalScope::new(std::env::current_dir().ok(), dirs::home_dir())
    }

    /// The canonical path of `folder` when the tools may read it, else a refusal
    /// the agent can act on. `client_roots` are the client's declared roots
    /// (`None` when it declared none); otherwise the start directory is the root.
    pub fn check(&self, folder: &str, client_roots: Option<&[PathBuf]>) -> Result<PathBuf, String> {
        let raw = Path::new(folder);
        let raw = match (&self.start_dir, raw.is_relative()) {
            (Some(start), true) => start.join(raw),
            _ => raw.to_path_buf(),
        };
        let canon = std::fs::canonicalize(&raw)
            .map_err(|e| format!("Cannot read folder {folder:?}: {e}."))?;

        if let Some(home) = self.home.as_deref().and_then(canonical) {
            if home.starts_with(&canon) {
                return Err(format!(
                    "Refusing to read {folder:?}: it is the home folder or contains it. \
                     Give the project folder itself (a folder inside the current project)."
                ));
            }
            for rel in SENSITIVE {
                let lexical = home.join(rel);
                let resolved = canonical(&lexical);
                for cred in std::iter::once(&lexical).chain(resolved.as_ref()) {
                    if canon.starts_with(cred) || cred.starts_with(&canon) {
                        return Err(format!(
                            "Refusing to read {folder:?}: it is or contains the credential \
                             location ~/{rel}, which is never uploaded. Give a folder that \
                             holds only the project's files."
                        ));
                    }
                }
            }
        }

        let roots: Vec<PathBuf> = match client_roots {
            Some(r) => r.iter().filter_map(|p| canonical(p)).collect(),
            None => self.start_dir.iter().filter_map(|p| canonical(p)).collect(),
        };
        if roots.iter().any(|r| canon.starts_with(r)) {
            return Ok(canon);
        }
        let shown = if roots.is_empty() {
            "none".to_string()
        } else {
            roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let source = if client_roots.is_some() {
            "the workspace roots the client declared"
        } else {
            "the directory the MCP server was started in"
        };
        Err(format!(
            "Refusing to read {folder:?}: local folders must be inside {source} ({shown}). \
             Use a folder inside it, or ask the user to open that folder as the project \
             (or add it as a workspace root) and try again."
        ))
    }
}

fn canonical(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// The client's declared filesystem roots, or `None` when it declared none
/// (no `roots` capability, no `file://` roots, or no answer in time).
#[allow(deprecated)]
pub async fn client_roots(peer: &Peer<RoleServer>) -> Option<Vec<PathBuf>> {
    let info = peer.peer_info()?;
    info.capabilities.roots.as_ref()?;
    let listed = tokio::time::timeout(ROOTS_WAIT, peer.list_roots())
        .await
        .ok()?
        .ok()?;
    let paths: Vec<PathBuf> = listed
        .roots
        .iter()
        .filter_map(|r| file_uri_path(&r.uri))
        .collect();
    (!paths.is_empty()).then_some(paths)
}

fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let url = reqwest::Url::parse(uri).ok()?;
    (url.scheme() == "file").then(|| url.to_file_path().ok())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake home with a project and credential folders, and a folder outside it.
    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        project: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let project = home.join("code/app");
        let outside = base.join("outside");
        for d in [
            &project.join("src"),
            &outside,
            &home.join(".ssh"),
            &home.join(".aws"),
            &home.join(".config/gcloud"),
            &home.join(".quome"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(home.join(".ssh/id_ed25519"), "k").unwrap();
        Fixture {
            _tmp: tmp,
            home,
            project,
            outside,
        }
    }

    fn s(p: &Path) -> &str {
        p.to_str().unwrap()
    }

    #[test]
    fn a_folder_inside_the_start_dir_is_allowed() {
        let f = fixture();
        let scope = LocalScope::new(Some(f.project.clone()), Some(f.home.clone()));
        assert_eq!(scope.check(s(&f.project), None).unwrap(), f.project);
        assert_eq!(
            scope.check(s(&f.project.join("src")), None).unwrap(),
            f.project.join("src")
        );
        // Relative paths resolve against the start directory.
        assert_eq!(scope.check("src", None).unwrap(), f.project.join("src"));
    }

    #[test]
    fn a_folder_outside_the_start_dir_is_refused() {
        let f = fixture();
        let scope = LocalScope::new(Some(f.project.clone()), Some(f.home.clone()));
        let e = scope.check(s(&f.outside), None).unwrap_err();
        assert!(e.contains("Refusing") && e.contains("started in"), "{e}");
    }

    #[test]
    fn dot_dot_escape_is_refused() {
        let f = fixture();
        let scope = LocalScope::new(Some(f.project.clone()), Some(f.home.clone()));
        let sneaky = format!("{}/src/../../../../outside", s(&f.project));
        let e = scope.check(&sneaky, None).unwrap_err();
        assert!(e.contains("must be inside"), "{e}");
        let e = scope.check("../../../outside", None).unwrap_err();
        assert!(e.contains("must be inside"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_refused() {
        let f = fixture();
        let link = f.project.join("elsewhere");
        std::os::unix::fs::symlink(&f.outside, &link).unwrap();
        let scope = LocalScope::new(Some(f.project.clone()), Some(f.home.clone()));
        let e = scope.check(s(&link), None).unwrap_err();
        assert!(e.contains("must be inside"), "{e}");
    }

    #[test]
    fn home_and_its_parents_are_refused_even_inside_a_root() {
        let f = fixture();
        let root = f.home.parent().unwrap().to_path_buf();
        let scope = LocalScope::new(Some(root.clone()), Some(f.home.clone()));
        for p in [&f.home, &root] {
            let e = scope.check(s(p), None).unwrap_err();
            assert!(e.contains("home folder"), "{e}");
        }
    }

    #[test]
    fn credential_folders_are_refused_even_inside_a_root() {
        let f = fixture();
        let scope = LocalScope::new(Some(f.home.clone()), Some(f.home.clone()));
        for rel in [".ssh", ".aws", ".config/gcloud", ".quome", ".config"] {
            let e = scope.check(s(&f.home.join(rel)), None).unwrap_err();
            assert!(e.contains("credential location"), "{rel}: {e}");
        }
        // The project under the same root is still fine.
        assert!(scope.check(s(&f.project), None).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_a_credential_folder_is_refused() {
        let f = fixture();
        let link = f.project.join("keys");
        std::os::unix::fs::symlink(f.home.join(".ssh"), &link).unwrap();
        let scope = LocalScope::new(Some(f.home.clone()), Some(f.home.clone()));
        let e = scope.check(s(&link), None).unwrap_err();
        assert!(e.contains("~/.ssh"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_credential_folder_that_is_itself_a_symlink_is_refused_at_its_target() {
        let f = fixture();
        let dotfiles = f.home.join("dotfiles/kube");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::os::unix::fs::symlink(&dotfiles, f.home.join(".kube")).unwrap();
        let scope = LocalScope::new(Some(f.home.clone()), Some(f.home.clone()));
        let e = scope.check(s(&dotfiles), None).unwrap_err();
        assert!(e.contains("~/.kube"), "{e}");
    }

    #[test]
    fn client_roots_replace_the_start_dir() {
        let f = fixture();
        let scope = LocalScope::new(Some(f.project.clone()), Some(f.home.clone()));
        let roots = vec![f.outside.clone()];
        assert_eq!(scope.check(s(&f.outside), Some(&roots)).unwrap(), f.outside);
        let e = scope.check(s(&f.project), Some(&roots)).unwrap_err();
        assert!(e.contains("workspace roots the client declared"), "{e}");
    }

    #[test]
    fn a_missing_folder_says_so() {
        let f = fixture();
        let scope = LocalScope::new(Some(f.project.clone()), Some(f.home.clone()));
        let e = scope.check(s(&f.project.join("nope")), None).unwrap_err();
        assert!(e.contains("Cannot read folder"), "{e}");
    }

    #[test]
    fn file_uris_become_paths() {
        assert_eq!(
            file_uri_path("file:///tmp/my%20app"),
            Some(PathBuf::from("/tmp/my app"))
        );
        assert_eq!(file_uri_path("https://example.com/x"), None);
        assert_eq!(file_uri_path("not a uri"), None);
    }
}
