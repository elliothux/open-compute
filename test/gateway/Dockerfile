# This image checks the exact managed Caddyfile fixture against the custom provider.
# It is a test environment, not a release image or an ocd runtime override.
FROM golang:1.26.4-bookworm@sha256:b305420a68d0f229d91eb3b3ed9e519fcf2cf5461da4bef997bf927e8c0bfd2b

ENV GOTOOLCHAIN=local
WORKDIR /src
COPY packages/caddy/ packages/caddy/
COPY test/gateway/ test/gateway/
COPY test/gateway/managed.caddyfile /data/gateway/managed.caddyfile
RUN mkdir -p /data/gateway/run /data/gateway/storage
WORKDIR /src/packages/caddy
RUN go mod download
RUN go test ./...
RUN go build -trimpath -o /usr/local/bin/oc-caddy .
RUN /usr/local/bin/oc-caddy list-modules | grep -Fx 'dns.providers.opencompute'
RUN /usr/local/bin/oc-caddy adapt \
    --config /src/test/gateway/Caddyfile \
    --adapter caddyfile --validate >/dev/null
