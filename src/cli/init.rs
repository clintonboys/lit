use anyhow::{Context, Result};
use colored::Colorize;

use crate::core::repo::LitRepo;

pub async fn run(_defaults: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;

    let already_has_config = cwd.join("lit.toml").exists();
    let already_has_git = cwd.join(".git").exists();

    if already_has_config && already_has_git {
        anyhow::bail!(
            "Already a lit repository (lit.toml and .git exist in {})\n\
             Hint: Use `lit status` to see the current state.",
            cwd.display()
        );
    }

    // Write lit.toml only if it doesn't exist
    if !already_has_config {
        let project_name = cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "my-project".to_string());

        let config_content = format!(
            r#"[project]
name = "{}"
version = "0.1.0"
mapping = "manifest"

[language]
default = "python"
version = "3.12"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
seed = 42

[model.api]
key_env = "LIT_API_KEY"
"#,
            project_name
        );

        std::fs::write(cwd.join("lit.toml"), &config_content)
            .context("Failed to write lit.toml")?;
    }

    // Create directories (idempotent)
    std::fs::create_dir_all(cwd.join("prompts"))
        .context("Failed to create prompts/ directory")?;
    std::fs::create_dir_all(cwd.join("code.lock"))
        .context("Failed to create code.lock/ directory")?;
    std::fs::create_dir_all(cwd.join(".lit"))
        .context("Failed to create .lit/ directory")?;

    // Initialize git repo (if not already one)
    let repo = if already_has_git {
        LitRepo::open(&cwd)?
    } else {
        LitRepo::init(&cwd)?
    };

    // Write .gitignore
    repo.write_gitignore()?;

    // Create initial commit
    repo.stage_all()?;
    let hash = repo.commit("lit init")?;

    let short_hash = &hash[..7.min(hash.len())];

    if already_has_config {
        eprintln!(
            "{} Initialized git for existing lit project",
            "✓".green().bold()
        );
        eprintln!("  Git: initial commit {}", short_hash.yellow());
    } else {
        eprintln!(
            "{} Initialized lit repository in {}",
            "✓".green().bold(),
            cwd.display()
        );
        eprintln!("  Created: {}", "lit.toml, prompts/, code.lock/, .lit/".dimmed());
        eprintln!("  Git:     initial commit {}", short_hash.yellow());
    }
    eprintln!();
    eprintln!("{}", "Next steps:".bold());
    if !already_has_config {
        eprintln!("  1. Edit {} with your project settings", "lit.toml".cyan());
        eprintln!("  2. Create prompt files in {}", "prompts/".cyan());
    }
    eprintln!("  Run {} to generate code", "lit regenerate".cyan());
    eprintln!("  Run {} to commit", "lit commit -m \"your message\"".cyan());

    Ok(())
}
