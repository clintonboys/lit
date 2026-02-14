//! Consistent colored terminal output for lit CLI.
//!
//! All user-facing output goes through these helpers so colors are uniform.
//! Colors are automatically disabled when stdout/stderr is not a TTY.

use colored::Colorize;

// ---------- Prefixes / Labels ----------

/// Format a header line: "=== Title ==="
pub fn header(title: &str) -> String {
    format!("=== {} ===", title.bold())
}

/// Format a sub-header: "lit: project-name v0.1.0"
pub fn project_header(name: &str, version: &str) -> String {
    format!("{}: {} {}", "lit".bold().cyan(), name.bold(), format!("v{}", version).dimmed())
}

// ---------- Status indicators ----------

/// Green checkmark + message (success)
pub fn success(msg: &str) -> String {
    format!("{} {}", "✓".green().bold(), msg)
}

/// Yellow warning + message
pub fn warning(msg: &str) -> String {
    format!("{} {}", "⚠".yellow().bold(), msg)
}

/// Red error + message
pub fn error(msg: &str) -> String {
    format!("{} {}", "✗".red().bold(), msg)
}

/// Dim info/hint message
pub fn hint(msg: &str) -> String {
    format!("{}", msg.dimmed())
}

// ---------- File change indicators ----------

/// Green "+" for new files
pub fn file_new(path: &str) -> String {
    format!("  {} {}", "+".green().bold(), path.green())
}

/// Yellow "~" for modified files
pub fn file_modified(path: &str) -> String {
    format!("  {} {}", "~".yellow().bold(), path.yellow())
}

/// Red "-" for deleted files
pub fn file_deleted(path: &str) -> String {
    format!("  {} {}", "-".red().bold(), path.red())
}

// ---------- Progress ----------

/// Progress counter: "(1/12)"
pub fn progress(current: usize, total: usize) -> String {
    format!("{}", format!("({}/{})", current, total).dimmed())
}

/// Format a generating step: "  Generating prompts/foo.prompt.md... (1/3)"
pub fn generating(prompt_path: &str, current: usize, total: usize) -> String {
    format!(
        "  {} {} {}",
        "Generating".cyan(),
        prompt_path.bold(),
        progress(current, total)
    )
}

/// Format a cached hit: "  ✓ prompts/foo.prompt.md (cached)"
pub fn cached(prompt_path: &str) -> String {
    format!(
        "  {} {} {}",
        "✓".green().bold(),
        prompt_path,
        "(cached)".dimmed()
    )
}

/// Format a skipped prompt
pub fn skipped(prompt_path: &str) -> String {
    format!(
        "  {} {} {}",
        "—".dimmed(),
        prompt_path.dimmed(),
        "(skipped)".dimmed()
    )
}

/// Format generation result: "    ✓ 2 file(s), 1,234 in / 567 out, 2.3s"
pub fn gen_result(files: usize, tokens_in: u64, tokens_out: u64, duration_ms: u64) -> String {
    format!(
        "    {} {} {}, {}",
        "✓".green().bold(),
        format!("{} file(s)", files).bold(),
        format!("{} in / {} out tokens", tokens_in, tokens_out).dimmed(),
        format!("{:.1}s", duration_ms as f64 / 1000.0).dimmed(),
    )
}

// ---------- Summary formatting ----------

/// Format a key-value summary line with aligned values
pub fn summary_line(key: &str, value: &str) -> String {
    format!("  {:<20} {}", format!("{}:", key).dimmed(), value)
}

/// Format a cost value
pub fn cost(amount: &str) -> String {
    format!("{}", amount.yellow())
}

/// Format a commit hash (short, colored)
pub fn commit_hash(hash: &str) -> String {
    format!("{}", hash.yellow())
}

/// Format a commit message
pub fn commit_message(msg: &str) -> String {
    msg.to_string()
}

/// Format a datetime string (dimmed)
pub fn datetime(dt: &str) -> String {
    format!("{}", dt.dimmed())
}

// ---------- Section headers ----------

/// Bold section label: "New prompts:", "Modified code files:", etc.
pub fn section(label: &str) -> String {
    format!("{}", label.bold())
}

/// Format the "lit regenerate" header line
pub fn regen_header(regen_count: usize, total_count: usize) -> String {
    format!(
        "{} {} prompt(s) to generate {}",
        "lit regenerate:".bold().cyan(),
        regen_count.to_string().bold(),
        format!("(of {} total)", total_count).dimmed()
    )
}

// ---------- Patch indicators ----------

/// Green checkmark for applied patch
pub fn patch_applied(file_path: &str) -> String {
    format!(
        "    {} Applied manual patch to {}",
        "✓".green().bold(),
        file_path.bold()
    )
}

/// Yellow warning for conflicted patch
pub fn patch_conflict(file_path: &str) -> String {
    format!(
        "    {} Conflict in {} {}",
        "⚠".yellow().bold(),
        file_path.bold(),
        "(manual patch vs new generation)".dimmed()
    )
}
