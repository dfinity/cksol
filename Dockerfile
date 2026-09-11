# Use this with
#
#  docker build -t cksol_minter .
#  or use ./scripts/docker-build
#
# The base image ships rustc/cargo 1.93.0, gcc, curl and ca-certificates.
# To update, pick a new tag from https://hub.docker.com/_/rust, run
# `docker buildx imagetools inspect rust:<tag>` and update the sha256 accordingly.
# The check below asserts that the image's rustc matches rust-toolchain.toml.
FROM --platform=linux/amd64 rust:1.93.0-bookworm@sha256:d0a4aa3ca2e1088ac0c81690914a0d810f2eee188197034edf366ed010a2b382 AS deps

ENV TZ=UTC

# Pin apt to the Debian snapshot that the base image was built from (the
# timestamp is documented in the image's /etc/apt/sources.list.d/debian.sources),
# so that the installed packages do not drift with the live mirrors.
# Bump DEBIAN_SNAPSHOT together with the base image.
ARG DEBIAN_SNAPSHOT=20260202T000000Z
# clang is needed to compile the C sources of zstd-sys for the wasm32 target.
RUN ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && echo $TZ > /etc/timezone && \
    sed -i \
        -e "s|^URIs: http://deb.debian.org/debian-security$|URIs: https://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT}|" \
        -e "s|^URIs: http://deb.debian.org/debian$|URIs: https://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT}|" \
        /etc/apt/sources.list.d/debian.sources && \
    grep -q "snapshot.debian.org/archive/debian/" /etc/apt/sources.list.d/debian.sources && \
    grep -q "snapshot.debian.org/archive/debian-security/" /etc/apt/sources.list.d/debian.sources && \
    apt-get -yq -o Acquire::Check-Valid-Until=false update && \
    apt-get -yqq install --no-install-recommends clang && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /cksol

COPY ./rust-toolchain.toml ./rust-toolchain.toml
RUN expected="$(awk -F'"' '/^channel/ {print $2}' rust-toolchain.toml)" && \
    actual="$(rustc --version | awk '{print $2}')" && \
    [ "$expected" = "$actual" ] || { echo "rustc $actual in base image != $expected in rust-toolchain.toml" >&2; exit 1; }

# The base image ships the host toolchain only; install the components and
# target listed in rust-toolchain.toml explicitly so that rustup does not
# download them lazily during the build.
RUN rustup target add wasm32-unknown-unknown && \
    rustup component add rustfmt clippy

# Install ic-wasm as a pinned binary with SHA-256 verification.
# Bump IC_WASM_VERSION and IC_WASM_SHA256 together when upgrading.
ARG IC_WASM_VERSION=0.3.5
ARG IC_WASM_SHA256=2debd76da946b4f74b6796caa62459d58c4dfef947a1f2614b56267baabf5c2d
RUN curl --proto '=https' --tlsv1.2 -fsSL --retry 5 --retry-delay 5 \
        "https://github.com/dfinity/ic-wasm/releases/download/${IC_WASM_VERSION}/ic-wasm-linux64" \
        -o /usr/local/bin/ic-wasm && \
    echo "${IC_WASM_SHA256}  /usr/local/bin/ic-wasm" | sha256sum -c - && \
    chmod +x /usr/local/bin/ic-wasm && \
    ic-wasm --version

# Pre-build all cargo dependencies. Because cargo doesn't have a build option
# to build only the dependencies, we pretend that our project consists of
# empty crates with the real manifests. When we COPY the actual files we make
# sure to `touch` the sources so that cargo knows to rebuild them.
COPY Cargo.lock .
COPY Cargo.toml .
COPY libs/types/Cargo.toml libs/types/Cargo.toml
COPY libs/types-internal/Cargo.toml libs/types-internal/Cargo.toml
COPY minter/Cargo.toml minter/Cargo.toml
COPY integration_tests/Cargo.toml integration_tests/Cargo.toml
COPY ./scripts/build ./scripts/build
RUN mkdir -p libs/types/src && touch libs/types/src/lib.rs \
    && mkdir -p libs/types-internal/src && touch libs/types-internal/src/lib.rs \
    && mkdir -p minter/src && echo "fn main() {}" > minter/src/main.rs && touch minter/src/lib.rs \
    && mkdir -p integration_tests/src && touch integration_tests/src/lib.rs \
    && ./scripts/build --only-dependencies --cksol_minter \
    && rm -rf libs minter integration_tests \
    && rm Cargo.toml \
    && rm Cargo.lock

FROM deps AS build

COPY . .

RUN touch minter/src/main.rs minter/src/lib.rs libs/types/src/lib.rs libs/types-internal/src/lib.rs

RUN ./scripts/build --cksol_minter
RUN sha256sum cksol_minter.wasm.gz

FROM scratch AS scratch_cksol_minter
COPY --from=build /cksol/cksol_minter.wasm.gz /
