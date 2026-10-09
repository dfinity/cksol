# List available recipes
default:
    @just --list

# Run all checks, build, and tests
ci: lint build test

# Format and lint checks
lint:
    cargo fmt --all -- --check
    cargo sort --workspace --check
    cargo clippy --locked --verbose --tests --benches --workspace -- -D clippy::all
    cargo clippy --locked --verbose --target wasm32-unknown-unknown -p cksol_minter -- -D clippy::all

# Build canister WASM (native; fast loop for development)
build:
    ./scripts/build --cksol_minter

# Build canister WASM reproducibly via Docker (bit-identical across hosts)
docker-build:
    ./scripts/docker-build

# Run all tests
test: unit-tests integration-tests

# Run unit tests
unit-tests:
    cargo test --locked --workspace --exclude cksol-int-tests

# Run integration tests
integration-tests: _maybe-build
    cargo test --locked --package cksol-int-tests -- --test-threads 2 --nocapture

_maybe-build:
    {{ if env("CKSOL_MINTER_WASM_PATH", "") == "" { "just build" } else { "true" } }}

# Measure test coverage
coverage:
    ./scripts/coverage

# Run canbench benchmarks
bench:
    cd minter && MISE_ENV=bench mise exec -- canbench

# Run canbench and persist results for regression checks
bench-check:
    cd minter && MISE_ENV=bench mise exec -- canbench --persist
