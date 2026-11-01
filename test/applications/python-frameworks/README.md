# Python framework fixtures

These three projects own the Django, Flask and FastAPI application sources for
P21 package qualification. Their application bytes match the reviewed static uploads. Earlier native
component inputs remain retained separately, including Django before its allowed
host update. The HTML template adds only the trailing
newline required by repository formatting; its rendered bytes remain unchanged.
The original run evidence remains in repository `.temp/`.
Each project fixes Python 3.14, SDK 1.9.2, uv 0.12.3 and framework
package versions. Applications and local data live under `src/` so upstream
collection excludes project configuration and host virtualenv files. Each `cloudflare.config.ts` uses the repository-pinned cf API.
Python-only builds call user-installed PyWrangler through `scripts/build-python.ts`,
without a version check or automatic installation. It prepares dependencies and
uses Wrangler solely to generate official Cloudflare Build Output. cf owns all
uploads, deployment, authentication and resource management. `cloudflare.config.ts`
is authoritative; the two bridge-generated inputs are disposable. Local HTML/JSON
data rules and vendored SDK files are handled by upstream Wrangler. See the
[main fixture workflow](../python-main/README.md) for tool/interpreter preparation.
From the repository root, run `bun run build:python /absolute/framework-project`
against a fresh self-contained `.temp/` copy. Each fixture now has its own
`type: module` package and `bun run build` entry, with cf/TypeScript checks before
the Python-only build.
TODO(P25): remove this bridge when cf has a usable, qualified Python builder.

| Project    | Application behavior                                                                                            | Existing evidence                                                                              |
| ---------- | --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `django/`  | WSGI body, repeated query params, status/header, stream/close, HEAD, 404 and framework exception                | `.temp/p20-python-django/qualification-02.json`                                                |
| `flask/`   | WSGI body, query params, status/header, HEAD, template data/escaping, 404, exception and context-bearing stream | `.temp/p20-python-flask/qualification-02.json`; stream remains failing in `diagnostic-01.json` |
| `fastapi/` | ASGI typed input/coercion, validation 422, sync route, stream, 404, exception and OpenAPI schema                | `.temp/p20-python-fastapi/qualification-01.json`                                               |

The fixtures expect `REVISION` and a test-owned `TOKEN`. Django accepts both the component
`worker.test` host and the ordinary daemon's `.localhost` Worker subdomain. Its
input was rebuilt and recaptured after that application-only setting change;
prior capture bytes remain in `.temp/p21-framework-gates/previous-django-input/`. Flask's
template is a declared Data module. The earlier MarkupSafe component used its
official plain-Python build. Fresh PyWrangler sync now selects the official
Pyodide MarkupSafe 3.0.3 PyEmscripten wheel; the previous 3.0.4 declaration failed
because no target wheel was available. FastAPI uses a PyEmscripten pydantic-core wheel.
A host-native wheel cannot replace either qualified input. Fresh sync/build for all three projects now succeeds under
`.temp/p21-python-build/frameworks-02/`. Django retains 3,776 modules, Flask 224
and FastAPI 424. Pinned cf prebuilt captures passed review: all 19 SDK source MIME/size/digests
match the inventory, main bytes match maintained source, and locks fix the SDK
wheel digest. Original captures and dependency locks are retained under
[`test/fixtures/python-frameworks/`](../../fixtures/python-frameworks/README.md).
This does not qualify framework execution.

Flask `/stream` deliberately retains the unresolved `stream_with_context`
failure. An ordinary HTTP/template pass must not omit that route and claim a full
Flask streaming pass. SDK 1.9.2 remains the latest published SDK on the 2026-10-04 PyPI review;
its installed WSGI source matches upstream and does not preserve a context across
the initial iterator advance and later async pull/close callbacks. P21 forbids
forking the SDK, so a local application shim is not an acceptance fix.

The original captured modules also passed Django and FastAPI native
prepare/restore/HTTP scenarios using the formally verified R4 workerd. Flask
imported MarkupSafe 3.0.3 and passed POST, then reproduced the SDK 1.9.2
context-bearing stream failure. This is component evidence, not ordinary daemon
acceptance. Native stderr includes the test-owned framework secret on the
intentional exception path and does not qualify production log sanitization.
See `.temp/p21-python-build/frameworks-02/qualification-02.json`.

These sources own the three registered `python-frameworks` cases in the
`p21_python_frameworks` executable. Django and ordinary Flask completed their full daemon cases in
`20261005T001633-3d63ba3c`; the corrected FastAPI case passed separately
in `.temp/p21-final-preflight/fastapi-minimum-case-01.log`. All three registered
cases then passed together in complete workspace coverage. The cases include Django/FastAPI stream assertions, immutable prepare/restore,
Version promotion/rollback, process restart, rejected syntax, ciphertext corruption
and restoration, log sanitization and cleanup. On 2026-10-04 the user explicitly
deferred context-bearing Flask streams as the known upstream issue
[workers-py#287](https://github.com/cloudflare/workers-py/issues/287). The Flask
route and failure evidence remain; its ordinary HTTP/template and lifecycle case
still runs, without claiming streaming support. Restore its stream checks after
an upstream fix. Static uploads are now captured from official PyWrangler-generated Build Output
consumed by cf against an authenticated loopback fixture. Build interpreter
preparation was explicitly authorized. Use the same `prepare-python-main.ts` capture tool with a third argument
`django`, `flask` or `fastapi` for a framework; its default remains the ordinary
main fixture. It captures the declared fixture Script and requires its local
package/data input, preserving the original multipart bytes. The registered
ordinary daemon scenarios qualify these framework paths; full upstream options
and the deferred Flask context-bearing stream remain outside that scope.
