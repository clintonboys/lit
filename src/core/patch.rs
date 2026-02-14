use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

/// Info about a detected manual patch
#[derive(Debug, Clone)]
pub struct PatchInfo {
    /// Output file path (relative to code.lock/)
    pub output_path: PathBuf,
    /// Unified diff of the manual edit
    #[allow(dead_code)]
    pub diff: String,
    /// Number of lines added
    pub lines_added: usize,
    /// Number of lines removed
    pub lines_removed: usize,
}

/// Stored patch data — saved as JSON for reliable round-tripping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPatch {
    /// The original LLM-generated content (the "base" for 3-way merge)
    pub original_content: String,
    /// The manually-edited content (the user's version)
    pub manual_content: String,
    /// Unified diff for display purposes
    pub diff: String,
}

/// Result of applying a patch to new generated content
#[derive(Debug)]
pub enum PatchResult {
    /// Patch applied cleanly — contains the merged content
    Applied(String),
    /// Patch conflicts with new content — contains conflict-marker version
    Conflict(String),
}

/// Manages manual patches to code.lock/ files.
///
/// Patches are stored as unified diffs in `.lit/patches/<output-path>.patch`.
/// They represent hand-edits to generated code that should survive regeneration.
///
/// Design principle: Patches are temporary escape hatches. The long-term goal
/// is always to update the prompt so the LLM generates the right code.
pub struct PatchStore {
    patches_dir: PathBuf,
}

impl PatchStore {
    /// Create a new PatchStore backed by the given directory (typically `.lit/patches/`).
    pub fn new(patches_dir: PathBuf) -> Self {
        Self { patches_dir }
    }

    /// Ensure the patches directory exists.
    pub fn init(&self) -> Result<()> {
        std::fs::create_dir_all(&self.patches_dir)
            .with_context(|| format!("Failed to create patches dir: {}", self.patches_dir.display()))
    }

    /// Detect manual patches by comparing generated (expected) content against
    /// actual files on disk.
    ///
    /// `generated` is what the LLM produced (or what's in cache).
    /// `actual` is what's actually in code.lock/ on disk.
    ///
    /// Returns PatchInfo for each file that differs.
    pub fn detect_patches(
        generated: &HashMap<PathBuf, String>,
        actual: &HashMap<PathBuf, String>,
    ) -> Vec<PatchInfo> {
        let mut patches = Vec::new();

        for (path, gen_content) in generated {
            if let Some(actual_content) = actual.get(path) {
                if gen_content != actual_content {
                    let diff = TextDiff::from_lines(gen_content, actual_content);
                    let unified = diff
                        .unified_diff()
                        .context_radius(3)
                        .header(
                            &format!("a/{}", path.display()),
                            &format!("b/{}", path.display()),
                        )
                        .to_string();

                    let mut lines_added = 0;
                    let mut lines_removed = 0;
                    for change in diff.iter_all_changes() {
                        match change.tag() {
                            ChangeTag::Insert => lines_added += 1,
                            ChangeTag::Delete => lines_removed += 1,
                            ChangeTag::Equal => {}
                        }
                    }

                    patches.push(PatchInfo {
                        output_path: path.clone(),
                        diff: unified,
                        lines_added,
                        lines_removed,
                    });
                }
            }
        }

        // Sort for deterministic output
        patches.sort_by(|a, b| a.output_path.cmp(&b.output_path));
        patches
    }

    /// Save a patch to disk.
    ///
    /// Stores both the original generated content and the manually-edited content
    /// as JSON at `.lit/patches/<output-path>.patch`.
    pub fn save_patch(
        &self,
        output_path: &Path,
        original_content: &str,
        manual_content: &str,
    ) -> Result<()> {
        let diff = TextDiff::from_lines(original_content, manual_content);
        let unified = diff
            .unified_diff()
            .context_radius(3)
            .header(
                &format!("a/{}", output_path.display()),
                &format!("b/{}", output_path.display()),
            )
            .to_string();

        let stored = StoredPatch {
            original_content: original_content.to_string(),
            manual_content: manual_content.to_string(),
            diff: unified,
        };

        let patch_path = self.patch_file_path(output_path);
        if let Some(parent) = patch_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create patch dir: {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&stored)
            .context("Failed to serialize patch")?;
        std::fs::write(&patch_path, json)
            .with_context(|| format!("Failed to write patch: {}", patch_path.display()))
    }

