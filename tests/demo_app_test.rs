//! Integration test: parse the real lit-demo-crud project

use std::path::{Path, PathBuf};

// We need to reference the crate's modules
use lit::core::config::LitConfig;
use lit::core::dag::Dag;
use lit::core::prompt::{Prompt, discover_prompts};

fn demo_app_root() -> PathBuf {
    // The demo app is a sibling directory to the lit project
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("../lit-demo-crud")
}

#[test]
fn test_parse_demo_app_config() {
    let root = demo_app_root();
    let config_path = root.join("lit.toml");
    assert!(config_path.exists(), "Demo app lit.toml not found at {:?}", config_path);

    let config = LitConfig::from_file(&config_path).unwrap();

    assert_eq!(config.project.name, "lit-demo-crud");
    assert_eq!(config.project.mapping, "manifest");
    assert_eq!(config.language.default, "python");
    assert_eq!(config.language.version, "3.12");
    assert_eq!(config.model.provider, "anthropic");
    assert_eq!(config.model.temperature, 0.0);
    assert_eq!(config.model.seed, Some(42));

    let framework = config.framework.as_ref().unwrap();
    assert_eq!(framework.name, "fastapi");
}

#[test]
fn test_discover_demo_app_prompts() {
    let root = demo_app_root();
    let prompts_dir = root.join("prompts");
    assert!(prompts_dir.exists(), "Demo app prompts/ not found");

    let prompt_paths = discover_prompts(&prompts_dir).unwrap();

    assert_eq!(
        prompt_paths.len(),
        12,
        "Expected 12 prompt files, found {}: {:?}",
        prompt_paths.len(),
        prompt_paths
    );

    // Verify all are .prompt.md files
    for path in &prompt_paths {
        assert!(
            path.to_str().unwrap().ends_with(".prompt.md"),
            "Not a prompt file: {:?}",
            path
        );
    }
}

#[test]
fn test_parse_all_demo_app_prompts() {
    let root = demo_app_root();
    let config = LitConfig::from_file(&root.join("lit.toml")).unwrap();
    let prompts_dir = root.join("prompts");
    let prompt_paths = discover_prompts(&prompts_dir).unwrap();

    let mut parsed_prompts = Vec::new();

    for prompt_path in &prompt_paths {
        let prompt = Prompt::from_file(prompt_path, &root, &config)
            .unwrap_or_else(|e| panic!("Failed to parse {:?}: {}", prompt_path, e));

        // Every prompt in manifest mode must have outputs
        assert!(
            !prompt.frontmatter.outputs.is_empty(),
            "Prompt {:?} has no outputs",
            prompt.path
        );

        parsed_prompts.push(prompt);
    }

    // Verify specific prompts
    let structure = parsed_prompts
        .iter()
        .find(|p| p.path.to_str().unwrap().contains("structure"))
        .expect("structure.prompt.md not found");
    assert!(structure.frontmatter.imports.is_empty(), "structure should have no imports");
    assert_eq!(structure.frontmatter.outputs.len(), 5, "structure should produce 5 __init__.py files");

    let database = parsed_prompts
        .iter()
        .find(|p| p.path.to_str().unwrap().contains("database"))
        .expect("database.prompt.md not found");
    assert_eq!(database.frontmatter.imports.len(), 1, "database should import structure");
    assert_eq!(database.frontmatter.outputs.len(), 1);

    let user_model = parsed_prompts
        .iter()
        .find(|p| p.path.to_str().unwrap().contains("models/user"))
        .expect("models/user.prompt.md not found");
    assert_eq!(user_model.frontmatter.imports.len(), 1, "user model should import base");

    let item_model = parsed_prompts
        .iter()
        .find(|p| p.path.to_str().unwrap().contains("models/item"))
        .expect("models/item.prompt.md not found");
    assert_eq!(item_model.frontmatter.imports.len(), 2, "item model should import base and user");

    let items_api = parsed_prompts
        .iter()
        .find(|p| p.path.to_str().unwrap().contains("api/items"))
        .expect("api/items.prompt.md not found");
    assert_eq!(items_api.frontmatter.imports.len(), 4, "items api should have 4 imports");

    // Verify total import/output counts across all prompts
    let total_outputs: usize = parsed_prompts.iter().map(|p| p.frontmatter.outputs.len()).sum();
    let total_imports: usize = parsed_prompts.iter().map(|p| p.frontmatter.imports.len()).sum();

    assert_eq!(total_outputs, 16, "Expected 16 total output files (11 original + 5 __init__.py)");
    assert!(total_imports > 0, "Expected some imports");

    println!("\n=== Demo App Parse Results ===");
    println!("Prompts found: {}", parsed_prompts.len());
    println!("Total outputs: {}", total_outputs);
    println!("Total imports: {}", total_imports);
    for p in &parsed_prompts {
        println!(
            "  {} → {} output(s), {} import(s)",
            p.path.display(),
            p.frontmatter.outputs.len(),
            p.frontmatter.imports.len()
        );
    }
}

