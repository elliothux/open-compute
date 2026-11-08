# Reviewed cf Python Workflow input

`upload.multipart` is the original request from pinned cf
`workers versions create --prebuilt`, captured against a loopback server after
a fresh build by user-installed PyWrangler. `manifest.json` records the upload
and module MIME/size/digest; `pylock.toml` records the builder's package input.

The 30-module upload has SHA-256
`b1021fba2c4d82e74c4b7eed15412e78ee1088c4cdd491a90eeee31621852a96`.
All 19 SDK sources match `../python-main/sdk-inventory.json`; the main module
matches `../../applications/python-workflows/src/main.py` exactly.
The capture tool's Workflow catalog response is developer-only and loopback-only.

The ordinary daemon case compares Python and JavaScript self-owned Workflows,
with both languages observing effects in the same KV. The current v4 adapter
rejects cross-Script Workflow bindings; this case does not claim a direct
JavaScript binding to the Python Workflow. It checks completed-step replay,
pause/restart, events, retry/non-retryable errors, termination and immutable
version retention across promotion/rollback using the real SQLite scheduler.

The full ordinary case passed on 2026-10-05 in aggregate
`20261005T015559-880c0034` (1 case, 214.52 seconds, no ignored cases).
Workflow definition updates are separate from Worker HTTP deployment changes.
After HTTP rollback, the case verifies that the definition still uses its
second Version, explicitly rebinds it through public `PUT /workflows/{name}`,
then verifies that new instances use the first Worker Version. Existing
instances retain their original immutable identity. This does not claim hosted
automatic rollback management parity.

Build/capture inputs, failed runs and original SDK diagnostics remain under
`.temp/`. The later runtime setup failure in that aggregate is retained
separately. This scoped pass does not constitute workspace coverage or final P21
acceptance. No workerd rebuild or SDK modification was performed.

## cf 1.0.0-beta.21 recapture (2026-11-08)

The pinned cf moved to 1.0.0-beta.21, so these captures were rebuilt from fresh maintained project copies under `.temp/p21-recapture-beta21/` with PyWrangler 1.17.6, uv 0.12.3 and the official build bridge, then captured with `prepare-python-main.ts`. Each rebuilt `pylock.toml` is byte-identical to the reviewed lock, and executable modules match the previous capture.
