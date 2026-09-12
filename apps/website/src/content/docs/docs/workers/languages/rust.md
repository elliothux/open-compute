---
title: "Rust"
description: "Build a workers-rs project to WebAssembly and deploy it to open-compute."
---

Cloudflare supports Rust Workers through the `workers-rs` crate. `worker-build` compiles Rust to WebAssembly and emits an ES module shim; project-local Wrangler uploads both as a standard module Worker. This output shape is supported by open-compute.

## Create the project

Install the `wasm32-unknown-unknown` target and generate the official template:

```sh
rustup target add wasm32-unknown-unknown
cargo install cargo-generate
cargo generate cloudflare/workers-rs
```

Keep Wrangler installed in the generated project. The open-compute launcher deliberately resolves the nearest project-local Wrangler rather than a global executable.

## Write the Worker

Use the `event` macro in `src/lib.rs` to expose the fetch handler:

```rust
use worker::*;

#[event(fetch)]
async fn main(_request: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::ok("Hello from Rust!")
}
```

The Wrangler configuration points at the generated shim and runs `worker-build` before upload:

```json
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "hello-rust",
  "main": "build/worker/shim.mjs",
  "compatibility_date": "2026-09-08",
  "build": {
    "command": "worker-build --release"
  }
}
```

All dependencies must compile for `wasm32-unknown-unknown`. Install `worker-build` explicitly or keep the build command produced by the official template.

## Develop and deploy

Use Wrangler for the local loop, then use open-compute to select and authenticate the real target:

```sh
npx wrangler dev
ocd wrangler deploy
```

`ocd wrangler deploy` runs the same custom build command, then uploads the generated JavaScript shim and `.wasm` module. No Rust toolchain is needed on the production host after the immutable deployment has been created.

Cloudflare reference: [Rust language support](https://developers.cloudflare.com/workers/languages/rust/).
