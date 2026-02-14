use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use colored::Colorize;

use chrono::Utc;

use crate::core::cache::Cache;
use crate::core::config::LitConfig;
use crate::core::dag::Dag;
use crate::core::generation_record::{
    GenerationRecord, GenerationSummary, PromptRecord, estimate_cost, format_cost,
    format_tokens,
};
use crate::core::generator::Generator;
use crate::core::patch::{PatchResult, PatchStore};
use crate::core::prompt::{Prompt, discover_prompts};
use crate::core::style;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::openai::OpenAiProvider;

pub async fn run(path: Option<PathBuf>, all: bool, no_cache: bool, no_patches: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, root) = LitConfig::find_and_load(&cwd)?;

    // Discover and parse all prompts
    let prompts_dir = root.join("prompts");
    if !prompts_dir.exists() {
        anyhow::bail!(
            "No prompts/ directory found in {}\n\
             Hint: Create prompt files in prompts/ first, then run `lit regenerate`.",
            root.display()
        );
    }

    let prompt_paths = discover_prompts(&prompts_dir)?;
    if prompt_paths.is_empty() {
        anyhow::bail!(
            "No .prompt.md files found in prompts/\n\
             Hint: Create a prompt file like prompts/hello.prompt.md and try again."
        );
    }

    let mut prompts_vec = Vec::new();
    for p in &prompt_paths {
        prompts_vec.push(
            Prompt::from_file(p, &root, &config)
                .with_context(|| format!("Failed to parse {}", p.display()))?,
        );
    }

    // Build DAG
    let dag = Dag::build(&prompts_vec)?;

    // Build prompts map
    let prompts_map: HashMap<PathBuf, Prompt> = prompts_vec
        .into_iter()
        .map(|p| (p.path.clone(), p))
        .collect();

    // Determine regeneration set
    let regeneration_set = if all {
        dag.order().to_vec()
    } else if let Some(ref specific_path) = path {
        let relative = if specific_path.is_absolute() {
            specific_path
                .strip_prefix(&root)
                .unwrap_or(specific_path)
                .to_path_buf()
        } else {
            specific_path.clone()
        };
        let set = dag.regeneration_set(&[relative.clone()]);
        if set.is_empty() {
            anyhow::bail!(
                "Prompt {} not found in DAG.\n\nAvailable prompts:\n{}",
                relative.display(),
                dag.order()
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        set
    } else {
        dag.order().to_vec()
    };

    eprintln!("{}", style::regen_header(regeneration_set.len(), dag.len()));

    // Resolve API key
    let api_key = config.resolve_api_key().context(
        "Failed to resolve API key.\n\
         Hint: Set the environment variable specified in lit.toml [model.api] key_env,\n\
         e.g.: export LIT_API_KEY=sk-ant-..."
    )?;

    // Create provider
    let provider: Box<dyn crate::providers::LlmProvider> = match config.model.provider.as_str() {
        "anthropic" => Box::new(AnthropicProvider::new(api_key)),
        "openai" => Box::new(OpenAiProvider::new(api_key)),
        other => anyhow::bail!(
            "Provider '{}' is not supported.\n\
             Hint: Supported providers: anthropic, openai",
            other
        ),
    };

    // Write static files first
    let code_lock_dir = root.join("code.lock");
    let mut static_files_written = 0;
    for sf in &config.r#static {
        let full_path = code_lock_dir.join(&sf.path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create directory {}", parent.display())
            })?;
        }
        std::fs::write(&full_path, &sf.content).with_context(|| {
            format!("Failed to write static file {}", full_path.display())
        })?;
        static_files_written += 1;
    }
    if static_files_written > 0 {
        eprintln!("  Static files written: {}", static_files_written.to_string().dimmed());
    }

    // Load existing code from code.lock/ for context
    let existing_code = load_existing_code(&code_lock_dir);

    // Initialize cache
    let cache = if no_cache {
        eprintln!("  {}", "Cache disabled (--no-cache)".dimmed());
        None
    } else {
        let cache_dir = root.join(".lit").join("cache");
        let c = Cache::new(cache_dir);
        c.init().context("Failed to initialize cache directory")?;
        Some(c)
    };

    // Create generator and run pipeline
    let generator = Generator::new(provider, config.clone());
    let result = generator
        .run_pipeline(
            &dag,
            &prompts_map,
            &regeneration_set,
            &existing_code,
            cache.as_ref(),
        )
        .await?;

    // Load patch store
    let patch_store = if no_patches {
        eprintln!("  {}", "Patches disabled (--no-patches)".dimmed());
        None
    } else {
        let ps = PatchStore::new(root.join(".lit").join("patches"));
        let _ = ps.init();
        Some(ps)
    };

    // Write generated files to code.lock/, applying patches
    let mut files_written = 0;
    let mut patches_applied = 0;
    let mut patches_conflicted = 0;
    for output in &result.outputs {
        for (file_path, content) in &output.files {
            let mut final_content = content.clone();

            // Check if there's a saved patch for this file
            if let Some(ref ps) = patch_store {
                if let Some(stored_patch) = ps.load_patch(file_path) {
                    match ps.apply_patch(
                        &stored_patch.original_content,
                        content,
                        &stored_patch.manual_content,
                    ) {
                        PatchResult::Applied(merged) => {
                            eprintln!("{}", style::patch_applied(&file_path.display().to_string()));
                            final_content = merged;
                            patches_applied += 1;

                            if let Err(e) = ps.save_patch(
                                file_path,
                                content,
                                &final_content,
                            ) {
                                eprintln!(
                                    "    {}", style::warning(&format!("Failed to update patch: {}", e))
                                );
                            }
                        }
                        PatchResult::Conflict(conflict) => {
                            eprintln!("{}", style::patch_conflict(&file_path.display().to_string()));
                            eprintln!(
                                "      {}",
                                "Wrote conflict markers — please resolve manually".dimmed()
                            );
                            final_content = conflict;
                            patches_conflicted += 1;
                        }
                    }
                }
            }

            let full_path = code_lock_dir.join(file_path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create directory {}", parent.display())
                })?;
            }
            std::fs::write(&full_path, &final_content).with_context(|| {
                format!("Failed to write {}", full_path.display())
            })?;
            files_written += 1;
        }
    }

    // Build per-prompt records and compute costs
    let pricing_override = config.model.pricing.as_ref().map(|p| {
        crate::core::generation_record::ModelPricing::new(p.input_per_million, p.output_per_million)
    });
    let mut prompt_records = Vec::new();
    let mut total_cost = 0.0;
    for output in &result.outputs {
        let cost = estimate_cost(
            &output.model,
            output.tokens_in,
            output.tokens_out,
            pricing_override.as_ref(),
        );
        total_cost += cost;

        prompt_records.push(PromptRecord {
            prompt_path: output.prompt_path.clone(),
            output_files: output.files.keys().cloned().collect(),
            input_hash: output.input_hash.clone(),
            from_cache: output.from_cache,
            tokens_in: output.tokens_in,
            tokens_out: output.tokens_out,
            duration_ms: output.duration_ms,
            model: output.model.clone(),
            cost_usd: cost,
        });
    }

    // Write generation record
    let generation_record = GenerationRecord {
        timestamp: Utc::now(),
        project: config.project.name.clone(),
        model: config.model.model.clone(),
        temperature: config.model.temperature,
        seed: config.model.seed,
        language: config.language.default.clone(),
        framework: config.framework.as_ref().map(|fw| fw.name.clone()),
        prompts: prompt_records,
        summary: GenerationSummary {
            total_prompts: result.outputs.len() + result.skipped.len(),
            cache_hits: result.cache_hits,
            cache_misses: result.cache_misses,
            skipped: result.skipped.len(),
            total_tokens_in: result.total_tokens_in,
            total_tokens_out: result.total_tokens_out,
            total_cost_usd: total_cost,
            total_duration_ms: result.total_duration_ms,
            total_files_written: files_written,
            patches_applied,
            patches_conflicted,
        },
    };

    let generations_dir = root.join(".lit").join("generations");
    if let Err(e) = generation_record.write(&generations_dir) {
        eprintln!("  {}", style::warning(&format!("Failed to write generation record: {}", e)));
    }

    // Summary
    eprintln!();
    eprintln!("{}", style::header("Generation complete"));
    eprintln!(
        "  {:<20} {}",
        "Prompts generated:".dimmed(),
        result.outputs.len().to_string().bold()
    );
    eprintln!(
        "  {:<20} {}",
        "Prompts skipped:".dimmed(),
        result.skipped.len().to_string().dimmed()
    );
    if result.cache_hits > 0 || result.cache_misses > 0 {
        eprintln!(
            "  {:<20} {} hit(s), {} miss(es)",
            "Cache:".dimmed(),
            result.cache_hits.to_string().green(),
            result.cache_misses.to_string().yellow()
        );
    }
    eprintln!(
        "  {:<20} {}",
        "Files written:".dimmed(),
        files_written.to_string().bold()
    );
    if patches_applied > 0 || patches_conflicted > 0 {
        eprintln!(
            "  {:<20} {} applied, {} conflict(s)",
            "Patches:".dimmed(),
            patches_applied.to_string().green(),
            if patches_conflicted > 0 {
                patches_conflicted.to_string().red()
            } else {
                patches_conflicted.to_string().dimmed()
            }
        );
    }
    eprintln!(
        "  {:<20} {} in / {} out",
        "Tokens:".dimmed(),
        format_tokens(result.total_tokens_in).dimmed(),
        format_tokens(result.total_tokens_out).dimmed()
    );
    eprintln!(
        "  {:<20} {}",
        "Cost:".dimmed(),
        style::cost(&format_cost(total_cost))
    );
    eprintln!(
        "  {:<20} {}",
        "Time:".dimmed(),
        format!("{:.1}s", result.total_duration_ms as f64 / 1000.0).dimmed()
    );

    Ok(())
}

/// Load existing files from code.lock/ directory for use as context
fn load_existing_code(code_lock_dir: &std::path::Path) -> HashMap<PathBuf, String> {
    let mut code = HashMap::new();

    if !code_lock_dir.exists() {
        return code;
    }

    fn walk_dir(dir: &std::path::Path, base: &std::path::Path, out: &mut HashMap<PathBuf, String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk_dir(&path, base, out);
                } else if path.is_file() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(relative) = path.strip_prefix(base) {
                            out.insert(relative.to_path_buf(), content);
                        }
                    }
                }
            }
        }
    }

    walk_dir(code_lock_dir, code_lock_dir, &mut code);
    code
}
