use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use colored::Colorize;

use crate::core::cache::Cache;
use crate::core::config::LitConfig;
use crate::core::dag::Dag;
use crate::core::prompt::Prompt;
use crate::core::style;
use crate::providers::{GenerationRequest, GenerationResponse, LlmProvider};

// ---------- Public types ----------

/// Result of generating code from a single prompt
#[derive(Debug, Clone)]
pub struct GenerationOutput {
    /// Prompt that was generated
    pub prompt_path: PathBuf,
    /// Output file path → generated content
    pub files: HashMap<PathBuf, String>,
    /// Input tokens consumed
    pub tokens_in: u64,
    /// Output tokens generated
    pub tokens_out: u64,
    /// Generation time in milliseconds
    pub duration_ms: u64,
    /// Model that was used
    pub model: String,
    /// Whether this result came from cache
    pub from_cache: bool,
    /// Input hash for caching
    pub input_hash: String,
}

/// Result of running the full pipeline
#[derive(Debug)]
pub struct PipelineResult {
    /// All generation outputs, in DAG order
    pub outputs: Vec<GenerationOutput>,
    /// Total input tokens
    pub total_tokens_in: u64,
    /// Total output tokens
    pub total_tokens_out: u64,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
    /// Prompts that were skipped (not in regen set)
    pub skipped: Vec<PathBuf>,
    /// Number of cache hits
    pub cache_hits: usize,
    /// Number of cache misses (fresh LLM calls)
    pub cache_misses: usize,
}

/// The code generation pipeline
pub struct Generator {
    provider: Box<dyn LlmProvider>,
    config: LitConfig,
}

// ---------- Implementation ----------

impl Generator {
    pub fn new(provider: Box<dyn LlmProvider>, config: LitConfig) -> Self {
        Self { provider, config }
    }

    /// Generate code from a single prompt.
    ///
    /// `context` is a map of import path → generated code content from upstream prompts.
    pub async fn generate_prompt(
        &self,
        prompt: &Prompt,
        context: &HashMap<PathBuf, String>,
    ) -> Result<GenerationOutput> {
        let start = Instant::now();

        // Assemble the system prompt
        let system_prompt = self.build_system_prompt(prompt);

        // Assemble context from imported prompts
        let context_str = self.build_context(prompt, context);

        // Resolve model config (per-prompt override or project default)
        let (model, temperature, seed) = self.resolve_model_config(prompt);

        let request = GenerationRequest {
            system_prompt,
            context: context_str,
            user_prompt: prompt.body.clone(),
            model: model.clone(),
            temperature,
            seed,
        };

        let response: GenerationResponse = self
            .provider
            .generate(request)
            .await
            .with_context(|| format!("Failed to generate code for {}", prompt.path.display()))?;

        // Parse response into files
        let files = parse_response(&response.content, &prompt.frontmatter.outputs)?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(GenerationOutput {
            prompt_path: prompt.path.clone(),
            files,
            tokens_in: response.tokens_in,
            tokens_out: response.tokens_out,
            duration_ms,
            model: response.model,
            from_cache: false,
            input_hash: String::new(), // filled in by run_pipeline
        })
    }

