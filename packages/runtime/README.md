# Runtime 源码组织

这里是单机 Cloudflare Workers Platform 的受信任 workerd runtime 层。`src/` 按产品领域组织，
每个领域保留自己的实现与协议类型；tenant Worker 只看部署声明生成的 facade，不接触内部
authority、SQLite/Local object path/S3 credential 或 loader 控制能力。

| 目录 | 职责 |
| --- | --- |
| `gateway/` | 内部入口和租户出站网关 |
| `loader/` | 部署快照、模块装配、绑定注入及 `wrappers/` |
| `assets/` | Static Assets 路由、binding 与私有对象读取协议 |
| `services/` | Service Binding facade、调用 scope、原生 RPC/fetch 与释放协议 |
| `kv/` | KV 传输 |
| `d1/` | D1 facade、二进制传输及协议 |
| `r2/` | R2 facade、流式传输及协议 |
| `ai/` | Workers AI Markdown Conversion facade 与部署授权私有传输 |
| `queues/` | Queue producer facade 与绑定权限 |
| `durable-objects/` | DO host、路由、ID 编码、alarm shim 与协议 |
| `workflows/` | Workflow host、执行控制器、runner、facade 与序列化 |
| `bindings/` | 多个领域共同使用的绑定权限和后端能力类型 |

新增 Cloudflare API 必须先进入平台 contract/capability，再沿通用 snapshot、binding facade 与可信
transport 接入；不能根据 vinext、模块路径或 fixture 名称在 runtime 中增加框架专用分支。Workers AI
只声明并实现已验收的 Markdown Conversion 子集，推理、Models 与 Gateway 等成员稳定 fail closed。

行为测试位于 `tests/<领域>/`。构建测试及共享测试加载器留在 `tests/`；
Rust 与 JS 共用的序列化 fixture 继续位于 `tests/fixtures/`。

从仓库根目录执行：

```sh
bun run build
bun run check:generated
bun run test:js
```

`build` 先严格检查源码和构建脚本，根 build 也检查 toolchain、examples 和 scripts；
无需在完整构建前重复 typecheck。只需类型反馈时可独立执行 `bun run typecheck`。
`build.ts` 递归编译 TS，生成的 JS 保留领域相对路径；例如
`src/d1/facade.ts` 对应 `d1/facade.js`，不同领域的同名文件不会互相覆盖。
可用 `--output-dir` 指定独立的构建目录，再用 `--check` 校验同一目录。

`dist/` 完全由 TS 生成，`config.capnp` 使用相同的领域模块路径。
`manifest.json` 记录所有生成模块的 SHA-256；每个部署描述符都包含该清单的摘要，
包括没有产品绑定的 Worker。内部模块统一使用保留的 `__open_compute__/` 命名空间。

此实现按 day1 切换，不保留历史 JS、旧摘要格式或旧模块路径的兼容分支。
Rust 构建将已生成的 JS、Cap'n Proto、正式 lock 和目标平台的官方 gzip 内嵌进可执行文件。
必须显式设置 `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE`；不搜索缓存或下载。
生产启动只离线物化这些内嵌字节，不调用 Bun、Node、TypeScript 或 Rolldown。
CI 会检查严格类型、生成产物一致性和行为测试；真实 workerd Gate 仍是运行时验收依据。

`dist/` 不进入 Git。`manifest.json` 同时记录源码、配置、构建脚本和锁文件摘要；Rust 构建检查
这些输入与当前检出一致，拒绝过期资产。`--check` 只比较当前源码生成的完整文件集合与字节，
不重复执行类型检查。相同字节的显式重建保留 mtime，避免无变化时触发 Cargo 重编译。
构建行为测试用两个独立空目录验证可复现性，不依赖已提交的 JS 基线。
