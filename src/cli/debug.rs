use crate::cli::DebugCommands;
use crate::core::config::LitConfig;
use crate::core::dag::Dag;
use crate::core::prompt::{Prompt, discover_prompts};

pub async fn run(what: DebugCommands) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, root) = LitConfig::find_and_load(&cwd)?;

    match what {
        DebugCommands::Config => dump_config(&config, &root),
        DebugCommands::Prompts => dump_prompts(&config, &root)?,
        DebugCommands::Dag => dump_dag(&config, &root)?,
        DebugCommands::All => {
            dump_config(&config, &root);
            println!();
            dump_prompts(&config, &root)?;
            println!();
            dump_dag(&config, &root)?;
        }
    }

    Ok(())
}

fn dump_config(config: &LitConfig, root: &std::path::Path) {
    println!("=== CONFIG (lit.toml) ===");
    println!();
    println!("  project.name:       {}", config.project.name);
    println!("  project.version:    {}", config.project.version);
    println!("  project.mapping:    {}", config.project.mapping);
    println!("  language.default:   {}", config.language.default);
    println!("  language.version:   {}", config.language.version);
    if let Some(ref fw) = config.framework {
        println!("  framework.name:     {}", fw.name);
        println!("  framework.version:  {}", fw.version);
    } else {
        println!("  framework:          (none)");
    }
    println!("  model.provider:     {}", config.model.provider);
    println!("  model.model:        {}", config.model.model);
    println!("  model.temperature:  {}", config.model.temperature);
    println!(
        "  model.seed:         {}",
        config
            .model
            .seed
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    if let Some(ref api) = config.model.api {
        let key_status = std::env::var(&api.key_env)
            .map(|k| format!("set ({}...)", &k[..k.len().min(8)]))
            .unwrap_or_else(|_| "NOT SET".to_string());
        println!("  model.api.key_env:  {} [{}]", api.key_env, key_status);
    }
    println!("  repo root:          {}", root.display());
}

fn dump_prompts(
    config: &LitConfig,
    root: &std::path::Path,
) -> anyhow::Result<()> {
    let prompts_dir = root.join("prompts");
    if !prompts_dir.exists() {
        println!("=== PROMPTS ===");
        println!();
        println!("  (no prompts/ directory found)");
        return Ok(());
    }

    let prompt_paths = discover_prompts(&prompts_dir)?;

    println!("=== PROMPTS ({} files) ===", prompt_paths.len());
    println!();

    let mut ok_count = 0;
    let mut err_count = 0;

    for path in &prompt_paths {
        match Prompt::from_file(path, root, config) {
            Ok(prompt) => {
                ok_count += 1;
                println!("  {} ✓", prompt.path.display());
                println!(
                    "    outputs: [{}]",
                    prompt
                        .frontmatter
                        .outputs
                        .iter()
                        .map(|o| o.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                if prompt.frontmatter.imports.is_empty() {
                    println!("    imports: (none — root node)");
                } else {
                    println!(
                        "    imports: [{}]",
                        prompt
                            .frontmatter
                            .imports
                            .iter()
                            .map(|i| i.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if let Some(ref model) = prompt.frontmatter.model {
                    println!("    model override: {} ({})", model.model, model.provider);
                }
                if let Some(ref lang) = prompt.frontmatter.language {
                    println!("    language override: {}", lang);
                }

                let body_imports = prompt.body_imports();
                if !body_imports.is_empty() {
                    println!(
                        "    @import() refs in body: [{}]",
                        body_imports
                            .iter()
                            .map(|i| i.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                println!("    body: {} chars, {} lines",
                    prompt.body.len(),
                    prompt.body.lines().count()
                );
                println!();
            }
            Err(e) => {
                err_count += 1;
                println!("  {} ✗ ERROR", path.display());
                println!("    {}", e);
                println!();
            }
        }
    }

    println!("  --- Summary: {} ok, {} errors ---", ok_count, err_count);

    Ok(())
}

fn dump_dag(
    config: &LitConfig,
    root: &std::path::Path,
) -> anyhow::Result<()> {
    let prompts_dir = root.join("prompts");
    if !prompts_dir.exists() {
        println!("=== DAG ===");
        println!();
        println!("  (no prompts/ directory found)");
        return Ok(());
    }

    let prompt_paths = discover_prompts(&prompts_dir)?;
    let mut prompts = Vec::new();
    for path in &prompt_paths {
        if let Ok(prompt) = Prompt::from_file(path, root, config) {
            prompts.push(prompt);
        }
    }

    // Build the real DAG with topological sort
    println!("=== DAG ===" );
    println!();

    match Dag::build(&prompts) {
        Ok(dag) => {
            // Generation order (topological sort)
            println!("  Generation order ({} prompts):", dag.len());
            for (i, path) in dag.order().iter().enumerate() {
                let node = dag.get(path).unwrap();
                let dep_count = node.imports.len();
                if dep_count == 0 {
                    println!("    {}. {} (root)", i + 1, path.display());
                } else {
                    println!("    {}. {} ({} deps)", i + 1, path.display(), dep_count);
                }
            }
            println!();

            // Root nodes
            let roots = dag.roots();
            println!("  Root nodes ({}):", roots.len());
            for r in &roots {
                println!("    → {}", r.prompt_path.display());
            }
            println!();

            // Leaf nodes
            let leaves = dag.leaves();
            println!("  Leaf nodes ({}):", leaves.len());
            for l in &leaves {
                println!("    ← {}", l.prompt_path.display());
            }
            println!();

            // Dependency edges
            println!("  Dependency edges:");
            for path in dag.order() {
                let node = dag.get(path).unwrap();
                if node.imports.is_empty() {
                    println!("    {} (root)", path.display());
                } else {
                    for import in &node.imports {
                        println!("    {} ← {}", path.display(), import.display());
                    }
                }
            }
            println!();

            // Reverse dependencies
            println!("  Reverse dependencies (who depends on me):");
            for path in dag.order() {
                let node = dag.get(path).unwrap();
                if node.dependents.is_empty() {
                    println!("    {} → (leaf)", path.display());
                } else {
                    let deps: Vec<String> = node.dependents.iter().map(|d| d.display().to_string()).collect();
                    println!("    {} → [{}]", path.display(), deps.join(", "));
                }
            }
            println!();

            // Regeneration examples
            for root in &roots {
                let regen = dag.regeneration_set(&[root.prompt_path.clone()]);
                println!(
                    "  If {} changes → {} prompt(s) need regeneration",
                    root.prompt_path.display(),
                    regen.len()
                );
            }
            println!();

            println!("  Validation:");
            println!("    ✓ No cycles detected");
            println!("    ✓ No output conflicts");
            println!("    ✓ All imports resolve");
        }
        Err(e) => {
            println!("  ✗ DAG BUILD FAILED:");
            println!("    {}", e);
        }
    }

    Ok(())
}
