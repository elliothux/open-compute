# Reviewed cf Python runtime/FFI input

`upload.multipart` is the original request from pinned cf
`workers versions create --prebuilt`, captured on loopback after a fresh build
by user-installed PyWrangler. `manifest.json` records every module MIME,
size and digest; `pylock.toml` records the official builder package input.

The 194-module upload has SHA-256
`515bb1afe95d4672ddc69e8050813511f55db8356af516a8a68e3c4634d77a4d`.
All 19 SDK sources match `../python-main/sdk-inventory.json`. The main module
and all seven helpers (`cache_cases.py`, `image_cases.py`, `ai_cases.py`,
`vector_cases.py`, `artifact_cases.py`, `search_cases.py`, `http_client_cases.py`) match
`../../applications/python-runtime/src/` exactly.

The ordinary daemon case uses actual KV, D1 and R2 shared by Python SDK,
raw JavaScript FFI and a JavaScript Worker. It checks structured conversion,
bytes, Web Crypto and explicit PyProxy callback release, native HTTP/TCP,
waitUntil persistence, declared stdlib limitations, temporary filesystem loss
on fresh daemon/version restore, secret-safe errors/logs, immutable artifacts
and promotion/rollback. JavaScript comparisons cover common operations;
PyProxy release and Python stdlib checks belong only to Python.