#[test]
fn test_demo_app_dag() {
    let root = demo_app_root();
    let config = LitConfig::from_file(&root.join("lit.toml")).unwrap();
    let prompts_dir = root.join("prompts");
    let prompt_paths = discover_prompts(&prompts_dir).unwrap();

    let mut prompts = Vec::new();
    for path in &prompt_paths {
        prompts.push(Prompt::from_file(path, &root, &config).unwrap());
    }

    // DAG should build without errors (no cycles, no output conflicts)
    let dag = Dag::build(&prompts).unwrap();

    // 12 prompts (including structure.prompt.md)
    assert_eq!(dag.len(), 12);

    // structure.prompt.md is the only root (no imports)
    let roots = dag.roots();
    assert_eq!(roots.len(), 1, "Expected exactly 1 root node");
    assert!(
        roots[0].prompt_path.to_str().unwrap().contains("structure"),
        "Root should be structure, got: {}",
        roots[0].prompt_path.display()
    );

    // Leaf nodes: api/items, api/users, tests/test_items, tests/test_users
    let leaves = dag.leaves();
    assert_eq!(leaves.len(), 4, "Expected 4 leaf nodes (2 API endpoints + 2 test files)");

    // Topological order: structure must come first, then database
    let order = dag.order();
    assert_eq!(order.len(), 12);
    assert!(
        order[0].to_str().unwrap().contains("structure"),
        "First in generation order should be structure, got: {}",
        order[0].display()
    );

    let structure_pos = order.iter().position(|p| p.to_str().unwrap().contains("structure")).unwrap();
    let database_pos = order.iter().position(|p| p.to_str().unwrap().contains("database")).unwrap();
    assert!(structure_pos < database_pos, "structure must come before database");

    // base.prompt.md must come before user.prompt.md and item.prompt.md
    let base_pos = order.iter().position(|p| p.to_str().unwrap().contains("models/base")).unwrap();
    let user_pos = order.iter().position(|p| p.to_str().unwrap().contains("models/user")).unwrap();
    let item_pos = order.iter().position(|p| p.to_str().unwrap().contains("models/item")).unwrap();
    assert!(base_pos < user_pos, "base must come before user model");
    assert!(base_pos < item_pos, "base must come before item model");
    assert!(user_pos < item_pos, "user must come before item (item imports user)");

    // Regeneration set: changing structure should cascade to ALL 12 prompts
    let regen_all = dag.regeneration_set(&[PathBuf::from("prompts/config/structure.prompt.md")]);
    assert_eq!(
        regen_all.len(), 12,
        "Changing structure should regenerate all 12 prompts, got {}",
        regen_all.len()
    );

    // Regeneration set: changing database should cascade to 11 (all except structure)
    let regen_db = dag.regeneration_set(&[PathBuf::from("prompts/config/database.prompt.md")]);
    assert_eq!(
        regen_db.len(), 11,
        "Changing database should regenerate 11 prompts (all except structure), got {}",
        regen_db.len()
    );

    // Regeneration set: changing just user schema should cascade to
    // api/users, schemas/item, api/items, tests/test_items, tests/test_users
    let regen_user_schema = dag.regeneration_set(&[PathBuf::from("prompts/schemas/user.prompt.md")]);
    assert!(
        regen_user_schema.len() >= 4,
        "Changing user schema should cascade to at least 4 prompts, got {}",
        regen_user_schema.len()
    );
    assert!(
        regen_user_schema.contains(&PathBuf::from("prompts/schemas/user.prompt.md")),
        "Regen set should include the changed prompt itself"
    );

    // Regeneration set: changing a leaf (test_items) should only regen itself
    let regen_leaf = dag.regeneration_set(&[PathBuf::from("prompts/tests/test_items.prompt.md")]);
    assert_eq!(
        regen_leaf.len(), 1,
        "Changing a leaf should only regenerate itself, got {}",
        regen_leaf.len()
    );

    println!("\n=== Demo App DAG Results ===");
    println!("Generation order:");
    for (i, path) in order.iter().enumerate() {
        println!("  {}. {}", i + 1, path.display());
    }
}
