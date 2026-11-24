//! `quome desktop connect` through the real binary, for what only the binary shows. The
//! download, archive, checksum, signature and probe cases are unit tests in
//! `src/desktop/app.rs`, and connect's use of them in `src/commands/desktop.rs`.
use std::process::{Command, Output};

#[allow(dead_code)] // used on the platforms that run the test below
fn quome(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quome"))
        .args(args)
        .env("HOME", home)
        .env("QUOME_NO_AUTO_UPDATE", "1")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("QUOME_DESKTOP_DEV")
        .output()
        .unwrap()
}

/// Anywhere but an Apple silicon Mac, connect stops before it downloads or installs.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn elsewhere_connect_installs_no_app() {
    let h = tempfile::tempdir().unwrap();
    let o = quome(
        h.path(),
        &["desktop", "connect", "http://127.0.0.1:9", "--org", "acme"],
    );
    assert_eq!(o.status.code(), Some(69));
    assert!(String::from_utf8_lossy(&o.stderr)
        .contains("Linking a computer as a desktop works on Apple silicon Macs only, for now."));
    assert!(!h.path().join("Applications").exists());
}
