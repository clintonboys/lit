use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::PatchCommands;
use crate::core::cache::Cache;
use crate::core::config::LitConfig;
use crate::core::dag::Dag;
use crate::core::patch::PatchStore;
use crate::core::prompt::{Prompt, discover_prompts};

pub async fn run(action: PatchCommands) -> Result<()> {
    match action {
        PatchCommands::Save => save().await,
        PatchCommands::List => list().await,
        PatchCommands::Drop { path } => drop_patch(path).await,
        PatchCommands::Show { path } => show(path).await,
    }
}

/// `lit patch save` — detect manual edits to code.lock/ and save them as patches
async fn save() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, root) = LitConfig::find_and_load(&cwd)?;

    let code_lock_dir = root.join("code.lock");
    if !code_lock_dir.exists() {
        anyhow::bail!("No code.lock/ directory found. Run `lit regenerate` first.");
    }

    // Load actual code from disk
    let actual_code = load_code_from_dir(&code_lock_dir);

    // Load cached generation results to get the "original generated" content
    let cache_dir = root.join(".lit").join("cache");
    let cache = Cache::new(cache_dir);

    // Build the generated content map from cache entries
    let generated_code = load_generated_from_cache(&root, &config, &cache)?;

    if generated_code.is_empty() {
        eprintln!("No cached generation results found. Run `lit regenerate` first to build the cache.");
        return Ok(());
    }

    // Detect patches
    let patches = PatchStore::detect_patches(&generated_code, &actual_code);

    if patches.is_empty() {
        eprintln!("No manual edits detected. code.lock/ matches cached generation.");
        return Ok(());
    }

    // Save detected patches
    let patch_store = PatchStore::new(root.join(".lit").join("patches"));
    patch_store.init()?;

    for patch_info in &patches {
        let original = generated_code
            .get(&patch_info.output_path)
            .map(|s| s.as_str())
            .unwrap_or("");
        let manual = actual_code
            .get(&patch_info.output_path)
            .map(|s| s.as_str())
            .unwrap_or("");

        patch_store.save_patch(&patch_info.output_path, original, manual)?;

        eprintln!(
            "  Saved patch: {} (+{} -{} lines)",
            patch_info.output_path.display(),
            patch_info.lines_added,
            patch_info.lines_removed
        );
    }

    eprintln!("\n{} patch(es) saved to .lit/patches/", patches.len());
    Ok(())
}

/// `lit patch list` — show all tracked patches
async fn list() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let patch_store = PatchStore::new(root.join(".lit").join("patches"));
    let patches = patch_store.list_patches();

    if patches.is_empty() {
        eprintln!("No patches tracked. Use `lit patch save` to save manual edits.");
        return Ok(());
    }

    eprintln!("Tracked patches:");
    for path in &patches {
        if let Some(stored) = patch_store.load_patch(path) {
            let lines: Vec<&str> = stored.diff.lines().collect();
            let added = lines.iter().filter(|l| l.starts_with('+')).count();
            let removed = lines.iter().filter(|l| l.starts_with('-')).count();
            eprintln!("  {} (+{} -{})", path.display(), added, removed);
        } else {
            eprintln!("  {}", path.display());
        }
    }
    eprintln!("\n{} patch(es) total", patches.len());
    Ok(())
}

/// `lit patch drop <path>` — discard a patch
async fn drop_patch(path: PathBuf) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let patch_store = PatchStore::new(root.join(".lit").join("patches"));

    if !patch_store.has_patch(&path) {
        anyhow::bail!("No patch found for {}", path.display());
    }

    patch_store.drop_patch(&path)?;
    eprintln!("Dropped patch for {}", path.display());
    eprintln!("The generated version will be used on next regeneration.");
    Ok(())
}

/// `lit patch show <path>` — show the diff for a patch
async fn show(path: PathBuf) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (_config, root) = LitConfig::find_and_load(&cwd)?;

    let patch_store = PatchStore::new(root.join(".lit").join("patches"));

    if let Some(stored) = patch_store.load_patch(&path) {
        println!("{}", stored.diff);
    } else {
        anyhow::bail!("No patch found for {}", path.display());
    }
    Ok(())
}

/// Load all code files from a directory
fn load_code_from_dir(dir: &std::path::Path) -> HashMap<PathBuf, String> {
    let mut code = HashMap::new();
    if !dir.exists() {
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

    walk_dir(dir, dir, &mut code);
    code
}

/// Load what the LLM generated from cache entries.
///
/// Walks the DAG, computes input hashes, and loads cache entries.
/// Returns a map of output file path → generated content for all cached prompts.
fn load_generated_from_cache(
    root: &std::path::Path,
    config: &LitConfig,
    cache: &Cache,
) -> Result<HashMap<PathBuf, String>> {
    let prompts_dir = root.join("prompts");
    if !prompts_dir.exists() {
        return Ok(HashMap::new());
    }

    let prompt_paths = discover_prompts(&prompts_dir)?;
    let mut prompts_vec = Vec::new();
    for p in &prompt_paths {
        prompts_vec.push(
            Prompt::from_file(p, root, config)
                .with_context(|| format!("Failed to parse {}", p.display()))?,
        );
    }

    let dag = Dag::build(&prompts_vec)?;

    let prompts_map: HashMap<PathBuf, Prompt> = prompts_vec
        .into_iter()
        .map(|p| (p.path.clone(), p))
        .collect();

    let language = &config.language.default;
    let framework = config.framework.as_ref().map(|fw| fw.name.as_str());

    let mut input_hashes: HashMap<PathBuf, String> = HashMap::new();
    let mut generated_code: HashMap<PathBuf, String> = HashMap::new();

    for prompt_path in dag.order() {
        let prompt = prompts_map
            .get(prompt_path)
            .with_context(|| format!("Prompt {} not found", prompt_path.display()))?;

        let (model, temperature, seed) = resolve_model_config(prompt, config);

        let import_hashes: Vec<(&std::path::Path, &str)> = prompt
            .frontmatter
            .imports
            .iter()
            .filter_map(|import_path| {
                input_hashes
                    .get(import_path)
                    .map(|h| (import_path.as_path(), h.as_str()))
            })
            .collect();

        let input_hash = Cache::compute_input_hash(
            &prompt.raw,
            &import_hashes,
            &model,
            temperature,
            seed,
            language,
            framework,
        );

        input_hashes.insert(prompt_path.clone(), input_hash.clone());

        // Try to load from cache
        if let Some(cached) = cache.get(&input_hash) {
            for (path, content) in cached.files {
                generated_code.insert(path, content);
            }
        }
    }

    Ok(generated_code)
}

/// Resolve model config for a prompt (mirrors Generator::resolve_model_config)
fn resolve_model_config(prompt: &Prompt, config: &LitConfig) -> (String, f64, Option<u64>) {
    if let Some(ref model_override) = prompt.frontmatter.model {
        (
            model_override.model.clone(),
            model_override.temperature,
            model_override.seed,
        )
    } else {
        (
            config.model.model.clone(),
            config.model.temperature,
            config.model.seed,
        )
    }
}
