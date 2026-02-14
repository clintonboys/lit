# lit — Prompt-First Version Control

## v1 Specification

**Status**: Draft
**Date**: 2025-02-14

---

## 1. Overview

`lit` is a version control system where **prompts are the source of truth** and generated code is a derived artifact. Developers write prompts in markdown. `lit` compiles those prompts into code via LLMs, tracks history, and manages the prompt-to-code pipeline.

Generated code lives in `code.lock/` — a pinned, reproducible output analogous to `package-lock.json`. The prompts are the source. The code is the build artifact.

`lit` wraps `git`. Under the hood, git provides the storage layer (commits, branches, history, remotes). Lit provides a prompt-oriented workflow on top: it understands prompts, manages the generation pipeline, tracks the DAG, and presents a prompt-first interface. This means lit repos are valid git repos — they can be hosted on GitHub, use existing CI/CD, and interoperate with standard git tooling when needed.

---

## 2. Core Concepts

### 2.1 Prompts as Source

A prompt is a markdown file (`.prompt.md`) that describes what code to generate. Prompts live in the `prompts/` directory. Each prompt declares its outputs, dependencies, and optionally overrides model settings.

### 2.2 `code.lock/` — The Generated Artifact

`code.lock/` is a directory tree containing all generated source files. It is fully derived from prompts. In the default workflow it is **read-only** — developers do not hand-edit files in `code.lock/`.

### 2.3 The DAG

Prompts can depend on other prompts. `lit` resolves these dependencies into a directed acyclic graph (DAG) and generates code in topological order. When prompt B imports prompt A, lit generates A first, then feeds A's output as context when generating B.

### 2.4 Git as Storage Layer

