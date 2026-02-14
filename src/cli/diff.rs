use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use colored::Colorize;

use crate::core::config::LitConfig;
use crate::core::dag::Dag;
use crate::core::prompt::{Prompt, discover_prompts};
use crate::core::repo::LitRepo;
use crate::core::style;

pub async fn run(code: bool, all: bool, summary: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, root) = LitConfig::find_and_load(&cwd)?;

    let repo = LitRepo::open(&root)?;

    if summary {
        return run_summary(&config, &root, &repo).await;
    }

    let diff = if all {
        repo.diff_all()?
    } else if code {
        repo.diff_code()?
    } else {
        // Default: show prompt diffs
        repo.diff_prompts()?
    };

    if diff.is_empty() {
        let scope = if all {
            "prompts, code, or config"
        } else if code {
            "code.lock/"
        } else {
            "prompts/"
        };
        eprintln!("No changes in {}.", scope);
    } else {
        print!("{}", diff);
    }

    Ok(())
}

async fn run_summary(config: &LitConfig, root: &std::path::Path, repo: &LitRepo) -> Result<()> {
    let status = repo.status()?;

    // Collect all changed prompts
    let has_prompt_changes = !status.prompts_modified.is_empty()
        || !status.prompts_new.is_empty()
        || !status.prompts_deleted.is_empty();
    let has_code_changes = !status.code_modified.is_empty() || !status.code_new.is_empty();

    if !has_prompt_changes && !has_code_changes {
        eprintln!("No changes in prompts or code.");
        return Ok(());
    }

    // Get per-file diff stats for prompts
    let diff_stats = repo.diff_prompt_stats()?;
    let stats_map: HashMap<PathBuf, (usize, usize)> = diff_stats
        .into_iter()
        .map(|s| (s.path, (s.insertions, s.deletions)))
        .collect();

    eprintln!("{}", style::header("Changes Summary"));

    // -- Prompts section --
    if has_prompt_changes {
        eprintln!("  {}", "Prompts:".bold());

        for path in &status.prompts_new {
            let line_count = std::fs::read_to_string(root.join(path))
                .map(|c| c.lines().count())
                .unwrap_or(0);
            eprintln!(
                "    {}  {}",
                style::file_new(&path.display().to_string()),
                format!("(new, {} lines)", line_count).dimmed()
            );
        }

        for path in &status.prompts_modified {
            let stat_str = if let Some((ins, del)) = stats_map.get(path) {
                format!(
                    "({} {} lines)",
                    format!("+{}", ins).green(),
                    format!("-{}", del).red()
                )
            } else {
                "(modified)".to_string()
            };
            eprintln!(
                "    {}  {}",
                style::file_modified(&path.display().to_string()),
                stat_str
            );
        }

        for path in &status.prompts_deleted {
            eprintln!("    {}", style::file_deleted(&path.display().to_string()));
        }
    }

    // -- DAG impact section --
    if has_prompt_changes {
        let prompts_dir = root.join("prompts");
        if prompts_dir.exists() {
            if let Ok(prompt_paths) = discover_prompts(&prompts_dir) {
                let prompts_vec: Vec<Prompt> = prompt_paths
                    .iter()
                    .filter_map(|p| Prompt::from_file(p, root, config).ok())
                    .collect();

                if let Ok(dag) = Dag::build(&prompts_vec) {
                    // Combine modified + new as changed (deleted won't be in DAG)
                    let changed: Vec<PathBuf> = status
                        .prompts_modified
                        .iter()
                        .chain(status.prompts_new.iter())
                        .cloned()
                        .collect();

                    let regen_set = dag.regeneration_set(&changed);

                    if !regen_set.is_empty() {
                        eprintln!();
                        eprintln!(
                            "  {}",
                            "Impact (prompts that will regenerate):".bold()
                        );

                        // Build a set of directly-changed prompts for annotation
                        let directly_changed: std::collections::HashSet<PathBuf> =
                            changed.iter().cloned().collect();

                        for regen_path in &regen_set {
                            let reason = if directly_changed.contains(regen_path) {
                                String::new()
                            } else {
                                // Find which of its imports are in the regen set
                                if let Some(node) = dag.nodes().get(regen_path) {
                                    let import_names: Vec<String> = node
                                        .imports
                                        .iter()
                                        .filter(|imp| {
                                            regen_set.contains(*imp)
                                                || directly_changed.contains(*imp)
                                        })
                                        .map(|imp| {
                                            // Extract a short name: prompts/models/user.prompt.md → "user model"
                                            let stem = imp
                                                .file_stem()
                                                .and_then(|s| s.to_str())
                                                .unwrap_or("?")
                                                .replace(".prompt", "");
                                            let parent = imp
                                                .parent()
                                                .and_then(|p| p.file_name())
                                                .and_then(|s| s.to_str())
                                                .unwrap_or("");
                                            if parent == "prompts" || parent.is_empty() {
                                                stem
                                            } else {
                                                format!("{} {}", stem, parent)
                                            }
                                        })
                                        .collect();

                                    if import_names.is_empty() {
                                        String::new()
                                    } else {
                                        format!("  (imports {})", import_names.join(", "))
                                            .dimmed()
                                            .to_string()
                                    }
                                } else {
                                    String::new()
                                }
                            };

                            eprintln!(
                                "    {} {}{}",
                                "→".cyan(),
                                regen_path.display(),
                                reason
                            );
                        }

                        // -- Generated code affected --
                        let mut affected_outputs: Vec<PathBuf> = Vec::new();
                        for regen_path in &regen_set {
                            if let Some(node) = dag.nodes().get(regen_path) {
                                for output in &node.outputs {
                                    let code_path =
                                        PathBuf::from("code.lock").join(output);
                                    affected_outputs.push(code_path);
                                }
                            }
                        }
                        affected_outputs.sort();
                        affected_outputs.dedup();

                        if !affected_outputs.is_empty() {
                            eprintln!();
                            eprintln!("  {}", "Generated code affected:".bold());
                            for path in &affected_outputs {
                                eprintln!(
                                    "    {}",
                                    style::file_modified(&path.display().to_string())
                                );
                            }
                        }

                        // -- Summary line --
                        let total = dag.len();
                        let regen_count = regen_set.len();
                        let unchanged = total - regen_count;
                        eprintln!();
                        eprintln!(
                            "  {} prompt(s) will regenerate, {} unchanged",
                            regen_count.to_string().yellow(),
                            unchanged.to_string().dimmed()
                        );
                    }
                }
            }
        }
    }

    // -- Code-only changes (hand-edits not from prompt changes) --
    if has_code_changes && !has_prompt_changes {
        eprintln!("  {}", "Code modifications (hand-edits):".bold());
        for path in &status.code_modified {
            eprintln!(
                "    {}",
                style::file_modified(&path.display().to_string())
            );
        }
        for path in &status.code_new {
            eprintln!("    {}", style::file_new(&path.display().to_string()));
        }
    }

    Ok(())
}
