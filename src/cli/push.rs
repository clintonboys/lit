use anyhow::{Context, Result};
use std::process::Command;

use crate::core::config::LitConfig;

/// Thin wrapper around `git push`.
///
/// We shell out to git for remote operations because git2's transport layer
/// requires complex SSH/credential setup. The system git already handles
/// credentials, SSH keys, and proxies correctly.
pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    eprintln!("Pushing to remote...");

    let output = Command::new("git")
        .arg("push")
        .current_dir(&root)
        .output()
        .context("Failed to run `git push`. Is git installed?")?;

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
        anyhow::bail!("git push failed (exit code: {:?})", output.status.code());
    }

    eprintln!("Push complete.");
    Ok(())
}