The Cache API routes use native `js.caches`, with either official SDK
Request/Response conversion or raw JavaScript FFI. SDK 1.9.2 has no separate
Cache wrapper. JavaScript uses the same public API. The sole runtime case
checks default/named namespaces, put/match/delete, ETag and Range results,
ignoreMethod, invalid puts, Worker isolation, and shared Cache API state
across promotion, fresh daemon restart and rollback. A write in the second
version remains visible after rollback; default and named namespaces stay
independent. Default version isolation belongs to automatic Workers caching. See the [official Cache contract](https://developers.cloudflare.com/workers/runtime-apis/cache/).

The asset files are built through the explicit `--assets-directory public`
Python bridge input. Runtime routing and the `ASSETS` binding come from cf
configuration. `assets-session.json`, `assets-upload.multipart` and
`assets-upload.json` preserve cf's real session request, base64 batch and
file MIME/digests. The published text file has SHA-256
`a13f6494814a78b620aba0ef6ea0ff195bb0feadb472e6ac869b2da5db118f2a`.
The ordinary case redeems actual daemon upload sessions, then checks Python
SDK/raw FFI and JavaScript Fetcher GET, HEAD, ETag/304, missing files and invalid
methods. It checks a second immutable asset version and the original bytes
following fresh restart and rollback. Binding Range requests return the full
200 response, matching the [pinned upstream Asset Worker](https://github.com/cloudflare/workers-sdk/blob/52b0dc0e99b5bbdb86c06ed53fb703048efc36cf/packages/workers-shared/asset-worker/src/handler.ts)
and its [stored-stream reader](https://github.com/cloudflare/workers-sdk/blob/52b0dc0e99b5bbdb86c06ed53fb703048efc36cf/packages/workers-shared/asset-worker/src/worker.ts).
Cache API range matching is a separate contract.

The `IMAGES` binding uses the existing shared `ImagesBindingImpl`. The official
SDK already recognizes that class and converts arguments/results; raw FFI and
JavaScript use the same binding. The sole ordinary case sends one Rust-generated
PNG to all three callers and compares info, PNG/JPEG/WebP transform bytes,
draw plus rotation, custom response headers, and `image()` streaming. Rust decodes
the results to check their formats, dimensions and opacity. Invalid stream,
unsupported options and malformed image bytes must reject with sanitized codes.
The same assertions follow fresh daemon restart, promotion and rollback.
These are tests for the current raster subset, using the [official Images methods](https://developers.cloudflare.com/images/optimization/binding/)
as contract input; current upstream text/hosted-image surfaces are not qualified
by these cases. Source and static checks do not prove runtime parity.

The official SDK recognizes the shared `Ai` and `VectorizeIndexImpl` classes.
`ai_cases.py` covers the declared local Markdown Conversion subset: single and
batch documents, handle transforms, text output, supported formats, and sanitized
invalid-document/option errors. Inference remains unsupported. See the
[official binding methods](https://developers.cloudflare.com/workers-ai/features/markdown-conversion/usage/binding/).
`vector_cases.py` covers insert, duplicate insert, upsert, deletion, describe,
ID lookup, query/queryById, namespace and indexed metadata filtering across all
three callers, using the [official Vectorize API](https://developers.cloudflare.com/vectorize/reference/client-api/).
Each mutation waits for its exact durable frontier through the real operator
API before a read. Index data survives fresh restart and Worker rollback;
rollback changes the Worker version and leaves mutable index data intact.
Referenced index deletion must reject without changing authority or active
versions. JSON number comparisons preserve numeric values independently of the
language's integer/fractional spelling. The FFI route distinguishes JavaScript
null from undefined and encodes `jsnull` explicitly in the test response.

`artifact_cases.py` uses the declared Artifacts extension through the existing
shared `ArtifactsBinding`. The SDK does not recognize that class, so both the
official entrypoint's env and the explicit raw env use native FFI conversion.
There is no separate Python facade or SDK wrapper. The ordinary case covers
create/get/list, cursor pagination, fork/delete, token issue/revoke and persisted
token states, namespace isolation, sanitized errors, and read-only management
authorization. Issued secrets are checked inside the Worker and removed from
its response; the Rust harness also rejects a returned token. The same repo/token
IDs, metadata and states must survive fresh daemon restart, version promotion
and rollback. These cases
cover the declared pinned subset. [Current upstream Artifacts docs](https://developers.cloudflare.com/artifacts/api/workers-binding/)
describe a newer disposable capability with `info()` and Git content methods;
those methods are not qualified by the existing property-based extension.
No historical/new capability selector or parallel implementation is added.

`search_cases.py` calls the existing namespace and instance AI Search classes.
SDK 1.9.2 does not wrap these classes, so official entrypoint env and raw env
both use explicit native FFI. The JavaScript Worker uses the same backend.
The ordinary case covers namespace CRUD, string/Blob/stream item upload and
polling, nested item info/logs/chunks/download, jobs, vector and filtered keyword
search, namespace federation, chat and SSE. It compares complete query responses;
fresh chat UUIDs and stream timestamps are validated before being removed from
the comparison. Item IDs and mutable data must persist across fresh daemon
restart, Worker promotion and rollback. Invalid queries, rename attempts,
missing/cross-namespace instances, read-only management writes and referenced
instance deletion must fail without changing authority.

The provider fixture is a bounded loopback HTTP service under the real daemon
AI configuration, with a fixed custom tokenizer and explicit embedding/chat
aliases. It exercises the actual index/storage/provider pipeline; it does not
prove Cloudflare-hosted model parity. These assertions cover the declared local
subset, informed by the [official AI Search binding contract](https://developers.cloudflare.com/ai-search/api/search/workers-binding/).
Full current upstream option/provider coverage remains unqualified. Input and
static evidence are in `.temp/p21-python-ai-search/`; the declared local subset
is covered by the registered ordinary runtime case.

The stdlib expectations come from the official Cloudflare Python docs.
Current Pyodide docs describe 314.0.7 while the formal runtime pins 314.0.6;
documentation is a contract input, not evidence that this pin passes.
Temporary files are not durable and the case does not require state to survive
arbitrary isolate selection between requests. Host networking uses explicit
loopback HTTP and TCP fixtures under the documented operator-owned policy.

Build/capture and validation evidence is preserved in
`.temp/python-runtime/`, `.temp/p21-python-cache/` and
`.temp/p21-python-assets-runtime/`, `.temp/p21-python-images/` and
`.temp/p21-python-ai-vectorize/` and `.temp/p21-python-artifacts/` and `.temp/p21-python-ai-search/`.
Previous raw inputs remain in those
evidence directories. The asset capture helper is a bounded developer probe
for one nonempty upload bucket; it is not a general Assets uploader. The ordinary product case has passed; source review and capture alone do not
prove complete P21 acceptance.
No workerd rebuild or SDK modification was performed.

The complete ordinary case passed in `20261005T025346-81a3b2a3` (473.78 seconds,
1 pass, 0 ignored), including promotion, fresh restart, rollback and cleanup.
The registered case also passed in complete workspace coverage. Final
uninstrumented acceptance is recorded separately.
