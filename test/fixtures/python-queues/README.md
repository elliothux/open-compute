# Reviewed cf Python Queue input

`upload.multipart` is the original request captured from the pinned cf CLI's
`workers versions create --prebuilt` command against a loopback capture server.
`manifest.json` records its content type, full upload digest, metadata and
per-module MIME/size/digest. `pylock.toml` records the official builder's package
input, including the unchanged `workers-runtime-sdk` 1.9.2 wheel digest.

The current 30-module upload has SHA-256
`4aa54bba3236f99584b8bf258801ffbc8a4418e286aba952bef51b845c4cc6fb`.
All 19 SDK source entries match `../python-main/sdk-inventory.json`; the main
module matches `../../applications/python-queues/src/main.py` exactly. Shared
Rust capture validation repeats these checks before admitting any input.

The V8 fixture contains 50,000 Chinese characters in both producer languages.
The same registered case requires native version-15 bytes in SQLite and exact
Unicode restoration by Python and JavaScript consumers after restart and rollback.

Fresh build and cf capture evidence lives in `.temp/p21-queue-v8-wire/build-01/`,
`capture-01/` and `input-qualification-01.json`. The prior reviewed fixture and
package lock remain in `previous-queue-input-01/`; all earlier diagnostics remain
in `.temp/python-queues/`. The official SDK and pylock are unchanged. No native
workerd build was performed.

The full ordinary Queue case passed on 2026-10-05 in aggregate
`20261005T005821-7ea6b5f5` (1 case, 266.78 seconds, no ignored cases).
It rejects deletion of referenced queues before and after restart and proves
continued exact cross-language delivery. `force` permits backlog purge only;
it does not bypass reference protection. The later DO failure in that aggregate
is retained separately. This scoped pass does not constitute workspace coverage
or final P21 acceptance.

## cf 1.0.0-beta.21 recapture (2026-11-08)

The pinned cf moved to 1.0.0-beta.21, so these captures were rebuilt from fresh maintained project copies under `.temp/p21-recapture-beta21/` with PyWrangler 1.17.6, uv 0.12.3 and the official build bridge, then captured with `prepare-python-main.ts`. Each rebuilt `pylock.toml` is byte-identical to the reviewed lock, and executable modules match the previous capture.
