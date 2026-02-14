use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Valid mapping modes for prompt → code file mapping
const VALID_MAPPING_MODES: &[&str] = &["direct", "manifest", "modular", "inferred"];

/// A static file entry: path → content (written as-is, no LLM needed)
#[derive(Debug, Clone, Deserialize)]
pub struct StaticFile {
    pub path: String,
    #[serde(default = "default_static_content")]
    pub content: String,
}

fn default_static_content() -> String {
    String::new()
}

/// Project configuration from lit.toml
#[derive(Debug, Clone, Deserialize)]
pub struct LitConfig {
    pub project: ProjectConfig,
    pub language: LanguageConfig,
    pub framework: Option<FrameworkConfig>,
    pub model: ModelConfig,
    #[serde(default)]
    pub r#static: Vec<StaticFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub version: String,
    pub mapping: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LanguageConfig {
    pub default: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FrameworkConfig {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    pub temperature: f64,
    pub seed: Option<u64>,
    pub api: Option<ApiConfig>,
    pub pricing: Option<PricingConfig>,
}

/// Optional per-million-token pricing override.
///
/// When set in `lit.toml` under `[model.pricing]`, these values override
/// the built-in pricing defaults for cost estimation.
///
/// ```toml
/// [model.pricing]
/// input_per_million = 3.0
/// output_per_million = 15.0
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct PricingConfig {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    pub key_env: String,
}

impl LitConfig {
    /// Load and validate configuration from a lit.toml file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        Self::from_str(&content)
    }

    /// Parse and validate configuration from a TOML string
    pub fn from_str(content: &str) -> Result<Self> {
        let config: LitConfig =
            toml::from_str(content).context("Failed to parse lit.toml")?;

        config.validate()?;
        Ok(config)
    }

    /// Find and load lit.toml by walking up from the given directory
    pub fn find_and_load(start_dir: &Path) -> Result<(Self, PathBuf)> {
        let mut current = start_dir.to_path_buf();
        loop {
            let config_path = current.join("lit.toml");
            if config_path.exists() {
                let config = Self::from_file(&config_path)?;
                return Ok((config, current));
            }
            if !current.pop() {
                bail!(
                    "Not a lit repository: lit.toml not found in {} or any parent directory",
                    start_dir.display()
                );
            }
        }
    }

    /// Validate the configuration
    fn validate(&self) -> Result<()> {
        // Validate mapping mode
        if !VALID_MAPPING_MODES.contains(&self.project.mapping.as_str()) {
            bail!(
                "Invalid mapping mode '{}' in lit.toml. Must be one of: {}",
                self.project.mapping,
                VALID_MAPPING_MODES.join(", ")
            );
        }

        // Validate temperature range
        if self.model.temperature < 0.0 || self.model.temperature > 2.0 {
            bail!(
                "Invalid temperature {} in lit.toml. Must be between 0.0 and 2.0",
                self.model.temperature
            );
        }

        // Validate provider
        let valid_providers = &["anthropic", "openai"];
        if !valid_providers.contains(&self.model.provider.as_str()) {
            bail!(
                "Invalid model provider '{}' in lit.toml. Must be one of: {}",
                self.model.provider,
                valid_providers.join(", ")
            );
        }

        Ok(())
    }

    /// Resolve the API key from the environment variable specified in config
    pub fn resolve_api_key(&self) -> Result<String> {
        let key_env = self
            .model
            .api
            .as_ref()
            .map(|api| api.key_env.as_str())
            .unwrap_or("LIT_API_KEY");

        std::env::var(key_env).with_context(|| {
            format!(
                "API key not found. Set the {} environment variable.\n\
                 Hint: export {}=your-api-key",
                key_env, key_env
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = r#"
[project]
name = "test-app"
version = "0.1.0"
mapping = "manifest"

[language]
default = "python"
version = "3.12"

[framework]
name = "fastapi"
version = "0.100"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
seed = 42

[model.api]
key_env = "LIT_API_KEY"
"#;

    #[test]
    fn test_parse_valid_config() {
        let config = LitConfig::from_str(VALID_CONFIG).unwrap();
        assert_eq!(config.project.name, "test-app");
        assert_eq!(config.project.version, "0.1.0");
        assert_eq!(config.project.mapping, "manifest");
        assert_eq!(config.language.default, "python");
        assert_eq!(config.language.version, "3.12");
        assert_eq!(config.model.provider, "anthropic");
        assert_eq!(config.model.model, "claude-sonnet-4-5-20250929");
        assert_eq!(config.model.temperature, 0.0);
        assert_eq!(config.model.seed, Some(42));

        let framework = config.framework.unwrap();
        assert_eq!(framework.name, "fastapi");
        assert_eq!(framework.version, "0.100");

        let api = config.model.api.unwrap();
        assert_eq!(api.key_env, "LIT_API_KEY");
    }

    #[test]
    fn test_parse_config_without_framework() {
        let toml = r#"
[project]
name = "test"
version = "0.1.0"
mapping = "direct"

[language]
default = "rust"
version = "1.75"

[model]
provider = "openai"
model = "gpt-4"
temperature = 0.5
"#;
        let config = LitConfig::from_str(toml).unwrap();
        assert!(config.framework.is_none());
        assert!(config.model.seed.is_none());
        assert!(config.model.api.is_none());
    }

    #[test]
    fn test_invalid_mapping_mode() {
        let toml = r#"
[project]
name = "test"
version = "0.1.0"
mapping = "invalid_mode"

[language]
default = "python"
version = "3.12"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
"#;
        let err = LitConfig::from_str(toml).unwrap_err();
        assert!(
            err.to_string().contains("Invalid mapping mode"),
            "Expected mapping mode error, got: {}",
            err
        );
    }

    #[test]
    fn test_invalid_temperature() {
        let toml = r#"
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
temperature = 3.0
"#;
        let err = LitConfig::from_str(toml).unwrap_err();
        assert!(
            err.to_string().contains("Invalid temperature"),
            "Expected temperature error, got: {}",
            err
        );
    }

    #[test]
    fn test_invalid_provider() {
        let toml = r#"
[project]
name = "test"
version = "0.1.0"
mapping = "manifest"

[language]
default = "python"
version = "3.12"

[model]
provider = "google"
model = "gemini"
temperature = 0.0
"#;
        let err = LitConfig::from_str(toml).unwrap_err();
        assert!(
            err.to_string().contains("Invalid model provider"),
            "Expected provider error, got: {}",
            err
        );
    }

    #[test]
    fn test_missing_required_fields() {
        let toml = r#"
[project]
name = "test"
"#;
        let err = LitConfig::from_str(toml).unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse"),
            "Expected parse error, got: {}",
            err
        );
    }

    #[test]
    fn test_resolve_api_key_from_env() {
        let config = LitConfig::from_str(VALID_CONFIG).unwrap();
        // Set the env var, resolve, then clean up
        unsafe { std::env::set_var("LIT_API_KEY", "test-key-123") };
        let key = config.resolve_api_key().unwrap();
        assert_eq!(key, "test-key-123");
        unsafe { std::env::remove_var("LIT_API_KEY") };
    }

    #[test]
    fn test_resolve_api_key_missing() {
        let config = LitConfig::from_str(VALID_CONFIG).unwrap();
        // Make sure the env var is not set
        unsafe { std::env::remove_var("LIT_API_KEY") };
        let err = config.resolve_api_key().unwrap_err();
        assert!(
            err.to_string().contains("LIT_API_KEY"),
            "Expected env var name in error, got: {}",
            err
        );
    }

    #[test]
    fn test_all_mapping_modes() {
        for mode in VALID_MAPPING_MODES {
            let toml = format!(
                r#"
[project]
name = "test"
version = "0.1.0"
mapping = "{mode}"

[language]
default = "python"
version = "3.12"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
"#
            );
            let config = LitConfig::from_str(&toml);
            assert!(config.is_ok(), "Mapping mode '{}' should be valid", mode);
        }
    }

    #[test]
    fn test_static_files_parsing() {
        let toml = r##"
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

[[static]]
path = "src/__init__.py"

[[static]]
path = "src/config/__init__.py"
content = "# Config package"
"##;
        let config = LitConfig::from_str(toml).unwrap();
        assert_eq!(config.r#static.len(), 2);
        assert_eq!(config.r#static[0].path, "src/__init__.py");
        assert_eq!(config.r#static[0].content, "");
        assert_eq!(config.r#static[1].path, "src/config/__init__.py");
        assert_eq!(config.r#static[1].content, "# Config package");
    }

    #[test]
    fn test_no_static_files() {
        // Existing configs without [[static]] should still parse fine
        let config = LitConfig::from_str(VALID_CONFIG).unwrap();
        assert!(config.r#static.is_empty());
    }

    #[test]
    fn test_pricing_override() {
        let toml = r#"
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

[model.pricing]
input_per_million = 5.0
output_per_million = 25.0
"#;
        let config = LitConfig::from_str(toml).unwrap();
        let pricing = config.model.pricing.unwrap();
        assert_eq!(pricing.input_per_million, 5.0);
        assert_eq!(pricing.output_per_million, 25.0);
    }

    #[test]
    fn test_no_pricing_override() {
        // Existing configs without [model.pricing] should still parse fine
        let config = LitConfig::from_str(VALID_CONFIG).unwrap();
        assert!(config.model.pricing.is_none());
    }
}