Lit uses git for all version control operations. A lit repo **is** a git repo with additional structure:
- The `.git/` directory stores all history (git's native object model)
- The `.lit/` directory stores lit-specific metadata (DAG state, generation records, cost tracking)
- `lit commit` performs a git commit under the hood, plus records generation metadata in `.lit/`

This means:
- `git log` works on a lit repo (you see standard git commits)
- `git push` / `git pull` work for remotes (GitHub, GitLab, etc.)
- `git branch` / `git merge` work on prompts just like normal files
- Lit commands (`lit diff`, `lit log`, `lit status`) add prompt-aware intelligence on top

### 2.5 Commits

A `lit commit` creates a **git commit** containing:
- All prompts
- All generated code (`code.lock/`)
- The `lit.toml` configuration

Plus lit records in `.lit/generations/`:
- The model configuration used for this generation
- The dependency DAG snapshot
- Token usage and cost metadata per prompt

---

## 3. Repository Structure

```
my-project/
  .git/                  # git's storage layer (standard git repo)
  .lit/                  # lit-specific metadata
    generations/         # generation records indexed by git commit hash
    cache/               # input-hash cache for skipping unchanged generations
    config               # local config (API keys — in .gitignore)
  .gitignore             # ignores .lit/config, .env, etc.
  lit.toml               # project configuration (committed)
  prompts/               # the source — prompt files (committed)
    auth/
      login.prompt.md
      signup.prompt.md
    models/
      user.prompt.md
  code.lock/             # the generated artifact (committed, read-only)
    src/
      auth/
        login.py
        signup.py
      models/
        user.py
```

---

## 4. Project Configuration — `lit.toml`

```toml
[project]
name = "my-app"
version = "0.1.0"
mapping = "manifest"     # one of: direct, manifest, modular, inferred

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
# API key resolved from environment variable
key_env = "LIT_API_KEY"
```

### 4.1 Mapping Modes

The `mapping` field in `lit.toml` selects how prompts map to generated code files. This is a repo-wide setting.

| Mode | Value | Description |
|------|-------|-------------|
| Direct | `direct` | 1 prompt = 1 file. Output path mirrors the prompt path. `prompts/models/user.prompt.md` generates `code.lock/models/user.py`. |
| Manifest | `manifest` | 1 prompt = N files. Each prompt declares its output files in frontmatter. **Recommended for v1.** |
| Modular | `modular` | A directory of prompts forms a "module". A `module.lit.md` file provides module-level config. Multiple prompt files within the directory contribute context to generation. |
| Inferred | `inferred` | The LLM decides what files to create. Lit captures whatever it produces. Least predictable, most flexible. |

**v1 recommendation**: Use `manifest` mode. It balances flexibility with predictability.

**Constraint (v1)**: No two prompts may declare the same output file. Each file in `code.lock/` is owned by exactly one prompt. Sub-file merging is a future feature.

---

## 5. Prompt Format

Prompts are markdown files with YAML frontmatter.

### 5.1 Example Prompt

```markdown
---
outputs:
  - src/models/user.py
  - tests/test_user.py
imports:
  - prompts/models/base.prompt.md
model:
  provider: anthropic
  model: claude-opus-4-6
  temperature: 0.0
---

# User Model

Create a SQLAlchemy User model with the following fields:

- `id`: UUID primary key, auto-generated
- `email`: unique, indexed, not null
- `password_hash`: string, not null
- `created_at`: timestamp with timezone, defaults to now
- `is_active`: boolean, defaults to true

Use the base model class from @import(prompts/models/base.prompt.md).

Also generate a comprehensive test suite using pytest.
```

### 5.2 Frontmatter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `outputs` | Yes (in `manifest` mode) | List of file paths (relative to `code.lock/`) this prompt generates. |
| `imports` | No | List of prompt paths this prompt depends on. Their generated code is fed as context. |
| `model` | No | Per-prompt model override. Inherits from `lit.toml` if absent. |
| `language` | No | Override the project default language for this prompt. |

### 5.3 Import Syntax

Within the markdown body, `@import(path/to/prompt.prompt.md)` references another prompt. This is syntactic sugar — the actual dependency is declared in `imports` frontmatter. The inline reference tells the LLM "use the code generated from that prompt."

When generating code for a prompt with imports, lit:
1. Resolves the import DAG
2. Generates imported prompts first (if not already generated)
3. Includes the generated code from imports as context in the LLM call

---

## 6. The Dependency DAG

### 6.1 Resolution

On `lit commit` or `lit regenerate`, lit:

1. Parses all prompts and their `imports` fields
2. Builds a DAG
3. Detects cycles (error if found)
4. Topologically sorts the DAG
5. Generates code in order, passing upstream outputs as context to downstream prompts

### 6.2 Change Detection

When a prompt changes, lit determines what to regenerate using two signals:

1. **Declared outputs** (frontmatter): The prompt says what files it produces.
2. **Historical tracking**: Lit records what files each prompt actually produced in the last commit. If a prompt's outputs change between commits, lit handles the diff (removes orphaned files, creates new ones).

**Cascade rule**: If prompt A changes and prompt B imports A, then B is also regenerated. Changes propagate downstream through the DAG.

---

## 7. Generation Pipeline

When `lit commit` runs:

```
1. Parse all prompts
2. Build DAG, topological sort
3. Detect which prompts changed since last commit
4. Determine regeneration set (changed prompts + downstream dependents)
5. For each prompt in topological order:
   a. Gather context:
      - Project config (language, framework)
      - Imported prompts' generated code
      - The prompt body itself
   b. Call LLM with assembled context
   c. Parse LLM response into output files
   d. Write files to code.lock/
   e. Record generation metadata (tokens, cost, model, seed)
6. Snapshot everything into a commit object
```

### 7.1 LLM Request Structure

Each generation call sends:

```
System: You are generating code for a {language} project using {framework}.
        Output format: For each file, use the header "=== FILE: path/to/file ==="
        followed by the file contents.

Context: [generated code from imported prompts]

User: [prompt body]
```

The output delimiter (`=== FILE: ... ===`) lets lit parse multi-file responses.

### 7.2 Determinism

To maximize reproducibility:
- Model, temperature, and seed are pinned in config
- The exact model identifier is stored per commit
- If the model provider supports deterministic output (e.g., seed parameter), lit uses it

**Note**: Perfect determinism is not guaranteed across model versions. The commit stores both the prompt inputs and generated outputs, so history is always reproducible by replaying the stored outputs rather than re-calling the LLM.

---

## 8. Commands

### 8.1 `lit init`

Initialize a new lit repository.

```
$ lit init
```

Under the hood:
1. Runs `git init` (if not already a git repo)
2. Creates `lit.toml` with defaults (prompts user for language, framework, model provider)
3. Creates `prompts/` directory
4. Creates `code.lock/` directory
5. Creates `.lit/` metadata directory
6. Creates `.gitignore` (ignoring `.lit/config`, `.env`, etc.)
7. Creates initial git commit ("lit init")

### 8.2 `lit add <path>`

Track a new or modified prompt file.

```
$ lit add prompts/models/user.prompt.md
$ lit add prompts/auth/              # add all prompts in directory
$ lit add .                          # add all prompts
```

Adds prompts to the "tracked" set. In v1 there is no staging area — `lit add` marks prompts as tracked, and `lit commit` operates on all tracked prompts.

### 8.3 `lit commit`

Generate code from changed prompts and create a commit.

```
$ lit commit -m "Add user model and auth endpoints"
```

Steps:
1. Identify changed prompts (vs. last git commit)
2. Run the generation pipeline (Section 7)
3. Write generation metadata to `.lit/generations/<git-commit-hash>.json`
4. `git add` all prompts, `code.lock/`, `lit.toml`, and `.lit/generations/`
5. `git commit` with the provided message

The git commit contains the full snapshot. The `.lit/generations/` record adds lit-specific metadata (DAG, tokens, cost, model config) keyed by git commit hash.

### 8.4 `lit status`

Show the state of prompts and generated code.

```
$ lit status

On commit a1b2c3d

Changes not yet committed:
  modified:   prompts/models/user.prompt.md
  new:        prompts/api/orders.prompt.md

code.lock/ is up to date with last commit.
```

If `code.lock/` has been hand-edited (diverged from what lit generated):
```
code.lock/ has diverged from prompts:
  modified:   code.lock/src/models/user.py  (hand-edited)
```

### 8.5 `lit diff`

Show prompt changes since last commit.

```
$ lit diff                        # prompt diffs (default)
$ lit diff --code                 # generated code diffs
$ lit diff --all                  # both
$ lit diff <commit1> <commit2>   # diff between two commits
```

Default is prompt-only diffs. Code diffs are available with `--code`.

### 8.6 `lit log`

Show commit history.

```
$ lit log

commit a1b2c3d
Date:   2025-02-14 10:30:00
Model:  claude-sonnet-4-5-20250929
Cost:   $0.03 (1,247 tokens)

    Add user model and auth endpoints

commit f4e5d6c
Date:   2025-02-14 09:15:00
Model:  claude-sonnet-4-5-20250929
Cost:   $0.01 (523 tokens)

    Initial project setup
```

### 8.7 `lit regenerate`

Re-derive `code.lock/` from current prompts without creating a commit.

```
$ lit regenerate                  # regenerate all
$ lit regenerate prompts/auth/    # regenerate specific prompts + dependents
```

Useful for:
- Testing prompt changes before committing
- Recovering if `code.lock/` was accidentally modified
- Verifying reproducibility

### 8.8 `lit checkout <ref>`

Restore prompts and generated code from a previous commit.

```
$ lit checkout a1b2c3d
$ lit checkout HEAD~3
```

Under the hood: runs `git checkout`. Restores both prompts and `code.lock/` to the exact state at that commit. Does not regenerate — uses the stored outputs (they're in the git commit).

### 8.9 `lit push` / `lit pull` / `lit clone`

Thin wrappers around git remotes.

```
$ lit push                          # git push
$ lit pull                          # git pull + validate lit structure
$ lit clone <url>                   # git clone + validate lit structure
```

These exist so the lit CLI is self-contained. Developers can also use `git push`, `git pull`, `git clone` directly — the repo is a valid git repo.

`lit clone` additionally verifies that the cloned repo has a valid lit structure (`lit.toml`, `prompts/`, `code.lock/`) and warns if `.lit/config` needs to be set up (API keys).

### 8.10 `lit cost`

Show token and cost tracking.

```
$ lit cost                          # total spend across all commits
$ lit cost --last                   # cost of last commit
$ lit cost --breakdown              # per-prompt cost breakdown
```

---

## 9. Storage Architecture

### 9.1 Two-Layer Model

Lit uses a two-layer storage architecture:

| Layer | Location | Contents | Committed to git? |
|-------|----------|----------|-------------------|
| **Git layer** | `.git/` | All file history (prompts, code.lock, lit.toml) | N/A (is git) |
| **Lit metadata layer** | `.lit/` | Generation records, DAG snapshots, cost data, local config | Partially (see below) |

**What's committed** (in `.lit/`):
- `.lit/generations/` — generation metadata per commit (DAG, tokens, cost, model config)

**What's local-only** (in `.gitignore`):
- `.lit/config` — API keys, local preferences
- `.lit/cache/` — input-hash cache for skipping unchanged generations

### 9.2 `.lit/` Directory Layout

```
.lit/
  config                  # local config: API keys, user prefs (gitignored)
  cache/                  # input-hash → output cache (gitignored)
    <sha256>.json         # cached generation result
  generations/            # generation metadata (committed)
    <git-commit-hash>.json
```

### 9.3 Generation Record Structure

Each `lit commit` writes a generation record at `.lit/generations/<git-commit-hash>.json`:

```json
{
  "git_commit": "a1b2c3d...",
  "timestamp": "2025-02-14T10:30:00Z",
  "dag": {
    "prompts/models/user.prompt.md": {
      "imports": [],
      "outputs": ["src/models/user.py", "tests/test_user.py"],
      "input_hash": "abc123..."
    },
    "prompts/auth/login.prompt.md": {
      "imports": ["prompts/models/user.prompt.md"],
      "outputs": ["src/auth/login.py"],
      "input_hash": "def456..."
    }
  },
  "model_config": {
    "provider": "anthropic",
    "model": "claude-sonnet-4-5-20250929",
    "temperature": 0.0,
    "seed": 42
  },
  "generation_metadata": {
    "total_tokens": 1247,
    "total_cost_usd": 0.03,
    "duration_ms": 4521,
    "prompts_regenerated": ["prompts/models/user.prompt.md"],
    "prompts_cached": ["prompts/auth/login.prompt.md"],
    "per_prompt": {
      "prompts/models/user.prompt.md": {
        "tokens_in": 340,
        "tokens_out": 907,
        "cost_usd": 0.02,
        "duration_ms": 3200,
        "cached": false
      }
    }
  }
}
```

### 9.4 Input-Hash Caching

To avoid regenerating unchanged prompts, lit computes an **input hash** for each prompt:

```
input_hash = SHA-256(
  prompt_content +
  imported_prompts_input_hashes +
  model_config +
  project_language +
  project_framework
)
```

If a prompt's input hash matches the cached value, its generation is skipped and the previous output is reused. This is inspired by Bazel's input-addressed caching.

### 9.5 Interoperability with Git

Because lit wraps git, standard git operations work:

| Operation | Git command | Lit equivalent |
|-----------|-------------|----------------|
| View history | `git log` | `lit log` (adds cost/token info) |
| Push to remote | `git push` | `lit push` (thin wrapper) |
| Pull from remote | `git pull` | `lit pull` (thin wrapper) |
| Clone | `git clone` | `lit clone` (clone + validate lit structure) |
| Branch | `git branch` | Works directly, no lit wrapper needed in v1 |
| Merge | `git merge` | Works directly on prompts (they're just files) |
| Diff | `git diff` | `lit diff` (prompt-aware, hides code.lock by default) |
| Blame | `git blame` | Works on prompts directly |

Developers can always "escape hatch" to raw git commands. Lit commands are convenience wrappers that add prompt-awareness.

---

## 10. Cost and Token Tracking

Every generation records:
- Input tokens
- Output tokens
- Estimated cost (based on known model pricing)
- Duration

Accessible via:
```
$ lit log              # shows cost per commit
$ lit cost             # summary of total spend
$ lit cost --breakdown # per-prompt cost breakdown
```

---

## 11. Error Handling

### 11.1 Generation Failures

If the LLM fails to generate valid code for a prompt:
- Lit reports the error with the prompt path and LLM response
- The commit is aborted (no partial commits)
- Previously generated files in the pipeline are discarded
- The user fixes the prompt and retries

### 11.2 DAG Cycles

If circular imports are detected:
```
$ lit commit
Error: Circular dependency detected:
  prompts/a.prompt.md → prompts/b.prompt.md → prompts/a.prompt.md
```

### 11.3 Output Conflicts

If two prompts declare the same output file:
```
$ lit commit
Error: Output conflict — multiple prompts claim "src/models/user.py":
  - prompts/models/user.prompt.md
  - prompts/legacy/user.prompt.md
```

---

## 12. `.litignore`

Similar to `.gitignore`. Specifies files/patterns that lit should not track or include.

```
# .litignore
.env
*.secret
node_modules/
__pycache__/
```

---

## 13. Future Work (Post-v1)

The following are explicitly out of scope for v1 but inform the architecture:

- **Staging area**: `lit add` as a true staging step before commit
- **Lit forge**: Custom server with prompt-aware collaboration (beyond what GitHub provides)
- **`lit build` / `lit run`**: Build and execute generated code
- **Sub-file merging**: Multiple prompts contributing to the same file via AST-aware merging
- **Multi-language monorepo**: Different languages per prompt in the same repo
- **Reconciliation**: `lit reconcile` to sync hand-edited code back into prompts
- **`lit review`**: PR-like review workflow for prompt changes (could layer on GitHub PRs)
- **Prompt linting**: Static analysis of prompts for common issues
- **Model migration**: Tools to re-pin prompts to newer models
- **`lit diff --summary`**: LLM-generated human-readable summary of code changes
- **Prompt optimization**: Auto-tune prompt wording based on output quality (inspired by DSPy)

**Note**: Branching, merging, and remotes are available in v1 via git. Future versions may add prompt-aware wrappers (e.g., `lit merge` that understands DAG conflicts).

---

## 14. Design Rationale

### 14.1 Why Wrap Git (Not Replace It)

An earlier version of this spec had lit replacing git entirely with its own commit/branch/history model. Research into prior art (Eve, various MDD tools, Dark) showed that tools requiring developers to abandon familiar workflows face severe adoption risk.

By wrapping git:
- **Zero friction for remotes**: GitHub, GitLab, Bitbucket work out of the box. No custom "forge" server needed for collaboration.
- **Branching/merging for free**: Git's battle-tested merge machinery works on prompts (they're text files). No need to build our own.
- **Escape hatch**: Developers can always `git log`, `git blame`, `git bisect` their way through the repo. Lit doesn't trap them.
- **CI/CD integration**: Existing CI/CD systems understand git. Lit repos trigger builds, PRs, and deployments without custom integration.
- **Incremental adoption**: Teams can start using lit in an existing git repo without migrating history.

The cost is that lit's metadata (`.lit/generations/`) lives alongside git's metadata, creating a two-layer model. This is manageable — it's the same pattern as tools like Terraform (wraps cloud APIs) and Prisma (wraps SQL).

### 14.2 Why Commit Generated Code

The software community debates whether generated code should be committed. Lit commits `code.lock/` because:
1. **LLM generation is non-deterministic**: Unlike protobuf or codegen, the same prompt may produce different code across runs. Committing the output ensures reproducibility.
2. **Generation is expensive**: LLM API calls cost money and take time. Not every developer should need to regenerate.
3. **Code review**: Reviewers may want to inspect what the LLM actually produced, not just the prompt.
4. **Offline access**: The codebase works without API keys or internet access.

### 14.3 Conceptual Lineage

```
Literate Programming (Knuth 1984)
    Source: interleaved prose + code → Tangle extracts code
                     |
                     | "What if code wasn't hand-written?"
                     v
Schema-based Codegen (Protobuf, OpenAPI, Prisma)
    Source: formal spec → Deterministic compiler → code
                     |
                     | "What if the spec was natural language?"
                     v
lit (2025)
    Source: markdown prompts → LLM "compiler" → code.lock/
```

---

## 15. Implementation Notes

### 15.1 Language

`lit` is implemented in **Rust**.

### 15.2 Key Crates (Preliminary)

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing |
| `serde` / `serde_json` / `toml` | Config and data serialization |
| `sha2` | Input-hash caching |
| `reqwest` | HTTP client for LLM API calls |
| `tokio` | Async runtime for concurrent generation |
| `comrak` | Markdown/frontmatter parsing |
| `similar` | Diffing algorithm (for prompt-aware diffs) |
| `git2` | Libgit2 bindings — git operations without shelling out |

### 15.3 Concurrency

Prompts at the same level of the DAG (no interdependencies) can be generated concurrently. Lit should parallelize LLM calls where the DAG allows.

### 15.4 Project Structure (Rust)

```
src/
  main.rs               # CLI entry point
  cli/                   # command implementations
    init.rs
    add.rs
    commit.rs
    diff.rs
    status.rs
    log.rs
    regenerate.rs
    checkout.rs
    push.rs
    pull.rs
    clone.rs
    cost.rs
  core/
    config.rs            # lit.toml parsing
    prompt.rs            # prompt parsing (frontmatter + body)
    dag.rs               # dependency resolution
    generator.rs         # LLM interaction and code generation
    cache.rs             # input-hash caching
    repo.rs              # repository operations (wraps git2)
    diff.rs              # prompt-aware diffing logic
    generation_record.rs # .lit/generations/ read/write
  providers/
    mod.rs               # trait for LLM providers
    anthropic.rs
    openai.rs
```
