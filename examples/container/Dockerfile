# Build context contains exactly the native Linux release executable named ocd.
# Use the same libc baseline as the Linux release builder; workerd is not a static ELF.
FROM ubuntu:24.04
LABEL org.opencontainers.image.title="open-compute" \
      org.opencontainers.image.url="https://open-compute.dev"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
COPY --chmod=0555 ocd /opt/open-compute/ocd
USER 65532:65532
WORKDIR /var/lib/open-compute
VOLUME ["/var/lib/open-compute"]
ENTRYPOINT ["/opt/open-compute/ocd"]
CMD ["--config", "/etc/open-compute/config.toml", "run"]
