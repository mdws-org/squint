# Build environment for Anthropic's OSS Scanner (https://github.com/anthropics/oss-scanner).
# The scanner builds this with the repository as the context and then removes network access, so every crate,
# toolchain and fuzz target the scan needs is fetched and compiled here. Only the Rust engine in core/ builds on
# Linux; the macOS app in app/ needs Xcode and is out of scope (see threat_model.md).
FROM debian:bookworm

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    CARGO_NET_OFFLINE=false

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential clang pkg-config git python3 \
    && rm -rf /var/lib/apt/lists/*

# Stable builds the engine as CI does; nightly is what cargo-fuzz needs.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
    && rustup toolchain install nightly --profile minimal \
    && cargo install cargo-fuzz --locked

# A login shell (bash -l) resets PATH from /etc/profile and loses /usr/local/cargo/bin;
# linking the toolchain into /usr/local/bin keeps it reachable from either kind of shell.
RUN ln -s /usr/local/cargo/bin/cargo /usr/local/cargo/bin/rustc /usr/local/cargo/bin/rustup \
      /usr/local/cargo/bin/cargo-fuzz /usr/local/bin/

COPY . /src
WORKDIR /src/core

# Fetch every dependency while the network is still up, for the engine and the fuzz crate.
RUN cargo fetch --locked && (cd fuzz && cargo fetch --locked)

# The engine and its CLI (target/debug/squint), with debug info, as CI builds them.
RUN cargo build --locked --all-targets

# The libFuzzer targets in core/fuzz/fuzz_targets/, built under ASan; run one offline with
#   cd /src/core && cargo +nightly fuzz run <target> -- -max_total_time=60
RUN cargo +nightly fuzz build --debug-assertions

ENV CARGO_NET_OFFLINE=true

# The cheap tests, so a broken image shows up in the log without failing the build.
RUN cargo test --locked || echo "WARNING: cargo test reported failures"
