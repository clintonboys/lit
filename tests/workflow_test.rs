//! Integration tests: full lit workflow using core modules directly.
//!
//! These tests exercise the end-to-end flow without shelling out to the `lit` binary:
//! init repo → write prompts → commit → modify → status → diff → log → checkout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lit::core::cache::Cache;
use lit::core::config::LitConfig;
use lit::core::dag::Dag;
use lit::core::generation_record::{
    GenerationRecord, GenerationSummary, PromptRecord, estimate_cost,
};
use lit::core::generator::parse_response;
use lit::core::prompt::{Prompt, discover_prompts};
use lit::core::repo::LitRepo;

// ---------- Helpers ----------

/// Create a minimal lit.toml config string
fn minimal_config(name: &str) -> String {
    format!(
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
        name
    )
}

/// Create a simple prompt file content with outputs
fn simple_prompt(output_path: &str) -> String {
    format!(
        r#"---
outputs:
  - {}
---

# Hello

Generate a hello world module.
"#,
        output_path
    )
}

/// Create a prompt with imports
fn prompt_with_import(output_path: &str, import_path: &str) -> String {
    format!(
        r#"---
outputs:
  - {}
imports:
  - {}
---

# Dependent

Generate code that depends on @import({}).
"#,
        output_path, import_path, import_path
    )
}

/// Set up a full lit project directory with config, prompts dir, code.lock, .lit
fn setup_lit_project(dir: &Path, name: &str) {
    std::fs::write(dir.join("lit.toml"), minimal_config(name)).unwrap();
    std::fs::create_dir_all(dir.join("prompts")).unwrap();
    std::fs::create_dir_all(dir.join("code.lock")).unwrap();
    std::fs::create_dir_all(dir.join(".lit")).unwrap();
    std::fs::create_dir_all(dir.join(".lit/generations")).unwrap();
    std::fs::create_dir_all(dir.join(".lit/cache")).unwrap();
}

// ---------- Tests ----------

/// Test: full workflow init → add prompt → commit → modify → status → diff → commit → log → checkout
#[test]
fn test_full_workflow() {
    let dir = tempfile::tempdir().unwrap();

    // 1. Initialize repo
    let repo = LitRepo::init(dir.path()).unwrap();
    setup_lit_project(dir.path(), "test-project");
    repo.write_gitignore().unwrap();
    repo.stage_all().unwrap();
    let init_hash = repo.commit("lit init").unwrap();
    assert!(!init_hash.is_empty());

    // 2. Add a prompt file
    std::fs::write(
        dir.path().join("prompts/hello.prompt.md"),
        simple_prompt("src/hello.py"),
    )
    .unwrap();

    // 3. Check status — should show new prompt
    let status = repo.status().unwrap();
    assert!(status.has_changes());
    assert_eq!(status.prompts_new.len(), 1);
    assert!(
        status.prompts_new[0]
            .to_string_lossy()
            .contains("hello.prompt.md")
    );

    // 4. Check diff — for a brand-new untracked file, the diff may be empty
    // (git2 diff_tree_to_workdir_with_index shows diff vs HEAD, but unstaged new files
    // may not appear). Instead, verify the diff works after the commit cycle.
    // We'll test diff with modifications below.

    // 5. Simulate code generation by writing to code.lock
    std::fs::create_dir_all(dir.path().join("code.lock/src")).unwrap();
    std::fs::write(
        dir.path().join("code.lock/src/hello.py"),
        "def hello():\n    print('Hello, World!')\n",
    )
    .unwrap();

    // 6. Commit
    repo.stage_all().unwrap();
    let commit_hash = repo.commit("Add hello prompt + generated code").unwrap();
    assert!(!commit_hash.is_empty());

    // Status should be clean
    let status = repo.status().unwrap();
    assert!(!status.has_changes(), "Expected clean after commit");

    // 7. Modify the prompt
    std::fs::write(
        dir.path().join("prompts/hello.prompt.md"),
        simple_prompt("src/hello.py").replace("hello world", "greeting"),
    )
    .unwrap();

    // 8. Status should show modified prompt
    let status = repo.status().unwrap();
    assert!(status.has_changes());
    assert_eq!(status.prompts_modified.len(), 1);

    // 9. Diff should show the modification
    let diff = repo.diff_prompts().unwrap();
    assert!(!diff.is_empty(), "Expected prompt diff after modify");

    // 10. Update generated code and commit again
    std::fs::write(
        dir.path().join("code.lock/src/hello.py"),
        "def greet(name: str) -> str:\n    return f'Hello, {name}!'\n",
    )
    .unwrap();
    repo.stage_all().unwrap();
    let second_hash = repo.commit("Update hello prompt").unwrap();

    // 11. Log should show 3 commits
    let log = repo.log(10).unwrap();
    assert_eq!(log.len(), 3); // init + first + second
    assert_eq!(log[0].message, "Update hello prompt");
    assert_eq!(log[1].message, "Add hello prompt + generated code");
    assert_eq!(log[2].message, "lit init");

    // 12. Checkout first commit, verify file is restored
    repo.checkout_ref(&commit_hash).unwrap();
    let content =
        std::fs::read_to_string(dir.path().join("code.lock/src/hello.py")).unwrap();
    assert!(content.contains("Hello, World!"), "Should be original code after checkout");

    // 13. Checkout back to latest
    repo.checkout_ref(&second_hash).unwrap();
    let content =
        std::fs::read_to_string(dir.path().join("code.lock/src/hello.py")).unwrap();
    assert!(
        content.contains("greet(name"),
        "Should be updated code after checkout to latest"
    );
}

