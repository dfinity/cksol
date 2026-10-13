# Building and Testing

## Prerequisites

Install [`mise`](https://mise.jdx.dev/) and then provision the pinned toolchain
(Rust with the `wasm32-unknown-unknown` target, `cargo-sort`,
[`ic-wasm`](https://github.com/dfinity/ic-wasm), `cargo-llvm-cov`, `jq` and `mdbook`):

```sh
mise install
```

Tool versions are defined in [`mise.toml`](https://github.com/dfinity/cksol/blob/main/mise.toml) and their download
checksums in [`mise.lock`](https://github.com/dfinity/cksol/blob/main/mise.lock), so local, CI and the reproducible
Docker build stay in sync.

`canbench` is kept out of that set, because only the benchmarks need it and
building it from source costs about two minutes. It lives in
[`mise.bench.toml`](https://github.com/dfinity/cksol/blob/main/mise.bench.toml) instead, which mise loads only for the
`bench` environment:

```sh
MISE_ENV=bench mise install
```

> [!NOTE]
> Upstream publishes a single Linux x86-64 `canbench` binary, and mise offers it
> for every platform. On macOS or Windows that binary will install but not run,
> so build it from source instead: `cargo install --locked canbench`.

Additionally:

- `gzip` — used by `./scripts/build` to compress the output Wasm.

## Building

Build the minter Wasm:

```sh
./scripts/build --cksol_minter
```

The resulting `cksol_minter.wasm.gz` will be written to the `wasms/` directory.

You can also build the Rust workspace directly (without Wasm post-processing):

```sh
cargo build
```

## Testing

The test suite has three parts:

**Unit tests** — no external dependencies:

```sh
cargo test --lib
```

**PocketIC integration tests** — require the minter Wasm, which is read from `wasms/cksol_minter.wasm.gz` (written by both `./scripts/docker-build` and `./scripts/build --cksol_minter`), unless the `CKSOL_MINTER_WASM_PATH` environment variable points to it:

```sh
./scripts/build --cksol_minter
cargo test -p cksol-int-tests --test tests
```

**Solana validator integration tests** — require the [Solana CLI](https://solana.com/docs/intro/installation) to be installed, so that each test can start its own `solana-test-validator`:

```sh
cargo test -p cksol-int-tests --test solana_test_validator
```

> [!CAUTION]
> Running `cargo test` without arguments will attempt all tests, including the Solana validator suite, and will fail if the Solana CLI is not installed.

**Coverage** — measures the unit tests only, since the integration tests drive a prebuilt Wasm that host instrumentation cannot observe:

```sh
./scripts/coverage
```

This writes `coverage/`, including a browsable report at `coverage/html/index.html`. CI runs the same script and publishes the summary and the report as a build artifact.

## Documentation

This book is built with [mdbook](https://rust-lang.github.io/mdBook/) from the `docs/` directory:

```sh
just book        # build into docs/book
just book-serve  # serve locally with live reload
```

Pushes to `main` publish it to [dfinity.github.io/cksol](https://dfinity.github.io/cksol/).
