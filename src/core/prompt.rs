use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::core::config::{LitConfig, ModelConfig};

// ---------- Public types ----------

/// A parsed prompt file
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Path to the .prompt.md file, relative to repo root
    pub path: PathBuf,
    /// Parsed frontmatter
    pub frontmatter: PromptFrontmatter,
    /// Markdown body (everything after the frontmatter)
    pub body: String,
    /// Full raw file content
    pub raw: String,
}

/// Parsed YAML frontmatter from a .prompt.md file
#[derive(Debug, Clone)]
pub struct PromptFrontmatter {
    /// Output file paths (relative to code.lock/)
    pub outputs: Vec<PathBuf>,
    /// Import paths to other .prompt.md files
    pub imports: Vec<PathBuf>,
    /// Per-prompt model override
    pub model: Option<ModelConfig>,
    /// Per-prompt language override
    pub language: Option<String>,
}

// ---------- Raw frontmatter (for YAML deserialization) ----------

/// Internal: raw YAML frontmatter before conversion to PromptFrontmatter
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    #[serde(default)]
    outputs: Vec<String>,
    #[serde(default)]
    imports: Vec<String>,
    #[serde(default)]
    model: Option<ModelConfig>,
    #[serde(default)]
    language: Option<String>,
}

// ---------- Implementation ----------

impl Prompt {
    /// Parse a prompt from a file path
    pub fn from_file(path: &Path, repo_root: &Path, config: &LitConfig) -> Result<Self> {
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            repo_root.join(path)
        };

        let raw = std::fs::read_to_string(&full_path)
            .with_context(|| format!("Failed to read prompt file: {}", full_path.display()))?;

        let relative_path = full_path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_path_buf();

        Self::parse(&raw, relative_path, config)
    }

    /// Parse a prompt from raw string content
    pub fn parse(raw: &str, path: PathBuf, config: &LitConfig) -> Result<Self> {
        let (frontmatter_str, body) = split_frontmatter(raw).with_context(|| {
            format!(
                "Failed to parse frontmatter in {}",
                path.display()
            )
        })?;

        let raw_fm: RawFrontmatter = serde_yaml::from_str(&frontmatter_str).with_context(|| {
            format!(
                "Failed to parse YAML frontmatter in {}",
                path.display()
            )
        })?;

        let frontmatter = PromptFrontmatter {
            outputs: raw_fm.outputs.into_iter().map(PathBuf::from).collect(),
            imports: raw_fm.imports.into_iter().map(PathBuf::from).collect(),
            model: raw_fm.model,
            language: raw_fm.language,
        };

        let prompt = Prompt {
            path: path.clone(),
            frontmatter,
            body: body.to_string(),
            raw: raw.to_string(),
        };

        prompt.validate(config)?;

        Ok(prompt)
    }

    /// Validate the prompt against the project config
    fn validate(&self, config: &LitConfig) -> Result<()> {
        // In manifest mode, outputs are required
        if config.project.mapping == "manifest" && self.frontmatter.outputs.is_empty() {
            bail!(
                "Prompt {} has no outputs declared. In 'manifest' mode, \
                 the 'outputs' field is required in frontmatter.",
                self.path.display()
            );
        }

        // Validate import paths end with .prompt.md
        for import in &self.frontmatter.imports {
            if import.extension().and_then(|e| e.to_str()) != Some("md") {
                bail!(
                    "Invalid import '{}' in {}. Import paths must end with .prompt.md",
                    import.display(),
                    self.path.display()
                );
            }
        }

        // Check that @import() references in body match imports in frontmatter
        let body_imports = extract_body_imports(&self.body);
        for body_import in &body_imports {
            let body_path = PathBuf::from(body_import);
            if !self.frontmatter.imports.contains(&body_path) {
                eprintln!(
                    "Warning: @import({}) found in body of {} but not declared in frontmatter imports",
                    body_import,
                    self.path.display()
                );
            }
        }

        Ok(())
    }

    /// Extract @import() references from the body text
    pub fn body_imports(&self) -> Vec<PathBuf> {
        extract_body_imports(&self.body)
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
}

// ---------- Discovery ----------

/// Discover all .prompt.md files under a directory
pub fn discover_prompts(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut prompts = Vec::new();
    discover_prompts_recursive(dir, &mut prompts)?;
    prompts.sort();
    Ok(prompts)
}

