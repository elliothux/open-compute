# P1.8 WebSocket Hibernation Gate

- Verdict: **No-Go**
- Release under test: `platformd 0.1.0`, checkout baseline `19dd7227fed39a1c3871f7498977bf4416849844`
- Stock workerd pin: `v1.20260826.1`, expected output `workerd 2026-08-26`
- Production path: checked-in `runtime/config.capnp`, `do-facade.js`, `do-host.js` and the public platformd WebSocket bridge
- Fixture: `p0_7_durable_objects_gate` basic WebSocket restart/close matrix plus `p1_conformance` capability shape

WH-01 fails at the product boundary: the production DO facade intentionally does not expose `ctx.acceptWebSocket()`. Consequently WH-02 through WH-12 are not claimed and no partial method, compatibility flag, frame/session persistence, or platformd replay shim is present. `basic_websocket=supported` remains covered by the P0.7 stock-workerd Gate; `hibernatable_websocket=unsupported` is emitted by `platformd capabilities --json`.

Re-run with `./scripts/test-p1-8.sh`. A future Go decision must replace this verdict only after all WH-01 through WH-12 pass on the exact new workerd pin.
