# lit — v1 Implementation Plan

## Build Philosophy

Each milestone produces a working binary that does something useful. No milestone depends on code that hasn't been built yet. We build from the inside out: core data structures first, then the pipeline, then the CLI commands.

**Testing**: Every module gets unit tests as it's built. Integration tests come at each milestone boundary.

---

## Dependency Graph (what must exist before what)

```
lit.toml parsing (config)
    ↓
prompt parsing (frontmatter + body)
    ↓
DAG resolution (imports, topological sort, cycle detection)
    ↓
LLM provider trait + Anthropic implementation
    ↓
generation pipeline (assemble context → call LLM → parse response → write files)
    ↓
input-hash caching
    ↓
generation records (.lit/generations/)
    ↓
git operations (repo.rs wrapping git2)
    ↓
CLI commands (init, add, commit, status, diff, log, checkout, regenerate, push, pull, clone, cost)
```

---

## Milestone 0: Project Skeleton

**Goal**: `cargo build` succeeds, CLI parses commands, nothing actually works yet.

### Tasks

1. `cargo init lit` — create the Rust project
2. Set up `Cargo.toml` with all dependencies:
   ```toml
   [dependencies]
   clap = { version = "4", features = ["derive"] }
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   serde_yaml = "0.9"
   toml = "0.8"
   sha2 = "0.10"
   reqwest = { version = "0.12", features = ["json"] }
   tokio = { version = "1", features = ["full"] }
   comrak = "0.28"
   similar = "2"
   git2 = "0.19"
   anyhow = "1"
   thiserror = "2"
   chrono = { version = "0.4", features = ["serde"] }
   colored = "2"
   ```
3. Create directory structure:
   ```
   src/
     main.rs
     cli/
       mod.rs
       init.rs, add.rs, commit.rs, diff.rs, status.rs,
       log.rs, regenerate.rs, checkout.rs, push.rs,
       pull.rs, clone.rs, cost.rs
     core/
       mod.rs
       config.rs, prompt.rs, dag.rs, generator.rs,
       cache.rs, repo.rs, diff.rs, generation_record.rs
     providers/
       mod.rs
       anthropic.rs, openai.rs
   ```
4. Define the CLI with clap (all commands exist but print "not implemented yet"):
   ```rust
   #[derive(Parser)]
   enum Commands {
       Init, Add { path: PathBuf }, Commit { #[arg(short)] message: String },
       Status, Diff { ... }, Log, Regenerate { path: Option<PathBuf> },
       Checkout { ref_: String }, Push, Pull, Clone { url: String }, Cost { ... },
   }
   ```
5. `cargo build` passes, `lit --help` shows all commands

### Definition of Done
- `lit --help` prints usage with all subcommands
- `lit init` prints "not implemented yet"
- All crate dependencies resolve

---

## Milestone 1: Config + Prompt Parsing

**Goal**: Lit can read `lit.toml` and parse `.prompt.md` files into structured data.

### Tasks

#### 1a. `config.rs` — lit.toml parsing

```rust
pub struct LitConfig {
    pub project: ProjectConfig,    // name, version, mapping mode
    pub language: LanguageConfig,  // default language, version
    pub framework: FrameworkConfig,// name, version
    pub model: ModelConfig,        // provider, model, temperature, seed, key_env
}
```

- Deserialize with `toml` crate
- Validate: mapping mode is one of `direct|manifest|modular|inferred`
- Resolve API key from environment variable (`key_env` → `std::env::var`)
- Error with clear message if `lit.toml` not found or invalid

**Tests**:
- Parse a valid `lit.toml` → all fields correct
- Missing required fields → descriptive error
- Invalid mapping mode → error
- API key resolution from env var

#### 1b. `prompt.rs` — Prompt parsing

```rust
pub struct Prompt {
    pub path: PathBuf,             // relative path from repo root
    pub frontmatter: PromptFrontmatter,
    pub body: String,              // markdown body (everything after frontmatter)
    pub raw: String,               // full file content
}

pub struct PromptFrontmatter {
    pub outputs: Vec<PathBuf>,     // relative to code.lock/
    pub imports: Vec<PathBuf>,     // paths to other .prompt.md files
    pub model: Option<ModelOverride>,
    pub language: Option<String>,
}
```

