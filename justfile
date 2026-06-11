# Default: run the full CI suite locally
default: ci

# Full CI check — mirrors what GitHub Actions runs
ci: fmt clippy test audit

# Type-check only (fast)
check:
    cargo check

# Run all tests
test:
    cargo test

# Lint with clippy (warnings = errors)
clippy:
    cargo clippy -- -D warnings

# Check formatting
fmt:
    cargo fmt --check

# Fix formatting in place
fmt-fix:
    cargo fmt

# Security advisory audit
audit:
    cargo audit

# License and dependency policy check (requires cargo-deny)
deny:
    cargo deny check

# Build release binary
build:
    cargo build --release

# Build container image (amd64)
container:
    podman build -t mcp-google-workspace:latest .

# Run the server locally (stdio, for Claude Code)
run *ARGS:
    cargo run -- {{ARGS}}

# Run HTTP server locally
run-http port="3000" *ARGS:
    cargo run -- --services drive,gmail,calendar --http 127.0.0.1:{{port}} {{ARGS}}

# Scan container image for vulnerabilities (requires trivy)
scan:
    trivy image mcp-google-workspace:latest

# Install development tools
setup:
    cargo install cargo-audit cargo-deny
    @echo "Tools installed. Run 'just ci' to verify."

# Install git pre-commit hook
hook:
    @echo '#!/bin/sh' > .git/hooks/pre-commit
    @echo 'set -e' >> .git/hooks/pre-commit
    @echo 'cargo fmt --check' >> .git/hooks/pre-commit
    @echo 'cargo clippy -- -D warnings' >> .git/hooks/pre-commit
    @echo 'cargo test' >> .git/hooks/pre-commit
    @chmod +x .git/hooks/pre-commit
    @echo "Pre-commit hook installed."

# Show line counts by category
stats:
    @echo "=== Code vs Tests ==="
    @for f in src/*.rs; do \
        name=$$(basename $$f); \
        total=$$(wc -l < $$f); \
        ts=$$(grep -n '^#\[cfg(test)\]' $$f | head -1 | cut -d: -f1); \
        if [ -n "$$ts" ]; then code=$$((ts - 1)); tests=$$((total - code)); \
        else code=$$total; tests=0; fi; \
        printf "%-14s %4d code  %4d tests\n" "$$name" "$$code" "$$tests"; \
    done
