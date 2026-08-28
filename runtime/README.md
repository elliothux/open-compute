# Runtime 源码组织

`src/` 按领域组织，每个领域保留自己的实现与协议类型：

| 目录 | 职责 |
| --- | --- |
| `gateway/` | 内部入口和租户出站网关 |
| `loader/` | 部署快照、模块装配、绑定注入及 `wrappers/` |
| `kv/` | KV 传输 |
| `d1/` | D1 facade、二进制传输及协议 |
| `r2/` | R2 facade、流式传输及协议 |
| `queues/` | Queue producer facade 与绑定权限 |
| `durable-objects/` | DO host、路由、ID 编码、alarm shim 与协议 |
| `workflows/` | Workflow host、执行控制器、runner、facade 与序列化 |
| `bindings/` | 多个领域共同使用的绑定权限和后端能力类型 |

行为测试位于 `tests/<领域>/`。构建测试及共享测试加载器留在 `tests/`；
Rust 与 JS 共用的序列化 fixture 继续位于 `tests/fixtures/`。

从仓库根目录执行：

```sh
bun run typecheck
bun run build
bun run check:generated
bun run test:js
```

`build.ts` 递归编译 TS，生成的 JS 保留领域相对路径；例如
`src/d1/facade.ts` 对应 `d1/facade.js`，不同领域的同名文件不会互相覆盖。
可用 `--output-dir` 指定独立的构建目录，再用 `--check` 校验同一目录。

`system-workers/` 完全由 TS 生成，`config.capnp` 使用相同的领域模块路径。
`manifest.json` 记录所有生成模块的 SHA-256；每个部署描述符都包含该清单的摘要，
包括没有产品绑定的 Worker。内部模块统一使用保留的 `__open_compute__/` 命名空间。

此实现按 day1 切换，不保留历史 JS、旧摘要格式或旧模块路径的兼容分支。
Rust 构建将已生成的 JS、Cap'n Proto、正式 lock 和目标平台的官方 gzip 内嵌进可执行文件。
必须显式设置 `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE`；不搜索缓存或下载。
生产启动只离线物化这些内嵌字节，不调用 Bun、Node、TypeScript 或 Rolldown。
CI 会检查严格类型、生成产物一致性和行为测试；真实 workerd Gate 仍是运行时验收依据。
