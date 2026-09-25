# open-compute workerd fork

open-compute **不使用 Cloudflare 官方发布的 stock workerd 二进制**。生产运行时来自
[`elliothux/workerd`](https://github.com/elliothux/workerd)，它是
[`cloudflare/workerd`](https://github.com/cloudflare/workerd) 的定制 fork。`ocd` 会把与当前目标匹配的固定
workerd archive 内嵌进自己的单二进制发行物；启动时离线校验并物化，不从 `PATH` 查找，也不会下载或切换到 stock workerd。

当前 formal pin 的唯一 authority 是
[`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)：

| 身份           | 当前值                                                                                     |
| -------------- | ------------------------------------------------------------------------------------------ |
| fork release   | `v1.20260918.1-open-compute-i102.1c7b89be`                                                 |
| fork revision  | `1c7b89bea323a39a8511271913820f9fcf39306d`                                                 |
| upstream base  | `679c09e5eea0af8a04062e1875e99c75af532e3b`（Cloudflare `v1.20260918.1`）                   |
| `--version`    | `workerd 2026-09-18`                                                                       |
| build workflow | [Open Compute binaries #10](https://github.com/elliothux/workerd/actions/runs/35592596362) |

`workerd --version` 只显示上游日期，不能证明拿到的是本 fork。需要同时核对 revision、目标和 lock 中的 binary
SHA-256。

## Fork 扩展

fork 保留 upstream Worker runtime、module validation、RPC、Durable Objects 与 Loader 基础实现，只在 standalone
宿主缺少的边界增加下列能力：

| 扩展                       | fork 提供的能力                                                                                                                                                                   | open-compute 中的用途                                                                                                      |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Dynamic Worker Loader      | delegated Loader namespace、受约束 capability delegation、原生 `load` / `get`、entrypoint/RPC、Dynamic Durable Object facets、tails、in-flight accounting 与撤销生命周期          | 向普通 Worker 提供正式的 Worker Loader binding，同时保持 account、Script、Version 与 binding namespace 隔离                |
| Workers Standard limits    | standalone CPU、memory、startup CPU、subrequest 与 simultaneous outbound-connection enforcement；Dynamic Worker/entrypoint/delegated Loader ceiling 组合；超限 isolate 摘除与恢复 | 执行 Wrangler/v4 Settings 与 immutable Version 中的 limits，而不是只解析参数或依赖 Cloudflare 私有宿主                     |
| Native host extensions     | 私有 `HostExtensionFactory` / `HostExtensionPort`、generation broker fd 4、session-scoped Cap'n Proto unary/stream transport                                                      | 让 operator-owned native Provider 通过普通 Service Binding facade 服务 Worker；该 ABI 不是 Cloudflare 标准 API             |
| Dynamic binding forwarding | 私有 `openComputePrivateEnv`、host-issued Loader grant、handler-only capability table 与 generation/revocation fence                                                              | 由 `open-compute:worker-loader` 显式转发 KV、D1、R2、Queue 和普通值，同时不把根 binding 变成可 structured-clone 的公共对象 |
| Reproducible binaries      | 四目标优化构建 workflow、固定编译器/Bazel/config 与 canonical archive 生成输入                                                                                                    | 为 formal pin、Git LFS build input 和跨平台产品 Gate 提供可复现来源                                                        |

这些扩展不代表 Cloudflare 托管平台采用相同内部实现。公开 Cloudflare-compatible surface 与 open-compute
私有扩展仍分别记录；实验性 Loader 控制、完整 Workers for Platforms 和 dispatch namespace 不会因为使用 fork 而自动获得支持。
实现与边界详见 [W1 Worker Loader](../implemented/w1-dynamic-workers-worker-loader.md)、
[W2 Standard limits](../implemented/w2-standard-limits.md)、
[W3 native bindings](../implemented/w3-user-extensible-native-bindings.md) 和
[I102 binding forwarding](../implemented/i102-dynamic-worker-binding-forwarding.md)。

## 单独下载 workerd

以下是 open-compute `v0.2.2` 固定并验证的**未压缩可执行文件**。链接指向 release tag 下的 Git LFS 对象，而不是
Cloudflare release；SHA-256 必须与 formal lock 一致。

二进制沿用 workerd 的 Apache 2.0 license，并包含 upstream source tree 记录的第三方组件；它们由 open-compute
项目发布和支持，不是 Cloudflare 官方发行物。

| Target           | 下载                                                                                               | Binary SHA-256                                                     | 发行范围                             |
| ---------------- | -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------------ |
| macOS ARM64      | [workerd](https://github.com/elliothux/open-compute/raw/v0.2.2/share/workerd/darwin-arm64/workerd) | `f9adf7bd167f5ddedb1d952e0729219763ac11d59034e5c37bca1f707f7cf33d` | 正式输入                             |
| Linux GNU ARM64  | [workerd](https://github.com/elliothux/open-compute/raw/v0.2.2/share/workerd/linux-arm64/workerd)  | `289e5ce01435333e7aed351f2094b8adb6ff3fdf2d84fe2443166f0654b18a2b` | 正式输入                             |
| Linux GNU x86-64 | [workerd](https://github.com/elliothux/open-compute/raw/v0.2.2/share/workerd/linux-x64/workerd)    | `7b8a7e2a6cdd77ec3a2996c0f1ac494af42927b595df2e4344604185e5553f3d` | 正式输入                             |
| macOS x86-64     | [workerd](https://github.com/elliothux/open-compute/raw/v0.2.2/share/workerd/darwin-x64/workerd)   | `80d741fb70a0df2c3effc4045cc9934f3fe72fdad559dc8321dd15965c171619` | 仅手动构建，不属于正式 `ocd` release |

例如下载 Linux x86-64 版本：

```sh
curl -fL https://github.com/elliothux/open-compute/raw/v0.2.2/share/workerd/linux-x64/workerd -o workerd
echo '7b8a7e2a6cdd77ec3a2996c0f1ac494af42927b595df2e4344604185e5553f3d  workerd' | sha256sum -c -
chmod +x workerd
./workerd --version
```

macOS 可把校验命令替换为 `shasum -a 256 workerd`。单独 binary 适合源码/配置验证和 fork 调试；它不包含 `ocd`
拥有的实例注册、SQLite authority、deployment compilation、secret、Gateway、supervision 与恢复逻辑，不能替代完整
open-compute 安装。

## 升级与源码

fork 源码由 [`third_party/workerd/`](../../third_party/workerd/) submodule 固定。每次升级必须一起更新 fork revision、
upstream base、四目标二进制、archive/binary digest、compatibility date/flags、Git LFS 对象和真实 runtime Gate；不能只替换
其中一个文件。上游能力与 fork-ahead 审查见 [workerd upstream](workerd-upstream.md)，构建和内嵌合同见
[单二进制分发](single-binary.md)。
