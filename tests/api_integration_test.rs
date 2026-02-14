//! Real API integration test.
//!
//! This test makes a real call to the Anthropic API and validates the full
//! generation pipeline end-to-end. It is ignored by default — run it with:
//!
//!     cargo test -- --ignored
//!
//! Requires: LIT_API_KEY environment variable set to a valid Anthropic API key.

use std::collections::HashMap;
use std::path::PathBuf;

use lit::core::cache::Cache;
use lit::core::config::LitConfig;
use lit::core::dag::Dag;
use lit::core::generator::Generator;
use lit::core::prompt::{Prompt, discover_prompts};
use lit::providers::anthropic::AnthropicProvider;

/// Helper: create a minimal lit project with one prompt
fn setup_single_prompt_project(dir: &std::path::Path) {
    let config = r#"[project]
name = "api-test"
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
"#;
    std::fs::write(dir.join("lit.toml"), config).unwrap();
    std::fs::create_dir_all(dir.join("prompts")).unwrap();
    std::fs::create_dir_all(dir.join("code.lock")).unwrap();
    std::fs::create_dir_all(dir.join(".lit/cache")).unwrap();
}

/// Full pipeline test against the real Anthropic API.
///
/// Creates a project with one simple prompt, runs the generation pipeline,
/// and validates that:
/// 1. The API returns a valid response
/// 2. The response parser extracts the correct file
/// 3. The generated code looks like real Python
/// 4. Token counts are non-zero
/// 5. Caching works (second run hits cache)
#[tokio::test]
#[ignore] // Requires LIT_API_KEY
async fn test_real_api_single_prompt() {
    let api_key = match std::env::var("LIT_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("Skipping: LIT_API_KEY not set");
            return;
        }
    };

    let dir = tempfile::tempdir().unwrap();
    setup_single_prompt_project(dir.path());

    // Write a simple prompt
    std::fs::write(
        dir.path().join("prompts/greet.prompt.md"),
        r#"---
outputs:
  - src/greet.py
---

# Greeting Module

Create a simple Python module with:
- A function `greet(name: str) -> str` that returns "Hello, {name}!"
- A function `farewell(name: str) -> str` that returns "Goodbye, {name}!"
- Type hints on all functions
"#,
    )
    .unwrap();

    let config = LitConfig::from_file(&dir.path().join("lit.toml")).unwrap();
    let paths = discover_prompts(&dir.path().join("prompts")).unwrap();
    assert_eq!(paths.len(), 1);

    let mut prompts = Vec::new();
    for p in &paths {
        prompts.push(Prompt::from_file(p, dir.path(), &config).unwrap());
    }

    let dag = Dag::build(&prompts).unwrap();
    let prompts_map: HashMap<PathBuf, Prompt> = prompts
        .into_iter()
        .map(|p| (p.path.clone(), p))
        .collect();

    let provider = Box::new(AnthropicProvider::new(api_key));
    let generator = Generator::new(provider, config.clone());

    // Initialize cache
    let cache = Cache::new(dir.path().join(".lit/cache"));
    cache.init().unwrap();

    // First run — should be a cache miss (real API call)
    let result = generator
        .run_pipeline(
            &dag,
            &prompts_map,
            &dag.order().to_vec(),
            &HashMap::new(),
            Some(&cache),
        )
        .await
        .expect("Pipeline should succeed");

    assert_eq!(result.outputs.len(), 1, "Should have 1 output");
    assert_eq!(result.cache_misses, 1, "Should be 1 cache miss");
    assert_eq!(result.cache_hits, 0);

    let output = &result.outputs[0];
    assert!(!output.from_cache, "First run should not be from cache");
    assert!(output.tokens_in > 0, "Should have input tokens");
    assert!(output.tokens_out > 0, "Should have output tokens");
    assert!(output.files.contains_key(&PathBuf::from("src/greet.py")));

    let code = &output.files[&PathBuf::from("src/greet.py")];
    assert!(code.contains("def greet"), "Generated code should contain greet function");
    assert!(code.contains("def farewell"), "Generated code should contain farewell function");
    assert!(code.contains("str"), "Generated code should have type hints");

    eprintln!("Generated code:\n{}", code);
    eprintln!(
        "Tokens: {} in / {} out, Duration: {}ms",
        output.tokens_in, output.tokens_out, output.duration_ms
    );

    // Second run — should be a cache hit (no API call)
    let api_key2 = std::env::var("LIT_API_KEY").unwrap();
    let provider2 = Box::new(AnthropicProvider::new(api_key2));
    let generator2 = Generator::new(provider2, config);

    let result2 = generator2
        .run_pipeline(
            &dag,
            &prompts_map,
            &dag.order().to_vec(),
            &HashMap::new(),
            Some(&cache),
        )
        .await
        .expect("Second pipeline run should succeed");

    assert_eq!(result2.cache_hits, 1, "Second run should be a cache hit");
    assert_eq!(result2.cache_misses, 0, "Second run should have 0 misses");
    assert!(result2.outputs[0].from_cache, "Second run should be from cache");

    // Cached output should match
    let cached_code = &result2.outputs[0].files[&PathBuf::from("src/greet.py")];
    assert_eq!(code, cached_code, "Cached output should match original");
}
