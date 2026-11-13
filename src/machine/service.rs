//! Where the machine's background service is installed.
use std::path::{Path, PathBuf};

pub const LAUNCHD_LABEL: &str = "cloud.quome.models";
pub const SYSTEMD_UNIT: &str = "quome-models.service";

/// `~/Library/LaunchAgents/cloud.quome.models.plist`.
pub fn launchd_plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

/// `~/.config/systemd/user/quome-models.service`.
pub fn systemd_unit_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user").join(SYSTEMD_UNIT)
}

/// True when this platform's service definition exists.
pub fn installed(home: &Path) -> bool {
    if cfg!(target_os = "macos") {
        launchd_plist_path(home).exists()
    } else {
        systemd_unit_path(home).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_follows_the_platform_file() {
        let h = tempfile::tempdir().unwrap();
        assert!(!installed(h.path()));
        let p = if cfg!(target_os = "macos") {
            launchd_plist_path(h.path())
        } else {
            systemd_unit_path(h.path())
        };
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "x").unwrap();
        assert!(installed(h.path()));
    }
}
