# Python Main Gate input

`sdk-inventory.json` fixes the 19 runtime Python/JavaScript/PTH source files of
unmodified `workers-runtime-sdk` 1.9.2 and records its published wheel digest.
It contains hashes, not replacement SDK code. Installation metadata and typing
files are not executable source and are not fixed by this inventory.

`manifest.json` and `upload.multipart` are the original reviewed capture from
`.temp/p21-python-prepared-integrity/capture-01/` on 2026-10-04. A fresh maintained project copy
ran the Python-only bridge after the explicitly authorized Emscripten interpreter
preparation. PyWrangler resolved and installed SDK 1.9.2, then upstream Wrangler
produced Build Output. Pinned cf uploaded the prebuilt result to an authenticated
loopback fixture: 32 modules, including all 19 SDK runtime sources whose MIME,
size and digests match `sdk-inventory.json`. `pylock.toml` confirms the published
wheel digest. Capture SHA-256 is
`36104a730abecfbe4b6f087684a4778297cd8039434f712e85f23c59e90f8663`.
The previous input is retained under
`.temp/p21-python-prepared-integrity/previous-main-input-01/`. The new main adds
an HMAC proof of the active secret; all 19 official SDK sources and local package
data remain unchanged. Its identical official pylock is now retained alongside
the input, and the shared Capture validator checks SDK version and wheel digest.

This qualifies fresh package preparation and cf prebuilt format handoff; it does
not qualify the ordinary daemon/runtime Gate. Earlier failed and component-only
runs remain historical evidence. For regeneration, build under a new `.temp/`
directory using the [build workflow](../../applications/python-main/README.md),
then capture with `test/conformance/applications/prepare-python-main.ts`, review
original bytes and provenance, and retain original capture files. Never fabricate
SDK/Build Output. PyWrangler is user-installed with no version checks; all upload,
deploy, authentication and resource operations use cf.

TODO(P25): remove the Python-only PyWrangler bridge when cf's official Python
builder passes equivalent build/upload qualification, as tracked in `docs/p25-platform-follow-ups.md`.

The Gate checks the capture, SDK sources and maintained Python main before
starting the ordinary daemon. Only test-owned binding IDs, variable values and
secrets change between successful uploads; SDK/package module bytes remain
immutable. Negative uploads modify or omit only the specified application module
or package data, leaving all 19 SDK sources unchanged. They cover missing/mistyped
main, unknown flag, duplicate/reserved modules, main/package syntax corruption,
a top-level import exception containing a declared fixture secret, missing package
module, missing package data and corrupt package JSON. Each preparation rejection
must leave the active deployment and retained prepared records intact, publish no
prepared record for the rejected version, clear compile/prepare leases, and leave
shared KV/D1/R2 data accessible to the original Python and JavaScript peers.
Response and daemon-log checks reject the import-exception secret. These package
failure assertions passed in the complete ordinary Main case. Real SQLite, SigV4,
daemon/runtime restarts, promotion/rollback and encrypted-object corruption
exercise the production path. Missing input is a failure and never a skip.
The corruption steps distinguish truncation from same-size byte modification,
require a GET of the exact damaged object, and leave both prepared records intact.
Secret HMAC proofs check promotion, restart, rollback and restored-object recovery
without returning the secret. The final workspace run must retain these ordinary-daemon assertions.

The same single case also uploads an ordinary JavaScript peer bound to those
exact KV, D1 and R2 resources. Both languages read each other's writes and
deletes. Peer reads check persisted state across Python promotion, rollback,
daemon/runtime restarts and failed snapshot restore; JavaScript owns no Python
prepared record. This test-owned JavaScript multipart does not supply or replace
the required cf Python capture. These assertions passed against the formal runtime in
`20261005T001106-bb74c1a5` (299.38 seconds, 1 pass/0 ignored).

The ordinary daemon case passed in complete workspace coverage. Final
uninstrumented acceptance is recorded separately; compilation, capture and the
formal workerd release alone do not prove product acceptance.

## cf 1.0.0-beta.21 recapture (2026-11-08)

The pinned cf moved to 1.0.0-beta.21, so these captures were rebuilt from fresh maintained project copies under `.temp/p21-recapture-beta21/` with PyWrangler 1.17.6, uv 0.12.3 and the official build bridge, then captured with `prepare-python-main.ts`. Each rebuilt `pylock.toml` is byte-identical to the reviewed lock, and executable modules match the previous capture.