    /// Run the full generation pipeline across the DAG.
    ///
    /// Generates prompts in topological order.
    /// `regeneration_set` specifies which prompts to actually generate
    /// (others are skipped and their existing output is used as context).
    /// If `cache` is Some, the pipeline will check/store results in the cache.
    pub async fn run_pipeline(
        &self,
        dag: &Dag,
        prompts: &HashMap<PathBuf, Prompt>,
        regeneration_set: &[PathBuf],
        existing_code: &HashMap<PathBuf, String>,
        cache: Option<&Cache>,
    ) -> Result<PipelineResult> {
        let pipeline_start = Instant::now();

        // Map of output file → generated content (accumulated as we go)
        let mut generated_code: HashMap<PathBuf, String> = existing_code.clone();
        let mut outputs = Vec::new();
        let mut skipped = Vec::new();
        let mut total_tokens_in = 0u64;
        let mut total_tokens_out = 0u64;
        let mut cache_hits = 0usize;
        let mut cache_misses = 0usize;

        // Map of prompt path → input hash (so downstream prompts can include
        // their imports' hashes for cascading invalidation)
        let mut input_hashes: HashMap<PathBuf, String> = HashMap::new();

        let regen_set: std::collections::HashSet<&PathBuf> =
            regeneration_set.iter().collect();

        let language = &self.config.language.default;
        let framework = self.config.framework.as_ref().map(|fw| fw.name.as_str());

        for prompt_path in dag.order() {
            let prompt = prompts
                .get(prompt_path)
                .with_context(|| format!("Prompt {} not found in prompts map", prompt_path.display()))?;

            // Compute input hash for this prompt (needed for both cache lookup
            // and for downstream prompts that import this one)
            let (model, temperature, seed) = self.resolve_model_config(prompt);

            // Collect import hashes: for each import, use its computed input hash
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

            // Store the hash for downstream use regardless of whether we're in the regen set
            input_hashes.insert(prompt_path.clone(), input_hash.clone());

            // Track position for progress display
            let prompt_index = outputs.len() + skipped.len() + 1;
            let prompt_total = dag.order().len();

            if !regen_set.contains(prompt_path) {
                skipped.push(prompt_path.clone());
                continue;
            }

            // Check cache
            if let Some(c) = cache {
                if let Some(cached) = c.get(&input_hash) {
                    eprintln!(
                        "  {} {} {} {}",
                        "✓".green().bold(),
                        prompt.path.display(),
                        "(cached)".dimmed(),
                        style::progress(prompt_index, prompt_total)
                    );

                    // Store cached files for downstream prompts
                    for (path, content) in &cached.files {
                        generated_code.insert(path.clone(), content.clone());
                    }

                    outputs.push(GenerationOutput {
                        prompt_path: prompt.path.clone(),
                        files: cached.files,
                        tokens_in: cached.tokens_in,
                        tokens_out: cached.tokens_out,
                        duration_ms: 0,
                        model: model.clone(),
                        from_cache: true,
                        input_hash: input_hash.clone(),
                    });

                    cache_hits += 1;
                    continue;
                }
            }

            // Cache miss — call the LLM
            cache_misses += 1;

            // Build context from imports: for each import, gather its output files
            let mut context: HashMap<PathBuf, String> = HashMap::new();
            for import_path in &prompt.frontmatter.imports {
                if let Some(import_prompt) = prompts.get(import_path) {
                    for output in &import_prompt.frontmatter.outputs {
                        if let Some(code) = generated_code.get(output) {
                            context.insert(output.clone(), code.clone());
                        }
                    }
                }
            }

            eprintln!(
                "  {} {} {} {}",
                "Generating".cyan(),
                prompt.path.display().to_string().bold(),
                format!("({} context file(s))", context.len()).dimmed(),
                style::progress(prompt_index, prompt_total)
            );

            let mut output = self.generate_prompt(prompt, &context).await?;
            output.input_hash = input_hash.clone();

            // Store generated files for downstream prompts to use as context
            for (path, content) in &output.files {
                generated_code.insert(path.clone(), content.clone());
            }

            // Store in cache
            if let Some(c) = cache {
                let cache_entry = crate::core::cache::CachedGeneration {
                    input_hash: input_hash.clone(),
                    files: output.files.clone(),
                    tokens_in: output.tokens_in,
                    tokens_out: output.tokens_out,
                };
                if let Err(e) = c.put(&cache_entry) {
                    eprintln!("    {} {}", "⚠".yellow().bold(), format!("Failed to write cache: {}", e).dimmed());
                }
            }

            total_tokens_in += output.tokens_in;
            total_tokens_out += output.tokens_out;

            eprintln!(
                "    {} {} {}, {}",
                "✓".green().bold(),
                format!("{} file(s)", output.files.len()).bold(),
                format!("{} in / {} out tokens", output.tokens_in, output.tokens_out).dimmed(),
                format!("{:.1}s", output.duration_ms as f64 / 1000.0).dimmed()
            );

            outputs.push(output);
        }

        let total_duration_ms = pipeline_start.elapsed().as_millis() as u64;

        Ok(PipelineResult {
            outputs,
            total_tokens_in,
            total_tokens_out,
            total_duration_ms,
            skipped,
            cache_hits,
            cache_misses,
        })
    }

