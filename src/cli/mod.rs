pub mod init;
pub mod add;
pub mod commit;
pub mod diff;
pub mod status;
pub mod log;
pub mod regenerate;
pub mod checkout;
pub mod push;
pub mod pull;
pub mod clone;
pub mod cost;
pub mod debug;
pub mod patch;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "lit")]
#[command(about = "Prompt-first version control — prompts are source, code is the artifact")]
#[command(version)]
pub struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new lit repository
    Init {
        /// Use default settings (non-interactive)
        #[arg(long)]
        defaults: bool,
    },

    /// Track a new or modified prompt file
    Add {
        /// Path to a .prompt.md file or directory
        path: PathBuf,
    },

    /// Generate code from changed prompts and create a commit
    Commit {
        /// Commit message
        #[arg(short, long)]
        message: String,
    },

    /// Show the state of prompts and generated code
    Status,

    /// Show prompt changes since last commit
    Diff {
        /// Show generated code diffs instead of prompt diffs
        #[arg(long)]
        code: bool,

        /// Show both prompt and code diffs
        #[arg(long)]
        all: bool,

        /// Show structured change summary with DAG impact analysis
        #[arg(long)]
        summary: bool,
    },

    /// Show commit history
    Log {
        /// Maximum number of commits to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },

    /// Re-derive code.lock/ from current prompts without committing
    Regenerate {
        /// Path to specific prompt or directory to regenerate
        path: Option<PathBuf>,

        /// Force regenerate all prompts (ignore cache)
        #[arg(long)]
        all: bool,

        /// Skip the input-hash cache (force fresh LLM calls)
        #[arg(long)]
        no_cache: bool,

        /// Ignore manual patches (regenerate purely from prompts)
        #[arg(long)]
        no_patches: bool,
    },

    /// Manage manual patches to generated code
    Patch {
        #[command(subcommand)]
        action: PatchCommands,
    },

    /// Restore prompts and code from a previous commit
    Checkout {
        /// Commit hash or ref (e.g., HEAD~3)
        #[arg(name = "ref")]
        ref_: String,
    },

    /// Push to remote (thin wrapper around git push)
    Push,

    /// Pull from remote (thin wrapper around git pull)
    Pull,

    /// Clone a lit repository
    Clone {
        /// Repository URL
        url: String,
    },

    /// Show token and cost tracking
    Cost {
        /// Show cost of last commit only
        #[arg(long)]
        last: bool,

        /// Show per-prompt cost breakdown
        #[arg(long)]
        breakdown: bool,
    },

    /// Inspect internal state (config, prompts, DAG)
    Debug {
        /// What to inspect
        #[command(subcommand)]
        what: DebugCommands,
    },
}

#[derive(Subcommand)]
pub enum PatchCommands {
    /// Save current manual edits to code.lock/ as patches
    Save,
    /// List all tracked patches
    List,
    /// Discard a patch (the prompt version wins)
    Drop {
        /// Output file path to drop the patch for
        path: PathBuf,
    },
    /// Show the diff for a specific patch
    Show {
        /// Output file path to show the patch for
        path: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum DebugCommands {
    /// Dump parsed lit.toml config
    Config,
    /// Dump all parsed prompts with frontmatter
    Prompts,
    /// Show the dependency DAG
    Dag,
    /// Show everything (config + prompts + DAG)
    All,
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        match self.command {
            Commands::Init { defaults } => init::run(defaults).await,
            Commands::Add { path } => add::run(path).await,
            Commands::Commit { message } => commit::run(message).await,
            Commands::Status => status::run().await,
            Commands::Diff { code, all, summary } => diff::run(code, all, summary).await,
            Commands::Log { limit } => log::run(limit).await,
            Commands::Regenerate { path, all, no_cache, no_patches } => {
                regenerate::run(path, all, no_cache, no_patches).await
            }
            Commands::Patch { action } => patch::run(action).await,
            Commands::Checkout { ref_ } => checkout::run(ref_).await,
            Commands::Push => push::run().await,
            Commands::Pull => pull::run().await,
            Commands::Clone { url } => clone::run(url).await,
            Commands::Cost { last, breakdown } => cost::run(last, breakdown).await,
            Commands::Debug { what } => debug::run(what).await,
        }
    }
}
