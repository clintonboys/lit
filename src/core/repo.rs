use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{
    DiffOptions, IndexAddOption, Repository, Signature, StatusOptions, StatusShow,
};

/// Information about a single commit
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}

/// Status of the working tree
#[derive(Debug)]
pub struct RepoStatus {
    /// Modified prompt files
    pub prompts_modified: Vec<PathBuf>,
    /// New (untracked) prompt files
    pub prompts_new: Vec<PathBuf>,
    /// Deleted prompt files
    pub prompts_deleted: Vec<PathBuf>,
    /// Modified code.lock files
    pub code_modified: Vec<PathBuf>,
    /// New code.lock files
    pub code_new: Vec<PathBuf>,
    /// Modified config/meta files (lit.toml, .lit/*)
    pub config_modified: Vec<PathBuf>,
    /// Other modified files
    pub other_modified: Vec<PathBuf>,
    /// HEAD commit hash (None if no commits)
    pub head_commit: Option<String>,
}

/// Per-file diff statistics (insertions/deletions)
#[derive(Debug)]
pub struct FileDiffStat {
    pub path: PathBuf,
    pub insertions: usize,
    pub deletions: usize,
}

/// Git repository wrapper for lit operations.
///
/// All git interactions go through this struct so the rest of the codebase
/// doesn't need to deal with git2 directly.
pub struct LitRepo {
    repo: Repository,
    root: PathBuf,
}

impl LitRepo {
    /// Initialize a new git repository at the given path.
    pub fn init(path: &Path) -> Result<Self> {
        let repo = Repository::init(path)
            .with_context(|| format!("Failed to init git repo at {}", path.display()))?;

        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        let root = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());