- Split file on `---` delimiters to extract YAML frontmatter
- Parse frontmatter with `serde_yaml`
- Extract `@import(...)` references from body and validate they match `imports` frontmatter
- Handle all four mapping modes:
  - `manifest`: `outputs` required in frontmatter
  - `direct`: `outputs` derived from prompt path (strip `.prompt.md`, apply language extension)
  - `modular`: look for `module.lit.md` in same directory
  - `inferred`: `outputs` left empty (filled after generation)

**Tests**:
- Parse valid prompt with all frontmatter fields
- Parse prompt with no optional fields (no imports, no model override)
- Missing required `outputs` in manifest mode → error
- `@import()` in body without matching `imports` → warning
- Direct mode path derivation: `prompts/models/user.prompt.md` → `code.lock/models/user.py`

### Definition of Done
- `LitConfig::from_file("lit.toml")` works
- `Prompt::from_file("prompts/foo.prompt.md", &config)` works
- All unit tests pass

---

## Milestone 2: DAG Resolution

**Goal**: Lit can build a dependency graph from a set of prompts, detect cycles, and produce a generation order.

### Tasks

#### 2a. `dag.rs` — Dependency graph

```rust
pub struct Dag {
    nodes: HashMap<PathBuf, DagNode>,  // prompt path → node
    order: Vec<PathBuf>,               // topological order
}

pub struct DagNode {
    pub prompt_path: PathBuf,
    pub imports: Vec<PathBuf>,
    pub dependents: Vec<PathBuf>,      // reverse edges (who depends on me)
    pub outputs: Vec<PathBuf>,
}
```

- `Dag::build(prompts: &[Prompt])` → constructs the graph
- Topological sort (Kahn's algorithm — iterative, no recursion)
- Cycle detection: if sort doesn't consume all nodes, there's a cycle. Report it with the cycle path.
- `Dag::regeneration_set(changed: &[PathBuf])` → returns all prompts that need regeneration (changed + all downstream dependents via transitive closure)
- Validate no output conflicts (two prompts claiming same file)

**Tests**:
- Linear chain: A → B → C → generates order [A, B, C]
- Diamond: A → B, A → C, B → D, C → D → valid order, D last
- Cycle: A → B → A → error with cycle path
- Independent prompts: A, B (no deps) → both in output, order doesn't matter
- Regeneration set: change A in A → B → C → returns {A, B, C}
- Regeneration set: change B in A → B, C (independent) → returns {B} only
- Output conflict detection

### Definition of Done
- DAG builds correctly from parsed prompts
- Cycles detected and reported with path
- Regeneration set computed correctly
- Output conflicts detected
- All unit tests pass

---

## Milestone 3: LLM Provider + Generation Pipeline

**Goal**: Lit can call an LLM and produce code files from a prompt.

### Tasks

#### 3a. `providers/mod.rs` — Provider trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn generate(&self, request: GenerationRequest) -> Result<GenerationResponse>;
    fn name(&self) -> &str;
}

pub struct GenerationRequest {
    pub system_prompt: String,
    pub context: String,       // imported code
    pub user_prompt: String,   // the prompt body
    pub model: String,
    pub temperature: f64,
    pub seed: Option<u64>,
}

pub struct GenerationResponse {
    pub content: String,       // raw LLM response
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub model: String,
}
```

#### 3b. `providers/anthropic.rs` — Anthropic implementation

- Use `reqwest` to call the Anthropic Messages API
- Handle streaming (for progress display) — or start with non-streaming for simplicity
- Parse response to extract content and usage metadata
- Error handling: rate limits (retry with backoff), auth failures, network errors

#### 3c. `providers/openai.rs` — OpenAI implementation

- Same trait, OpenAI Chat Completions API
- Can be a stub in milestone 3 — prioritize Anthropic

#### 3d. `generator.rs` — Generation pipeline

```rust
pub struct Generator {
    provider: Box<dyn LlmProvider>,
    config: LitConfig,
}