    // ---------- Internal ----------

    fn build_system_prompt(&self, prompt: &Prompt) -> String {
        let language = prompt
            .frontmatter
            .language
            .as_deref()
            .unwrap_or(&self.config.language.default);

        let lang_version = &self.config.language.version;

        let framework_str = self
            .config
            .framework
            .as_ref()
            .map(|fw| format!("Framework: {} {}\n", fw.name, fw.version))
            .unwrap_or_default();

        // List the declared output file paths so the LLM knows exactly what to produce
        let outputs_str = prompt
            .frontmatter
            .outputs
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "You are a code generator. You generate production-quality code based on the prompt provided.\n\
             \n\
             Language: {} {}\n\
             {}\
             Rules:\n\
             - Output ONLY raw code, no explanations or commentary\n\
             - Do NOT wrap code in markdown code fences (no ``` or ```python etc.)\n\
             - Use the exact output file format specified below\n\
             - Each output file must be wrapped in a file delimiter\n\
             - Match the coding conventions of the language and framework\n\
             - Include proper imports, type hints, and error handling\n\
             \n\
             Declared output file(s):\n\
             {}\n\
             \n\
             Output format:\n\
             For each file, use this exact delimiter format:\n\
             \n\
             === FILE: path/to/file.ext ===\n\
             <file content here>\n\
             \n\
             You MUST use the EXACT file paths listed above as declared outputs.\n\
             Do not invent your own file paths — use the paths exactly as shown.\n\
             Do not include any text before the first === FILE: === delimiter or after the last file's content.",
            language, lang_version, framework_str, outputs_str
        )
    }

    fn build_context(
        &self,
        _prompt: &Prompt,
        context: &HashMap<PathBuf, String>,
    ) -> String {
        if context.is_empty() {
            return String::new();
        }

        let mut parts = Vec::new();
        for (path, code) in context {
            parts.push(format!(
                "### {}\n```\n{}\n```",
                path.display(),
                code
            ));
        }
        parts.join("\n\n")
    }

    fn resolve_model_config(&self, prompt: &Prompt) -> (String, f64, Option<u64>) {
        if let Some(ref model_override) = prompt.frontmatter.model {
            (
                model_override.model.clone(),
                model_override.temperature,
                model_override.seed,
            )
        } else {
            (
                self.config.model.model.clone(),
                self.config.model.temperature,
                self.config.model.seed,
            )
        }
    }
}

// ---------- Response parser ----------

/// Strip markdown code fences from LLM output.
///
/// LLMs often wrap code in ```python ... ``` even when told not to.
/// This strips the opening fence (```lang or ```) and closing fence (```).
fn strip_markdown_fences(content: &str) -> String {
    let trimmed = content.trim();
    let lines: Vec<&str> = trimmed.lines().collect();

    if lines.is_empty() {
        return content.to_string();
    }

    let first = lines[0].trim();
    let last = lines.last().map(|l| l.trim()).unwrap_or("");

    // Check if wrapped in code fences
    let starts_with_fence = first.starts_with("```");
    let ends_with_fence = last == "```" && lines.len() > 1;

    if starts_with_fence && ends_with_fence {
        // Strip both fences
        let inner = &lines[1..lines.len() - 1];
        let result = inner.join("\n");
        if result.ends_with('\n') {
            result
        } else {
            format!("{}\n", result)
        }
    } else if starts_with_fence && !ends_with_fence {
        // Only opening fence (unusual but handle it)
        let inner = &lines[1..];
        let result = inner.join("\n");
        if result.ends_with('\n') {
            result
        } else {
            format!("{}\n", result)
        }
    } else if !starts_with_fence && ends_with_fence {
        // Only closing fence
        let inner = &lines[..lines.len() - 1];
        let result = inner.join("\n");
        if result.ends_with('\n') {
            result
        } else {
            format!("{}\n", result)
        }
    } else {
        content.to_string()
    }
}

