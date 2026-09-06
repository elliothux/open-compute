# P1：Dynamic Workers / Worker Loader 实施记录

2026-09-06：**P1 声明子集实现与验证完成。** 完整 Dynamic Workers API 仍有 6 个明确 blocked 成员；
资源预算执行属于 P2。本次未发布或推送远端资源。

## 实现范围

P1 为普通 Worker 提供真实原生 `WorkerLoader` binding，支持 `load()`、`get()`、同步 `WorkerStub`、
entrypoint/RPC 和动态 Durable Object facets。七类模块沿用 workerd 的编译器与验证器。
租户 Worker 本身仍由系统 Loader 装载；本补丁使原生 Loader capability 可以受控地跨一次 env 边界。
没有新增一套 JavaScript Loader facade、源码重写加载器或 stock/fork 运行时选择分支。

`worker_loaders = [{ binding = "LOADER" }]` 经固定 Wrangler 编译为 `{name,type:"worker_loader"}`，
进入 closed v4 binding、immutable Version descriptor、SQLite authority 和 RuntimeSource。
公开 Env 直接引用上游 WorkerLoader 声明；factory 与 facet grant 只存在于可信包装层。

## 原生边界与维护

源码固定在 `third_party/workerd/` 的独立 Git 子仓库。fork revision 为
`b3e1a27840299f493d9425dc4d9972381d02ef23`，upstream base 为
`dd8133e9b9656fb39f1434247a80aa7a249ee204`。
原生提交依次实现 delegation/计数、private host facets 和跨日期持久化拒绝回归；均为本地提交。

| 关注点 | 归属与约束 |
| --- | --- |
| capability | `api/worker-loader.*`、`io-channels.*` 与 Frankenvalue；系统 Loader、factory 和 delegated capability 权限分开 |
| namespace/cache | standalone server；最多 1024 namespace，每个 named cache 64 项；活跃引用不可淘汰，失败 startup 清理只移除同一实例 |
| invocation | 独立 `dynamic-worker-limiter.*`，每个 caller IoContext 的 Worker 4 / DO 10 distinct child；同一身份并发只计一次 |
| 生命周期 | startup、actor 与响应流持有引用；流结束/取消释放计数；撤销拒绝新调用，已接纳调用可排空；shutdown 取消也执行 unlink |
| tails | 每次 invocation 单独继承受保护 collector；动态后代继承，静态 service 边界清除；与 user tails 并行投递 |
| facets | factory 从 host context 生成私有创建能力，原生类引用本地使用；禁止再次委派、RPC 和持久化，host 结束或撤销后失效 |

宿主配置新增字段使用新的 Cap'n Proto ordinal。通用能力、引用计数和 native lifecycle 留在 workerd；
账号、Script、SQLite、路由、日志策略与资源协议留在 open-compute。
升级上游时重点检查 WorkerLoader/Frankenvalue 转移、IoContext 生命周期、actor facets、stream completion、
collector 传播和 server cleanup，再跑原生日期/GC 变体与真实产品 Gate。不得只消除文本冲突而省略行为验证。
遵循子仓库 AGENTS、CONTRIBUTING、Bazel target 与已有 C++/JS 测试布局。

## 平台 authority 与隔离

公开 namespace 由 account ID、不可复用的 Script UUID 和 binding name 做域分离 SHA-256 派生。
Version 升级与回滚共享该 Script/binding namespace；另一个账号、Script 或 binding 无法猜中系统缓存。
`get(id)` 的 ID 只在被授予的 namespace 内生效；同 ID 的代码必须不可变，缓存命中与 callback 次数不是公共保证。

普通 Worker cache 按 immutable Version 和入口模式寻址，route/observability generation 不再进入代码 key。
DO dispatch 在 SQLite 中核验当前 active Version 并签发当前 route fence；每次调用获得独立 collector identity。
Script 删除先完成现有 drain 和持久化删除，再分批撤销历史 Version 的 namespace；回收结果不确定时
返回失败并请求现有监督器回收 generation。重建同名 Script 产生新 UUID。

动态 DO 类通过 private HostFacets grant 接入已有逻辑 registry，RPC 只传已认证的逻辑身份，
不把不可转移的原生 ActorClass 发过 RPC。原生逻辑深度检查使 flattening 后仍受四层限制。
不同 Script 的同名 DO class 使用独立资源 UUID；rename/rollback 保留资源身份与存储。

命名 `ctx.exports` service 复用现有 env/context wrapper，经隐藏的原生 loopback bridge 保留 scoped props、
正确产品 binding 和 user tail；其他原生选项仍由 workerd 解析。primitive options 与 null/primitive props
按实际 native 行为拒绝，未包装的内部描述与私有能力不进入租户 env。

租户 outbound 继续使用现有 public-address-only Network；没有新增宽权限 host network 或出口代理。
公开错误继续脱敏，loader key、source、平台令牌及内部 topology 不进入响应。

## 固定二进制与构建

四平台优化产物已放入 `share/workerd/{darwin-arm64,darwin-x64,linux-arm64,linux-x64}/workerd`，
由 `.gitattributes` 的 Git LFS filter 管理。唯一正式 pin 是 `packages/runtime/workerd.lock.json`，
包含 source revision、upstream base、工具链、archive/binary 摘要、version、日期与 flags。
上游 npm types 的 gitHead 单独记录，不冒充 fork revision。

