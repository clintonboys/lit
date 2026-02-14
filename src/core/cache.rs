use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Cached generation output for a single prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedGeneration {
    pub input_hash: String,
    pub files: HashMap<PathBuf, String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// Input-hash cache for skipping unchanged prompt generations.
///
/// Cache entries are stored as JSON files in `.lit/cache/<hash>.json`.
/// The cache is local-only (gitignored) — an optimization, not required for correctness.
pub struct Cache {
    cache_dir: PathBuf,
}

impl Cache {
    /// Create a new cache backed by the given directory.
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Ensure the cache directory exists.
    pub fn init(&self) -> Result<()> {
        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("Failed to create cache dir: {}", self.cache_dir.display()))
    }

    /// Compute an input hash for a prompt.
    ///
    /// The hash is over:
    /// - The prompt file content (frontmatter + body)
    /// - The input hashes of all imports (sorted for determinism)
    /// - The model config (model name, temperature, seed)
    /// - The language and framework
    /// - The system prompt version (so parser changes invalidate cache)
    ///
    /// This means if ANY upstream prompt changes, the hash cascades.
    pub fn compute_input_hash(
        prompt_content: &str,
        import_hashes: &[(&Path, &str)], // (import_path, import's input_hash)
        model: &str,
        temperature: f64,
        seed: Option<u64>,
        language: &str,
        framework: Option<&str>,
    ) -> String {
        let mut hasher = Sha256::new();

        // Version tag — bump this to invalidate all caches when the generation
        // logic changes (e.g., system prompt format, parser updates)
        hasher.update(b"lit-cache-v1\n");

        // Prompt content
        hasher.update(prompt_content.as_bytes());
        hasher.update(b"\n---imports---\n");

        // Import hashes (sorted by path for determinism)
        let mut sorted_imports: Vec<_> = import_hashes.to_vec();
        sorted_imports.sort_by_key(|(path, _)| path.to_path_buf());
        for (path, hash) in &sorted_imports {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(b":");
            hasher.update(hash.as_bytes());
            hasher.update(b"\n");
        }

        hasher.update(b"---model---\n");
        hasher.update(model.as_bytes());
        hasher.update(b"\n");
        hasher.update(format!("temp:{}\n", temperature).as_bytes());
        if let Some(s) = seed {
            hasher.update(format!("seed:{}\n", s).as_bytes());
        }

        hasher.update(b"---lang---\n");
        hasher.update(language.as_bytes());
        hasher.update(b"\n");
        if let Some(fw) = framework {
            hasher.update(fw.as_bytes());
            hasher.update(b"\n");
        }

        format!("{:x}", hasher.finalize())
    }

    /// Look up a cached generation by input hash.
    pub fn get(&self, input_hash: &str) -> Option<CachedGeneration> {
        let path = self.cache_dir.join(format!("{}.json", input_hash));
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Store a generation result in the cache.
    pub fn put(&self, entry: &CachedGeneration) -> Result<()> {
        let path = self.cache_dir.join(format!("{}.json", entry.input_hash));
        let content = serde_json::to_string_pretty(entry)
            .context("Failed to serialize cache entry")?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write cache entry: {}", path.display()))
    }

    /// Remove a cache entry.
    #[allow(dead_code)]
    pub fn remove(&self, input_hash: &str) -> Result<()> {
        let path = self.cache_dir.join(format!("{}.json", input_hash));
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to remove cache entry: {}", path.display()))?;
        }
        Ok(())
    }

    /// Clear all cache entries.
    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        if self.cache_dir.exists() {
            std::fs::remove_dir_all(&self.cache_dir)
                .with_context(|| format!("Failed to clear cache: {}", self.cache_dir.display()))?;
            std::fs::create_dir_all(&self.cache_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_same_inputs_same_hash() {
        let h1 = Cache::compute_input_hash(
            "prompt content",
            &[(Path::new("a.prompt.md"), "hash_a")],
            "claude-sonnet-4-5-20250929",
            0.0,
            Some(42),
            "python",
            Some("fastapi"),
        );
        let h2 = Cache::compute_input_hash(
            "prompt content",
            &[(Path::new("a.prompt.md"), "hash_a")],
            "claude-sonnet-4-5-20250929",
            0.0,
            Some(42),
            "python",
            Some("fastapi"),
        );
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_changed_prompt_different_hash() {
        let h1 = Cache::compute_input_hash(
            "prompt v1",
            &[],
            "claude-sonnet-4-5-20250929",
            0.0,
            None,
            "python",
            None,
        );
        let h2 = Cache::compute_input_hash(
            "prompt v2",
            &[],
            "claude-sonnet-4-5-20250929",
            0.0,
            None,
            "python",
            None,
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_changed_import_different_hash() {
        let h1 = Cache::compute_input_hash(
            "prompt",
            &[(Path::new("dep.prompt.md"), "hash_v1")],
            "claude-sonnet-4-5-20250929",
            0.0,
            None,
            "python",
            None,
        );
        let h2 = Cache::compute_input_hash(
            "prompt",
            &[(Path::new("dep.prompt.md"), "hash_v2")],
            "claude-sonnet-4-5-20250929",
            0.0,
            None,
            "python",
            None,
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_changed_model_different_hash() {
        let h1 = Cache::compute_input_hash(
            "prompt",
            &[],
            "claude-sonnet-4-5-20250929",
            0.0,
            None,
            "python",
            None,
        );
        let h2 = Cache::compute_input_hash(
            "prompt",
            &[],
            "gpt-4",
            0.0,
            None,
            "python",
            None,
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_changed_temperature_different_hash() {
        let h1 = Cache::compute_input_hash("p", &[], "m", 0.0, None, "py", None);
        let h2 = Cache::compute_input_hash("p", &[], "m", 0.5, None, "py", None);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_changed_seed_different_hash() {
        let h1 = Cache::compute_input_hash("p", &[], "m", 0.0, Some(42), "py", None);
        let h2 = Cache::compute_input_hash("p", &[], "m", 0.0, Some(99), "py", None);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_import_order_does_not_matter() {
        let h1 = Cache::compute_input_hash(
            "prompt",
            &[
                (Path::new("b.prompt.md"), "hash_b"),
                (Path::new("a.prompt.md"), "hash_a"),
            ],
            "model",
            0.0,
            None,
            "python",
            None,
        );
        let h2 = Cache::compute_input_hash(
            "prompt",
            &[
                (Path::new("a.prompt.md"), "hash_a"),
                (Path::new("b.prompt.md"), "hash_b"),
            ],
            "model",
            0.0,
            None,
            "python",
            None,
        );
        assert_eq!(h1, h2, "Import order should not affect hash");
    }

    #[test]
    fn test_cache_store_and_retrieve() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        cache.init().unwrap();

        let mut files = HashMap::new();
        files.insert(PathBuf::from("src/main.py"), "print('hello')\n".to_string());

        let entry = CachedGeneration {
            input_hash: "abc123".to_string(),
            files: files.clone(),
            tokens_in: 100,
            tokens_out: 200,
        };

        cache.put(&entry).unwrap();

        let retrieved = cache.get("abc123").unwrap();
        assert_eq!(retrieved.input_hash, "abc123");
        assert_eq!(retrieved.files, files);
        assert_eq!(retrieved.tokens_in, 100);
        assert_eq!(retrieved.tokens_out, 200);
    }

    #[test]
    fn test_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        cache.init().unwrap();

        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        cache.init().unwrap();

        let entry = CachedGeneration {
            input_hash: "abc123".to_string(),
            files: HashMap::new(),
            tokens_in: 0,
            tokens_out: 0,
        };
        cache.put(&entry).unwrap();
        assert!(cache.get("abc123").is_some());

        cache.clear().unwrap();
        assert!(cache.get("abc123").is_none());
    }
}
