# Python main fixture

This source owns the ordinary Python Worker for the planned `python-main`
Gate: local package/data imports, vars, a required secret, KV/D1/R2 writes and
reads, streaming, sanitized exceptions and host-network fetch.
The secret-proof route returns an HMAC of a fixed test challenge, allowing the
Gate to verify the actual bound secret after promotion, restart and rollback
without returning plaintext. It is test Worker code, not a platform endpoint.

`pyproject.toml` fixes Python 3.14, workers-runtime-sdk 1.9.2 and uv 0.12.3.
`cloudflare.config.ts` uses the repository-pinned cf configuration API. Resource
identifiers are fixture declarations; the Gate must bind isolated resources.
Python-only development builds use `scripts/build-python.ts`: the project's
`.venv/bin/pywrangler` or the user's PATH installation. Missing PyWrangler is an
error with `uv tool install workers-py` guidance. We do not bundle, install or
check its version. Install Wrangler yourself as a development dependency with
`bun add --dev wrangler`; PyWrangler proxies the local build to it. Authentication,
uploads, deployment and resource management continue to use cf.

Keep the Python entrypoint and local package/data in a separate directory inside
the project, such as `src/main.py`. The bridge rejects project-root or external
entrypoints before sync because upstream collection would include configuration
and host virtualenv files.

`cloudflare.config.ts` remains authoritative. The bridge exclusively creates a
disposable date/flags input for `pywrangler sync`, removes it, then uses upstream
`build --experimental-new-config --experimental-cf-build-output`. A disposable
builder-options file declares local JSON/HTML data. Existing configs are never
overwritten; both temporary inputs are removed on normal completion or failure.
The bridge has no alternate builder. Upstream tooling owns SDK/package bytes and
Build Output; the daemon never invokes it. Python interpreter downloads are
explicit (uv defaults to `never` here), and npx is offline. Prepare the build
interpreter requested by your installed PyWrangler before fresh sync.

Build a self-contained copy under a new repository `.temp/` directory for
qualification (including `package.json` with `type: module`):

```sh
bun run build:python /absolute/python-project
# Or, inside this project:
bun run build
# Upload/deploy already generated output using cf, never PyWrangler:
ocd cf deploy --prebuilt
```

TODO(P25): remove this bridge, its scripts/dependency/tests and these temporary
Wrangler inputs when the official cf Python builder produces equivalent qualified
Build Output. See [PyWrangler removal TODO](../../../docs/p25-platform-follow-ups.md).

After generating official Python Build Output in a project under `.temp/`, run:

```sh
node test/conformance/applications/prepare-python-main.ts /absolute/prebuilt-project /absolute/new-destination
```

The capture runs pinned cf `workers versions create --prebuilt` against an
authenticated loopback fixture. It records original multipart bytes, MIME, wire/canonical module names, module
digests, executable and output config digests. It supplies only the fixed test
`TOKEN` value and the metadata reads required by cf; it uploads one Version. It rejects unknown API
requests and incomplete SDK/package input, retains failure evidence and refuses
an existing destination. It never builds dependencies, downloads a runtime or
writes to remote resources. Product Gates consume reviewed static input and do
not run this tool.

The reviewed dependency-generated Main Gate input is retained under
`test/fixtures/python-main/`. Fresh `p21-python-prepared-integrity/build-02`
sync/build and pinned cf capture qualified all 19 unchanged SDK source hashes,
the official package lock and 32 modules on 2026-10-04. The preceding `real-03`
input remains retained evidence. The
build interpreter was explicitly downloaded with user authorization; production
runtime pins are unchanged by that preparation. The complete ordinary case passed in
`20261005T001106-bb74c1a5` (299.38 seconds, 1 pass/0 ignored). Earlier `real-02` format handoff evidence used
previously installed SDK bytes and remains component-only historical evidence.
The ordinary case verifies daemon admission, encrypted artifacts, restart,
rollback and package failures. The registered case also passed in complete workspace
coverage. Final uninstrumented acceptance is recorded separately; build/capture
alone does not qualify the product.
On 2026-10-03, pinned cf's Python delegate `cloudflare-py-dev-server` returned
404 from PyPI, and this fixture's `cf build` exited with no installed builder.
The scoped logs are retained under
`.temp/upstream-cf-merge/20261003-122327/python-builder-discovery-01/`.
