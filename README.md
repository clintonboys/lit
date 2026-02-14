<p align="center">
  <img src="assets/lit-logo.svg" alt="lit" width="240" />
</p>

<h3 align="center">Version control with good vibes</h3>
<p align="center"><em>Prompts are source, code is the artifact</em></p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.86%2B-orange.svg" alt="Rust 1.86+"></a>
  <img src="https://img.shields.io/badge/tests-128-brightgreen.svg" alt="Tests: 128">
  <img src="https://img.shields.io/badge/lines-7.6k-informational.svg" alt="Lines: 7.6k">
  <a href="SPEC.md"><img src="https://img.shields.io/badge/spec-v1%20complete-blueviolet.svg" alt="Spec: v1 complete"></a>
  <a href="https://www.anthropic.com/claude"><img src="https://img.shields.io/badge/supports-Anthropic%20Claude-cc785c.svg" alt="Anthropic Claude"></a>
  <a href="https://openai.com/"><img src="https://img.shields.io/badge/supports-OpenAI%20GPT-412991.svg" alt="OpenAI GPT"></a>
</p>

---

`lit` is a version control system that treats LLM agent prompts as the source of truth for software projects. Generated code lives in a `code.lock/` directory and is committed alongside prompts in git. The code generation itself is also handled by `lit`. 

## The name

`lit` is a working title, meant to evoke three separate sources of inspiration for the project:

- sounds similar to `git`, which is its spiritual predecessor and which provides much of its low-level functionality
- is short for "literature", which is suggestive of its natural-language source of truth rather than programming languages
- evokes the "vibes" that make up the coding