impl Generator {
    pub async fn generate_prompt(
        &self,
        prompt: &Prompt,
        context: &HashMap<PathBuf, String>,  // imported prompts' generated code
    ) -> Result<GenerationOutput>;

    pub async fn run_pipeline(
        &self,
        dag: &Dag,
        prompts: &HashMap<PathBuf, Prompt>,
        regeneration_set: &HashSet<PathBuf>,
        cache: &Cache,
    ) -> Result<PipelineResult>;
}

pub struct GenerationOutput {
    pub files: HashMap<PathBuf, String>,  // output path → file content
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
}
```

- `generate_prompt`: Assembles the system prompt, context, and user prompt. Calls the LLM. Parses the response using `=== FILE: path ===` delimiters. Returns structured output.
- `run_pipeline`: Walks the DAG in topological order. For each prompt in the regeneration set:
  1. Check cache (skip if input hash matches)
  2. Gather context from imported prompts' outputs
  3. Call `generate_prompt`
  4. Collect results
- Concurrent generation: prompts at the same DAG level can be generated in parallel with `tokio::join!` or `futures::join_all`

**Response parser**:
```
=== FILE: src/models/user.py ===
class User:
    ...

=== FILE: tests/test_user.py ===
def test_user():
    ...
```
- Split on `=== FILE: ... ===` headers
- Validate output paths match declared `outputs` in frontmatter (in manifest mode)
- In inferred mode, accept whatever paths the LLM produces

**Tests**:
- Mock provider that returns canned responses
- Response parser: single file output
- Response parser: multi-file output
- Response parser: malformed response → error
- Pipeline with linear DAG: A → B, generates in order
- Pipeline with regeneration set: only regenerates changed + dependents
- System prompt assembly includes language and framework

### Definition of Done
- Can call Anthropic API and get a response
- Response parser extracts files correctly
- Pipeline generates code in DAG order
- Mock-based tests pass without API calls

---

## Milestone 4: Input-Hash Caching

**Goal**: Lit skips regenerating prompts that haven't changed.

### Tasks

#### 4a. `cache.rs` — Input-hash caching

```rust
pub struct Cache {
    cache_dir: PathBuf,  // .lit/cache/
}

impl Cache {
    pub fn compute_input_hash(
        prompt: &Prompt,
        import_hashes: &HashMap<PathBuf, String>,
        model_config: &ModelConfig,
        language: &str,
        framework: &str,
    ) -> String;

    pub fn get(&self, input_hash: &str) -> Option<CachedGeneration>;
    pub fn put(&self, input_hash: &str, output: &GenerationOutput);
}

pub struct CachedGeneration {
    pub files: HashMap<PathBuf, String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
}
```

- `compute_input_hash`: SHA-256 of (prompt content + sorted import hashes + model config + language + framework)
- Cache stored as JSON files in `.lit/cache/<hash>.json`
- Cache is local-only (gitignored) — it's an optimization, not required for correctness

**Tests**:
- Same inputs → same hash
- Changed prompt content → different hash
- Changed import → different hash (cascading)
- Changed model config → different hash
- Cache hit: returns stored output
- Cache miss: returns None

### Definition of Done
- Cache computes stable hashes
- Cache stores and retrieves generation outputs
- Pipeline skips cached prompts
- All unit tests pass

---

## Milestone 5: Generation Records

**Goal**: Lit writes `.lit/generations/<hash>.json` after each generation.

### Tasks

#### 5a. `generation_record.rs`

```rust
pub struct GenerationRecord {
    pub git_commit: String,
    pub timestamp: DateTime<Utc>,
    pub dag: HashMap<PathBuf, DagEntry>,
    pub model_config: ModelConfig,
    pub generation_metadata: GenerationMetadata,
}

pub struct DagEntry {
    pub imports: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub input_hash: String,
}

