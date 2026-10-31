---
title: "Python"
description: "Build Python Workers with cf and your installed PyWrangler."
---

Cloudflare announced [Python Workers general availability](https://blog.cloudflare.com/python-workers-ga/)
on September 21, 2026. open-compute qualifies the specific deployment and library
paths described here; upstream GA does not establish full open-compute parity.

Cloudflare Python Workers use the `workers` SDK, a `WorkerEntrypoint` class, and the
`python_workers` compatibility flag. open-compute implements a deployment-time
preparation process and embeds its pinned Pyodide runtime in `ocd`; production
startup does not download a Python runtime or install packages.

open-compute supports ordinary Python deployments with multi-module imports,
package data and supported Pyodide/PyEmscripten dependencies. The supported framework
paths include Django and FastAPI HTTP and streaming, and Flask HTTP and templates.
Prepared deployments preserve their identity and resource access across restart
and rollback.

The embedded interpreter bundle supplies Python runtime bytes. A prepared
application snapshot separately captures deployment initialization; embedding
Pyodide alone does not remove application cold-start work.

The public upload API currently supports only self-owned Durable Object namespaces
and Workflow bindings. Cross-Script bindings are rejected; the Python and JavaScript
Workers use separately owned DO namespaces and Workflow definitions.

## HTTP clients

The ordinary deployment qualification uses unmodified Pyodide `requests` 2.33.1
and `httpx` 0.28.1 wheels selected by PyWrangler. It exercises requests and both
sync/async httpx against a controlled HTTP server: Unicode request/response data,
status errors, complete response streams, actual timeouts, connection refusal,
native subrequest limits and recovery after failures, restart and rollback.
The selected requests/urllib3 transport reports its timeout as `ConnectionError`
with a timeout cause, rather than `requests.Timeout`; callers must handle that
exception shape. Async httpx task cancellation is checked separately with bounded cleanup;
cancelling a Python task does not guarantee immediate abort of the underlying
Fetch. TLS, database drivers, higher-level AI clients and HTTP MCP remain in
[#128](https://github.com/elliothux/open-compute/issues/128).

## Known upstream issue: Flask streaming

The unmodified runtime SDK 1.9.2 has a
[WSGI context handling failure](https://github.com/cloudflare/workers-py/issues/287)
when Flask streams a response with `stream_with_context`. Later stream reads or
cleanup can lose the request context and fail. This was reproduced with stock
workerd, independently of the open-compute daemon; hosted Cloudflare behavior has
not been verified. Flask streams that depend on request/app context are excluded
from the current support scope. Ordinary Flask HTTP and templates are supported. The SDK remains unmodified; this limitation will be
revalidated after an upstream fix.

## Python project build tooling

Run `node scripts/build-python.ts /absolute/python-project` from the
repository root. The project must contain `pyproject.toml`, a `type: module`
`package.json`, and a Python entrypoint in `cloudflare.config.ts`. Keep the
entrypoint and local package/data in a separate application directory inside the
project, such as `src/main.py`; the bridge rejects root or external entrypoints to
prevent development files from entering the bundle.

The build script calls the user's installed PyWrangler (`workers-py`), preferring
the project's `.venv/bin/pywrangler` and then PATH. Missing PyWrangler produces an
error with `uv tool install workers-py` guidance. It is neither bundled nor
automatically installed, and its version is not checked. Install Wrangler locally as a
Python project development dependency. PyWrangler
prepares dependencies and proxies only the local build to Wrangler. Prepare the
build interpreter required by your PyWrangler installation explicitly; this
bridge disables implicit Python interpreter and Wrangler downloads. Add this
command to the project's build script; the user or CI starts the build, and the
script invokes PyWrangler internally.

`cloudflare.config.ts` remains the sole Worker configuration. The bridge generates
and removes two temporary builder inputs; it does not overwrite existing
Wrangler files. Upstream tooling generates `.cloudflare/output/v0/`, including
the unmodified SDK and local JSON/HTML package data. Authentication, resource
management, uploads and deployment use cf with `--prebuilt`. The daemon does not
install or build Python dependencies.

For static assets, add `--assets-directory public` to the same build command.
The directory must be a separate directory inside the Python project. This option
selects files for the upstream builder; declare the binding with `bindings.assets()`
and routing under `worker.assets` in `cloudflare.config.ts`. It does not move
Worker configuration into a Wrangler file. The ordinary runtime case compares Assets SDK/FFI/JavaScript responses and
immutable asset bytes after promotion, restart and rollback.

This temporary Python-only PyWrangler bridge and its Wrangler inputs will be removed
once the official cf Python builder produces equivalent, qualified Build Output.
Keep one build path when switching.

## Dynamic Python limitation

Python children created at request time through
[Dynamic Workers](/docs/workers/runtime-apis/bindings/#dynamic-workers) are not
qualified for use. Their fresh-isolate Pyodide initialization must fit the existing
one-second startup CPU limit; open-compute does not raise that limit or use an
ordinary deployment's prepared artifact for a Dynamic child. A warm cache or a
successful ordinary preparation does not qualify this path. See
[behavior differences](/docs/platform/deviations/) for `OC-WKR-LIMIT-001`.
The current formal-pin baseline and remaining implementation work are tracked in
[the Python implementation record](https://github.com/elliothux/open-compute/blob/main/docs/implemented/p21-python-workers.md)
and [#126](https://github.com/elliothux/open-compute/issues/126).

## TCP qualification boundary

The upstream Python socket bridge uses the existing Workers
`cloudflare:sockets.connect()` capability. The ordinary Python fixture verifies
native TCP through JavaScript FFI; Python database-driver socket/asyncio and TLS
qualification remain in [#128](https://github.com/elliothux/open-compute/issues/128).
This does not provide inbound TCP listeners, Hyperdrive or a cross-request
connection pool. Outbound IP access follows the host network and operator-owned
filtering described in [behavior differences](/docs/platform/deviations/).

Cloudflare reference: [Python Workers](https://developers.cloudflare.com/workers/languages/python/).
