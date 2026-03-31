# Use this with
#
#  docker build -t cksol_minter .
#  or use ./scripts/docker-build
#
# The docker image. To update, run `docker pull ubuntu` locally, and update the
# sha256:... accordingly.
FROM --platform=linux/amd64 ubuntu@sha256:626ffe58f6e7566e00254b638eb7e0f3b11d4da9675088f4781a50ae288f3322 AS deps

ENV TZ=UTC

RUN ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && echo $TZ > /etc/timezone && \
    apt -yq update && \
    apt -yqq install --no-install-recommends curl ca-certificates \
        build-essential pkg-config libssl-dev llvm-dev liblmdb-dev clang cmake jq

# Install Rust and Cargo in /opt
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/cargo \
    PATH=/cargo/bin:$PATH

WORKDIR /cksol

RUN mkdir -p ./scripts
COPY ./scripts/bootstrap ./scripts/bootstrap
COPY ./rust-toolchain.toml ./rust-toolchain.toml

RUN ./scripts/bootstrap

# Pre-build all cargo dependencies. Because cargo doesn't have a build option
# to build only the dependecies, we pretend that our project is a simple, empty
# `lib.rs`. When we COPY the actual files we make sure to `touch` lib.rs so
# that cargo knows to rebuild it with the new content.
COPY Cargo.lock .
COPY Cargo.toml .
COPY ./scripts/build ./scripts/build
RUN mkdir -p libs/types/src && touch libs/types/src/lib.rs \
    && mkdir -p libs/types-internal/src && touch libs/types-internal/src/lib.rs \
    && mkdir -p minter/src && echo "fn main() {}" > minter/src/main.rs && touch minter/src/lib.rs \
    && mkdir -p integration_tests/src && touch integration_tests/src/lib.rs \
    && mkdir -p integration_tests/tests && touch integration_tests/tests/tests.rs \
    && ./scripts/build --only-dependencies \
    && rm -rf libs minter integration_tests \
    && rm Cargo.toml \
    && rm Cargo.lock

FROM deps AS build

COPY . .

RUN touch minter/src/main.rs

RUN ./scripts/build --cksol_minter
RUN sha256sum cksol_minter.wasm.gz

FROM scratch AS scratch_cksol_minter
COPY --from=build /cksol/cksol_minter.wasm.gz /
