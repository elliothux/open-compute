# open-compute workerd fork

open-compute **不使用 Cloudflare 官方发布的 stock workerd 二进制**。生产运行时来自
[`elliothux/workerd`](https://github.com/elliothux/workerd)，它是
[`cloudflare/workerd`](https://github.com/cloudflare/workerd) 的定制 fork。`ocd` 会把与当前目标匹配的固定
workerd archive 内嵌进自己的单二进制发行物；启动时离线校验并物化，不从 `PATH` 查找，也不会下载或切换到 stock workerd。

当前 formal pin 的唯一 authority 是
[`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)：

| 身份                       | 当前值                                                                                             |
| -------------------------- | -------------------------------------------------------------------------------------------------- |
| fork release               | `v1.20260930.0-open-compute-r24.9de631551`                                                         |
| fork revision              | `9de631551791218aed0c4cd570163cece3c00590`                                                         |
| upstream base              | `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`（正式 pin 的 upstream base）                            |
| `--version`                | `workerd 2026-09-30`                                                                               |
| private Container protocol | `open-compute-container-protocol-v4`                                                               |
| publish workflow           | [Open Compute binaries 41025742244](https://github.com/elliothux/workerd/actions/runs/41025742244) |

2026-10-30（UTC），R24 的四个平台构建和原生资格均已通过。
每个平台通过八个 Container／KJ HTTP owning target、52 个普通 Rust target 和十一项 Root 回归。
八个 reactor target 和 Rust lint 也已通过。
两个仅适用于 sanitizer 的 Rust target 保留不兼容状态，不计入通过数。
不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
archive 使用 GNU `gzip -9n`，mtime 为 0 且无文件名。
Release 包含逐目标 build-inputs、catalog 与 `SHA256SUMS`。
下表摘要来自已验证的资产与正式 lock。

R24 修复 KJ 对 HTTP/1.0 响应的默认连接关闭语义。
没有显式 `Connection: keep-alive` 时，客户端不复用该连接。
HTTP/1.1 和显式 keep-alive 的既有路径保留。
同一固定 KJ 源码的回归在修补前失败，修补后 140 项 HTTP 测试全部通过。
R23 对 Tokio readiness 的修补保留。
R24 的 binary source 与源码 checkout 均固定 `9de631551791218aed0c4cd570163cece3c00590`。
formal lock 记录同一次四平台构建和发布的输入。
R22 新增的官方实验目录快照入口和 private Container protocol v4 均保留。
目录参数通过既有弱引用回调传入 Broker。
R21 在 `output()` 等待输出和退出码时持有 process handle。
输出流排空后，该 handle 可以释放。
R20 让 snapshot capture 在异步 lease 查询后使用既有 Root control event。
R24 保留 weak owner、exact lease、公开 binding 回调与 Socket 所有权边界。
原生资格不代表完整产品验收。
R24 的父仓库产品、coverage、Final 和三平台 package 仍待验收。

历史 R16 的两次 Darwin x64 资格均超时；具体原因仍未确认。
R17 的 v3 协议已包含 named images、snapshot host callback、exec 子进程证明与 inactivity 修复。
R17 将 Rosetta 跨架构 medium 测试预算从 30 秒设为 60 秒。
R24 保留这个有界预算，未增加产品超时。
R18 的 Linux ARM64 资格在一条测试的 NUL 终止符断言失败。
R19 相对 R18 仅把该断言改为精确字节数组比较。
历史记录见 [workerd 方案](../workerd/README.md#r19-发布与输入校验2026-10-13)。
R22 与更早结果只适用于各自输入，不能替代 R24 产品验收。

`workerd --version` 只显示上游日期，不能证明拿到的是本 fork。需要同时核对 revision、目标和 lock 中的 binary
SHA-256。

## Fork 扩展

fork 保留 upstream Worker runtime、module validation、RPC、Durable Objects 与 Loader 基础实现，只在 standalone
宿主缺少的边界增加下列能力：

| 扩展                         | fork 提供的能力                                                                                                                                                                   | open-compute 中的用途                                                                                                      |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Dynamic Worker Loader        | delegated Loader namespace、受约束 capability delegation、原生 `load` / `get`、entrypoint/RPC、Dynamic Durable Object facets、tails、in-flight accounting 与撤销生命周期          | 向普通 Worker 提供正式的 Worker Loader binding，同时保持 account、Script、Version 与 binding namespace 隔离                |
| Workers Standard limits      | standalone CPU、memory、startup CPU、subrequest 与 simultaneous outbound-connection enforcement；Dynamic Worker/entrypoint/delegated Loader ceiling 组合；超限 isolate 摘除与恢复 | 执行 cf/v4 Settings 与 immutable Version 中的 limits，而不是只解析参数或依赖 Cloudflare 私有宿主                           |
| Native host extensions       | 私有 `HostExtensionFactory` / `HostExtensionPort`、generation broker fd 4、session-scoped Cap'n Proto unary/stream transport                                                      | 让 operator-owned native Provider 通过普通 Service Binding facade 服务 Worker；该 ABI 不是 Cloudflare 标准 API             |
| Dynamic binding forwarding   | 私有 `openComputePrivateEnv`、host-issued Loader grant、handler-only capability table 与 generation/revocation fence                                                              | 由 `open-compute:worker-loader` 显式转发 KV、D1、R2、Queue 和普通值，同时不把根 binding 变成可 structured-clone 的公共对象 |
| Dynamic Container attachment | 原生 Container facet、owner 内的 outbound callback capability、活动跟踪与递归撤销                                                                                                 | 仅向声明的 DO class 附加原生 Container；持久租约、资源和 Engine 终止由平台 authority 管理                                  |
| Compatibility catalog        | `compatibility-catalog` 从编入 binary 的 maximum date 与 `CompatibilityFlags` schema annotations 生成确定性 JSON；不加载配置或启动 listener                                       | 让 build、CLI、management API、SDK 与 Dashboard 从 exact binary 发现同一 date/flag 合同                                    |
| Reproducible binaries        | 四目标优化构建 workflow、固定编译器/Bazel/config 与 canonical release archive 生成输入                                                                                            | 为 formal pin、GitHub Release build input 和跨平台产品 Gate 提供可复现来源                                                 |

这些扩展不代表 Cloudflare 托管平台采用相同内部实现。公开 Cloudflare-compatible surface 与 open-compute
私有扩展仍分别记录；实验性 Loader 控制、完整 Workers for Platforms 和 dispatch namespace 不会因为使用 fork 而自动获得支持。
实现与边界详见 [W1 Worker Loader](../implemented/w1-dynamic-workers-worker-loader.md)、
[W2 Standard limits](../implemented/w2-standard-limits.md)、
[W3 native bindings](../implemented/w3-user-extensible-native-bindings.md) 和
[I102 binding forwarding](../implemented/i102-dynamic-worker-binding-forwarding.md)。

## 单独下载 workerd

以下 archive 来自 fork 的不可变 GitHub Release，而不是 Cloudflare release。下载后应同时核对 archive 与解压 binary
SHA-256；唯一 authority 仍是 formal lock。

二进制沿用 workerd 的 Apache 2.0 license，并包含 upstream source tree 记录的第三方组件；它们由 open-compute
项目发布和支持，不是 Cloudflare 官方发行物。

| Target           | Archive                                                                                                                                            | Archive SHA-256                                                    | Binary SHA-256                                                     |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------ |
| macOS ARM64      | [workerd-darwin-arm64.gz](https://github.com/elliothux/workerd/releases/download/v1.20260930.0-open-compute-r24.9de631551/workerd-darwin-arm64.gz) | `de67b3130fed356cd628101c9c7dcd65346e9224ecff0be61c1d05aebb92f0d9` | `8825bee9a58e6950f34d446b4cb489b25920df94a674311055aa87d58be13ee3` |
| Linux GNU ARM64  | [workerd-linux-arm64.gz](https://github.com/elliothux/workerd/releases/download/v1.20260930.0-open-compute-r24.9de631551/workerd-linux-arm64.gz)   | `38911aa2dcc48e8c94347ae50ddd1486e9b13110e9ad06fd3aa068f4fa459c62` | `03581276f1c38012ae547a9b71d20445d526561223b3df5d6ebcfbc99a6b8b64` |
| Linux GNU x86-64 | [workerd-linux-64.gz](https://github.com/elliothux/workerd/releases/download/v1.20260930.0-open-compute-r24.9de631551/workerd-linux-64.gz)         | `2eb996e69b5127bb3762110608b7d7e3e7f6b65250ef88ef50b75c3b42fad39e` | `4a0fb864a43d6cbb254deecd4cb7eed05f3d97c0d733f095480c6ab16a758bf8` |
| macOS x86-64     | [workerd-darwin-64.gz](https://github.com/elliothux/workerd/releases/download/v1.20260930.0-open-compute-r24.9de631551/workerd-darwin-64.gz)       | `d4fc01f9b934a548a36154d18b23df8b58990ef5515f7d4b348d5b5273c829d8` | `5161f8fe51aa1198c687e37e7ddaf847255ecfb1e1483c83c8c92426b3ed15f2` |

例如下载 Linux x86-64 版本：

```sh
curl -fL https://github.com/elliothux/workerd/releases/download/v1.20260930.0-open-compute-r24.9de631551/workerd-linux-64.gz -o workerd.gz
echo '2eb996e69b5127bb3762110608b7d7e3e7f6b65250ef88ef50b75c3b42fad39e  workerd.gz' | sha256sum -c -
gzip -dc workerd.gz > workerd
echo '4a0fb864a43d6cbb254deecd4cb7eed05f3d97c0d733f095480c6ab16a758bf8  workerd' | sha256sum -c -
chmod +x workerd
./workerd --version
```

macOS 可把校验命令替换为 `shasum -a 256 workerd`。单独 binary 适合源码/配置验证和 fork 调试；它不包含 `ocd`
拥有的实例注册、SQLite authority、deployment compilation、secret、Gateway、supervision 与恢复逻辑，不能替代完整
open-compute 安装。

## 升级与源码

fork 源码由 [`third_party/workerd/`](../../third_party/workerd/) submodule 固定。每次升级应一起更新 fork revision、
upstream base、四目标 release asset、archive/binary digest、binary maximum、compatibility catalog schema/digest、system Worker
date/flags 和真实 runtime Gate；不能只替换其中一个文件。四目标 workflow 应执行 introspection 两次并比较全部
catalog 字节；根 build 再对 host binary 重算 catalog，任何 revision/binary/maximum/catalog drift 都在 Cargo 前失败。上游能力与
fork-ahead 审查见 [workerd upstream](workerd-upstream.md)，构建和内嵌合同见
[单二进制分发](single-binary.md)。