/// Test: DAG-driven regeneration set cascade
#[test]
fn test_dag_cascade_regeneration() {
    let dir = tempfile::tempdir().unwrap();
    setup_lit_project(dir.path(), "dag-test");

    let config = LitConfig::from_file(&dir.path().join("lit.toml")).unwrap();

    // Create three prompts: A → B → C
    std::fs::write(
        dir.path().join("prompts/a.prompt.md"),
        simple_prompt("src/a.py"),
    )
    .unwrap();

    std::fs::write(
        dir.path().join("prompts/b.prompt.md"),
        prompt_with_import("src/b.py", "prompts/a.prompt.md"),
    )
    .unwrap();

    std::fs::write(
        dir.path().join("prompts/c.prompt.md"),
        prompt_with_import("src/c.py", "prompts/b.prompt.md"),
    )
    .unwrap();

    // Discover and parse
    let paths = discover_prompts(&dir.path().join("prompts")).unwrap();
    assert_eq!(paths.len(), 3);

    let mut prompts = Vec::new();
    for p in &paths {
        prompts.push(Prompt::from_file(p, dir.path(), &config).unwrap());
    }

    // Build DAG
    let dag = Dag::build(&prompts).unwrap();
    assert_eq!(dag.len(), 3);

    // Check order: A before B before C
    let order = dag.order();
    let a_pos = order
        .iter()
        .position(|p| p.to_str().unwrap().contains("/a."))
        .unwrap();
    let b_pos = order
        .iter()
        .position(|p| p.to_str().unwrap().contains("/b."))
        .unwrap();
    let c_pos = order
        .iter()
        .position(|p| p.to_str().unwrap().contains("/c."))
        .unwrap();
    assert!(a_pos < b_pos);
    assert!(b_pos < c_pos);

    // Changing A should cascade to all 3
    let regen = dag.regeneration_set(&[PathBuf::from("prompts/a.prompt.md")]);
    assert_eq!(regen.len(), 3);

    // Changing B should cascade to B and C only
    let regen = dag.regeneration_set(&[PathBuf::from("prompts/b.prompt.md")]);
    assert_eq!(regen.len(), 2);
    assert!(regen.contains(&PathBuf::from("prompts/b.prompt.md")));
    assert!(regen.contains(&PathBuf::from("prompts/c.prompt.md")));

    // Changing C should only regen C
    let regen = dag.regeneration_set(&[PathBuf::from("prompts/c.prompt.md")]);
    assert_eq!(regen.len(), 1);
}

/// Test: cache hit/miss behavior
#[test]
fn test_cache_hit_miss() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let cache = Cache::new(cache_dir);
    cache.init().unwrap();

    // Compute a hash
    let hash = Cache::compute_input_hash(
        "prompt content v1",
        &[],
        "claude-sonnet-4-5-20250929",
        0.0,
        Some(42),
        "python",
        Some("fastapi"),
    );

    // Cache miss
    assert!(cache.get(&hash).is_none());

    // Store a result
    let cached = lit::core::cache::CachedGeneration {
        input_hash: hash.clone(),
        files: {
            let mut m = HashMap::new();
            m.insert(PathBuf::from("src/hello.py"), "print('hello')\n".to_string());
            m
        },
        tokens_in: 100,
        tokens_out: 50,
    };
    cache.put(&cached).unwrap();

    // Cache hit
    let result = cache.get(&hash).unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.tokens_in, 100);
    assert_eq!(result.tokens_out, 50);

    // Same content → same hash
    let hash2 = Cache::compute_input_hash(
        "prompt content v1",
        &[],
        "claude-sonnet-4-5-20250929",
        0.0,
        Some(42),
        "python",
        Some("fastapi"),
    );
    assert_eq!(hash, hash2);

    // Different content → different hash
    let hash3 = Cache::compute_input_hash(
        "prompt content v2",
        &[],
        "claude-sonnet-4-5-20250929",
        0.0,
        Some(42),
        "python",
        Some("fastapi"),
    );
    assert_ne!(hash, hash3);
    assert!(cache.get(&hash3).is_none()); // Miss for new content
}

