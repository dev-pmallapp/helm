# HELM — development targets
#
# Usage:
#   make test          Run all Rust tests
#   make pre-commit    Full check suite (format + lint + test)

.PHONY: check test clippy fmt fmt-check pre-commit test-python clean

CARGO_FLAGS := --offline --workspace --exclude helm-python

# ── Build & check ────────────────────────────────────────────
check:
	cargo check $(CARGO_FLAGS)

# ── Tests ────────────────────────────────────────────────────
test:
	cargo test $(CARGO_FLAGS)

test-python:
	PYTHONPATH=python python3 -m unittest discover -s python/tests -p "test_*.py" -v

# ── Formatting ───────────────────────────────────────────────
fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# ── Linting ──────────────────────────────────────────────────
clippy:
	cargo clippy $(CARGO_FLAGS) -- -D warnings

# ── Pre-commit gate ──────────────────────────────────────────
pre-commit: fmt-check clippy test
	@echo ""
	@echo "All checks passed — safe to commit."

# ── Cleanup ──────────────────────────────────────────────────
clean:
	cargo clean
