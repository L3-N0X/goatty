# goatty task runner: https://just.systems
# `just` lists the recipes. The nix dev shell (`nix develop`, or direnv) provides the toolchain.

set shell := ["bash", "-euo", "pipefail", "-c"]

# List the available recipes.
default:
    @just --list

# Debug build of the whole workspace.
build:
    cargo build --workspace

# Release build of the whole workspace.
release:
    cargo build --release --workspace

# Run the GUI (release build); extra arguments go to `goatty`, e.g. `just run --local`.
run *args:
    cargo run --release -p goatty-app --bin goatty -- {{args}}

# Run the daemon CLI (release build), e.g. `just daemon list`.
daemon *args:
    cargo run --release -p goatty-daemon --bin goattyd -- {{args}}

# Unit and daemon end-to-end tests; extra arguments go to `cargo test`.
test *args:
    cargo test --workspace {{args}}

# Format all code.
fmt:
    cargo fmt --all

# Check formatting without writing (as CI does).
fmt-check:
    cargo fmt --all --check

# Clippy with warnings denied (as CI does).
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Everything CI checks: formatting, clippy, tests (all with --locked).
ci:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked

# Headless VT parse throughput.
bench-vt:
    cargo run --release -p goatty-term --example vtbench

# Client/daemon echo latency + flood; args: proxy | slow 2m | ssh HOST.
bench-net *args:
    cargo run --release -p goatty-daemon --example netbench -- {{args}}

# Install the GPUI system libraries on Debian/Ubuntu (not needed inside the nix shell).
deps:
    ./scripts/ci-deps.sh

# Bump the pinned Zed revision: edit `rev` in Cargo.toml first, then run this.
update-gpui:
    cargo update -p gpui