/// Test: generation record round-trip
#[test]
fn test_generation_record_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let gen_dir = dir.path().join("generations");
    std::fs::create_dir_all(&gen_dir).unwrap();

    let record = GenerationRecord {
        timestamp: chrono::Utc::now(),
        project: "test-project".to_string(),
        model: "claude-sonnet-4-5-20250929".to_string(),
        temperature: 0.0,
        seed: Some(42),
        language: "python".to_string(),
        framework: Some("fastapi".to_string()),
        prompts: vec![
            PromptRecord {
                prompt_path: PathBuf::from("prompts/hello.prompt.md"),
                output_files: vec![PathBuf::from("src/hello.py")],
                input_hash: "abc123".to_string(),
                from_cache: false,
                tokens_in: 500,
                tokens_out: 200,
                duration_ms: 3000,
                model: "claude-sonnet-4-5-20250929".to_string(),
                cost_usd: 0.005,
            },
        ],
        summary: GenerationSummary {
            total_prompts: 1,
            cache_hits: 0,
            cache_misses: 1,
            skipped: 0,
            total_tokens_in: 500,
            total_tokens_out: 200,
            total_cost_usd: 0.005,
            total_duration_ms: 3000,
            total_files_written: 1,
            patches_applied: 0,
            patches_conflicted: 0,
        },
    };

    record.write(&gen_dir).unwrap();

    // Read it back
    let records = GenerationRecord::list(&gen_dir).unwrap();
    assert_eq!(records.len(), 1);

    let read_back = &records[0];
    assert_eq!(read_back.project, "test-project");
    assert_eq!(read_back.prompts.len(), 1);
    assert_eq!(read_back.prompts[0].tokens_in, 500);
    assert_eq!(read_back.summary.total_cost_usd, 0.005);
}

/// Test: cost estimation for known models
#[test]
fn test_cost_estimation() {
    // Claude Sonnet pricing should be non-zero
    let cost = estimate_cost("claude-sonnet-4-5-20250929", 1000, 500, None);
    assert!(cost > 0.0, "Cost should be > 0 for known model");

    // Unknown model → fallback pricing
    let cost_unknown = estimate_cost("unknown-model-xyz", 1000, 500, None);
    assert!(cost_unknown > 0.0, "Should have fallback pricing");
}

/// Test: error recovery — bad prompt then fix
#[test]
fn test_error_recovery_bad_prompt() {
    let dir = tempfile::tempdir().unwrap();
    setup_lit_project(dir.path(), "error-test");

    let config = LitConfig::from_file(&dir.path().join("lit.toml")).unwrap();

    // Write a prompt with a bad import (references nonexistent prompt)
    std::fs::write(
        dir.path().join("prompts/bad.prompt.md"),
        r#"---
outputs:
  - src/bad.py
imports:
  - prompts/nonexistent.prompt.md
---

# Bad

This imports a nonexistent prompt.
"#,
    )
    .unwrap();

    // Parse succeeds (imports aren't validated at parse time)
    let paths = discover_prompts(&dir.path().join("prompts")).unwrap();
    assert_eq!(paths.len(), 1);

    let mut prompts = Vec::new();
    for p in &paths {
        prompts.push(Prompt::from_file(p, dir.path(), &config).unwrap());
    }

    // DAG build should fail — import target doesn't exist
    let dag_result = Dag::build(&prompts);
    assert!(dag_result.is_err(), "DAG should fail with missing import");
    let err_msg = dag_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("nonexistent"),
        "Error should mention missing prompt: {}",
        err_msg
    );

    // Fix the prompt by removing the bad import
    std::fs::write(
        dir.path().join("prompts/bad.prompt.md"),
        simple_prompt("src/bad.py"),
    )
    .unwrap();

    // Re-parse and re-build DAG — should succeed
    let paths = discover_prompts(&dir.path().join("prompts")).unwrap();
    let mut prompts = Vec::new();
    for p in &paths {
        prompts.push(Prompt::from_file(p, dir.path(), &config).unwrap());
    }
    let dag = Dag::build(&prompts).unwrap();
    assert_eq!(dag.len(), 1);
}

