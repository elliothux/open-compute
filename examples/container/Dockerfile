# Copy only the selected, checksum-verified GitHub Release executable.
FROM ubuntu:24.04 AS base
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 65532 open-compute \
    && useradd -u 65532 -g 65532 -M -s /usr/sbin/nologin open-compute
COPY --chmod=0555 ocd.linux-${TARGETARCH} /opt/open-compute/ocd

FROM base AS init
RUN apt-get update \
    && apt-get install -y --no-install-recommends util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY --chmod=0555 init-volume.sh /usr/local/bin/init-volume.sh
ENTRYPOINT ["/usr/local/bin/init-volume.sh"]

FROM base AS runtime
LABEL org.opencontainers.image.title="open-compute" \
      org.opencontainers.image.url="https://open-compute.dev" \
      org.opencontainers.image.source="https://github.com/elliothux/open-compute"
USER 65532:65532
WORKDIR /var/lib/open-compute
VOLUME ["/var/lib/open-compute"]
ENTRYPOINT ["/opt/open-compute/ocd"]
CMD ["--no-update-check", "--system", "run"]
