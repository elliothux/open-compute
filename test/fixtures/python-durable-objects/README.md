# Reviewed cf Python Durable Object inputs

`active/` and `retired/` contain original multipart uploads captured from the
pinned cf CLI against a loopback server after fresh builds by user-installed
PyWrangler. Each manifest records the full request digest, metadata and module
MIME/size/digest; `pylock.toml` records the unchanged official SDK wheel input.

The active upload SHA-256 is
`ec7199dede0714a804c207673303bbddf9caa9b93d8eb62fa7d56b915909017a`;
the retired upload SHA-256 is
`aa6cb977f2f1780469241f7ea43a9598c20abe8e21fd045e5eed01f6f2af5048`.
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