        Ok(Self { root, repo })
    }

    /// Open an existing repository at (or above) the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path).with_context(|| {
            format!(
                "Failed to find git repo at or above {}",
                path.display()
            )
        })?;

        let workdir = repo
            .workdir()
            .ok_or_else(|| anyhow::anyhow!("Bare git repositories are not supported"))?;

        // Canonicalize and strip trailing slash for consistency with init()
        let root = workdir
            .canonicalize()
            .unwrap_or_else(|_| workdir.to_path_buf());

        Ok(Self { root, repo })
    }

    /// Get the repo root path.
    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    // ---------- Staging ----------

    /// Stage all relevant lit files for commit.
    ///
    /// Stages: prompts/**, code.lock/**, lit.toml, .lit/generations/**, .lit/patches/**
    /// Respects .gitignore.
    pub fn stage_all(&self) -> Result<()> {
        let mut index = self.repo.index().context("Failed to open git index")?;

        // Use add_all with pathspecs to add relevant paths
        // This respects .gitignore
        let pathspecs = [
            "prompts",
            "code.lock",
            "lit.toml",
            ".lit/generations",
            ".lit/patches",
            ".gitignore",
        ];

        index
            .add_all(pathspecs.iter(), IndexAddOption::DEFAULT, None)
            .context("Failed to stage files")?;

        // Also handle deleted files — update the index to remove them
        index
            .update_all(pathspecs.iter(), None)
            .context("Failed to update index for deleted files")?;

        index.write().context("Failed to write git index")?;

        Ok(())
    }

    /// Stage a specific file path.
    #[allow(dead_code)]
    pub fn stage_file(&self, path: &Path) -> Result<()> {
        let mut index = self.repo.index().context("Failed to open git index")?;
        index
            .add_path(path)
            .with_context(|| format!("Failed to stage {}", path.display()))?;
        index.write().context("Failed to write git index")?;
        Ok(())
    }

    // ---------- Commit ----------

    /// Create a commit with all staged changes.
    pub fn commit(&self, message: &str) -> Result<String> {
        let mut index = self.repo.index().context("Failed to open git index")?;
        let tree_oid = index.write_tree().context("Failed to write tree")?;
        let tree = self
            .repo
            .find_tree(tree_oid)
            .context("Failed to find tree")?;

        let sig = self.default_signature()?;

        let commit_oid = if let Ok(head) = self.repo.head() {
            // Normal commit with parent
            let parent = head
                .peel_to_commit()
                .context("Failed to find HEAD commit")?;
            self.repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
                .context("Failed to create commit")?
        } else {
            // Initial commit (no parent)
            self.repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[])
                .context("Failed to create initial commit")?
        };

        Ok(format!("{}", commit_oid))
    }

    /// Get the HEAD commit info, or None if there are no commits.
    pub fn head_commit(&self) -> Option<CommitInfo> {
        let head = self.repo.head().ok()?;
        let commit = head.peel_to_commit().ok()?;
        Some(commit_to_info(&commit))
    }

    // ---------- Log ----------

    /// Get commit history (newest first), up to `limit` entries.
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let head = match self.repo.head() {
            Ok(h) => h,
            Err(_) => return Ok(Vec::new()), // No commits yet
        };

        let head_commit = head
            .peel_to_commit()
            .context("Failed to find HEAD commit")?;

        let mut revwalk = self.repo.revwalk().context("Failed to create revwalk")?;
        revwalk
            .push(head_commit.id())
            .context("Failed to push HEAD to revwalk")?;

        let mut commits = Vec::new();
        for oid in revwalk {
            if commits.len() >= limit {
                break;
            }
            let oid = oid.context("Failed to read commit OID")?;
            let commit = self
                .repo
                .find_commit(oid)
                .with_context(|| format!("Failed to find commit {}", oid))?;
            commits.push(commit_to_info(&commit));
        }

        Ok(commits)
    }

    // ---------- Status ----------

    /// Get the working tree status, categorized by file type.
    pub fn status(&self) -> Result<RepoStatus> {
        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir);
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);

        let statuses = self
            .repo
            .statuses(Some(&mut opts))
            .context("Failed to get repo status")?;

        let mut result = RepoStatus {
            prompts_modified: Vec::new(),
            prompts_new: Vec::new(),
            prompts_deleted: Vec::new(),
            code_modified: Vec::new(),
            code_new: Vec::new(),
            config_modified: Vec::new(),
            other_modified: Vec::new(),
            head_commit: self.head_commit().map(|c| c.short_hash),
        };

        for entry in statuses.iter() {
            let path = match entry.path() {
                Some(p) => PathBuf::from(p),
                None => continue,
            };

            let status = entry.status();

            // Skip clean files
            if status.is_empty() {
                continue;
            }

            let is_new = status.is_wt_new() || status.is_index_new();
            let is_deleted = status.is_wt_deleted() || status.is_index_deleted();
            let is_modified = status.is_wt_modified()
                || status.is_index_modified()
                || status.is_wt_renamed()
                || status.is_index_renamed();

            let path_str = path.to_string_lossy();

            if path_str.starts_with("prompts/") {
                if is_new {
                    result.prompts_new.push(path);
                } else if is_deleted {
                    result.prompts_deleted.push(path);
                } else if is_modified {
                    result.prompts_modified.push(path);
                }
            } else if path_str.starts_with("code.lock/") {
                if is_new {
                    result.code_new.push(path);
                } else if is_modified {
                    result.code_modified.push(path);
                }
            } else if path_str == "lit.toml"
                || path_str.starts_with(".lit/")
                || path_str == ".gitignore"
            {
                if is_new || is_modified {
                    result.config_modified.push(path);
                }
            } else if is_new || is_modified || is_deleted {
                result.other_modified.push(path);
            }
        }

        Ok(result)
    }

    // ---------- Diff ----------

    /// Get a diff of prompts/ (working tree vs HEAD).
    pub fn diff_prompts(&self) -> Result<String> {
        self.diff_pathspec(&["prompts/"])
    }

    /// Get a diff of code.lock/ (working tree vs HEAD).
    pub fn diff_code(&self) -> Result<String> {
        self.diff_pathspec(&["code.lock/"])
    }

    /// Get a diff of all lit-related paths.
    pub fn diff_all(&self) -> Result<String> {
        self.diff_pathspec(&["prompts/", "code.lock/", "lit.toml"])
    }

    /// Get diff for specific pathspecs.
    /// Get per-file insertion/deletion counts for prompt changes.
    pub fn diff_prompt_stats(&self) -> Result<Vec<FileDiffStat>> {
        let mut opts = DiffOptions::new();
        opts.pathspec("prompts/");

        let head_tree = match self.repo.head() {
            Ok(head) => {
                let commit = head.peel_to_commit().context("Failed to peel HEAD")?;
                Some(commit.tree().context("Failed to get HEAD tree")?)
            }
            Err(_) => None,
        };

        let diff = self
            .repo
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
            .context("Failed to compute diff")?;

        let mut stats_map: std::collections::HashMap<PathBuf, (usize, usize)> =
            std::collections::HashMap::new();

        diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
            if let Some(path) = delta.new_file().path().or(delta.old_file().path()) {
                let entry = stats_map
                    .entry(PathBuf::from(path))
                    .or_insert((0, 0));
                match line.origin() {
                    '+' => entry.0 += 1,
                    '-' => entry.1 += 1,
                    _ => {}
                }
            }
            true
        })
        .context("Failed to compute diff stats")?;

        let mut result: Vec<FileDiffStat> = stats_map
            .into_iter()
            .map(|(path, (ins, del))| FileDiffStat {
                path,
                insertions: ins,
                deletions: del,
            })
            .collect();
        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    fn diff_pathspec(&self, pathspecs: &[&str]) -> Result<String> {
        let mut opts = DiffOptions::new();
        for spec in pathspecs {
            opts.pathspec(spec);
        }

        let head_tree = match self.repo.head() {
            Ok(head) => {
                let commit = head.peel_to_commit().context("Failed to peel HEAD")?;
                Some(commit.tree().context("Failed to get HEAD tree")?)
            }
            Err(_) => None, // No commits yet — diff against empty tree
        };

        let diff = self
            .repo
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
            .context("Failed to compute diff")?;

        let mut buf = Vec::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            // Include the origin character (+, -, space) for content lines
            let origin = line.origin();
            if origin == '+' || origin == '-' || origin == ' ' {
                buf.push(origin as u8);
            }
            buf.extend_from_slice(line.content());
            true
        })
        .context("Failed to format diff")?;

        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    // ---------- Checkout ----------

    /// Checkout a specific ref (commit hash, HEAD~N, branch name, etc.)
    pub fn checkout_ref(&self, ref_str: &str) -> Result<String> {
        // Parse the ref
        let obj = self
            .repo
            .revparse_single(ref_str)
            .with_context(|| format!("Failed to resolve ref '{}'", ref_str))?;

        let commit = obj
            .peel_to_commit()
            .with_context(|| format!("'{}' does not point to a commit", ref_str))?;

        // Checkout the tree
        let tree = commit.tree().context("Failed to get commit tree")?;
        self.repo
            .checkout_tree(
                tree.as_object(),
                Some(git2::build::CheckoutBuilder::new().force()),
            )
            .with_context(|| format!("Failed to checkout '{}'", ref_str))?;

        // Detach HEAD to this commit
        self.repo
            .set_head_detached(commit.id())
            .with_context(|| format!("Failed to set HEAD to '{}'", ref_str))?;

        Ok(format!("{}", commit.id()))
    }

    // ---------- .gitignore ----------

    /// Write a standard .gitignore for a lit project.
    pub fn write_gitignore(&self) -> Result<()> {
        let gitignore_content = "\
# Lit internal cache (local optimization, not committed)
.lit/cache/

# Python artifacts
__pycache__/
*.pyc
*.pyo
*.egg-info/
dist/
build/
.venv/
venv/

# IDE
.vscode/
.idea/
*.swp

# OS
.DS_Store
Thumbs.db
";
        let gitignore_path = self.root.join(".gitignore");
        std::fs::write(&gitignore_path, gitignore_content)
            .with_context(|| format!("Failed to write .gitignore at {}", gitignore_path.display()))
    }

    // ---------- Internal ----------

    fn default_signature(&self) -> Result<Signature<'_>> {
        // Try to get signature from git config, fall back to defaults
        self.repo.signature().or_else(|_| {
            Signature::now("lit", "lit@localhost")
                .context("Failed to create git signature")
        })
    }
}

