# lit — Prompt-first version control
# Quick-reference Makefile for building, testing, and validating

DEMO_APP := ../lit-demo-crud

.PHONY: build test install check validate clean

# ---------- Core ----------

build:
	cargo build

test:
	cargo test

install:
	cargo install --path .

# ---------- Validation ----------

# Run everything: build + test + install + validate against demo app
check: test install validate

# Validate lit against the demo CRUD app
validate:
	@echo "=== lit --version ==="
	@lit --version
	@echo ""
	@echo "=== lit status (demo app) ==="
	@cd $(DEMO_APP) && lit status
	@echo ""
	@echo "=== lit debug all (demo app) ==="
	@cd $(DEMO_APP) && lit debug all
	@echo ""
	@echo "=== VALIDATION PASSED ==="

# ---------- Quick checks ----------

# Just run unit tests (fast)
unit:
	cargo test --lib

# Just run integration tests (needs demo app)
integration:
	cargo test --test demo_app_test

# Show what lit can do right now
demo:
	@echo "--- lit --help ---"
	@lit --help
	@echo ""
	@echo "--- lit debug all (from demo app) ---"
	@cd $(DEMO_APP) && lit debug all

# ---------- Cleanup ----------

clean:
	cargo clean