fn discover_prompts_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            discover_prompts_recursive(&path, out)?;
        } else if is_prompt_file(&path) {
            out.push(path);
        }
    }

    Ok(())
}

/// Check if a file is a .prompt.md file
pub fn is_prompt_file(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|s| s.ends_with(".prompt.md"))
}

// ---------- Frontmatter parsing ----------

/// Split a prompt file into frontmatter and body
///
/// Frontmatter is delimited by `---` on its own line at the start of the file.
fn split_frontmatter(content: &str) -> Result<(String, String)> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        bail!(
            "Prompt file must start with YAML frontmatter (---). \
             Found: {}...",
            &trimmed[..trimmed.len().min(20)]
        );
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    // Skip the rest of the first --- line
    let after_newline = after_first
        .find('\n')
        .map(|i| &after_first[i + 1..])
        .unwrap_or(after_first);

    // Find the closing ---
    let closing_pos = after_newline
        .find("\n---")
        .or_else(|| {
            // Handle case where --- is at the very start of remaining content
            if after_newline.starts_with("---") {
                Some(0)
            } else {
                None
            }
        });

    match closing_pos {
        Some(pos) => {
            let frontmatter = after_newline[..pos].to_string();
            let rest = &after_newline[pos..];
            // Skip the closing --- line
            let body = rest
                .find('\n')
                .map(|i| &rest[i + 1..])
                .unwrap_or("");
            // Skip the second newline after ---
            let body = body
                .find('\n')
                .map(|i| &body[i + 1..])
                .unwrap_or(body);
            Ok((frontmatter, body.to_string()))
        }
        None => {
            bail!("Unterminated frontmatter: missing closing ---");
        }
    }
}