impl RepoStatus {
    /// Returns true if there are any changes to commit.
    pub fn has_changes(&self) -> bool {
        !self.prompts_modified.is_empty()
            || !self.prompts_new.is_empty()
            || !self.prompts_deleted.is_empty()
            || !self.code_modified.is_empty()
            || !self.code_new.is_empty()
            || !self.config_modified.is_empty()
    }

    /// Total number of changed files.
    pub fn total_changes(&self) -> usize {
        self.prompts_modified.len()
            + self.prompts_new.len()
            + self.prompts_deleted.len()
            + self.code_modified.len()
            + self.code_new.len()
            + self.config_modified.len()
    }
}

fn commit_to_info(commit: &git2::Commit) -> CommitInfo {
    let hash = format!("{}", commit.id());
    let short_hash = hash[..7.min(hash.len())].to_string();
    let message = commit
        .message()
        .unwrap_or("")
        .trim()
        .to_string();
    let author = commit
        .author()
        .name()
        .unwrap_or("Unknown")
        .to_string();
    let timestamp = commit.time().seconds();

    CommitInfo {
        hash,
        short_hash,
        message,
        author,
        timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_and_open() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();
        assert_eq!(repo.root(), canonical);

        // Can reopen it
        let repo2 = LitRepo::open(dir.path()).unwrap();
        assert_eq!(repo2.root(), canonical);
    }

    #[test]
    fn test_initial_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        // Write a file
        std::fs::write(dir.path().join("lit.toml"), "[project]\nname = \"test\"\n").unwrap();

        // Stage and commit
        repo.stage_file(Path::new("lit.toml")).unwrap();
        let hash = repo.commit("Initial commit").unwrap();
        assert!(!hash.is_empty());

        // Check HEAD
        let head = repo.head_commit().unwrap();
        assert_eq!(head.message, "Initial commit");
        assert_eq!(head.hash, hash);
    }

    #[test]
    fn test_log() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        // Empty log
        let log = repo.log(10).unwrap();
        assert!(log.is_empty());

        // First commit
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        repo.stage_file(Path::new("a.txt")).unwrap();
        repo.commit("First").unwrap();

        // Second commit
        std::fs::write(dir.path().join("b.txt"), "world").unwrap();
        repo.stage_file(Path::new("b.txt")).unwrap();
        repo.commit("Second").unwrap();

        let log = repo.log(10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].message, "Second"); // Newest first
        assert_eq!(log[1].message, "First");

        // Limit
        let log = repo.log(1).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].message, "Second");
    }

    #[test]
    fn test_status_categorization() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        // Create directory structure
        std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
        std::fs::create_dir_all(dir.path().join("code.lock")).unwrap();

        // Write files
        std::fs::write(dir.path().join("lit.toml"), "config").unwrap();
        std::fs::write(dir.path().join("prompts/a.prompt.md"), "prompt").unwrap();
        std::fs::write(dir.path().join("code.lock/a.py"), "code").unwrap();

        let status = repo.status().unwrap();
        assert!(status.head_commit.is_none()); // No commits yet
        assert!(status.prompts_new.iter().any(|p| p.to_str().unwrap().contains("a.prompt.md")));
        assert!(status.code_new.iter().any(|p| p.to_str().unwrap().contains("a.py")));
        assert!(status.config_modified.iter().any(|p| p.to_str().unwrap() == "lit.toml"));
        assert!(status.has_changes());
    }

    #[test]
    fn test_stage_all_and_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        // Create lit structure
        std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
        std::fs::create_dir_all(dir.path().join("code.lock")).unwrap();
        std::fs::write(dir.path().join("lit.toml"), "config").unwrap();
        std::fs::write(dir.path().join("prompts/a.prompt.md"), "prompt").unwrap();
        std::fs::write(dir.path().join("code.lock/a.py"), "code").unwrap();

        repo.stage_all().unwrap();
        let hash = repo.commit("All files").unwrap();
        assert!(!hash.is_empty());

        // After commit, status should be clean
        let status = repo.status().unwrap();
        assert!(!status.has_changes(), "Expected clean status after commit");
    }

    #[test]
    fn test_diff_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        std::fs::create_dir_all(dir.path().join("prompts")).unwrap();
        std::fs::write(dir.path().join("prompts/a.prompt.md"), "v1").unwrap();
        repo.stage_all().unwrap();
        repo.commit("v1").unwrap();

        // Modify
        std::fs::write(dir.path().join("prompts/a.prompt.md"), "v2").unwrap();

        let diff = repo.diff_prompts().unwrap();
        assert!(!diff.is_empty(), "Expected non-empty diff");
        assert!(diff.contains("-v1"), "Expected old content in diff");
        assert!(diff.contains("+v2"), "Expected new content in diff");
    }

    #[test]
    fn test_checkout_ref() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        // First commit
        std::fs::write(dir.path().join("file.txt"), "version 1").unwrap();
        repo.stage_file(Path::new("file.txt")).unwrap();
        let first_hash = repo.commit("First").unwrap();

        // Second commit
        std::fs::write(dir.path().join("file.txt"), "version 2").unwrap();
        repo.stage_file(Path::new("file.txt")).unwrap();
        repo.commit("Second").unwrap();

        // Verify we're on version 2
        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "version 2");

        // Checkout first commit
        let checked_out = repo.checkout_ref(&first_hash).unwrap();
        assert_eq!(checked_out, first_hash);

        // File should be version 1
        let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(content, "version 1");
    }

    #[test]
    fn test_write_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();
        repo.write_gitignore().unwrap();

        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".lit/cache/"));
        assert!(content.contains("__pycache__/"));
    }

    #[test]
    fn test_repo_status_has_changes() {
        let status = RepoStatus {
            prompts_modified: vec![],
            prompts_new: vec![],
            prompts_deleted: vec![],
            code_modified: vec![],
            code_new: vec![],
            config_modified: vec![],
            other_modified: vec![],
            head_commit: None,
        };
        assert!(!status.has_changes());
        assert_eq!(status.total_changes(), 0);

        let status2 = RepoStatus {
            prompts_modified: vec![PathBuf::from("prompts/a.prompt.md")],
            prompts_new: vec![],
            prompts_deleted: vec![],
            code_modified: vec![],
            code_new: vec![],
            config_modified: vec![],
            other_modified: vec![],
            head_commit: Some("abc1234".to_string()),
        };
        assert!(status2.has_changes());
        assert_eq!(status2.total_changes(), 1);
    }

    #[test]
    fn test_commit_info() {
        let dir = tempfile::tempdir().unwrap();
        let repo = LitRepo::init(dir.path()).unwrap();

        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        repo.stage_file(Path::new("f.txt")).unwrap();
        repo.commit("Test message").unwrap();

        let info = repo.head_commit().unwrap();
        assert_eq!(info.message, "Test message");
        assert_eq!(info.short_hash.len(), 7);
        assert!(info.timestamp > 0);
    }
}
