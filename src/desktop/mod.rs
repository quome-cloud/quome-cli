//! Linking this Mac to an organization as a desktop its agents can drive.
pub mod config;
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

#[cfg(test)]
mod tests {
    use super::*;

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
