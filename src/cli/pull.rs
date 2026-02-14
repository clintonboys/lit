use anyhow::{Context, Result};
use std::process::Command;

use crate::core::config::LitConfig;

/// Thin wrapper around `git pull`.
///
/// We shell out to git for remote operations because git2's transport layer
/// requires complex SSH/credential setup. The system git already handles
/// credentials, SSH keys, and proxies correctly.
pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    eprintln!("Pulling from remote...");

    let output = Command::new("git")
        .arg("pull")
        .current_dir(&root)
        .output()
        .context("Failed to run `git pull`. Is git installed?")?;

    // Forward git's output
    if !output.stdout.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprint!("{}", stdout);
    }
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprint!("{}", stderr);
    }

    if !output.status.success() {
        anyhow::bail!("git pull failed (exit code: {:?})", output.status.code());
    }

    eprintln!("Pull complete.");
    Ok(())
}
