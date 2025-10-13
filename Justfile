# justfile - Modern task runner (install: cargo install just)
# Run 'just --list' to see all available commands

# Default recipe
default:
    @just --list

# Build all projects in release mode
build:
    cargo build --release --all

# Build in development mode
build-dev:
    cargo build --all

# Run all tests
test:
    cargo test --all --all-features

# Run tests with output
test-verbose:
    cargo test --all --all-features -- --nocapture

# Run specific test
test-one TEST:
    cargo test {{TEST}} -- --nocapture

# Check code without building
check:
    cargo check --all --all-features

# Format code
fmt:
    cargo fmt --all

# Check formatting
fmt-check:
    cargo fmt --all -- --check

# Run clippy linter
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Fix clippy warnings automatically
clippy-fix:
    cargo clippy --all-targets --all-features --fix

# Run all checks (fmt, clippy, test)
ci: fmt-check clippy test

# Clean build artifacts
clean:
    cargo clean
    rm -rf target/

# Update dependencies
update:
    cargo update

# Check for outdated dependencies
outdated:
    cargo outdated

# Audit dependencies for security issues
audit:
    cargo audit

# Run benchmarks
bench:
    cargo bench --all

# Run benchmarks for specific test
bench-one BENCH:
    cargo bench {{BENCH}}

# Generate documentation
doc:
    cargo doc --no-deps --all-features --open

# Generate documentation with private items
doc-all:
    cargo doc --no-deps --all-features --document-private-items --open

# Run the trading engine
run-engine:
    RUST_LOG=info cargo run --release -p engine

# Run the trading engine in debug mode
run-engine-debug:
    RUST_LOG=debug cargo run -p engine

# Run the terminal UI
run-terminal:
    cargo run --release -p terminal

# Run the terminal UI in debug mode
run-terminal-debug:
    cargo run -p terminal

# Build and run Docker containers
docker-up:
    docker-compose up -d

# Stop Docker containers
docker-down:
    docker-compose down

# View Docker logs
docker-logs SERVICE="engine":
    docker-compose logs -f {{SERVICE}}

# Build Docker image
docker-build:
    docker build -t hft-engine -f Dockerfile.engine .

# Install development tools
install-tools:
    cargo install cargo-watch
    cargo install cargo-audit
    cargo install cargo-outdated
    cargo install cargo-flamegraph
    cargo install cargo-tarpaulin
    cargo install just

# Watch files and run checks on change
watch:
    cargo watch -x 'check --all' -x 'test --all'

# Watch and run specific command
watch-run COMMAND:
    cargo watch -x '{{COMMAND}}'

# Generate flame graph for performance profiling
flamegraph TARGET="engine":
    cargo flamegraph --bin {{TARGET}}

# Generate code coverage report
coverage:
    cargo tarpaulin --all-features --workspace --timeout 120 --out Html

# Run in release mode with profiling
profile TARGET="engine":
    cargo build --release -p {{TARGET}}
    CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --bin {{TARGET}}

# Deploy to production
deploy ENV="production":
    #!/usr/bin/env bash
    set -euxo pipefail
    echo "Deploying to {{ENV}}..."
    cargo build --release --all
    scp target/release/engine deploy@server:/opt/hft/
    ssh deploy@server 'sudo systemctl restart hft-engine'

# Create a new release
release VERSION:
    #!/usr/bin/env bash
    set -euxo pipefail
    echo "Creating release {{VERSION}}..."
    git tag -a v{{VERSION}} -m "Release v{{VERSION}}"
    git push origin v{{VERSION}}
    cargo build --release --all

# Lint SQL migrations (if using sqlx)
lint-sql:
    cargo sqlx prepare --check

# Run database migrations
migrate:
    cargo sqlx migrate run

# Initialize new model directory
init-models:
    #!/usr/bin/env bash
    mkdir -p models/crypto models/equity
    touch models/crypto/.gitkeep
    touch models/equity/.gitkeep
    echo "Model directories created"

# Verify all systems before commit
pre-commit: fmt-check clippy test
    @echo "✅ All checks passed!"

# Full CI pipeline locally
full-ci: clean install-tools ci bench doc coverage audit
    @echo "🎉 Full CI pipeline completed!"

# Quick development cycle
dev: fmt clippy test-verbose
    @echo "🚀 Development checks complete!"

# Production build with all optimizations
build-prod:
    RUSTFLAGS="-C target-cpu=native -C opt-level=3 -C lto=fat" \
    cargo build --release --all

# Check binary size
size:
    du -h target/release/engine
    du -h target/release/terminal

# Strip debug symbols from binaries
strip:
    strip target/release/engine
    strip target/release/terminal

# Analyze build dependencies
tree:
    cargo tree --all-features

# Show crate versions
versions:
    cargo tree --depth 1

# Security audit with fix suggestions
audit-fix:
    cargo audit fix --dry-run

# Update Cargo.lock without changing Cargo.toml
update-lock:
    cargo update --workspace

# Verify edition 2024 compatibility
verify-edition:
    #!/usr/bin/env bash
    echo "Checking edition 2024 in all Cargo.toml files..."
    find . -name "Cargo.toml" -exec grep -l "edition = \"2024\"" {} \;
    echo "✅ Edition verification complete"
