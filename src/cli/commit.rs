use anyhow::Result;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::repo::LitRepo;
use crate::core::style;

pub async fn run(message: String) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    // Open git repo
    let repo = LitRepo::open(&root)?;

    // Stage all lit-related files
    repo.stage_all()?;

    // Check if there are changes to commit
    let status = repo.status()?;
    if !status.has_changes() {
        eprintln!("{}", "Nothing to commit (working tree clean).".dimmed());
        eprintln!("{}", style::hint("Hint: Run `lit regenerate` to generate code, then commit."));
        return Ok(());
    }

    // Create commit
    let hash = repo.commit(&message)?;

    // Summary
    eprintln!(
        "{} Created commit {}",
        "✓".green().bold(),
        style::commit_hash(&hash[..7.min(hash.len())])
    );
    eprintln!();
    if !status.prompts_new.is_empty() || !status.prompts_modified.is_empty() {
        let prompt_count = status.prompts_new.len() + status.prompts_modified.len();
        eprintln!("  Prompts:   {} changed", prompt_count.to_string().green());
    }
    if !status.prompts_deleted.is_empty() {
        eprintln!("  Prompts:   {} deleted", status.prompts_deleted.len().to_string().red());
    }
    if !status.code_new.is_empty() || !status.code_modified.is_empty() {
        let code_count = status.code_new.len() + status.code_modified.len();
        eprintln!("  Code:      {} file(s)", code_count.to_string().green());
    }
    if !status.config_modified.is_empty() {
        eprintln!("  Config:    {} file(s)", status.config_modified.len().to_string().yellow());
    }
    eprintln!("  Total:     {} file(s)", status.total_changes().to_string().bold());

    Ok(())
}