/// Extract @import(...) references from prompt body text
fn extract_body_imports(body: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut remaining = body;

    while let Some(start) = remaining.find("@import(") {
        let after_import = &remaining[start + 8..];
        if let Some(end) = after_import.find(')') {
            let path = after_import[..end].trim().to_string();
            if !path.is_empty() {
                imports.push(path);
            }
        }
        remaining = &remaining[start + 8..];
    }

    imports
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> LitConfig {
        LitConfig::from_str(
            r#"
[project]
name = "test"
version = "0.1.0"
mapping = "manifest"

[language]
default = "python"
version = "3.12"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
"#,
        )
        .unwrap()
    }

    #[test]
    fn test_parse_simple_prompt() {
        let raw = r#"---
outputs:
  - src/models/user.py
imports:
  - prompts/models/base.prompt.md
---

# User Model

Create a User model.
"#;
        let config = test_config();
        let prompt = Prompt::parse(raw, PathBuf::from("prompts/models/user.prompt.md"), &config).unwrap();

        assert_eq!(prompt.path, PathBuf::from("prompts/models/user.prompt.md"));
        assert_eq!(prompt.frontmatter.outputs, vec![PathBuf::from("src/models/user.py")]);
        assert_eq!(
            prompt.frontmatter.imports,
            vec![PathBuf::from("prompts/models/base.prompt.md")]
        );
        assert!(prompt.body.contains("# User Model"));
        assert!(prompt.body.contains("Create a User model."));
    }

    #[test]
    fn test_parse_prompt_with_multiple_outputs() {
        let raw = r#"---
outputs:
  - src/models/user.py
  - tests/test_user.py
imports: []
---

# User Model + Tests
"#;
        let config = test_config();
        let prompt = Prompt::parse(raw, PathBuf::from("prompts/user.prompt.md"), &config).unwrap();

        assert_eq!(prompt.frontmatter.outputs.len(), 2);
        assert_eq!(prompt.frontmatter.outputs[0], PathBuf::from("src/models/user.py"));
        assert_eq!(prompt.frontmatter.outputs[1], PathBuf::from("tests/test_user.py"));
        assert!(prompt.frontmatter.imports.is_empty());
    }

    #[test]
    fn test_parse_prompt_with_multiple_imports() {
        let raw = r#"---
outputs:
  - src/api/items.py
imports:
  - prompts/models/item.prompt.md
  - prompts/models/user.prompt.md
  - prompts/schemas/item.prompt.md
  - prompts/config/database.prompt.md
---

# Item endpoints
"#;
        let config = test_config();
        let prompt = Prompt::parse(raw, PathBuf::from("prompts/api/items.prompt.md"), &config).unwrap();

        assert_eq!(prompt.frontmatter.imports.len(), 4);
    }

    #[test]
    fn test_parse_prompt_no_imports() {
        let raw = r#"---
outputs:
  - src/config/database.py
imports: []
---

# Database config
"#;
        let config = test_config();
        let prompt = Prompt::parse(raw, PathBuf::from("prompts/config/database.prompt.md"), &config).unwrap();

        assert!(prompt.frontmatter.imports.is_empty());
        assert_eq!(prompt.frontmatter.outputs.len(), 1);
    }

    #[test]
    fn test_parse_prompt_no_outputs_in_manifest_mode_fails() {
        let raw = r#"---
outputs: []
imports: []
---

# Empty prompt
"#;
        let config = test_config(); // manifest mode
        let err = Prompt::parse(raw, PathBuf::from("prompts/empty.prompt.md"), &config).unwrap_err();
        assert!(
            err.to_string().contains("no outputs"),
            "Expected no outputs error, got: {}",
            err
        );
    }

    #[test]
    fn test_parse_prompt_no_outputs_in_direct_mode_ok() {
        let raw = r#"---
imports: []
---

# A prompt in direct mode
"#;
        let config = LitConfig::from_str(
            r#"
[project]
name = "test"
version = "0.1.0"
mapping = "direct"

[language]
default = "python"
version = "3.12"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
"#,
        )
        .unwrap();

        let prompt = Prompt::parse(raw, PathBuf::from("prompts/foo.prompt.md"), &config);
        assert!(prompt.is_ok());
    }

    #[test]
    fn test_parse_prompt_with_model_override() {
        let raw = r#"---
outputs:
  - src/complex.py
imports: []
model:
  provider: anthropic
  model: claude-opus-4-6
  temperature: 0.0
---

# Complex logic
"#;
        let config = test_config();
        let prompt = Prompt::parse(raw, PathBuf::from("prompts/complex.prompt.md"), &config).unwrap();

        let model = prompt.frontmatter.model.unwrap();
        assert_eq!(model.model, "claude-opus-4-6");
    }

    #[test]
    fn test_parse_prompt_with_language_override() {
        let raw = r#"---
outputs:
  - src/frontend/app.ts
imports: []
language: typescript
---

# Frontend
"#;
        let config = test_config();
        let prompt = Prompt::parse(raw, PathBuf::from("prompts/frontend.prompt.md"), &config).unwrap();

        assert_eq!(prompt.frontmatter.language.as_deref(), Some("typescript"));
    }

    #[test]
    fn test_missing_frontmatter() {
        let raw = "# No frontmatter here\n\nJust a regular markdown file.\n";
        let config = test_config();
        let err = Prompt::parse(raw, PathBuf::from("prompts/bad.prompt.md"), &config).unwrap_err();
        assert!(
            err.to_string().contains("frontmatter"),
            "Expected frontmatter error, got: {}",
            err
        );
    }

    #[test]
    fn test_unterminated_frontmatter() {
        let raw = "---\noutputs:\n  - foo.py\n\n# Body without closing ---\n";
        let config = test_config();
        let err = Prompt::parse(raw, PathBuf::from("prompts/bad.prompt.md"), &config).unwrap_err();
        assert!(
            err.to_string().contains("frontmatter"),
            "Expected frontmatter error, got: {}",
            err
        );
    }

    #[test]
    fn test_extract_body_imports() {
        let body = r#"
Use the Base class from @import(prompts/models/base.prompt.md).
Also use @import(prompts/config/database.prompt.md) for DB access.
"#;
        let imports = extract_body_imports(body);
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0], "prompts/models/base.prompt.md");
        assert_eq!(imports[1], "prompts/config/database.prompt.md");
    }

    #[test]
    fn test_extract_body_imports_none() {
        let body = "# Just a heading\n\nNo imports here.\n";
        let imports = extract_body_imports(body);
        assert!(imports.is_empty());
    }

    #[test]
    fn test_is_prompt_file() {
        assert!(is_prompt_file(Path::new("prompts/models/user.prompt.md")));
        assert!(is_prompt_file(Path::new("foo.prompt.md")));
        assert!(!is_prompt_file(Path::new("README.md")));
        assert!(!is_prompt_file(Path::new("foo.rs")));
        assert!(!is_prompt_file(Path::new("prompt.md"))); // missing the .prompt. part
    }

    #[test]
    fn test_split_frontmatter() {
        let content = "---\nkey: value\n---\n\n# Body\n";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm.trim(), "key: value");
        assert!(body.contains("# Body"));
    }
}
