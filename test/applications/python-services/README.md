# Python Service Binding fixture

This application owns the Python input for `python-services`. Official SDK
1.9.2 and Python 3.14 are fixed in `pyproject.toml`. `Default` calls a same-account
JavaScript Worker through default and named Service bindings; `NamedApi` supplies
a Python named entrypoint. The JavaScript peer also calls both Python entrypoints.
Structured values include Unicode, null, booleans, arrays and numbers. A Python
callback uses the official `python_from_rpc` / `python_to_rpc` conversion functions;
the SDK bytes are never changed.

Build a fresh self-contained copy under repository `.temp/` with the user-installed
PyWrangler bridge, then capture its official Build Output with cf:

```sh
bun run build:python /absolute/python-services-project
node test/conformance/applications/prepare-python-main.ts /absolute/python-services-project /absolute/new-capture services
```

The existing authorized Emscripten build interpreter is sufficient. No runtime or
tool is downloaded implicitly, no PyWrangler version is checked, and cf owns
upload/deploy/auth/resource operations. See the [main fixture preparation](../python-main/README.md)
and the removal TODO in [PyWrangler removal TODO](../../../docs/p25-platform-follow-ups.md).

The reviewed original capture and dependency lock are retained under
[`test/fixtures/python-services/`](../../fixtures/python-services/README.md).
The registered real-daemon case checks fetch method/body/query/header/host, named
RPC, structured values and callback conversion in both languages, sanitized
failures, target promotion/rollback, Python secret/revision identity changes,
fresh-process restore, refusal of referenced target deletion, explicit forced
deletion and fail-closed calls before/after restart, snapshot encryption, logs and
cleanup. JavaScript inputs are ordinary test protocol fixtures; they never replace
the official Python capture. The complete ordinary case passed in
`20261005T004230-800b724a` (257.47 seconds, 1 pass/0 ignored). They do not cover every RPC
capability, stream, cancellation, permission or budget case in P21.