pub struct GenerationMetadata {
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub duration_ms: u64,
    pub prompts_regenerated: Vec<PathBuf>,
    pub prompts_cached: Vec<PathBuf>,
    pub per_prompt: HashMap<PathBuf, PromptGenerationMeta>,
}
```

- `GenerationRecord::write(path, &self)` — serialize to JSON
- `GenerationRecord::read(path)` — deserialize from JSON
- `GenerationRecord::for_commit(git_hash)` — find record in `.lit/generations/`
- Cost calculation: lookup model pricing, compute from token counts

**Tests**:
- Round-trip serialize/deserialize
- Read record for a specific commit hash
- Cost calculation for known models

### Definition of Done
- Generation records written and read correctly
- Integrated into the pipeline (pipeline writes record after generation)

---

## Milestone 6: Git Operations

**Goal**: Lit can init repos, commit, read history, and checkout via git2.

### Tasks

#### 6a. `repo.rs` — Git wrapper

```rust
pub struct LitRepo {
    repo: git2::Repository,
    root: PathBuf,
}

impl LitRepo {
    pub fn init(path: &Path) -> Result<Self>;
    pub fn open(path: &Path) -> Result<Self>;
    pub fn find_root() -> Result<Self>;  // walk up from cwd

    // Commit operations
    pub fn add_all(&self) -> Result<()>;  // stage prompts, code.lock, lit.toml, .lit/generations
    pub fn commit(&self, message: &str) -> Result<git2::Oid>;
    pub fn head_commit(&self) -> Result<git2::Commit>;

    // History
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>>;

    // Diff
    pub fn diff_prompts(&self) -> Result<String>;  // working tree vs HEAD, filtered to prompts/
    pub fn diff_code(&self) -> Result<String>;      // working tree vs HEAD, filtered to code.lock/

    // Status
    pub fn status(&self) -> Result<RepoStatus>;

    // Checkout
    pub fn checkout(&self, ref_: &str) -> Result<()>;

    // Remote operations
    pub fn push(&self) -> Result<()>;
    pub fn pull(&self) -> Result<()>;
}