So the name does fit very well, but it is also unfortunately the name of a well-known [web development framework](https://github.com/lit/lit) with 21.2k GitHub stars, so it will probably have to change. 

## Why Lit?

It's no longer a problem for the future: generated code is beginning to form a large part of production code bases *today*. Most serious companies are still extremely wary of "opening the floodgates" of "vibe code reviews" though, so generated code must be pored over and studied by engineers. This despite everyone harping on about how LLMs give us a new paradigm similar to how compiled languages replaced assembly. 

`lit` is an attempt to address that problem by "making prompts commitable and diffable". 

People won't vibe-code with prompt files and YAML frontmatter. They talk to AI agents and the agent changes files. That's fine, `lit` is not a replacement for that workflow, but for what comes after: when those generated files have to actually form part of a production code base. 

### The problem

You vibe-coded a feature with Claude/Cursor/Copilot and it works and makes it to production. Six months later, someone asks "why does this code exist?" and nobody knows. The chat history is gone, the AI session is lost, and the code is opaque. 

### How `lit` can help

**`lit` is for capturing, maintaining, and reproducing intent.**
**If you use it properly, it can truly make the prompts the source of truth for your project, and function as a living specification of the code. Code can truly become an artifact, like a giant `uv.lock` file.**

Here are some workflows where `lit` makes sense:

#### 1. Post-hoc formalization of "vibe coding"

Vibe-code something freely, and once it works, write the prompt that describes the *intent* — what this code should do, what assumptions it makes, what contract it fulfills. Then run `lit regenerate` to verify the prompt actually reproduces the code. This command actually handles the generation of the code (integration with LLMs). Now you have a reproducible (insofar as LLMs can be) spec committed alongside the code.

This is sort of like writing tests after prototyping something. 

```bash
# After vibe-coding a feature that works:
vi prompts/auth/login.prompt.md      # Describe the intent...
lit regenerate                       # Verify it reproduces. 
lit diff --code                      # Compare generated vs hand-written
lit commit -m "Capture login intent"
```

#### 2. Prompt-driven changes

Requirements change. Instead of asking an AI to "update this code" and hoping it gets it right, you change the prompt — the requirements spec — and regenerate. The diff shows the *change in intent*, not just the change in code. Code review becomes review of requirements.

```bash
vi prompts/models/user.prompt.md    # Add a new field to the schema
lit regenerate                      # Regenerate the code
lit diff                            # See the intent change
lit diff --code                     # See the code change
lit diff --summary                  # See DAG impact analysis
lit commit -m "Add user avatar field"
```

#### 3. Prompts as documentation

A new developer reads `prompts/` to understand intent, not just implementation. Each prompt file is a spec for what its generated code should do. The DAG (see below for implementation details: `lit` uses a [DAG](https://en.wikipedia.org/wiki/Directed_acyclic_graph) to model the connections between prompts and the code files they generate) shows how components relate.

```bash
lit debug dag                       # See how prompts depend on each other
cat prompts/api/users.prompt.md     # Read the spec for the users endpoint
```

### What `lit` is NOT

- Not a replacement for vibe-coding or AI agents
- Not an IDE plugin or chat interface
- Not particularly useful for throwaway prototypes or solo exploration

Lit is useful when you have a **production codebase**, a **team**, and you need **accountability** for why code exists and what it's supposed to do.

---

## Quick Start

### Install

`lit` is written in Rust. You need to install [Rust](https://github.com/rust-lang/rust) 1.86+ in order to build it from source. 

```bash
# From source (requires Rust 1.86+)
git clone https://github.com/clintonboys/lit && cd lit
cargo install --path .
```

### Set up your API key

Currently, `lit` requires an API key for a frontier LLM provider to be able to generate code. At present, Anthropic (Claude) and OpenAI (GPT) are supported. 

```bash
export LIT_API_KEY=sk-ant-...  # Anthropic API key
# or
export LIT_API_KEY=sk-...      # OpenAI API key
```

### Create a project

```bash
mkdir my-project && cd my-project
lit init --defaults
```

This creates:
- `lit.toml` — project configuration
- `prompts/` — where your prompt files live
- `code.lock/` — where generated code is written
- `.lit/` — internal state (cache, generation records)
- `.git` — git repository with initial commit: `lit` uses `git` for its storage layer and wraps all common git commands. 

### Write a prompt

Create `prompts/hello.prompt.md`:

```markdown
---
outputs:
  - src/hello.py
---

# Hello Module

Create a Python module with a `greet(name: str) -> str` function
that returns "Hello, {name}!" and a main block that greets "World".
```

### Generate and commit

```bash
lit regenerate          # Calls the LLM, writes code.lock/src/hello.py
lit status              # Shows new prompt + generated code
lit commit -m "Add hello prompt"
```

### Iterate

```bash
# Edit the prompt
vim prompts/hello.prompt.md

lit diff                # See what changed in prompts
lit diff --summary      # See DAG impact analysis
lit regenerate          # Re-generate (only changed prompts + dependents)
lit commit -m "Update hello prompt"
lit log                 # See commit history
lit cost                # See total token usage and cost
```

---

## How It Works

```
Literate Programming (Knuth, 1984)
  prose + code -> tangle extracts code
                    |
                    | "What if code wasn't hand-written?"
                    v
Schema-based Codegen (Protobuf, OpenAPI, Prisma)
  formal spec -> deterministic compiler -> code
                    |
                    | "What if the spec was natural language?"
                    v
lit (2025)
  markdown prompts -> LLM "compiler" -> code.lock/
```

1. **Prompts are source**: You write `.prompt.md` files with YAML frontmatter declaring output files and imports.
2. **DAG resolution**: Lit builds a dependency graph from `imports` and generates in topological order.
3. **Context flows through the DAG**: Each prompt receives the generated code of its imports as context.
4. **Input-hash caching**: Unchanged prompts are skipped (SHA-256 of prompt + imports + config).
5. **Everything is git-tracked**: Prompts, generated code, and generation metadata are committed together.

---

## DAG Impact Analysis

When you change a prompt, `lit diff --summary` shows the full cascade:

```
=== Changes Summary ===
  Prompts:
      ~ prompts/models/user.prompt.md  (+2 -0 lines)

  Impact (prompts that will regenerate):
    → prompts/models/user.prompt.md
    → prompts/models/item.prompt.md       (imports user models)
    → prompts/schemas/user.prompt.md      (imports user models)
    → prompts/api/users.prompt.md         (imports user models, user schemas)
    → prompts/schemas/item.prompt.md      (imports item models, user schemas)
    → prompts/api/items.prompt.md         (imports item models, item schemas)

  Generated code affected:
      ~ code.lock/src/models/user.py
      ~ code.lock/src/models/item.py
      ~ code.lock/src/schemas/user.py
      ~ code.lock/src/api/users.py
      ...

  8 prompt(s) will regenerate, 4 unchanged
```

Two lines added to a model prompt → 8 out of 12 prompts cascade. This is the kind of impact analysis you can't get from `git diff`.

---

## Prompt Format

```markdown
---
outputs:
  - src/models/user.py
imports:
  - prompts/models/base.prompt.md
---

# User Model

Create a SQLAlchemy model for a User with fields: id, email, name.

Use the Base class from @import(prompts/models/base.prompt.md).
```

### Frontmatter fields

| Field | Required | Description |
|-------|----------|-------------|
| `outputs` | Yes (manifest mode) | List of files this prompt generates |
| `imports` | No | Other prompts whose generated code is passed as context |
| `model` | No | Per-prompt model override (`model`, `temperature`, `seed`) |
| `language` | No | Override the project default language |

---

## Configuration (`lit.toml`)

```toml
[project]
name = "my-project"
version = "0.1.0"
mapping = "manifest"    # outputs declared in frontmatter

[language]
default = "python"
version = "3.12"

[framework]              # optional
name = "fastapi"
version = "0.115"

[model]
provider = "anthropic"   # or "openai"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
seed = 42

[model.api]
key_env = "LIT_API_KEY"  # env var containing your API key

[model.pricing]           # optional — override built-in cost estimation
input_per_million = 3.0   # USD per million input tokens
output_per_million = 15.0 # USD per million output tokens
```

### Supported providers

| Provider | Models | API key env var |
|----------|--------|-----------------|
| `anthropic` | Claude Sonnet, Haiku, Opus (all versions) | `ANTHROPIC_API_KEY` or custom via `key_env` |
| `openai` | GPT-4o, GPT-4o-mini, GPT-4 | `OPENAI_API_KEY` or custom via `key_env` |

---

## Commands

| Command | Description |
|---------|-------------|
| `lit init [--defaults]` | Initialize a new lit repository |
| `lit status` | Show the state of prompts and generated code |
| `lit add <path>` | Validate and track prompt files |
| `lit diff [--code] [--all] [--summary]` | Show changes since last commit |
| `lit regenerate [path] [--all] [--no-cache]` | Generate code from prompts |
| `lit commit -m "message"` | Stage all lit files and create a git commit |
| `lit log [-n N]` | Show commit history |
| `lit checkout <ref>` | Restore prompts and code from a previous commit |
| `lit cost [--last] [--breakdown]` | Show token usage and cost tracking |
| `lit push` / `lit pull` | Push/pull to git remote |
| `lit clone <url>` | Clone a lit repository |
| `lit patch save/list/drop/show` | Manage manual patches to generated code |
| `lit debug config/prompts/dag/all` | Inspect internal state |

---

## Manual Patches

Sometimes you need a quick one-line fix without regenerating. Lit supports this:

```bash
# Edit generated code directly
vim code.lock/src/models/user.py

# Save the edit as a patch
lit patch save

# Future regenerations will apply your patch on top of fresh LLM output
lit regenerate    # Your manual edits are preserved

# List and manage patches
lit patch list
lit patch show src/models/user.py
lit patch drop src/models/user.py   # Discard the patch
```

Patches are temporary escape hatches. The long-term goal is always to update the prompt so the LLM generates the right code.

---

## Project Structure

```
my-project/
  lit.toml                          # Configuration
  prompts/
    models/user.prompt.md           # Prompt files (source of truth)
    models/base.prompt.md
    api/users.prompt.md
  code.lock/
    src/models/user.py              # Generated code (artifact)
    src/models/base.py
    src/api/users.py
  .lit/
    cache/                          # Input-hash cache (gitignored)
    generations/                    # Generation records (committed)
    patches/                        # Manual patches (committed)
```

---

## Demo

See [lit-demo-crud](https://github.com/clintonboys/lit-demo-crud) — a complete FastAPI CRUD app generated from 12 prompts:

- 3 models (base, user, item) with SQLAlchemy
- 2 schema layers (user, item) with Pydantic v2
- 3 API modules (app, users, items) with FastAPI
- 2 test suites (16 tests total)
- Database config + package structure

Every line of Python was generated from prompts. Clone it, set your API key, run `lit regenerate`.

---

## Testing

```bash
cargo test                    # Run all tests (128 passing)
cargo test -- --ignored       # Run real API integration test (requires LIT_API_KEY)
```

## License

MIT
