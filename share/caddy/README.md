# Fixed Caddy build dependency

The four executable files are Git LFS objects built from [`packages/caddy`](../../packages/caddy).
[`caddy.lock.json`](../../packages/caddy/caddy.lock.json) is the only pin and records the Caddy
version, fixed Docker builder, source digests, build flags, and each target binary SHA-256.

`bun run build` verifies every binary. Cargo verifies and embeds only its selected target in the
same offline runtime payload as workerd. Production never searches `PATH`, installs a module, or
downloads Caddy. Update the Go source, module graph, lock, all four binaries, license evidence, and
Docker runtime check together.