pub struct RepoStatus {
    pub prompts_modified: Vec<PathBuf>,
    pub prompts_new: Vec<PathBuf>,
    pub prompts_deleted: Vec<PathBuf>,
    pub code_lock_modified: Vec<PathBuf>,  // hand-edits detected
    pub head_commit: Option<String>,
}
```

- Use `git2` crate for all operations (no shelling out to `git`)
- `add_all` stages: `prompts/**`, `code.lock/**`, `lit.toml`, `.lit/generations/**`
- `diff_prompts` uses git2's diff with pathspec filter on `prompts/`
- `status` compares working tree to HEAD, categorizes changes
- `find_root` walks up from cwd looking for `lit.toml` (like git looks for `.git/`)

**Tests**:
- Init creates `.git/` and lit structure
- Commit creates a git commit with correct files
- Log reads commit history
- Status detects modified/new/deleted prompts
- Status detects hand-edited code.lock files
- Checkout restores files to previous state

### Definition of Done
- All git operations work via git2
- Integration test: init → add prompt → commit → modify → status shows changes
- Checkout restores previous state

---

## Milestone 7: CLI Commands — Core Loop

**Goal**: The core workflow works end-to-end: `lit init` → write prompts → `lit commit` → `lit status` → `lit log`

### Tasks

Wire up the CLI commands to the core modules:

#### 7a. `cli/init.rs`
- Interactive prompts for project name, language, framework, model provider
- Or `--defaults` flag for non-interactive init
- Creates `lit.toml`, `prompts/`, `code.lock/`, `.lit/`, `.gitignore`
- Runs `git init` + initial commit

#### 7b. `cli/add.rs`
- Validate that the path is a `.prompt.md` file (or directory containing them)
- In v1: just validates and tracks. No staging area — `lit commit` picks up all tracked prompts.
- Store tracked prompts list in `.lit/tracked` (or rely on git staging)

#### 7c. `cli/commit.rs`
- Parse all prompts
- Build DAG
- Detect changes vs last commit
- Run generation pipeline
- Write code to `code.lock/`
- Write generation record to `.lit/generations/`
- Git add + git commit
- Print summary: files generated, tokens used, cost, duration

#### 7d. `cli/status.rs`
- Show current commit
- Show modified/new/deleted prompts
- Show if code.lock has diverged (hand-edits)

#### 7e. `cli/log.rs`
- Read git log
- For each commit, read `.lit/generations/<hash>.json` if it exists
- Display commit hash, date, message, model, cost

#### 7f. `cli/diff.rs`
- Default: show prompt diffs only (filter git diff to `prompts/`)
- `--code`: show code.lock diffs
- `--all`: show both

### Integration Test

```bash
lit init --defaults
# create prompts/hello.prompt.md with frontmatter
lit commit -m "Initial prompts"
# modify prompts/hello.prompt.md
lit status   # shows modified prompt
lit diff     # shows prompt diff
lit commit -m "Update hello prompt"
lit log      # shows two commits with cost info
```

### Definition of Done
- Full workflow works end-to-end with real Anthropic API calls
- `lit log` shows commits with cost data
- `lit status` correctly detects prompt changes and code.lock divergence

---

## Milestone 8: Remaining Commands

**Goal**: All v1 commands implemented.

### Tasks

#### 8a. `cli/regenerate.rs`
- Run the generation pipeline without committing
- Optional path argument to regenerate specific prompts + dependents
- `--all` flag to force regenerate everything (ignore cache)

#### 8b. `cli/checkout.rs`
- Parse ref (commit hash, HEAD~N)
- Run `git checkout`
- Print restored state summary

#### 8c. `cli/push.rs` / `cli/pull.rs` / `cli/clone.rs`
- Thin wrappers around git2 remote operations
- `lit clone` validates lit structure after cloning
- `lit clone` warns if `.lit/config` needs API key setup

#### 8d. `cli/cost.rs`
- Read all generation records from `.lit/generations/`
- Aggregate: total spend, per-commit, per-prompt breakdown
- `--last`: cost of most recent commit only
- `--breakdown`: table of per-prompt costs

### Definition of Done
- All commands work
- `lit regenerate` produces same output as previous commit (determinism test)
- `lit cost` shows accurate totals

---

## Milestone 9: Polish + Error Handling

**Goal**: Lit is pleasant to use and fails gracefully.

### Tasks

1. **Colored output**: Use `colored` crate for terminal output
   - Green for new files, yellow for modified, red for errors
   - Dim for metadata (cost, tokens)

2. **Progress indicators**: Show progress during generation
   - "Generating prompts/models/user.prompt.md... (1/3)"
   - Spinner or progress bar for LLM calls

3. **Error messages**: Every error includes:
   - What went wrong
   - Which file/prompt caused it
   - What to do about it (actionable suggestion)

4. **Edge cases**:
   - Empty repo (no prompts yet)
   - Prompt with no outputs in manifest mode
   - LLM returns empty response
   - LLM returns files not in declared outputs
   - Network timeout during generation (partial pipeline state)
   - `.lit/config` missing API key
   - Running lit commands outside a lit repo

5. **`.litignore`**: Implement ignore file parsing (can use `ignore` crate or simple glob matching)

6. **Help text**: Every command has descriptive help text and examples

### Definition of Done
- All error cases produce clear, actionable messages
- Output is colored and readable
- Progress shown during generation
- Edge cases handled gracefully

---

## Milestone 10: Testing + Documentation

**Goal**: Comprehensive test suite, ready for release.

### Tasks

1. **Unit tests**: Already built per-milestone. Verify coverage:
   - config.rs: parsing, validation, env var resolution
   - prompt.rs: frontmatter parsing, all mapping modes
   - dag.rs: topological sort, cycles, regeneration sets, output conflicts
   - generator.rs: response parsing, pipeline ordering (mock provider)
   - cache.rs: hash computation, storage, retrieval
   - generation_record.rs: serialization round-trip
   - repo.rs: git operations

2. **Integration tests** (in `tests/` directory):
   - Full workflow: init → add → commit → modify → commit → log → checkout
   - DAG with dependencies: A → B → C, modify A, verify B and C regenerated
   - Cache hit: commit same prompts twice, second commit is instant
   - Cost tracking: verify totals match per-prompt breakdown
   - Error recovery: bad prompt → commit fails → fix prompt → commit succeeds

3. **Mock provider**: A test-only LLM provider that returns deterministic responses based on prompt content. Used for all tests that don't need real API calls.

4. **One real API integration test**: Marked `#[ignore]` by default (requires API key). Tests the full pipeline against the real Anthropic API.

5. **README.md**: Quick start, installation, basic workflow example

6. **`lit --version`**: Version output

### Definition of Done
- `cargo test` passes all tests
- `cargo test -- --ignored` passes with API key set
- README has a working quick-start guide

---

## Demo App: Python CRUD (Dogfooding)

A separate git repo (`lit-demo-crud`) that is authored entirely via lit prompts. We build this alongside lit to validate every milestone against real usage.

### Demo App Structure

```
lit-demo-crud/
  lit.toml
  prompts/
    config/
      database.prompt.md       ← DB connection + session setup
    models/
      base.prompt.md           ← SQLAlchemy declarative base
      user.prompt.md           ← User model (imports base)
      item.prompt.md           ← Item model (imports base, user)
    schemas/
      user.prompt.md           ← Pydantic schemas for User
      item.prompt.md           ← Pydantic schemas for Item
    api/
      app.prompt.md            ← FastAPI app + middleware setup
      users.prompt.md          ← User CRUD endpoints (imports user model, user schemas)
      items.prompt.md          ← Item CRUD endpoints (imports item model, item schemas)
    tests/
      test_users.prompt.md     ← User endpoint tests
      test_items.prompt.md     ← Item endpoint tests
  code.lock/
    src/
      config/database.py
      models/base.py
      models/user.py
      models/item.py
      schemas/user.py
      schemas/item.py
      api/app.py
      api/users.py
      api/items.py
    tests/
      test_users.py
      test_items.py
```

### Demo App `lit.toml`

```toml
[project]
name = "lit-demo-crud"
version = "0.1.0"
mapping = "manifest"

[language]
default = "python"
version = "3.12"

[framework]
name = "fastapi"
version = "0.115"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
seed = 42

[model.api]
key_env = "LIT_API_KEY"
```

### Demo App DAG

```
database.prompt.md
    ↓
base.prompt.md
    ↓           ↓
user.prompt.md  item.prompt.md (also imports user)
    ↓               ↓
user schemas    item schemas
    ↓               ↓
app.prompt.md ← users.prompt.md  items.prompt.md
                    ↓                  ↓
              test_users.prompt.md  test_items.prompt.md
```

Three levels of the DAG means we test:
- Independent parallel generation (user + item models)
- Multi-import prompts (items imports both base and user)
- Deep chains (base → model → schema → endpoint → test)

### Interleaved Milestones

| Lit Milestone | Demo App Checkpoint |
|---------------|-------------------|
| M0: Skeleton | Create `lit-demo-crud/` repo manually with `lit.toml` and all prompt files (written by hand). This is the test fixture. |
| M1: Config + Prompts | Run config/prompt parser against demo app. Verify all 11 prompts parse correctly with frontmatter. |
| M2: DAG | Build DAG from demo app prompts. Verify topological order. Print it. |
| M3: LLM + Pipeline | **First real generation.** Run pipeline on demo app. Inspect `code.lock/` output. Try to run the FastAPI app. |
| M4: Caching | Run pipeline twice. Second run should skip all prompts (cache hit). Modify one prompt, verify only it + dependents regenerate. |
| M5: Gen Records | Verify `.lit/generations/` contains correct metadata after generation. |
| M6: Git Operations | `lit init` on demo app. First real `lit commit`. |
| M7: Core CLI | **Full dogfood loop**: `lit init` → write prompts → `lit commit` → `lit status` → `lit diff` → `lit log`. Run the generated FastAPI app with `uvicorn`. Hit endpoints with `curl`. |
| M8: Remaining | `lit regenerate` on demo app. `lit checkout` to previous version. `lit cost` to see total spend. |
| M9: Polish | Verify error messages when demo app prompts have issues (bad imports, missing outputs, etc.) |
| M10: Testing | Demo app serves as the real-world integration test. Document the full walkthrough in lit's README. |

### Example Prompt: `prompts/models/user.prompt.md`

```markdown
---
outputs:
  - src/models/user.py
imports:
  - prompts/models/base.prompt.md
---

# User Model

Create a SQLAlchemy model for a User with the following fields:

- `id`: Integer, primary key, auto-increment
- `email`: String(255), unique, indexed, not null
- `hashed_password`: String(255), not null
- `full_name`: String(255), nullable
- `is_active`: Boolean, default True
- `created_at`: DateTime, server default now()
- `updated_at`: DateTime, onupdate now()

Use the Base class from @import(prompts/models/base.prompt.md).

Include proper __repr__ and a relationship to items (one-to-many).
```

### Example Prompt: `prompts/api/users.prompt.md`

```markdown
---
outputs:
  - src/api/users.py
imports:
  - prompts/models/user.prompt.md
  - prompts/schemas/user.prompt.md
  - prompts/config/database.prompt.md
---

# User CRUD API Endpoints

Create a FastAPI APIRouter with the following endpoints:

- `POST /users/` — Create a new user. Hash the password with bcrypt.
- `GET /users/` — List users with pagination (skip, limit query params).
- `GET /users/{user_id}` — Get a single user by ID. Return 404 if not found.
- `PUT /users/{user_id}` — Update a user. Partial updates allowed.
- `DELETE /users/{user_id}` — Soft delete (set is_active=False).

Use the User model from @import(prompts/models/user.prompt.md).
Use the Pydantic schemas from @import(prompts/schemas/user.prompt.md).
Use the database session dependency from @import(prompts/config/database.prompt.md).

Include proper error handling and HTTP status codes.
```

---

## Build Order Summary

| Milestone | Delivers | Demo App Checkpoint | Est. |
|-----------|----------|-------------------|------|
| M0: Skeleton | CLI compiles, `lit --help` works | Create demo repo + all prompt files by hand | ½ day |
| M1: Config + Prompts | Parse `lit.toml` and `.prompt.md` | Parse all 11 demo prompts | 1 day |
| M2: DAG | Dependency graph + topological sort | Print demo app's DAG | 1 day |
| M3: LLM + Pipeline | Call Anthropic, generate code | **First generation of demo app** → try running it | 2-3 days |
| M4: Caching | Input-hash cache, skip unchanged | Modify 1 prompt, verify partial regen | ½ day |
| M5: Gen Records | `.lit/generations/` metadata | Verify metadata for demo app generation | ½ day |
| M6: Git Ops | `repo.rs` wrapping git2 | First `lit commit` on demo app | 2 days |
| M7: Core CLI | End-to-end `init→commit→status→log` | **Full dogfood**: run the CRUD app with uvicorn | 2 days |
| M8: Remaining CLI | regenerate, checkout, push/pull, cost | `lit cost` on demo app, checkout old version | 1-2 days |
| M9: Polish | Colors, progress, errors | Test error paths with broken demo prompts | 1-2 days |
| M10: Testing + Docs | Test suite, README | Demo app walkthrough in README | 1-2 days |

**Total estimated: ~12-16 days of focused work**

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| LLM response parsing is fragile | Use strict delimiter format, validate outputs against frontmatter, clear error on parse failure |
| git2 crate has complex API | Build repo.rs early (M6), isolate all git complexity there |
| Non-deterministic LLM output | Pin model+temp+seed, store outputs in commits, cache by input hash |
| Large generated codebases bloat git | Future concern — v1 accepts this tradeoff. Git LFS or shallow clones could help later. |
| Anthropic API rate limits | Implement exponential backoff in provider. Limit concurrent DAG-level parallelism. |
| comrak doesn't parse YAML frontmatter directly | Split on `---` manually before feeding to comrak. Frontmatter parsing is separate from markdown parsing. |
