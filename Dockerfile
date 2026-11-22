# syntax=docker/dockerfile:1.7
#
# Ferris development environment.
#
# The image is the contract: Rust, Node, the WebAssembly targets and the
# component tooling are installed here, so a checkout builds the same way on
# Linux, macOS or Windows (with Docker) without touching the host toolchain.
#
# Build:
#   docker build -t ferris-dev .
#   docker compose run --rm ferris bash
#
# Everything runs as an unprivileged user; the repository is mounted at
# /workspace by compose, not copied into the image.

# Every base image is pinned by tag *and* digest (DX-17): the tag documents the
# intent, the digest is what makes two builds a year apart produce the same
# image. The digests are the OCI image index of each tag, so amd64 and arm64
# both resolve. Bump the tag and the digest together.
FROM node:24.22.3-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54 AS node

FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e

ARG USER_NAME=ferris
ARG USER_ID=1000
ARG GROUP_ID=1000

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/usr/local/bin:$PATH

# Build essentials plus the small system libraries the workspace links
# against. TLS is rustls in the application crates; libssl-dev is only for the
# developer tools (cargo-leptos) and does not change the server's TLS stack.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        libsqlite3-dev \
        libssl-dev \
        pkg-config \
        sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# Node 24 is copied from the official image instead of Debian's older package:
# the Desk, the documentation site and cargo-leptos expect a current Node.
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -sf ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -sf ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# The two targets the repository uses: the Desk's browser bundle and the app
# component target. rust-toolchain.toml pins the same versions for editors.
RUN rustup target add wasm32-unknown-unknown wasm32-wasip2

# `wasm-bindgen` must match the crate version in Cargo.lock (0.2.128). The other
# two are the tools the CLI invokes for the desktop/component workflow, pinned to
# an exact version (DX-17) so a rebuild months later does not silently pick a new
# cargo-leptos/wasm-tools: `--locked` only freezes each tool's own dependency
# graph, it does not choose the version of the tool itself.
#   - cargo-leptos 0.3.12 (crates.io)
#   - wasm-tools 1.263.0 (crates.io; the version the dev image already had)
RUN cargo install wasm-bindgen-cli --version 0.2.128 --locked \
    && cargo install cargo-leptos --version 0.3.12 --locked \
    && cargo install wasm-tools --version 1.263.0 --locked

# Keep the uid/gid configurable so bind-mounted files stay owned by the host
# user on Linux. `--build-arg USER_ID=$(id -u) --build-arg GROUP_ID=$(id -g)`.
RUN groupadd -g "${GROUP_ID}" "${USER_NAME}" 2>/dev/null || true \
    && useradd --create-home --uid "${USER_ID}" --gid "${GROUP_ID}" \
        --shell /bin/bash "${USER_NAME}" \
    && mkdir -p /workspace \
    && chown -R "${USER_ID}:${GROUP_ID}" /workspace

USER ${USER_NAME}
WORKDIR /workspace

# Interactive by default; compose overrides it for `cargo`, `ferris`, etc.
CMD ["bash"]
