FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS build

RUN apt-get update && apt-get install -y --no-install-recommends clang cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --locked -p open-compute-service --bin ocd

FROM ubuntu:24.04@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl libgcc-s1 procps \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/debug/ocd /usr/local/bin/ocd
COPY test/gateway/runtime-smoke.sh /usr/local/bin/gateway-smoke
RUN --network=none /usr/local/bin/gateway-smoke /usr/local/bin/ocd
