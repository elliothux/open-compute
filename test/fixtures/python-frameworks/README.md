# Reviewed Python framework inputs

Each directory retains the original cf `workers versions create --prebuilt`
`manifest.json` and `upload.multipart`, plus its genuine PyWrangler `pylock.toml`.
Fresh source copies of `test/applications/python-frameworks/` resolved packages
and built under `.temp/p21-python-build/frameworks-02/` on 2026-10-04. The build
interpreter download was explicitly authorized. cf uploaded only to an
authenticated loopback fixture; no Cloudflare resources were changed.

| Input   | Modules | Original multipart SHA-256                                         |
| ------- | ------: | ------------------------------------------------------------------ |
| Django  |   3,776 | `a4ff8f0b0ba24bdff19a4ccc3eb64345e7e0ea6d5d82a13dc4ffa355a166dc58` |
| Flask   |     224 | `f78e2a28d31ce821f65331496579aec054d1b6a70840a2d9f8c1026c96e9a27a` |
| FastAPI |     424 | `c762bb2917b2fe3d70c3dc6aa37bdceef8e53ce8ce444923d49d8237ac03bf61` |

All 19 SDK runtime source MIME/size/digests match the shared
[`sdk-inventory.json`](../python-main/sdk-inventory.json), and each dependency lock
fixes its published SDK wheel digest. Main application bytes match the maintained
`src/main.py`. Other dependencies remain the unmodified package tooling output;
locks record wheel provenance. Host virtualenv and project configuration files are
excluded by the application source directory boundary.

Flask uses the official Pyodide MarkupSafe 3.0.3 PyEmscripten wheel rather than the
previous plain-Python component input. FastAPI uses PyEmscripten pydantic-core.
The original captured modules were run through production prepare/restore
and dispatch with the formally verified R4 binary. Django used the earlier input
before its allowed-host change; FastAPI used the capture retained here. Those
component scenarios passed; Flask imported the new MarkupSafe wheel and passed POST, then
failed when reading its context-bearing stream. Evidence is
`.temp/p21-python-build/frameworks-02/qualification-02.json`. Django and ordinary Flask passed their full daemon cases in
`20261005T001633-3d63ba3c`; the corrected FastAPI case passed separately
in `.temp/p21-final-preflight/fastapi-minimum-case-01.log`. All three canonical
cases then passed together in complete workspace coverage. Final uninstrumented
acceptance is recorded separately. The SDK 1.9.2 Flask
`stream_with_context` failure is tracked in
[workers-py#287](https://github.com/cloudflare/workers-py/issues/287). On 2026-10-04 the
user explicitly deferred context-bearing Flask streams from this iteration's
support scope. Keep the failing route, capture and diagnostic evidence; the
Flask case still qualifies ordinary HTTP/templates and the deployment lifecycle.
Do not claim a Flask streaming pass or replace the SDK. Restore stream completion,
concurrency and cancellation checks after an upstream fix.

Regenerate under a new `.temp/` directory using the Python-only bridge and the
same capture tool with a third `django`, `flask` or `fastapi` argument. Review
original bytes, SDK hashes and wheel provenance before retaining input. Do not
fabricate bundles or overwrite retained failure evidence. The bridge uses
user-installed PyWrangler with no version check. Upload/deploy/auth/resources use
cf. See [PyWrangler removal TODO](../../../docs/p25-platform-follow-ups.md) for the removal TODO.

The Django input was regenerated after declaring `.localhost` as an allowed
application host for the ordinary daemon Gate. The prior capture is retained under
`.temp/p21-framework-gates/previous-django-input/`. The reviewed fresh capture
uses the same unmodified dependency versions; no SDK or production host policy
was changed. See `django-input-qualification-01.json` in that evidence directory.

## cf 1.0.0-beta.21 recapture (2026-11-08)

The pinned cf moved to 1.0.0-beta.21, so these captures were rebuilt from fresh maintained project copies under `.temp/p21-recapture-beta21/` with PyWrangler 1.17.6, uv 0.12.3 and the official build bridge, then captured with `prepare-python-main.ts`. Each rebuilt `pylock.toml` is byte-identical to the reviewed lock, and executable modules match the previous capture. Only `dist-info/RECORD` files differ, in the hash of a host console-script launcher.