/// Test: multiple commits then log verification
#[test]
fn test_multi_commit_log() {
    let dir = tempfile::tempdir().unwrap();
    let repo = LitRepo::init(dir.path()).unwrap();
    setup_lit_project(dir.path(), "log-test");
    repo.stage_all().unwrap();
    repo.commit("init").unwrap();

    // Make 5 commits
    for i in 1..=5 {
        std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
        std::fs::write(
            dir.path().join("prompts/p.prompt.md"),
            format!("version {}", i),
        )
        .unwrap();
        repo.stage_all().unwrap();
        repo.commit(&format!("commit {}", i)).unwrap();
    }

    // Full log
    let log = repo.log(100).unwrap();
    assert_eq!(log.len(), 6); // init + 5

    // Limited log
    let log = repo.log(3).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].message, "commit 5"); // newest first
    assert_eq!(log[1].message, "commit 4");
    assert_eq!(log[2].message, "commit 3");
}

/// Test: response parser with complex multi-file output
#[test]
fn test_response_parser_complex() {
    let content = r#"=== FILE: src/__init__.py ===

=== FILE: src/models/user.py ===
from sqlalchemy import Column, Integer, String
from .base import Base

class User(Base):
    __tablename__ = "users"
    id = Column(Integer, primary_key=True)
    name = Column(String)

=== FILE: src/models/base.py ===
from sqlalchemy.ext.declarative import declarative_base

Base = declarative_base()
"#;
    let expected = vec![
        PathBuf::from("src/__init__.py"),
        PathBuf::from("src/models/user.py"),
        PathBuf::from("src/models/base.py"),
    ];
    let files = parse_response(content, &expected).unwrap();
    assert_eq!(files.len(), 3);
    assert!(files[&PathBuf::from("src/models/user.py")].contains("class User(Base)"));
    assert!(files[&PathBuf::from("src/models/base.py")].contains("declarative_base"));
}

/// Test: status detects hand-edited code.lock files
#[test]
fn test_status_detects_code_edits() {
    let dir = tempfile::tempdir().unwrap();
    let repo = LitRepo::init(dir.path()).unwrap();
    setup_lit_project(dir.path(), "code-edit-test");

    // Initial commit with generated code
    std::fs::create_dir_all(dir.path().join("code.lock/src")).unwrap();
    std::fs::write(
        dir.path().join("code.lock/src/main.py"),
        "def main():\n    pass\n",
    )
    .unwrap();
    repo.stage_all().unwrap();
    repo.commit("initial").unwrap();

    // Status should be clean
    let status = repo.status().unwrap();
    assert!(!status.has_changes());

    // Hand-edit the generated code
    std::fs::write(
        dir.path().join("code.lock/src/main.py"),
        "def main():\n    print('hand-edited')\n",
    )
    .unwrap();

    // Status should detect the modification
    let status = repo.status().unwrap();
    assert!(status.has_changes());
    assert_eq!(status.code_modified.len(), 1);
    assert!(
        status.code_modified[0]
            .to_string_lossy()
            .contains("main.py")
    );

    // Code diff should show the change
    let diff = repo.diff_code().unwrap();
    assert!(diff.contains("hand-edited"));
}

/// Test: config parsing from valid lit.toml
#[test]
fn test_config_parsing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lit.toml"), minimal_config("my-app")).unwrap();

    let config = LitConfig::from_file(&dir.path().join("lit.toml")).unwrap();
    assert_eq!(config.project.name, "my-app");
    assert_eq!(config.project.version, "0.1.0");
    assert_eq!(config.project.mapping, "manifest");
    assert_eq!(config.language.default, "python");
    assert_eq!(config.model.provider, "anthropic");
    assert_eq!(config.model.temperature, 0.0);
    assert_eq!(config.model.seed, Some(42));
}

/// Test: find_and_load walks up directories
#[test]
fn test_find_and_load_walks_up() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lit.toml"), minimal_config("walk-test")).unwrap();

    // Create a nested subdirectory
    let nested = dir.path().join("a/b/c");
    std::fs::create_dir_all(&nested).unwrap();

    // find_and_load from nested dir should find root lit.toml
    let (config, root) = LitConfig::find_and_load(&nested).unwrap();
    assert_eq!(config.project.name, "walk-test");
    assert_eq!(
        root.canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
}

