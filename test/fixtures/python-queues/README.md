# Reviewed cf Python Queue input

`upload.multipart` is the original request captured from the pinned cf CLI's
`workers versions create --prebuilt` command against a loopback capture server.
`manifest.json` records its content type, full upload digest, metadata and
per-module MIME/size/digest. `pylock.toml` records the official builder's package
input, including the unchanged `workers-runtime-sdk` 1.9.2 wheel digest.

The current 30-module upload has SHA-256
`72dbc8fcd35a0e09d3a233aaa0ff7fc19a67a72d3e593e4dba22bec43a10b73c`.
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
