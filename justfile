# itr — Agent-First Issue Tracker CLI
# Run `just` or `just --list` to see all recipes

default:
    @just --list

# ─── Build ───────────────────────────────────────────────────────────

# Debug build
build:
    cargo build

# Optimized release build
release:
    cargo build --release

# Check without producing binaries (faster)
check:
    cargo check

# Install to ~/.cargo/bin
install: release
    cargo install --path . --force

# ─── Test ────────────────────────────────────────────────────────────

# Run integration test suite (release build)
test: release
    ./tests/integration.sh

# Run integration tests against debug build
test-debug: build
    ./tests/integration.sh ./target/debug/itr

# ─── Lint & Format ──────────────────────────────────────────────────

# Run clippy
lint:
    cargo clippy --all-targets -- -D warnings

# Run cargo-deny (license, advisory, ban checks)
deny:
    cargo deny check

# Format all code
fmt:
    cargo fmt --all

# Check formatting without modifying
fmt-check:
    cargo fmt --all -- --check

# Full verification: build + lint + test + format check + deny
verify: release lint test fmt-check deny

# Lint + format + test + deny (CI-style)
ci: fmt-check lint deny test

# ─── Clean ───────────────────────────────────────────────────────────

# Remove build artifacts
clean:
    cargo clean

# ─── Issue Tracker ───────────────────────────────────────────────────

# Show next actionable task
next:
    itr ready -f json

# List all open issues
issues:
    itr list

# Add a new issue (usage: just issue "title")
issue title:
    itr add "{{title}}"

# Close an issue (usage: just close 3 "reason")
close id reason:
    itr close {{id}} "{{reason}}"

# Add a note to an issue (usage: just note 3 "summary")
note id summary:
    itr note {{id}} "{{summary}}"

# Project health
stats:
    itr stats

# ─── Info ────────────────────────────────────────────────────────────

# Show dependency tree
deps:
    cargo tree --depth 1

# Show binary size (release)
size: release
    ls -lh target/release/itr

# Show lines of code
loc:
    @echo "── Source lines ──"
    @find src -name '*.rs' | xargs wc -l | sort -n
