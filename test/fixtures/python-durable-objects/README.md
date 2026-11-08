# Reviewed cf Python Durable Object inputs

`active/` and `retired/` contain original multipart uploads captured from the
pinned cf CLI against a loopback server after fresh builds by user-installed
PyWrangler. Each manifest records the full request digest, metadata and module
MIME/size/digest; `pylock.toml` records the unchanged official SDK wheel input.

The active upload SHA-256 is
`67ddde8bb098a391d68dd25a65d002f1648963e7eb7149dd03f5ea12e50a073a`;
the retired upload SHA-256 is
`1aa6ad969774a9b9a99709b08aef7268289a3f88279d1f31226701f31bc267d9`.
Both have 30 modules. All 19 official SDK sources match
`../python-main/sdk-inventory.json`, and each main module matches its maintained
source under `../../applications/python-durable-objects/` exactly.

The normal cf configuration exports SQLite-backed `Counter`. Its `retire` mode
selects a separate source without that class or binding and declares the class
deleted. The native case verifies persisted class retirement after this declaration and
continued operation of an independent JavaScript namespace. Public uploads only
support self-owned namespaces; cross-Script DO bindings are rejected. It does
not claim cross-Script access or hosted deletion prerequisites are equivalent.

Build/capture evidence lives under `.temp/python-durable-objects/`.
The WebSocket extension's fresh build and cf upload capture live under
`.temp/p21-python-do-websocket/`; the previous active upload is retained there.
The official SDK and pylock remain unchanged. The same serial case now exercises
text/binary messages, attachments, clean close callbacks, missing Upgrade,
connections after restart/rollback, and owned-daemon SIGKILL recovery with
unchanged committed object state and prepared records. The full ordinary case passed on 2026-10-05 in aggregate
`20261005T013727-a1213d6b` (1 case, 243.66 seconds, no ignored cases).
Native object IDs preserve the official SDK's `toString()` and named identity;
alarms recover from persisted metadata. The case also covers immutable Versions,
secret changes, abort replacement, restart/rollback, SIGKILL recovery and class
retirement. It does not prove hibernation eviction, survival of live connections
across daemon death or the complete DO/PITR matrix. The later Workflow failure
in that aggregate is retained separately. This scoped pass does not constitute
workspace coverage or final P21 acceptance. No workerd rebuild was performed.

## cf 1.0.0-beta.21 recapture (2026-11-08)

The pinned cf moved to 1.0.0-beta.21, so these captures were rebuilt from fresh maintained project copies under `.temp/p21-recapture-beta21/` with PyWrangler 1.17.6, uv 0.12.3 and the official build bridge, then captured with `prepare-python-main.ts`. Each rebuilt `pylock.toml` is byte-identical to the reviewed lock, and executable modules match the previous capture. The active capture's same-Worker `OBJECTS` binding no longer carries `script_name`; that is the repository `@cloudflare/config` 0.28.0 patch.
