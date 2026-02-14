use std::path::PathBuf;

use anyhow::Result;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::prompt::is_prompt_file;
use crate::core::style;

pub async fn run(path: PathBuf) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let full_path = if path.is_absolute() {
        path.clone()
    } else {
        cwd.join(&path)
    };

    if !full_path.exists() {
        anyhow::bail!(
            "Path does not exist: {}\n\
             Hint: Check the path and try again.",
            full_path.display()
        );
    }

    if full_path.is_file() {
        if !is_prompt_file(&full_path) {
            anyhow::bail!(
                "{} is not a .prompt.md file.\n\
                 Hint: Lit only tracks prompt files (*.prompt.md).",
                path.display()
            );
        }

        let relative = full_path
            .strip_prefix(&root)
            .unwrap_or(&full_path);
        if !relative.starts_with("prompts") {
            eprintln!(
                "{} {} is not inside prompts/. Move it to prompts/ for lit to track it.",
                "⚠".yellow().bold(),
                relative.display()
            );
        }

        eprintln!("{} {}", "Tracked:".green(), relative.display());
    } else if full_path.is_dir() {
        let mut count = 0;
        for entry in walkdir(&full_path) {
            if is_prompt_file(&entry) {
                let relative = entry
                    .strip_prefix(&root)
                    .unwrap_or(&entry);
                eprintln!("  {} {}", "Tracked:".green(), relative.display());
                count += 1;
            }
        }
        if count == 0 {
            eprintln!(
                "{} No .prompt.md files found in {}",
                "⚠".yellow().bold(),
                path.display()
            );
        } else {
            eprintln!("{}", format!("{} prompt(s) tracked.", count).bold());
        }
    }

    eprintln!();
    eprintln!("{}", style::hint("Note: `lit commit` automatically stages all prompts. `lit add` is a validation helper."));

    Ok(())
}

fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walkdir(&path));
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files
}