根 `bun run build` 校验这些二进制，以固定 Bun 1.3.14 压缩成正式 gzip，放到摘要隔离的
`.temp/workerd-build/`。Cargo、Gate、开发脚本和 CI 默认使用这套输入；显式 archive 只允许同一正式 pin。
LFS pointer、丢失、超限、符号链接或损坏缓存均失败，不下载或退回 stock。
CI 以 `lfs: true` 检出；生产仍只分发内嵌目标 archive 的单个 `ocd`，离线启动。
LFS 对象和 fork 提交尚未推送，也未进行发布包装。

## 声明的限制

- custom limits，包括 `{}`，在 P1 原生边界拒绝。code-level stub 同步返回、首次调用异步失败；
  entrypoint/class options 同步失败。CPU、内存与 subrequest enforcement 属于 P2。
- 25 个 stable Loader members 中 19 个有类型与产品证据，4 个 custom-limit 和 2 个 experimental-control
  members 保持 blocked。整个 Dynamic Workers contract 不标 fully supported；非空 streaming tails 拒绝。
- Script DELETE 保留现有 generation background hold：调用过的 Version 尚未排空时返回 409，
  generation 结束后才能删除。本次没有取消 hold 或为每次部署重启整个 runtime。
- Python child 首次加载可能由 workerd 下载并校验固定 Pyodide bundle；这不是 tenant outbound，
  不宣称首次 Python 执行离线。ocd 启动离线契约不变。
- 本地 protected collector 自动记录 child 日志属于平台能力；Cloudflare 不默认将这些日志写入 parent Workers Logs。
- Anycast、全球 placement、跨地域复制、fleet autoscaling 与 Workers for Platforms 不在本目标范围。

详细合同、当前证据与负向路径见兼容审查记录；P2 仍由 `docs/workerd/` 下的活动设计拥有。

兼容性逐项审查见[审查记录](p1-worker-loader-compatibility-review.md)。

## 最终验证

最终输入：fork `b3e1a27840299f493d9425dc4d9972381d02ef23`，
formal lock SHA-256 `5f92b595764892c166b36e61a81ef0b5313178554edefbb0b49a01dad7efb303`，
测试源码摘要 `e8d88c8be7c8f5d8c466566da00b9acdecba72b72649411c839e4113bd4d47e6`。
本机 open-compute acceptance 为 macOS ARM64；其他平台的 native workerd 结果单列，不冒充四平台平台 Gate。

| 验证 | 实际结果 | 证据 |
| --- | --- | --- |
| 四平台优化 workerd | 构建完成，binary SHA 与 Git LFS OID / formal lock 一致 | `.temp/p1-native/bundled-artifacts.json`；各 revision/target 的 build.json |
| 原生回归 | delegation / tails / limits / facets、日期与 GC 变体及严格退出通过 | `.temp/p1-native/test-10.log` 至 `test-16.log`、`host-facets-*.log`、`final-native-*/` |
| Canonical gzip | macOS ARM64、Linux ARM64/x64 生成的四份摘要一致 | `.temp/p1-native/bundled-compression-*.log` 的成功轮 |
| 真实 Loader 定向产品用例 | 通过；冻结 tail options、模块、RPC、facet、Version/删除/重启 | `.temp/p1-native/p6-loader-product-09.log` |
| 静态与工具 | build、generated、fmt、Clippy、no-default、MSRV 1.98、metadata、boundaries 通过 | `.temp/p1-native/*bundled-*.log`、`bun-build-26.log` |
| JS / 工具 | CI JS 239 通过；随后修改的 loopback 16 通过；Gate tooling 25 通过；文档站点构建通过 | `js-ci-bundled-01.log`、`loopback-unit-03.log`、`gate-tooling-bundled-02.log`、`docs-build-01.log` |
| Coverage | 49 targets / 1,148 cases；109,949/122,031 lines，90.10% | `.temp/gate-run/20260906T123444-b5c4a625/report.json`；`target/llvm-cov/summary.json` |
| 最终未插桩 Gate | 49 targets / 1,148 cases 全部通过，单轮 | `.temp/gate-run/20260906T125221-613eb524/report.json`；`.temp/p1-native/workspace-final-01.log` |

Coverage 与最终 Gate 使用相同冻结源码和正式 runtime inputs。两轮都完成精确 discovery、用例计数与
清理核对；没有 ignored case 或自动重试。最终报告之后只归档文档、修正引用并更新 conformance 的文档输入摘要，
未修改运行代码、测试或构建输入。

完整 `bun run test:js` 的独立 vinext 历史 qualification 存在本次改动前的 lock 摘要漂移，
未重写历史 Go 记录。普通 workspace/CI 按既有策略不包含该独立应用 qualification。
Linux 特权 egress fixture、四平台 ocd 发行资格、发布包装和远端 Git/LFS 推送没有执行。

维护入口：[正式 pin](../../packages/runtime/workerd.lock.json)、[固定二进制](../../share/workerd/README.md)、
[兼容矩阵](../references/cloudflare-compatibility.md)、[P2 设计](../workerd/p2-workers-standard-limits.md)。
