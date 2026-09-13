---
title: "Rust"
description: "将 workers-rs 项目构建为 WebAssembly，并部署到 open-compute。"
---

Cloudflare 通过 `workers-rs` crate 支持 Rust Worker。`worker-build` 将 Rust 编译为 WebAssembly，并生成 ES module shim；项目内 Wrangler 会把两者作为标准 module Worker 上传。open-compute 支持这种输出结构。

## 创建项目

安装 `wasm32-unknown-unknown` target，并生成官方模板：

```sh
rustup target add wasm32-unknown-unknown
cargo install cargo-generate
cargo generate cloudflare/workers-rs
```

在生成的项目中保留 Wrangler 依赖。open-compute launcher 会查找最近的项目内 Wrangler，不会使用全局 executable。

## 编写 Worker

在 `src/lib.rs` 中通过 `event` macro 暴露 fetch handler：

```rust
use worker::*;

#[event(fetch)]
async fn main(_request: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::ok("Hello from Rust!")
}
```

Wrangler 配置指向生成的 shim，并在上传前运行 `worker-build`：

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

所有依赖都必须能为 `wasm32-unknown-unknown` 编译。请显式安装 `worker-build`，或保留官方模板生成的 build command。

## 开发与部署

本地开发使用 Wrangler；真实部署由 open-compute 选择 target 并注入认证：

```sh
npx wrangler dev
ocd wrangler deploy
```

`ocd wrangler deploy` 会运行同一个 custom build command，再上传生成的 JavaScript shim 和 `.wasm` module。不可变 deployment 创建后，生产主机不需要 Rust toolchain。

Cloudflare 参考：[Rust language support](https://developers.cloudflare.com/workers/languages/rust/)。