    /// Load a saved patch for a given output path.
    pub fn load_patch(&self, output_path: &Path) -> Option<StoredPatch> {
        let patch_path = self.patch_file_path(output_path);
        let content = std::fs::read_to_string(&patch_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Apply a saved patch to newly generated content.
    ///
    /// This is a line-based 3-way merge:
    /// - The "base" is the original generated content (from the saved patch's "before" side)
    /// - The "theirs" is the new LLM-generated content
    /// - The "ours" changes are the manual edits from the patch
    ///
    /// Simple strategy: if the patch hunks apply cleanly to the new content,
    /// return Applied. Otherwise return Conflict with markers.
    pub fn apply_patch(
        &self,
        original_generated: &str,
        new_generated: &str,
        manual_content: &str,
    ) -> PatchResult {
        // If new generated == original generated, the prompt hasn't changed.
        // Just use the manual content directly.
        if original_generated == new_generated {
            return PatchResult::Applied(manual_content.to_string());
        }

        // 3-way merge: find what the user changed (original→manual),
        // and what the LLM changed (original→new), then combine.
        //
        // Simple line-based approach:
        // 1. Compute user edits: original → manual
        // 2. Compute LLM edits: original → new
        // 3. If no overlapping changes, apply both sets of edits
        // 4. If overlapping, insert conflict markers

        let original_lines: Vec<&str> = original_generated.lines().collect();
        let manual_lines: Vec<&str> = manual_content.lines().collect();
        let new_lines: Vec<&str> = new_generated.lines().collect();

        // Collect line-level change ranges for user edits and LLM edits
        let user_changes = collect_line_changes_from_strings(original_generated, manual_content);
        let llm_changes = collect_line_changes_from_strings(original_generated, new_generated);

        // Check for overlapping changes
        let has_conflict = user_changes.iter().any(|uc| {
            llm_changes.iter().any(|lc| ranges_overlap(uc, lc))
        });

        if has_conflict {
            // Conflict: output with markers
            let conflict_content = format!(
                "<<<<<<< manual-patch\n\
                 {}\
                 =======\n\
                 {}\
                 >>>>>>> generated\n",
                manual_content,
                new_generated
            );
            PatchResult::Conflict(conflict_content)
        } else {
            // No overlapping changes — apply user edits to new generated content.
            // The simplest correct approach: start from the new LLM output and
            // replay user changes that don't conflict.
            //
            // Since there are no overlapping changes, we can apply user edits
            // by finding the lines the user modified and applying those changes
            // to the corresponding positions in the new content.
            //
            // Simplification: if user only made small edits and LLM made different
            // small edits, apply the user's manual_content lines that differ from
            // original, keeping the rest from new_generated.
            let result = merge_non_conflicting(
                &original_lines,
                &manual_lines,
                &new_lines,
            );
            PatchResult::Applied(result)
        }
    }

    /// List all tracked patches.
    pub fn list_patches(&self) -> Vec<PathBuf> {
        let mut patches = Vec::new();
        if self.patches_dir.exists() {
            collect_patches_recursive(&self.patches_dir, &self.patches_dir, &mut patches);
        }
        patches.sort();
        patches
    }

    /// Drop (delete) a saved patch.
    pub fn drop_patch(&self, output_path: &Path) -> Result<()> {
        let patch_path = self.patch_file_path(output_path);
        if patch_path.exists() {
            std::fs::remove_file(&patch_path)
                .with_context(|| format!("Failed to remove patch: {}", patch_path.display()))?;
        }
        // Clean up empty parent directories
        if let Some(parent) = patch_path.parent() {
            let _ = cleanup_empty_dirs(parent, &self.patches_dir);
        }
        Ok(())
    }

    /// Check if a patch exists for a given output path.
    pub fn has_patch(&self, output_path: &Path) -> bool {
        self.patch_file_path(output_path).exists()
    }

    // Internal: compute the path where a patch file is stored
    fn patch_file_path(&self, output_path: &Path) -> PathBuf {
        // e.g., output_path = "src/schemas/user.py"
        // patch file = ".lit/patches/src/schemas/user.py.patch"
        let mut patch_name = output_path.as_os_str().to_os_string();
        patch_name.push(".patch");
        self.patches_dir.join(PathBuf::from(patch_name))
    }
}

/// Represents a range of lines in the original that were changed
#[derive(Debug)]
struct LineChange {
    start: usize, // inclusive
    end: usize,   // exclusive
}

fn ranges_overlap(a: &LineChange, b: &LineChange) -> bool {
    a.start < b.end && b.start < a.end
}

/// Collect contiguous ranges of changed lines by diffing two strings
fn collect_line_changes_from_strings(original: &str, modified: &str) -> Vec<LineChange> {
    let diff = TextDiff::from_lines(original, modified);
    let mut changes = Vec::new();
    let mut current_range: Option<LineChange> = None;
    let mut orig_line = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                // Flush any pending change range
                if let Some(range) = current_range.take() {
                    changes.push(range);
                }
                orig_line += 1;
            }
            ChangeTag::Delete => {
                match &mut current_range {
                    Some(range) => range.end = orig_line + 1,
                    None => {
                        current_range = Some(LineChange {
                            start: orig_line,
                            end: orig_line + 1,
                        });
                    }
                }
                orig_line += 1;
            }
            ChangeTag::Insert => {
                // Insertions happen "at" the current original line position
                match &mut current_range {
                    Some(range) => range.end = orig_line.max(range.end),
                    None => {
                        current_range = Some(LineChange {
                            start: orig_line,
                            end: orig_line,
                        });
                    }
                }
            }
        }
    }

    if let Some(range) = current_range {
        changes.push(range);
    }

    changes
}

