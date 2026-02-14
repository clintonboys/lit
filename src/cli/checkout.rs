use anyhow::Result;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::repo::LitRepo;
use crate::core::style;

pub async fn run(ref_: String) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let repo = LitRepo::open(&root)?;

    // Check for uncommitted changes first
    let status = repo.status()?;
    if status.has_changes() {
        anyhow::bail!(
            "You have uncommitted changes ({} file(s)).\n\
             Hint: Commit them first with `lit commit -m \"message\"`, or see `lit status`.",
            status.total_changes()
        );
    }

    repo.checkout_ref(&ref_)?;

    // Show where we landed
    if let Some(head) = repo.head_commit() {
        eprintln!(
            "{} Checked out: {} {}",
            "✓".green().bold(),
            style::commit_hash(&head.short_hash),
            head.message
        );
    } else {
        eprintln!(
            "{} Checked out: {}",
            "✓".green().bold(),
            ref_
        );
    }

    Ok(())
}
