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
      git-lfs \
      libssl-dev \
      pkg-config \
      python3 \
    && git lfs install --system \
    && rm -rf /var/lib/apt/lists/* \
    && test "$(rustc --version | awk '{print $2}')" = 1.98.0 \
    && test "$(bun --version)" = 1.3.14 \
    && test "$(node --version)" = v26.8.1

RUN cargo install mbx --version 1.22.0 --locked

USER pwuser
RUN mbx settings set build_script_execution false \
    && mbx settings set target.views false \
    && mbx settings set restore_hardlink false

WORKDIR /workspace