/// Parse an LLM response into a map of file path → content.
///
/// Expected format:
/// ```text
/// === FILE: src/models/user.py ===
/// class User:
///     ...
///
/// === FILE: tests/test_user.py ===
/// def test_user():
///     ...
/// ```
pub fn parse_response(
    content: &str,
    expected_outputs: &[PathBuf],
) -> Result<HashMap<PathBuf, String>> {
    let mut files: HashMap<PathBuf, String> = HashMap::new();
    let delimiter = "=== FILE:";

    // Find all file sections
    let mut remaining = content;
    let mut sections: Vec<(PathBuf, String)> = Vec::new();

    while let Some(start) = remaining.find(delimiter) {
        let after_delim = &remaining[start + delimiter.len()..];

        // Find the end of the delimiter line (=== at the end)
        let line_end = after_delim.find('\n').unwrap_or(after_delim.len());
        let header_line = after_delim[..line_end].trim();

        // Extract file path (strip trailing ===)
        let file_path = header_line
            .trim_end_matches("===")
            .trim()
            .to_string();

        if file_path.is_empty() {
            remaining = &after_delim[line_end..];
            continue;
        }

        // Content is everything after this header until the next delimiter (or end)
        let content_start = if line_end < after_delim.len() {
            line_end + 1
        } else {
            line_end
        };
        let rest = &after_delim[content_start..];

        let content_end = rest.find(delimiter).unwrap_or(rest.len());
        let file_content = rest[..content_end].to_string();

        // Trim leading/trailing blank lines but preserve internal whitespace
        let trimmed = file_content.trim_matches('\n').to_string();
        // Strip markdown code fences if the LLM wrapped the code
        let defenced = strip_markdown_fences(&trimmed);
        // Ensure file ends with a newline
        let final_content = if defenced.ends_with('\n') {
            defenced
        } else {
            format!("{}\n", defenced)
        };

        sections.push((PathBuf::from(&file_path), final_content));

        remaining = &rest[content_end..];
    }

    if sections.is_empty() {
        // No delimiters found — maybe the LLM returned raw code.
        // If there's exactly one expected output, use the entire content.
        if expected_outputs.len() == 1 {
            let trimmed = content.trim().to_string();
            let defenced = strip_markdown_fences(&trimmed);
            let final_content = if defenced.ends_with('\n') {
                defenced
            } else {
                format!("{}\n", defenced)
            };
            files.insert(expected_outputs[0].clone(), final_content);
            return Ok(files);
        }

        bail!(
            "LLM response did not contain any === FILE: ... === delimiters.\n\
             Expected files: [{}]\n\
             Response starts with: {}...",
            expected_outputs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            &content[..content.len().min(200)]
        );
    }

    // Remap LLM paths to expected output paths.
    // If there's a 1:1 match between sections and expected outputs,
    // use the declared output paths (the LLM may have invented its own).
    if sections.len() == expected_outputs.len() {
        // Check if the LLM used the expected paths exactly
        let all_match = sections
            .iter()
            .all(|(path, _)| expected_outputs.contains(path));

        if !all_match {
            // LLM returned different paths — remap by position.
            // The order of sections matches the order of expected outputs.
            eprintln!(
                "    Note: remapping LLM file paths to declared outputs"
            );
            for (i, (llm_path, _)) in sections.iter().enumerate() {
                if llm_path != &expected_outputs[i] {
                    eprintln!(
                        "      {} → {}",
                        llm_path.display(),
                        expected_outputs[i].display()
                    );
                }
            }
            // Replace LLM paths with expected paths
            for (i, section) in sections.iter_mut().enumerate() {
                section.0 = expected_outputs[i].clone();
            }
        }
    }

    // Collect into files map
    for (path, content) in sections {
        files.insert(path, content);
    }

    // Check that all expected outputs were produced (warn but don't fail)
    for expected in expected_outputs {
        if !files.contains_key(expected) {
            eprintln!(
                "    Warning: expected output {} not found in LLM response",
                expected.display()
            );
        }
    }

    Ok(files)
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_file_response() {
        let content = r#"=== FILE: src/models/user.py ===
class User:
    id: int
    name: str

    def __init__(self, id: int, name: str):
        self.id = id
        self.name = name
"#;
        let expected = vec![PathBuf::from("src/models/user.py")];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files.contains_key(&PathBuf::from("src/models/user.py")));
        let code = &files[&PathBuf::from("src/models/user.py")];
        assert!(code.contains("class User:"));
        assert!(code.contains("def __init__"));
    }

    #[test]
    fn test_parse_multi_file_response() {
        let content = r#"=== FILE: src/models/user.py ===
class User:
    pass

=== FILE: tests/test_user.py ===
def test_user():
    user = User()
    assert user is not None
"#;
        let expected = vec![
            PathBuf::from("src/models/user.py"),
            PathBuf::from("tests/test_user.py"),
        ];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 2);
        assert!(files[&PathBuf::from("src/models/user.py")].contains("class User:"));
        assert!(files[&PathBuf::from("tests/test_user.py")].contains("def test_user"));
    }

    #[test]
    fn test_parse_response_no_delimiters_single_output() {
        // If LLM just returns raw code and there's one expected output, accept it
        let content = "class User:\n    pass\n";
        let expected = vec![PathBuf::from("src/models/user.py")];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[&PathBuf::from("src/models/user.py")].contains("class User:"));
    }

    #[test]
    fn test_parse_response_no_delimiters_multi_output_fails() {
        let content = "some code here";
        let expected = vec![
            PathBuf::from("src/a.py"),
            PathBuf::from("src/b.py"),
        ];
        let err = parse_response(content, &expected).unwrap_err();
        assert!(
            err.to_string().contains("did not contain any"),
            "Expected delimiter error, got: {}",
            err
        );
    }

    #[test]
    fn test_parse_response_with_preamble() {
        // Some LLMs add text before the first delimiter
        let content = r#"Here's the generated code:

=== FILE: src/app.py ===
from fastapi import FastAPI

app = FastAPI()
"#;
        let expected = vec![PathBuf::from("src/app.py")];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[&PathBuf::from("src/app.py")].contains("FastAPI"));
    }

    #[test]
    fn test_parse_response_preserves_content() {
        let content = r#"=== FILE: src/config/database.py ===
import os
from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

DATABASE_URL = os.environ.get("DATABASE_URL", "sqlite:///./test.db")

engine = create_engine(DATABASE_URL)
SessionLocal = sessionmaker(autocommit=False, autoflush=False, bind=engine)

def get_db():
    db = SessionLocal()
    try:
        yield db
    finally:
        db.close()
"#;
        let expected = vec![PathBuf::from("src/config/database.py")];
        let files = parse_response(content, &expected).unwrap();

        let code = &files[&PathBuf::from("src/config/database.py")];
        assert!(code.contains("import os"));
        assert!(code.contains("def get_db():"));
        assert!(code.contains("yield db"));
        assert!(code.ends_with('\n'));
    }

    #[test]
    fn test_parse_response_trims_file_endings() {
        // File content should end with exactly one newline
        let content = "=== FILE: src/a.py ===\ncode\n\n\n";
        let expected = vec![PathBuf::from("src/a.py")];
        let files = parse_response(content, &expected).unwrap();

        let code = &files[&PathBuf::from("src/a.py")];
        assert_eq!(code, "code\n");
    }

    #[test]
    fn test_parse_response_extra_file() {
        // LLM produces a file not in expected outputs — should still include it
        // (section count != expected count, so no remapping)
        let content = r#"=== FILE: src/a.py ===
code_a

=== FILE: src/bonus.py ===
bonus_code
"#;
        let expected = vec![PathBuf::from("src/a.py")];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 2);
        assert!(files.contains_key(&PathBuf::from("src/a.py")));
        assert!(files.contains_key(&PathBuf::from("src/bonus.py")));
    }

    #[test]
    fn test_parse_response_remaps_wrong_path_single() {
        // LLM uses a different path than declared — should remap to expected
        let content = r#"=== FILE: app/database.py ===
from sqlalchemy import create_engine
engine = create_engine("sqlite:///./test.db")
"#;
        let expected = vec![PathBuf::from("src/config/database.py")];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 1);
        // Should be remapped to the declared output path
        assert!(
            files.contains_key(&PathBuf::from("src/config/database.py")),
            "File should be remapped from app/database.py to src/config/database.py"
        );
        assert!(files[&PathBuf::from("src/config/database.py")].contains("create_engine"));
    }

    #[test]
    fn test_parse_response_remaps_wrong_paths_multi() {
        // LLM uses different paths for multiple files — remap by position
        let content = r#"=== FILE: app/models/user.py ===
class User:
    pass

=== FILE: app/tests/test_user.py ===
def test_user():
    pass
"#;
        let expected = vec![
            PathBuf::from("src/models/user.py"),
            PathBuf::from("tests/test_user.py"),
        ];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 2);
        assert!(files.contains_key(&PathBuf::from("src/models/user.py")));
        assert!(files.contains_key(&PathBuf::from("tests/test_user.py")));
        assert!(files[&PathBuf::from("src/models/user.py")].contains("class User:"));
        assert!(files[&PathBuf::from("tests/test_user.py")].contains("def test_user"));
    }

    #[test]
    fn test_parse_response_no_remap_when_correct() {
        // LLM uses the exact declared paths — no remapping needed
        let content = r#"=== FILE: src/config/database.py ===
from sqlalchemy import create_engine
"#;
        let expected = vec![PathBuf::from("src/config/database.py")];
        let files = parse_response(content, &expected).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files.contains_key(&PathBuf::from("src/config/database.py")));
    }

    // --- strip_markdown_fences tests ---

    #[test]
    fn test_strip_fences_both() {
        let input = "```python\nimport os\nprint('hello')\n```";
        let result = strip_markdown_fences(input);
        assert_eq!(result, "import os\nprint('hello')\n");
    }

    #[test]
    fn test_strip_fences_bare_backticks() {
        let input = "```\nimport os\n```";
        let result = strip_markdown_fences(input);
        assert_eq!(result, "import os\n");
    }

    #[test]
    fn test_strip_fences_trailing_only() {
        let input = "import os\nprint('hello')\n```";
        let result = strip_markdown_fences(input);
        assert_eq!(result, "import os\nprint('hello')\n");
    }

    #[test]
    fn test_strip_fences_none() {
        let input = "import os\nprint('hello')\n";
        let result = strip_markdown_fences(input);
        assert_eq!(result, "import os\nprint('hello')\n");
    }

    #[test]
    fn test_strip_fences_in_delimited_response() {
        // Simulates what the LLM actually does — wraps code in fences inside FILE delimiters
        let content = "=== FILE: src/config/database.py ===\n```python\nimport os\nengine = create_engine()\n```\n";
        let expected = vec![PathBuf::from("src/config/database.py")];
        let files = parse_response(content, &expected).unwrap();

        let code = &files[&PathBuf::from("src/config/database.py")];
        assert!(!code.contains("```"), "Markdown fences should be stripped");
        assert!(code.contains("import os"));
        assert!(code.contains("create_engine"));
    }

    #[test]
    fn test_strip_fences_in_fallback_response() {
        // No delimiters, single output, but LLM wraps in fences
        let content = "```python\nimport os\ndef main():\n    pass\n```";
        let expected = vec![PathBuf::from("src/main.py")];
        let files = parse_response(content, &expected).unwrap();

        let code = &files[&PathBuf::from("src/main.py")];
        assert!(!code.contains("```"), "Markdown fences should be stripped");
        assert!(code.contains("import os"));
        assert!(code.contains("def main"));
    }
}