/// Test: prompt parsing validates frontmatter
#[test]
fn test_prompt_parsing_validation() {
    let dir = tempfile::tempdir().unwrap();
    setup_lit_project(dir.path(), "prompt-test");

    let config = LitConfig::from_file(&dir.path().join("lit.toml")).unwrap();

    // Valid prompt
    std::fs::write(
        dir.path().join("prompts/good.prompt.md"),
        simple_prompt("src/good.py"),
    )
    .unwrap();
    let prompt = Prompt::from_file(
        &dir.path().join("prompts/good.prompt.md"),
        dir.path(),
        &config,
    )
    .unwrap();
    assert_eq!(prompt.frontmatter.outputs.len(), 1);
    assert_eq!(
        prompt.frontmatter.outputs[0],
        PathBuf::from("src/good.py")
    );
    assert!(prompt.body.contains("# Hello"));
}

/// Test: discover_prompts finds only .prompt.md files
#[test]
fn test_discover_prompts_filtering() {
    let dir = tempfile::tempdir().unwrap();
    let prompts_dir = dir.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();

    // Write various files
    std::fs::write(prompts_dir.join("a.prompt.md"), "---\noutputs:\n  - a.py\n---\n# A").unwrap();
    std::fs::write(prompts_dir.join("b.prompt.md"), "---\noutputs:\n  - b.py\n---\n# B").unwrap();
    std::fs::write(prompts_dir.join("readme.md"), "# README").unwrap();
    std::fs::write(prompts_dir.join("notes.txt"), "notes").unwrap();

    let paths = discover_prompts(&prompts_dir).unwrap();
    assert_eq!(paths.len(), 2, "Should find only .prompt.md files");
    for p in &paths {
        assert!(
            p.to_string_lossy().ends_with(".prompt.md"),
            "All discovered files should be .prompt.md: {:?}",
            p
        );
    }
}

/// Test: checkout with clean detection
#[test]
fn test_checkout_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let repo = LitRepo::init(dir.path()).unwrap();

    // Commit v1
    std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
    std::fs::write(
        dir.path().join("prompts/app.prompt.md"),
        "v1 content",
    )
    .unwrap();
    repo.stage_all().unwrap();
    let v1 = repo.commit("version 1").unwrap();

    // Commit v2
    std::fs::write(
        dir.path().join("prompts/app.prompt.md"),
        "v2 content",
    )
    .unwrap();
    repo.stage_all().unwrap();
    let v2 = repo.commit("version 2").unwrap();

    // Commit v3
    std::fs::write(
        dir.path().join("prompts/app.prompt.md"),
        "v3 content",
    )
    .unwrap();
    repo.stage_all().unwrap();
    let _v3 = repo.commit("version 3").unwrap();

    // Verify v3
    let content = std::fs::read_to_string(dir.path().join("prompts/app.prompt.md")).unwrap();
    assert_eq!(content, "v3 content");

    // Checkout v1
    repo.checkout_ref(&v1).unwrap();
    let content = std::fs::read_to_string(dir.path().join("prompts/app.prompt.md")).unwrap();
    assert_eq!(content, "v1 content");

    // Checkout v2
    repo.checkout_ref(&v2).unwrap();
    let content = std::fs::read_to_string(dir.path().join("prompts/app.prompt.md")).unwrap();
    assert_eq!(content, "v2 content");
}

/// Test: diff shows nothing when tree is clean
#[test]
fn test_diff_clean_tree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = LitRepo::init(dir.path()).unwrap();

    std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
    std::fs::write(dir.path().join("prompts/a.prompt.md"), "content").unwrap();
    repo.stage_all().unwrap();
    repo.commit("clean").unwrap();

    let diff = repo.diff_prompts().unwrap();
    assert!(diff.is_empty(), "Diff should be empty for clean tree");
}

/// Test: stage_all handles deleted files
#[test]
fn test_stage_all_deleted_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = LitRepo::init(dir.path()).unwrap();

    std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
    std::fs::write(dir.path().join("prompts/to-delete.prompt.md"), "delete me").unwrap();
    std::fs::write(dir.path().join("lit.toml"), "config").unwrap();
    repo.stage_all().unwrap();
    repo.commit("initial").unwrap();

    // Delete the prompt file
    std::fs::remove_file(dir.path().join("prompts/to-delete.prompt.md")).unwrap();

    // Status should show it as deleted
    let status = repo.status().unwrap();
    assert!(status.has_changes());
    assert_eq!(status.prompts_deleted.len(), 1);

    // Stage and commit the deletion
    repo.stage_all().unwrap();
    repo.commit("delete prompt").unwrap();

    // Should be clean again
    let status = repo.status().unwrap();
    assert!(!status.has_changes());
}
