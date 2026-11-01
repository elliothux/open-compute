# Python Queue qualification application

This application uses the unchanged `workers-runtime-sdk` 1.9.2. The owning
`python-queues` scenario deploys its genuine cf upload to an ordinary daemon
and compares Python and JavaScript against the same persisted Queue and KV.
Compilation and input qualification do not establish a passing product Gate.

The scenario covers json/text/bytes/v8 Date messages, single and batch sends,
message metadata, ack/retry precedence, exhausted retries into a DLQ, producer
validation failures, paused delivery across fresh daemon restarts, two Python
versions and secrets, rollback, a JavaScript consumer, and rejection of Queue deletion while immutable
Worker versions still reference it, with continued delivery before and after restart. KV stores observations; it does
not replace the durable Queue scheduler. Read-only management authorization is
checked separately from tenant binding permissions.

Consumer attachments are configured through the public Queue API after Worker
deployment and rollback. The fixture does not prove that an independently
configured API attachment follows a Worker promotion automatically. Broader crash
and stale-claim fencing, cancellation, quotas and binding permission coverage is
owned by the existing Queue product Gates; this application does not establish
those complete matrices by itself.

The authoritative config is `cloudflare.config.ts`. Install PyWrangler yourself
(`uv tool install workers-py`) and build with:

```sh
bun run --filter @open-compute/python-queues-fixture build
```

Only Python builds use the temporary user-installed PyWrangler bridge. cf owns
upload, deployment, authentication and resources; the bridge does not install
or validate the PyWrangler version. Its removal TODO and missing-tool behavior
are documented in [PyWrangler removal TODO](../../../docs/p25-platform-follow-ups.md).
Generated Python dependencies and Build Output remain untracked. The developer
capture tool consumes a fresh prebuilt copy under the repository's `.temp/`:

```sh
node test/conformance/applications/prepare-python-main.ts \
  /absolute/repository/.temp/queue-build \
  /absolute/repository/.temp/new-queue-capture queues
```

Its loopback catalog fixture serves cf's read-only Queue lookup. It creates no
Cloudflare resources. Product Gates consume reviewed static upload bytes and
never build or modify SDK modules. Type generation directs cf's config cache
(`WRANGLER_CACHE_DIR`) and its internal Request metadata cache
(`MINIFLARE_CACHE_DIR`) to the repository `.temp/`. Both names are upstream
cache interfaces; the command remains cf. No Miniflare result is used as
runtime or product Gate evidence.

The V8 message includes 50,000 Chinese characters. Both consumers verify the exact
value, and the registered case checks unmodified native V8 bytes in SQLite across
restart and rollback. The temporary Python build bridge uses user-installed
PyWrangler; cf captures and uploads its prebuilt output. The latest run passed delivery, retry, DLQ and lifecycle checks before exposing an
incorrect deletion expectation in the fixture. The corrected case is awaiting
a complete pass.
