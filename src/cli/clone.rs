use anyhow::{Context, Result};
use std::process::Command;

use crate::core::config::LitConfig;

/// Clone a lit repository from a remote URL.
///
/// Shells out to `git clone` for transport, then validates the result
/// is a valid lit project (has lit.toml).
pub async fn run(url: String) -> Result<()> {
    eprintln!("Cloning {}...", url);

    let output = Command::new("git")
        .args(["clone", &url])
        .output()
        .context("Failed to run `git clone`. Is git installed?")?;

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
        anyhow::bail!("git clone failed (exit code: {:?})", output.status.code());
    }

    // Try to figure out the clone directory name from the URL
    let repo_name = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git");

    let clone_dir = std::env::current_dir()?.join(repo_name);

    // Validate it's a lit project
    match LitConfig::find_and_load(&clone_dir) {
        Ok((config, _root)) => {
            eprintln!();
            eprintln!(
                "Cloned lit project: {} v{}",
                config.project.name, config.project.version
            );
            eprintln!("  cd {} && lit status", repo_name);
        }
        Err(_) => {
            eprintln!();
            eprintln!(
                "Warning: Cloned repository does not appear to be a lit project (no lit.toml found)."
            );
            eprintln!("  You can initialize it with: cd {} && lit init", repo_name);
        }
    }

    Ok(())
}
