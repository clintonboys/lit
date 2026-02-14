use anyhow::Result;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::prompt::discover_prompts;
use crate::core::repo::LitRepo;
use crate::core::style;

pub async fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, root) = LitConfig::find_and_load(&cwd)?;

    eprintln!("{}", style::project_header(&config.project.name, &config.project.version));

    // Try to open git repo for status
    let repo = match LitRepo::open(&root) {
        Ok(r) => r,
        Err(_) => {
            eprintln!("  {}", "(no git repository — run `lit init` first)".dimmed());
            return show_prompts_only(&root);
        }
    };

    let status = repo.status()?;

    // Show HEAD
    if let Some(ref hash) = status.head_commit {
        if let Some(head) = repo.head_commit() {
            eprintln!(
                "  HEAD: {} {}",
                style::commit_hash(hash),
                head.message
            );
        }
    } else {
        eprintln!("  HEAD: {}", "(no commits)".dimmed());
    }

    // Show prompt count
    let prompts_dir = root.join("prompts");
    if prompts_dir.exists() {
        if let Ok(paths) = discover_prompts(&prompts_dir) {
            eprintln!("  Prompts: {}", paths.len().to_string().bold());
        }
    }

    let _ = &config; // used for display above

    eprintln!();

    if !status.has_changes() {
        eprintln!("{}", "Nothing to commit (working tree clean).".dimmed());
        return Ok(());
    }

    // Prompts
    if !status.prompts_new.is_empty() {
        eprintln!("{}", style::section("New prompts:"));
        for p in &status.prompts_new {
            eprintln!("{}", style::file_new(&p.display().to_string()));
        }
    }
    if !status.prompts_modified.is_empty() {
        eprintln!("{}", style::section("Modified prompts:"));
        for p in &status.prompts_modified {
            eprintln!("{}", style::file_modified(&p.display().to_string()));
        }
    }
    if !status.prompts_deleted.is_empty() {
        eprintln!("{}", style::section("Deleted prompts:"));
        for p in &status.prompts_deleted {
            eprintln!("{}", style::file_deleted(&p.display().to_string()));
        }
    }

    // Code
    if !status.code_new.is_empty() {
        eprintln!("{}", style::section("New code files:"));
        for p in &status.code_new {
            eprintln!("{}", style::file_new(&p.display().to_string()));
        }
    }
    if !status.code_modified.is_empty() {
        eprintln!("{}", style::section("Modified code files (hand-edits?):"));
        for p in &status.code_modified {
            eprintln!("{}", style::file_modified(&p.display().to_string()));
        }
    }

    // Config
    if !status.config_modified.is_empty() {
        eprintln!("{}", style::section("Config changes:"));
        for p in &status.config_modified {
            eprintln!("{}", style::file_modified(&p.display().to_string()));
        }
    }

    eprintln!();
    eprintln!(
        "{} Use {}",
        format!("{} file(s) changed.", status.total_changes()).bold(),
        "lit commit -m \"message\"".cyan()
    );

    Ok(())
}

fn show_prompts_only(root: &std::path::Path) -> Result<()> {
    let prompts_dir = root.join("prompts");
    if !prompts_dir.exists() {
        eprintln!("{}", "No prompts/ directory found.".dimmed());
        return Ok(());
    }

    let paths = discover_prompts(&prompts_dir)?;
    eprintln!("  Prompts: {}", paths.len().to_string().bold());
    for p in &paths {
        eprintln!("    {}", p.display());
    }

    Ok(())
}
