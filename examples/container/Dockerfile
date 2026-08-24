# Offline image of the documented release layout. Build context is a packaged
# dest directory (bin/, runtime/, licenses/, share/).
FROM scratch
COPY bin /opt/open-compute/bin
COPY runtime /opt/open-compute/runtime
COPY licenses /opt/open-compute/licenses
COPY share /opt/open-compute/share
USER 65532:65532
WORKDIR /var/lib/open-compute
VOLUME ["/var/lib/open-compute"]
# Runtime tree is read-only in the image; the data volume is the only writable path.
ENTRYPOINT ["/opt/open-compute/bin/platformd"]
CMD ["--config", "/opt/open-compute/share/default-config.toml", "run"]
