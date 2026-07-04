# DevTerm — common developer commands.
#
# Task runner: `just` (https://github.com/casey/just).
#   Install:  cargo install just   |   winget install casey.just
#   Usage:    just <recipe>        |   just            (lists recipes)

# Package that produces the main binary `devterm`.
app := "devterm-app"
set windows-shell := ["C:\\Program Files\\Git\\bin\\sh.exe","-c"]
# Default log level for the `run` recipes; respects an existing RUST_LOG.
# Override per-run, e.g. PowerShell:  $env:RUST_LOG="debug"; just run
export RUST_LOG := env_var_or_default("RUST_LOG", "info")

alias dev := run
alias lint := clippy

# List available recipes (default when you run bare `just`).
default:
    @just --list

# --- run -------------------------------------------------------------------

# Run the app (debug build, fast to compile).
run:
    cargo run -p {{app}}

# Run the app (release build, optimized — use this to judge real performance).
run-release:
    cargo run -p {{app}} --release

# Auto-rebuild and re-run on file changes (needs: cargo install cargo-watch).
watch:
    cargo watch -x "run -p {{app}}"

# --- build -----------------------------------------------------------------

# Debug build of the whole workspace.
build:
    cargo build --workspace

# Optimized release build of the whole workspace.
release:
    cargo build --workspace --release

# Fast type-check, no codegen — quickest correctness signal.
check:
    cargo check --workspace --all-targets

# --- quality ---------------------------------------------------------------

# Run all tests (unit + property tests).
test:
    cargo test --workspace --all-features

# Format all code.
fmt:
    cargo fmt --all

# Verify formatting without changing files (CI gate).
fmt-check:
    cargo fmt --all --check

# Lint with warnings denied (CI gate).
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# License + advisory audit (needs: cargo install cargo-deny).
deny:
    cargo deny check

# The full local CI gate — mirrors .github/workflows/ci.yml. Run before pushing.
ci: fmt-check clippy test

# --- misc ------------------------------------------------------------------

# Build API docs and open them in the browser.
doc:
    cargo doc --workspace --no-deps --open

# Remove build artifacts.
clean:
    cargo clean
