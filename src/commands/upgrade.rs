use colored::Colorize;
use std::process::Command;

use crate::errors::{QuomeError, Result};
use crate::ui;

pub async fn execute() -> Result<()> {
    // Check if brew is available
    let brew_check = Command::new("brew").arg("--version").output();

    if brew_check.is_err() {
        return Err(QuomeError::ApiError(
            "Homebrew not found. Please install quome manually or install Homebrew first.".into(),
        ));
    }

    // Get current version
    let current_version = env!("CARGO_PKG_VERSION");
    println!("  {} {}", "Current version:".dimmed(), current_version);

    let sp = ui::spinner("Checking for updates...");

    // Update brew to get latest formula info
    let update = Command::new("brew").arg("update").output()?;
    if !update.status.success() {
        sp.finish_and_clear();
        let stderr = String::from_utf8_lossy(&update.stderr);
        return Err(QuomeError::ApiError(format!("brew update failed: {}", stderr)));
    }

    // Check what version is available
    let info = Command::new("brew")
        .args(["info", "quome-cloud/quome/quome", "--json=v2"])
        .output()?;
    sp.finish_and_clear();

    let latest_version = if info.status.success() {
        let json: serde_json::Value = serde_json::from_slice(&info.stdout)?;
        json["formulae"]
            .get(0)
            .and_then(|f| f["versions"]["stable"].as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    let latest = latest_version.as_deref().unwrap_or("unknown");
    println!("  {} {}", "Latest version:".dimmed(), latest);

    // Check if upgrade is needed
    if latest == current_version {
        println!();
        println!("{} quome is already up to date", "✓".green());
        return Ok(());
    }

    // Ask for confirmation
    println!();
    let confirm = inquire::Confirm::new(&format!(
        "Upgrade from {} to {}?",
        current_version, latest
    ))
    .with_default(true)
    .prompt()
    .map_err(|e| QuomeError::Io(std::io::Error::other(e.to_string())))?;

    if !confirm {
        println!("Upgrade cancelled.");
        return Ok(());
    }

    let sp = ui::spinner("Upgrading quome...");
    let upgrade = Command::new("brew")
        .args(["upgrade", "quome"])
        .output()?;
    sp.finish_and_clear();

    if !upgrade.status.success() {
        let stderr = String::from_utf8_lossy(&upgrade.stderr);
        let stdout = String::from_utf8_lossy(&upgrade.stdout);

        // Check if already up to date (can happen due to race)
        if stdout.contains("already installed") || stderr.contains("already installed") {
            println!("{} quome is already up to date", "✓".green());
        } else {
            return Err(QuomeError::ApiError(format!("brew upgrade failed: {}", stderr)));
        }
    } else {
        println!("{} Upgraded to {}", "✓".green(), latest);
    }

    Ok(())
}
