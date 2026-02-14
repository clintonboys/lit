use anyhow::Result;
use chrono::TimeZone;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::repo::LitRepo;

pub async fn run(limit: usize) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let repo = LitRepo::open(&root)?;
    let commits = repo.log(limit)?;

    if commits.is_empty() {
        eprintln!("{}", "No commits yet.".dimmed());
        return Ok(());
    }

    for commit in &commits {
        let datetime = chrono::Utc
            .timestamp_opt(commit.timestamp, 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        eprintln!(
            "{} {} — {}",
            commit.short_hash.yellow(),
            datetime.dimmed(),
            commit.message
        );
    }

    if commits.len() == limit {
        eprintln!();
        eprintln!(
            "{}",
            format!("(showing {} of possibly more — use -n to increase)", limit).dimmed()
        );
    }

    Ok(())
}
