FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS rust
FROM oven/bun:1.3.14@sha256:e10577f0db68676a7024391c6e5cb4b879ebd17188ab750cf10024a6d700e5c4 AS bun
FROM node:26.8.1-bookworm-slim@sha256:367679cf9792759492a486e4aa4b421764d71a9546a6dae8aab81a99eb797b3e AS node
FROM mcr.microsoft.com/playwright:v1.63.0-noble@sha256:eff16c30e6f3f4af0a03fa4b706120d5e9b0891c344a27d64559aff5900a4a27

ENV PATH=/usr/local/cargo/bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    RUSTUP_HOME=/usr/local/rustup

COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
COPY --from=node /usr/local/bin/node /usr/local/bin/node

RUN ln -s /usr/local/bin/bun /usr/local/bin/bunx \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
      build-essential \
      cmake \
      curl \
      git-lfs \
      libssl-dev \
      pkg-config \
      python3 \
      xz-utils \
    && git lfs install --system \
    && rm -rf /var/lib/apt/lists/* \
    && test "$(rustc --version | awk '{print $2}')" = 1.98.0 \
    && test "$(bun --version)" = 1.3.14 \
    && test "$(node --version)" = v26.8.1

# Linux release packaging links ocd against the glibc baseline with `cargo zigbuild`.
RUN curl --fail --location --output /tmp/zig.tar.xz \
      https://ziglang.org/download/0.14.1/zig-aarch64-linux-0.14.1.tar.xz \
    && echo 'f7a654acc967864f7a050ddacfaa778c7504a0eca8d2b678839c21eea47c992b  /tmp/zig.tar.xz' | sha256sum -c - \
    && curl --fail --location --output /tmp/cargo-zigbuild.tar.xz \
      https://github.com/rust-cross/cargo-zigbuild/releases/download/v0.24.1/cargo-zigbuild-aarch64-unknown-linux-gnu.tar.xz \
    && echo '70bb4c9cc71a6d16211b9079996e3cd588671053312adeff4d043499ed887601  /tmp/cargo-zigbuild.tar.xz' | sha256sum -c - \
    && tar -xJf /tmp/zig.tar.xz -C /opt \
    && ln -s /opt/zig-aarch64-linux-0.14.1/zig /usr/local/bin/zig \
    && tar -xJf /tmp/cargo-zigbuild.tar.xz -C /usr/local/bin --strip-components=1 \
      cargo-zigbuild-aarch64-unknown-linux-gnu/cargo-zigbuild \
    && rm /tmp/zig.tar.xz /tmp/cargo-zigbuild.tar.xz \
    && test "$(zig version)" = 0.14.1 \
    && test "$(cargo-zigbuild --version)" = 'cargo-zigbuild 0.24.1'

USER pwuser

WORKDIR /workspace
