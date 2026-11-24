//! Linking this Mac to an organization as a desktop its agents can drive.
pub mod app;
pub mod config;
pub mod doctor;
pub mod files;
pub mod serve;
pub mod service;
pub mod size;

/// Said, and exit 69, on any computer other than an Apple silicon Mac.
pub const UNSUPPORTED: &str =
    "Linking a computer as a desktop works on Apple silicon Macs only, for now.";
/// `EX_UNAVAILABLE`.
pub const UNSUPPORTED_EXIT: i32 = 69;

/// `Some((message, exit code))` unless `os` and `arch` are an Apple silicon Mac's.
pub fn unsupported(os: &str, arch: &str) -> Option<(&'static str, i32)> {
    (os != "macos" || arch != "aarch64").then_some((UNSUPPORTED, UNSUPPORTED_EXIT))
}

/// The oldest macOS that can be linked. Provisional until checked on hardware.
pub const MIN_MACOS: &str = "14.0";
/// Said, and exit 69, on a macOS older than `MIN_MACOS`.
pub const OLD_MACOS: &str = "Linking a computer as a desktop needs macOS 14 or later.";

/// True when `v` ("13.6", "14.10", "15") is clearly older than `MIN_MACOS`, compared
/// number by number. A version that does not parse is allowed: the app checks its own.
pub fn unsupported_version(v: &str) -> bool {
    fn parts(s: &str) -> Option<Vec<u64>> {
        s.trim().split('.').map(|p| p.parse().ok()).collect()
    }
    match (parts(v), parts(MIN_MACOS)) {
        (Some(have), Some(min)) => {
            for i in 0..have.len().max(min.len()) {
                let (a, b) = (
                    have.get(i).copied().unwrap_or(0),
                    min.get(i).copied().unwrap_or(0),
                );
                if a != b {
                    return a < b;
                }
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_before_14_is_refused() {
        assert!(unsupported_version("13.6"));
        assert!(unsupported_version("12"));
        for v in ["14.0", "14.10", "15", "26.1", "14", "garbage", ""] {
            assert!(!unsupported_version(v), "{v}");
        }
    }

    #[test]
    fn only_an_apple_silicon_mac_is_supported() {
        assert_eq!(unsupported("macos", "aarch64"), None);
        for (os, arch) in [
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("windows", "x86_64"),
        ] {
            assert_eq!(
                unsupported(os, arch),
                Some((UNSUPPORTED, 69)),
                "{os}/{arch}"
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn not_macos_exits_69() {
        assert_eq!(
            unsupported(std::env::consts::OS, std::env::consts::ARCH),
            Some((
                "Linking a computer as a desktop works on Apple silicon Macs only, for now.",
                69
            ))
        );
    }
}
