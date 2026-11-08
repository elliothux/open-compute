# Reviewed Python Service Binding input

`manifest.json` and `upload.multipart` are original cf `workers versions create
--prebuilt` output captured from a fresh official PyWrangler build. `pylock.toml`
records the actual dependency resolution. The build uses the already authorized
Emscripten Python interpreter; cf uploaded only to an authenticated loopback
fixture. The SDK is unmodified, with no synthesized builder or replacement bridge.

The 2026-10-04 capture retains 30 modules. Its original multipart SHA-256 is
`09bec3730d50a9bfb8cff720f7086fc5b935f75f342be688735064905c9d8a0f`.
All 19 SDK source MIME/size/digests match the shared
[`sdk-inventory.json`](../python-main/sdk-inventory.json); main source bytes match
[`src/main.py`](../../applications/python-services/src/main.py). The lock fixes
SDK 1.9.2 and its wheel SHA-256
`f928e20afddaaf01f216b3ced8884ca1e7675681bb44692add753317fc9ce89d`.
Metadata preserves default/named Worker exports and the two declared Service
bindings. The product case changes only isolated test binding values.

Build/capture evidence is retained under `.temp/python-services/`. The
complete ordinary daemon case passed in `20261005T004230-800b724a`
(257.47 seconds, 1 pass/0 ignored). The registered case also passed in complete
workspace coverage. Final uninstrumented acceptance is recorded separately. Regeneration uses a fresh `.temp/`
copy of the maintained application and `prepare-python-main.ts` with its third
argument `services`. Review original bytes, all SDK hashes and lock provenance
before retaining new input. Never overwrite failure evidence or fork the SDK.

## cf 1.0.0-beta.21 recapture (2026-11-08)

The pinned cf moved to 1.0.0-beta.21, so these captures were rebuilt from fresh maintained project copies under `.temp/p21-recapture-beta21/` with PyWrangler 1.17.6, uv 0.12.3 and the official build bridge, then captured with `prepare-python-main.ts`. Each rebuilt `pylock.toml` is byte-identical to the reviewed lock, and executable modules match the previous capture.