/// Merge non-conflicting edits from both user and LLM onto original.
///
/// For each line in the original:
/// - If the user changed it (but LLM didn't) → use user's version
/// - If the LLM changed it (but user didn't) → use LLM's version
/// - If neither changed it → use original (or equivalently, new)
fn merge_non_conflicting(
    original: &[&str],
    manual: &[&str],
    new_gen: &[&str],
) -> String {
    // Simple LCS-based merge: walk through and pick changes from whichever side modified each line
    // For now, use a simpler heuristic: the user's edits are applied on top of
    // the new generated content.
    //
    // Since we verified no overlapping changes, we can:
    // 1. Find lines user changed from original
    // 2. Find where those lines map to in new_gen
    // 3. Apply user's changes there
    //
    // Simplest correct approach for non-conflicting:
    // Start with new_gen, then for each line in original that the user changed
    // but the LLM kept the same, apply the user's change.

    let mut result_lines: Vec<String> = Vec::new();

    // Walk through the alignment of all three
    let mut o_idx = 0;
    let mut m_idx = 0;
    let mut n_idx = 0;

    while o_idx < original.len() || m_idx < manual.len() || n_idx < new_gen.len() {
        let o_line = original.get(o_idx).copied();
        let m_line = manual.get(m_idx).copied();
        let n_line = new_gen.get(n_idx).copied();

        match (o_line, m_line, n_line) {
            (Some(o), Some(m), Some(n)) => {
                if o == m && o == n {
                    // All three agree — emit the line
                    result_lines.push(n.to_string());
                    o_idx += 1;
                    m_idx += 1;
                    n_idx += 1;
                } else if o == n && o != m {
                    // User changed, LLM didn't → use user's version
                    result_lines.push(m.to_string());
                    o_idx += 1;
                    m_idx += 1;
                    n_idx += 1;
                } else if o == m && o != n {
                    // LLM changed, user didn't → use LLM's version
                    result_lines.push(n.to_string());
                    o_idx += 1;
                    m_idx += 1;
                    n_idx += 1;
                } else {
                    // Both changed (shouldn't happen since we checked for conflicts)
                    // Use the LLM version as fallback
                    result_lines.push(n.to_string());
                    o_idx += 1;
                    m_idx += 1;
                    n_idx += 1;
                }
            }
            (None, Some(m), None) => {
                // User added extra lines at the end
                result_lines.push(m.to_string());
                m_idx += 1;
            }
            (None, None, Some(n)) => {
                // LLM added extra lines at the end
                result_lines.push(n.to_string());
                n_idx += 1;
            }
            (Some(_o), Some(m), None) => {
                // Original and manual have lines, but new_gen is shorter
                // User's version of the remaining lines
                result_lines.push(m.to_string());
                o_idx += 1;
                m_idx += 1;
            }
            (Some(_o), None, Some(n)) => {
                // Original and new have lines, but manual is shorter (user deleted)
                // Since LLM kept these, and user deleted, user's edit wins
                o_idx += 1;
                n_idx += 1;
                // Skip this line (user deleted it)
                let _ = n; // suppress unused warning
            }
            (None, Some(m), Some(_n)) => {
                // Both manual and new added lines past original
                // Prefer manual since user explicitly added
                result_lines.push(m.to_string());
                m_idx += 1;
                n_idx += 1;
            }
            _ => break,
        }
    }

    let mut result = result_lines.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Recursively collect .patch files
fn collect_patches_recursive(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_patches_recursive(&path, base, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("patch") {
                // Convert patch file path back to output path
                // e.g., .lit/patches/src/schemas/user.py.patch → src/schemas/user.py
                if let Ok(relative) = path.strip_prefix(base) {
                    let rel_str = relative.to_string_lossy();
                    if let Some(stripped) = rel_str.strip_suffix(".patch") {
                        out.push(PathBuf::from(stripped));
                    }
                }
            }
        }
    }
}

