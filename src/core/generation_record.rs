use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A generation record captures the full metadata for a single `lit regenerate` run.
///
/// Stored as JSON in `.lit/generations/<timestamp>.json`.
/// These records power `lit cost` and provide an audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationRecord {
    /// Timestamp of when this generation ran
    pub timestamp: DateTime<Utc>,

    /// Project name from lit.toml
    pub project: String,

    /// Model configuration used
    pub model: String,
    pub temperature: f64,
    pub seed: Option<u64>,

    /// Language and framework
    pub language: String,
    pub framework: Option<String>,

    /// Per-prompt generation metadata
    pub prompts: Vec<PromptRecord>,

    /// Aggregate statistics
    pub summary: GenerationSummary,
}

/// Metadata for a single prompt's generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRecord {
    /// Prompt file path (relative to repo root)
    pub prompt_path: PathBuf,

    /// Output files produced
    pub output_files: Vec<PathBuf>,

    /// Input hash used for caching
    pub input_hash: String,

    /// Whether this was served from cache
    pub from_cache: bool,

    /// Tokens consumed (0 if from cache)
    pub tokens_in: u64,

    /// Tokens produced (0 if from cache)
    pub tokens_out: u64,

    /// Generation time in milliseconds (0 if from cache)
    pub duration_ms: u64,

    /// Model used for this prompt
    pub model: String,

    /// Estimated cost in USD
    pub cost_usd: f64,
}

/// Aggregate statistics for a generation run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationSummary {
    /// Total prompts processed
    pub total_prompts: usize,

    /// Prompts that hit cache
    pub cache_hits: usize,

    /// Prompts that required LLM calls
    pub cache_misses: usize,

    /// Prompts that were skipped (not in regeneration set)
    pub skipped: usize,

    /// Total input tokens
    pub total_tokens_in: u64,

    /// Total output tokens
    pub total_tokens_out: u64,

    /// Total estimated cost in USD
    pub total_cost_usd: f64,

    /// Total duration in milliseconds
    pub total_duration_ms: u64,

    /// Total files written
    pub total_files_written: usize,

    /// Patches applied
    pub patches_applied: usize,

    /// Patches conflicted
    pub patches_conflicted: usize,
}

/// Known model pricing (per million tokens, in USD)
#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl ModelPricing {
    pub fn new(input_per_million: f64, output_per_million: f64) -> Self {
        Self {
            input_per_million,
            output_per_million,
        }
    }
}

impl GenerationRecord {
    /// Write a generation record to disk.
    ///
    /// Records are stored at `.lit/generations/<timestamp>.json`.
    pub fn write(&self, generations_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(generations_dir).with_context(|| {
            format!(
                "Failed to create generations dir: {}",
                generations_dir.display()
            )
        })?;

        let filename = format!("{}.json", self.timestamp.format("%Y%m%d-%H%M%S"));
        let path = generations_dir.join(filename);

        let json =
            serde_json::to_string_pretty(self).context("Failed to serialize generation record")?;

        std::fs::write(&path, json)
            .with_context(|| format!("Failed to write generation record: {}", path.display()))
    }

    /// Read a generation record from a JSON file.
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read generation record: {}", path.display()))?;

        serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse generation record: {}",
                path.display()
            )
        })
    }

    /// List all generation records in a directory, sorted by timestamp (newest first).
    pub fn list(generations_dir: &Path) -> Result<Vec<GenerationRecord>> {
        if !generations_dir.exists() {
            return Ok(Vec::new());
        }

        let mut records = Vec::new();

        for entry in std::fs::read_dir(generations_dir)
            .with_context(|| format!("Failed to read generations dir: {}", generations_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json") {
                match Self::read(&path) {
                    Ok(record) => records.push(record),
                    Err(e) => {
                        eprintln!(
                            "Warning: skipping malformed generation record {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }

        // Sort newest first
        records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(records)
    }

    /// Get the most recent generation record.
    #[allow(dead_code)]
    pub fn latest(generations_dir: &Path) -> Result<Option<GenerationRecord>> {
        let records = Self::list(generations_dir)?;
        Ok(records.into_iter().next())
    }
}

/// Estimate the cost of a generation based on model and token counts.
///
/// If `pricing_override` is provided (from `[model.pricing]` in lit.toml),
/// it takes precedence over the built-in pricing table.
pub fn estimate_cost(
    model: &str,
    tokens_in: u64,
    tokens_out: u64,
    pricing_override: Option<&ModelPricing>,
) -> f64 {
    let pricing = match pricing_override {
        Some(p) => p.clone(),
        None => get_model_pricing(model),
    };
    let input_cost = (tokens_in as f64 / 1_000_000.0) * pricing.input_per_million;
    let output_cost = (tokens_out as f64 / 1_000_000.0) * pricing.output_per_million;
    input_cost + output_cost
}

/// Get pricing for a known model. Falls back to conservative defaults for unknown models.
///
/// Pricing as of February 2026. Override in lit.toml with `[model.pricing]` if these
/// become stale:
///
/// ```toml
/// [model.pricing]
/// input_per_million = 3.0
/// output_per_million = 15.0
/// ```
pub fn get_model_pricing(model: &str) -> ModelPricing {
    match model {
        // Claude Opus 4.5 / 4.6
        m if m.contains("claude-opus-4-5") || m.contains("claude-opus-4-6") => ModelPricing {
            input_per_million: 5.0,
            output_per_million: 25.0,
        },
        // Claude Opus 4 / 4.1
        m if m.contains("claude-3-opus") || m.contains("claude-opus-4") => ModelPricing {
            input_per_million: 15.0,
            output_per_million: 75.0,
        },

        // Claude Sonnet 4 / 4.5
        m if m.contains("claude-3-5-sonnet") || m.contains("claude-sonnet-4") => ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        },

        // Claude Haiku 4.5
        m if m.contains("claude-haiku-4-5") => ModelPricing {
            input_per_million: 1.0,
            output_per_million: 5.0,
        },
        // Claude Haiku 3.5
        m if m.contains("claude-3-5-haiku") || m.contains("claude-haiku-4") => ModelPricing {
            input_per_million: 0.80,
            output_per_million: 4.0,
        },
        // Claude Haiku 3
        m if m.contains("claude-3-haiku") => ModelPricing {
            input_per_million: 0.25,
            output_per_million: 1.25,
        },

        // OpenAI GPT-4o
        m if m.contains("gpt-4o") && !m.contains("mini") => ModelPricing {
            input_per_million: 2.50,
            output_per_million: 10.0,
        },
        // OpenAI GPT-4o-mini
        m if m.contains("gpt-4o-mini") => ModelPricing {
            input_per_million: 0.15,
            output_per_million: 0.60,
        },
        // OpenAI GPT-4 (legacy)
        m if m.starts_with("gpt-4") && !m.contains("gpt-4o") => ModelPricing {
            input_per_million: 30.0,
            output_per_million: 60.0,
        },

        // Unknown model — use Sonnet-tier pricing as a reasonable default
        _ => ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        },
    }
}

/// Format a cost in USD for display.
pub fn format_cost(cost_usd: f64) -> String {
    if cost_usd < 0.001 {
        format!("${:.4}", cost_usd)
    } else if cost_usd < 0.01 {
        format!("${:.3}", cost_usd)
    } else {
        format!("${:.2}", cost_usd)
    }
}

/// Format a token count for display (e.g., 1234 → "1,234", 1234567 → "1.2M").
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        // Add comma separators
        let s = tokens.to_string();
        let mut result = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }
        result.chars().rev().collect()
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> GenerationRecord {
        GenerationRecord {
            timestamp: Utc::now(),
            project: "test-project".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            temperature: 0.0,
            seed: Some(42),
            language: "python".to_string(),
            framework: Some("fastapi".to_string()),
            prompts: vec![
                PromptRecord {
                    prompt_path: PathBuf::from("prompts/models/user.prompt.md"),
                    output_files: vec![PathBuf::from("src/models/user.py")],
                    input_hash: "abc123".to_string(),
                    from_cache: false,
                    tokens_in: 500,
                    tokens_out: 1200,
                    duration_ms: 3500,
                    model: "claude-sonnet-4-5-20250929".to_string(),
                    cost_usd: 0.0195,
                },
                PromptRecord {
                    prompt_path: PathBuf::from("prompts/schemas/user.prompt.md"),
                    output_files: vec![PathBuf::from("src/schemas/user.py")],
                    input_hash: "def456".to_string(),
                    from_cache: true,
                    tokens_in: 0,
                    tokens_out: 0,
                    duration_ms: 0,
                    model: "claude-sonnet-4-5-20250929".to_string(),
                    cost_usd: 0.0,
                },
            ],
            summary: GenerationSummary {
                total_prompts: 2,
                cache_hits: 1,
                cache_misses: 1,
                skipped: 0,
                total_tokens_in: 500,
                total_tokens_out: 1200,
                total_cost_usd: 0.0195,
                total_duration_ms: 3500,
                total_files_written: 2,
                patches_applied: 0,
                patches_conflicted: 0,
            },
        }
    }

    #[test]
    fn test_serialize_deserialize_round_trip() {
        let record = sample_record();
        let json = serde_json::to_string_pretty(&record).unwrap();
        let deserialized: GenerationRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.project, "test-project");
        assert_eq!(deserialized.prompts.len(), 2);
        assert_eq!(deserialized.summary.total_prompts, 2);
        assert_eq!(deserialized.summary.cache_hits, 1);
        assert_eq!(deserialized.summary.total_tokens_in, 500);
        assert_eq!(deserialized.summary.total_cost_usd, 0.0195);
    }

    #[test]
    fn test_write_and_read_record() {
        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().join("generations");

        let record = sample_record();
        record.write(&gen_dir).unwrap();

        // Read it back
        let records = GenerationRecord::list(&gen_dir).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project, "test-project");
        assert_eq!(records[0].prompts.len(), 2);
    }

    #[test]
    fn test_list_multiple_records() {
        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().join("generations");

        let mut r1 = sample_record();
        r1.timestamp = Utc::now() - chrono::Duration::hours(2);
        r1.project = "project-1".to_string();
        r1.write(&gen_dir).unwrap();

        // Small delay to ensure different filename
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut r2 = sample_record();
        r2.timestamp = Utc::now();
        r2.project = "project-2".to_string();
        r2.write(&gen_dir).unwrap();

        let records = GenerationRecord::list(&gen_dir).unwrap();
        assert_eq!(records.len(), 2);
        // Newest first
        assert_eq!(records[0].project, "project-2");
        assert_eq!(records[1].project, "project-1");
    }

    #[test]
    fn test_latest_record() {
        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().join("generations");

        let mut r1 = sample_record();
        r1.timestamp = Utc::now() - chrono::Duration::hours(1);
        r1.project = "old".to_string();
        r1.write(&gen_dir).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut r2 = sample_record();
        r2.timestamp = Utc::now();
        r2.project = "new".to_string();
        r2.write(&gen_dir).unwrap();

        let latest = GenerationRecord::latest(&gen_dir).unwrap().unwrap();
        assert_eq!(latest.project, "new");
    }

    #[test]
    fn test_list_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().join("generations");
        // Dir doesn't exist
        let records = GenerationRecord::list(&gen_dir).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_latest_empty() {
        let dir = tempfile::tempdir().unwrap();
        let gen_dir = dir.path().join("generations");
        let latest = GenerationRecord::latest(&gen_dir).unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn test_estimate_cost_sonnet() {
        // 1000 input tokens + 2000 output tokens with Sonnet pricing
        let cost = estimate_cost("claude-sonnet-4-5-20250929", 1000, 2000, None);
        // Input: 1000/1M * $3.0 = $0.003
        // Output: 2000/1M * $15.0 = $0.03
        // Total: $0.033
        assert!((cost - 0.033).abs() < 0.0001, "Expected ~$0.033, got {}", cost);
    }

    #[test]
    fn test_estimate_cost_haiku() {
        let cost = estimate_cost("claude-3-5-haiku-20241022", 1000, 2000, None);
        // Input: 1000/1M * $0.80 = $0.0008
        // Output: 2000/1M * $4.0 = $0.008
        // Total: $0.0088
        assert!((cost - 0.0088).abs() < 0.0001, "Expected ~$0.0088, got {}", cost);
    }

    #[test]
    fn test_estimate_cost_unknown_model() {
        // Unknown models use Sonnet-tier defaults
        let cost = estimate_cost("some-future-model", 1000, 2000, None);
        let sonnet_cost = estimate_cost("claude-sonnet-4-5-20250929", 1000, 2000, None);
        assert_eq!(cost, sonnet_cost);
    }

    #[test]
    fn test_estimate_cost_with_override() {
        // Custom pricing should override built-in defaults
        let custom = ModelPricing::new(10.0, 50.0);
        let cost = estimate_cost("claude-sonnet-4-5-20250929", 1000, 2000, Some(&custom));
        // Input: 1000/1M * $10.0 = $0.01
        // Output: 2000/1M * $50.0 = $0.10
        // Total: $0.11
        assert!((cost - 0.11).abs() < 0.0001, "Expected ~$0.11, got {}", cost);
    }

    #[test]
    fn test_estimate_cost_opus_tiers() {
        // Opus 4.5/4.6 should be $5/$25, not $15/$75
        let cost_new = estimate_cost("claude-opus-4-5-20260101", 1_000_000, 0, None);
        assert!((cost_new - 5.0).abs() < 0.01, "Opus 4.5 input should be $5/MTok, got {}", cost_new);

        // Opus 4/4.1 should still be $15/$75
        let cost_old = estimate_cost("claude-opus-4-20250514", 1_000_000, 0, None);
        assert!((cost_old - 15.0).abs() < 0.01, "Opus 4 input should be $15/MTok, got {}", cost_old);
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(0.0), "$0.0000");
        assert_eq!(format_cost(0.0005), "$0.0005");
        assert_eq!(format_cost(0.005), "$0.005");
        assert_eq!(format_cost(0.05), "$0.05");
        assert_eq!(format_cost(1.50), "$1.50");
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1234), "1,234");
        assert_eq!(format_tokens(12345), "12,345");
        assert_eq!(format_tokens(123456), "123,456");
        assert_eq!(format_tokens(1234567), "1.2M");
    }

    #[test]
    fn test_prompt_record_cached_zero_cost() {
        let record = sample_record();
        let cached = &record.prompts[1];
        assert!(cached.from_cache);
        assert_eq!(cached.tokens_in, 0);
        assert_eq!(cached.tokens_out, 0);
        assert_eq!(cached.cost_usd, 0.0);
    }
}
