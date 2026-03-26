# Use this with
#
#  docker build -t cksol-minter .
#  or use ./scripts/docker-build
#
# The docker image. To update, run `docker pull ubuntu` locally, and update the
# sha256:... accordingly.
FROM --platform=linux/amd64 ubuntu@sha256:626ffe58f6e7566e00254b638eb7e0f3b11d4da9675088f4781a50ae288f3322 AS deps

ENV TZ=UTC

RUN ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && echo $TZ > /etc/timezone && \
    apt -yq update && \
    apt -yqq install --no-install-recommends curl ca-certificates \
        build-essential pkg-config libssl-dev llvm-dev liblmdb-dev clang cmake

# Install Rust and Cargo in /opt
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/cargo \
    PATH=/cargo/bin:$PATH

WORKDIR /cksol

COPY ./rust-toolchain.toml ./rust-toolchain.toml

RUN curl --fail https://sh.rustup.rs -sSf \
        | sh -s -- -y --default-toolchain "none" --no-modify-path && \
    rustup show && \
    cargo install ic-wasm --version 0.9.0

# Pre-build all cargo dependencies. Because cargo doesn't have a build option
# to build only the dependencies, we pretend that our project is a simple, empty
# `lib.rs`. When we COPY the actual files we make sure to `touch` lib.rs so
# that cargo knows to rebuild it with the new content.
COPY Cargo.lock .
COPY Cargo.toml .
COPY libs/types/Cargo.toml libs/types/Cargo.toml
COPY libs/types-internal/Cargo.toml libs/types-internal/Cargo.toml
COPY minter/Cargo.toml minter/Cargo.toml
COPY integration_tests/Cargo.toml integration_tests/Cargo.toml
RUN mkdir -p libs/types/src && touch libs/types/src/lib.rs \
    && mkdir -p libs/types-internal/src && touch libs/types-internal/src/lib.rs \
    && mkdir -p minter/src && echo "fn main() {}" > minter/src/main.rs && touch minter/src/lib.rs \
    && mkdir -p integration_tests/src && touch integration_tests/src/lib.rs \
    && mkdir -p integration_tests/tests && touch integration_tests/tests/tests.rs \
    && cargo build --locked --target wasm32-unknown-unknown --release --package cksol-minter \
    || true \
    && rm -rf libs minter integration_tests

FROM deps AS build

COPY . .

RUN touch minter/src/main.rs minter/src/lib.rs \
    libs/types/src/lib.rs libs/types-internal/src/lib.rs

RUN cargo build --locked --target wasm32-unknown-unknown --release --package cksol-minter

RUN ic-wasm target/wasm32-unknown-unknown/release/cksol-minter.wasm \
        -o cksol-minter.wasm \
        metadata candid:service -f minter/cksol-minter.did -v public \
        --keep-name-section \
    && ic-wasm cksol-minter.wasm -o cksol-minter.wasm shrink --keep-name-section \
    && gzip -fckn9 cksol-minter.wasm > cksol-minter.wasm.gz \
    && sha256sum cksol-minter.wasm.gz

FROM scratch AS scratch_cksol_minter
COPY --from=build /cksol/cksol-minter.wasm.gz /