/// Clean up empty directories up to (but not including) the stop directory
fn cleanup_empty_dirs(dir: &Path, stop_at: &Path) -> Result<()> {
    if dir == stop_at {
        return Ok(());
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        let count = entries.count();
        if count == 0 {
            std::fs::remove_dir(dir)?;
            if let Some(parent) = dir.parent() {
                return cleanup_empty_dirs(parent, stop_at);
            }
        }
    }
    Ok(())
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_no_patches() {
        let mut generated = HashMap::new();
        generated.insert(PathBuf::from("src/main.py"), "print('hello')\n".to_string());

        let mut actual = HashMap::new();
        actual.insert(PathBuf::from("src/main.py"), "print('hello')\n".to_string());

        let patches = PatchStore::detect_patches(&generated, &actual);
        assert!(patches.is_empty());
    }

    #[test]
    fn test_detect_single_patch() {
        let mut generated = HashMap::new();
        generated.insert(
            PathBuf::from("src/schemas/user.py"),
            "class User:\n    updated_at: datetime\n".to_string(),
        );

        let mut actual = HashMap::new();
        actual.insert(
            PathBuf::from("src/schemas/user.py"),
            "class User:\n    updated_at: Optional[datetime] = None\n".to_string(),
        );

        let patches = PatchStore::detect_patches(&generated, &actual);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].output_path, PathBuf::from("src/schemas/user.py"));
        assert_eq!(patches[0].lines_removed, 1);
        assert_eq!(patches[0].lines_added, 1);
        assert!(patches[0].diff.contains("--- a/src/schemas/user.py"));
        assert!(patches[0].diff.contains("+++ b/src/schemas/user.py"));
    }

    #[test]
    fn test_detect_multiple_patches() {
        let mut generated = HashMap::new();
        generated.insert(PathBuf::from("a.py"), "line1\n".to_string());
        generated.insert(PathBuf::from("b.py"), "line1\n".to_string());

        let mut actual = HashMap::new();
        actual.insert(PathBuf::from("a.py"), "line1\nline2\n".to_string());
        actual.insert(PathBuf::from("b.py"), "line1\n".to_string()); // unchanged

        let patches = PatchStore::detect_patches(&generated, &actual);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].output_path, PathBuf::from("a.py"));
    }

    #[test]
    fn test_save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());
        store.init().unwrap();

        let original = "old content\n";
        let manual = "new content\n";
        store
            .save_patch(Path::new("src/user.py"), original, manual)
            .unwrap();

        let loaded = store.load_patch(Path::new("src/user.py")).unwrap();
        assert_eq!(loaded.original_content, original);
        assert_eq!(loaded.manual_content, manual);
        assert!(loaded.diff.contains("--- a/src/user.py"));
        assert!(loaded.diff.contains("+++ b/src/user.py"));
    }

    #[test]
    fn test_load_missing_patch() {
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());
        store.init().unwrap();

        assert!(store.load_patch(Path::new("nonexistent.py")).is_none());
    }

    #[test]
    fn test_list_patches() {
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());
        store.init().unwrap();

        store.save_patch(Path::new("src/a.py"), "old1", "new1").unwrap();
        store.save_patch(Path::new("src/b.py"), "old2", "new2").unwrap();
        store.save_patch(Path::new("tests/test_a.py"), "old3", "new3").unwrap();

        let patches = store.list_patches();
        assert_eq!(patches.len(), 3);
        assert!(patches.contains(&PathBuf::from("src/a.py")));
        assert!(patches.contains(&PathBuf::from("src/b.py")));
        assert!(patches.contains(&PathBuf::from("tests/test_a.py")));
    }

    #[test]
    fn test_drop_patch() {
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());
        store.init().unwrap();

        store.save_patch(Path::new("src/user.py"), "old", "new").unwrap();
        assert!(store.has_patch(Path::new("src/user.py")));

        store.drop_patch(Path::new("src/user.py")).unwrap();
        assert!(!store.has_patch(Path::new("src/user.py")));
    }

    #[test]
    fn test_apply_patch_no_llm_change() {
        // LLM produces the same content as before — user's edits apply cleanly
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());

        let original = "line1\nline2\nline3\n";
        let manual = "line1\nline2_edited\nline3\n";
        let new_gen = "line1\nline2\nline3\n"; // same as original

        match store.apply_patch(original, new_gen, manual) {
            PatchResult::Applied(content) => {
                assert!(content.contains("line2_edited"), "User edit should be preserved");
                assert!(!content.contains("line2\n"), "Original line should be replaced");
            }
            PatchResult::Conflict(_) => panic!("Expected clean apply, got conflict"),
        }
    }

    #[test]
    fn test_apply_patch_non_overlapping_changes() {
        // User edits line 2, LLM edits line 4 — both should apply
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());

        let original = "line1\nline2\nline3\nline4\nline5\n";
        let manual = "line1\nline2_user\nline3\nline4\nline5\n"; // user changed line2
        let new_gen = "line1\nline2\nline3\nline4_llm\nline5\n"; // LLM changed line4

        match store.apply_patch(original, new_gen, manual) {
            PatchResult::Applied(content) => {
                assert!(content.contains("line2_user"), "User edit should be applied");
                assert!(content.contains("line4_llm"), "LLM edit should be preserved");
            }
            PatchResult::Conflict(_) => panic!("Expected clean apply, got conflict"),
        }
    }

    #[test]
    fn test_apply_patch_conflict() {
        // Both user and LLM edit the same line — conflict
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());

        let original = "line1\nline2\nline3\n";
        let manual = "line1\nline2_user\nline3\n"; // user changed line2
        let new_gen = "line1\nline2_llm\nline3\n"; // LLM also changed line2

        match store.apply_patch(original, new_gen, manual) {
            PatchResult::Conflict(content) => {
                assert!(content.contains("<<<<<<<"), "Should have conflict markers");
                assert!(content.contains("======="), "Should have conflict markers");
                assert!(content.contains(">>>>>>>"), "Should have conflict markers");
            }
            PatchResult::Applied(_) => panic!("Expected conflict, got clean apply"),
        }
    }

    #[test]
    fn test_has_patch() {
        let dir = tempfile::tempdir().unwrap();
        let store = PatchStore::new(dir.path().to_path_buf());
        store.init().unwrap();

        assert!(!store.has_patch(Path::new("src/user.py")));
        store.save_patch(Path::new("src/user.py"), "old", "new").unwrap();
        assert!(store.has_patch(Path::new("src/user.py")));
    }

    #[test]
    fn test_patch_file_path() {
        let store = PatchStore::new(PathBuf::from(".lit/patches"));
        let path = store.patch_file_path(Path::new("src/schemas/user.py"));
        assert_eq!(
            path,
            PathBuf::from(".lit/patches/src/schemas/user.py.patch")
        );
    }
}
