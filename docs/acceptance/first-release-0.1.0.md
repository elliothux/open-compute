# 首次发行 0.1.0 验收

日期：2026-09-05。状态：`v0.1.0` 已创建；发布验证失败，尚无公开 Release。

## 固定输入与范围

- 从 `main` 的 `62cb7a0c92059b9cc7a4c956915094d671d075b1` 创建 `codex/release-0.1.0`。
  最初未包含独立 R2 修复；2026-09-05 用户追加要求，将 issue #4 的本地提交
  `970650a17bd1906f699921ef7c5b8d88c6f1bed8` 一起合入后再发布。最终源码以合入 main 后的 annotated tag 为准。
- Cargo workspace 与 lockfile 已是 `0.1.0`，不制造无意义的版本或 lockfile 变动。
- workerd 保持 `v1.20260830.1`，revision `e9dda5963aba7ee4323960db795690ec78fec118`，
  compatibility date `2026-08-30`；四平台摘要以正式 `packages/runtime/workerd.lock.json` 为准。
- 修复 Local artifacts 在 Linux 上的时间戳整数类型与条件 import 编译失败；保留已有
  stale partial grace-period、符号链接拒绝和 crash-recovery 测试。
- 原生 package job 显式 `cargo fetch --locked`，通过统一 `single-binary` Gate 核对 discovery 与数量，
  并上传每个平台的 Gate 证据。
- release 先完成静态检查与 90% coverage，再运行 Linux/macOS 各一轮完整 Gate；每个 job 显式构建
  runtime assets。四平台产物经只读聚合，再由 release environment 中唯一写权限 job 发布。
- tag 必须对应已经通过 main push CI 的精确 commit，不覆盖既有 assets。用户明确授权本次
  尚未公开发布的 `v0.1.0` 在修复验收后重建；随后又追加纳入 issue #4，故最终 tag 必须包含 R2 修复。
  旧 tag object 与首次失败记录保留；不把这一例外扩展到已公开 Release。

## 发布内容与限制

仅发布 darwin-arm64、darwin-x64、linux-arm64、linux-x64 四个原生单文件 `ocd`，
以及 `release.json` 和 `SHA256SUMS`。生产启动离线，无 sidecar 或独立 workerd 下载。
下载和校验步骤见[发布参考](../references/releasing.md)，部署契约见
[单二进制指南](../references/single-binary.md)。

本次保持 workerd pin 与完整的 13 种文档格式支持。macOS 解析子进程缺少内存硬上限，
经明确决定接受为 0.1.0 限制；现有其他资源约束继续生效，见 [TODO](../macos-document-parser.md)。
撤回此前本地禁用格式的修改，生产解析源码、测试与 parser contract 摘要恢复到原实现。Workers Standard request/isolate 资源限制执行器
仍受 upstream 阻断，见 [P9 限制](../blocked/p9-workers-standard-limits.md)。长时 soak、签名/公证、
托管端差分与第三方应用重验不能由本次原生发行替代，其他缺口见[验收索引](README.md)。
完整 `bun run test:js` 的 vinext 冻结输入检查报告 `root lock digest drift`；215 个平台 JS 用例通过。
release 与普通 CI 使用相同的 `test:js:ci` 平台集合，不重写历史应用报告或冻结摘要来制造通过结果。

PR CI `33951624671` 的产品 Gates 通过，但 `p3-contract` 的 source baseline 摘要漂移导致整体失败；
其余 13 项合同检查通过。当前实现变更完成后重新冻结源码摘要，不改写历史报告。

恢复完整解析后的 PR CI `33953626108` 已全部通过。旧的重复 push 运行在取消后留下失败的 `ci`，
导致分支保护拒绝合并；修复为 PR + main push 触发，并让取消的工作流跳过汇总，避免重复 Gate 与错误失败状态。

## 待收集的发行证据

- [x] 本地 build、generated assets、215/215 平台 JS 测试、Python Gate 工具测试、文档构建、
  fmt、Clippy、no-default-features、MSRV 1.98、metadata、dependency boundaries、production hygiene。
- [ ] coverage 和最终单轮 workspace Gate。coverage 首次尝试在准备阶段因文档继续编辑触发
  `source or verified inputs changed during the Gate`，尚未执行产品用例；保留失败报告
  `.temp/gate-run/failed/20260905T144303-2687d286/report.json`。后续冻结全部源码和文档再执行。
  冻结尝试 `20260905T144805-3eb77e36` 的 service 单元测试 369/370，通过前置检查后在 Unix socket
  fixture 上遇到 macOS `SUN_LEN` 路径长度限制；两个 socket fixture 改用自动清理的短 OS 临时目录，
  不修改 production 文件系统校验。对应失败报告继续保留；修复后 service 单元测试 370/370 通过。
  本地缓存符号链接布局导致 profile 遍历不完整的尝试已中止并保留证据；修正布局后的
  `20260905T150007-fefe2ae0` 在 P0.7/P0.8 返回 `DO_STORAGE_LIMIT`：本机磁盘使用率超过默认
  95% DO 停写水位，剩余约 6 GiB。未降低水位、未删除旧缓存或失败证据；本地 coverage 和后续
  最终 workspace Gate 未通过，发行所需完整证据改由 GitHub 托管 runner 收集。
- [x] GitHub API 回读：main required `ci`（包含管理员，禁止 force push/delete）；release environment
  仅接受 `v*` tag；`Release tags` ruleset `22323316` 限制 tag 创建/更新/删除，只有 repository admin
  maintainer 可 bypass；默认 token read-only；immutable releases 已启用。
- [x] [发布 PR #5](https://github.com/elliothux/open-compute/pull/5) 已合入 main，源码
  `203d0470dccdd5bd5961660bbc580b2c286e0324`；[main CI](https://github.com/elliothux/open-compute/actions/runs/33955854993) 全部通过。
  annotated `v0.1.0` 指向该 commit，tag object `bdddb46e4800927837459c211db28845fbbb8386`。
- [发布运行](https://github.com/elliothux/open-compute/actions/runs/33957020994) 的 MSRV 与 Linux/macOS
  静态检查通过；macOS coverage 在 P2 exit Gate 报告 `ocd exited before readiness`，后续 Gate、
  packaging 与 publish 均未执行。未生成通过 coverage 门槛的证据，也未发布 assets。
- 排查发现共享进程夹具用新的 `Process` 替换旧对象时，先 spawn 新 daemon，再运行旧 guard 的
  orphan cleanup，二者竞争同一 child lease。本地旧夹具复现 `RUNTIME_INVALID`；修复为同一 guard
  跨代持有进程，仅替换已退出的 Child，生产 daemon 自行完成 orphan recovery。
  相关 P2、Workflow crash、Wrangler restart 入口统一采用该所有权模型。
  修复后的本地插桩 P2 运行通过启动/重启阶段，在后续步骤因本机磁盘水位返回 `DO_STORAGE_LIMIT`；
  报告保留于 `.temp/gate-run/failed/20260905T173937-14b7a082/report.json`，不能记为 Gate 通过。
  CI 原记录没有子进程 stderr，因此竞态与 CI 启动退出之间仍需托管运行验证；失败 artifact
  补充只收集保留的 `ocd.log`，不上传 fixture 数据库、配置或凭据。
- GitHub 托管 runner 的特权 egress 操作已获用户明确授权；本机不运行 sudo。
- [ ] Linux/macOS 单轮 Gate、90% coverage、Linux 特权 egress 及清理结果。
- [ ] 四平台 package report、单文件隔离/首启/重启/损坏拒绝、六个 assets 的回读校验。
- [ ] 正式 latest Release、逐目标文件大小/SHA-256、发布完成时间。

未完成上述验证前保留本文于 acceptance；完成后将实际结果归档到 implemented 并更新索引。

## Issue #4 纳入首次发行

本地 R2 修复的原始实现、Adobe S3Mock debug/release 对比、90.044691% coverage 和 1,140-case
最终 Gate 记录保持原样，见 [issue #4 完成记录](../implemented/github-issue-4-r2-upload.md)。
本次集成基于已合入的进程夹具修复；只重新计算合并输入的 conformance source digest，未更改 R2
生产实现。合并后的完整 CI、coverage、最终 Gate 和四平台 package 仍须以新源码实际执行，不能复用
旧提交的通过状态作为新版本的发行资格。此前不包含 R2 的 tag 暂不重建，不发布部分源码。

集成后的 macOS arm64 插桩 development Gate `p0-5` 两个注册用例通过，69.21 s；
报告为 `.temp/gate-run/20260905T184154-f82ad9af/report.json`。同尺寸上传的分片阶段为 11,193 ms，
最慢分片 517 ms；171 次轻量请求探测中最慢 141 ms，读回 SHA-256 与原记录一致。
这是带 coverage 插桩的 debug 观察，不与原记录的非插桩 debug/release 时间混为同一性能基线。

集成复核未发现新的 R2 兼容性问题：原始 R2 生产文件和回归与 `970650a17` 字节一致，
公开类型及 formal workerd pin 未变；完整发行验收仍等待合入后的最终源码。

| 集成检查面 | 结论 | 证据 |
| --- | --- | --- |
| multipart 返回值、ETag、SSE-C | aligned | pinned upstream declarations/source、合并后的真实 stock-workerd R2 Gate |
| PUT checksum、取消及资源持有 | aligned | 原始成功/失败/取消回归和已保留的完整验收；有界 blocking task 的资源所有权复核 |
| HTTP metadata 缺省和显式字段 | aligned | 原始 presence-mask 全组合及 Adobe 回读证据；官方 R2 metadata 合同 |
| 托管 Cloudflare differential | unverified | 本次没有执行新的外部部署，不扩大已有资格声明 |

全球复制/placement 仍是 `OC-R2-001` 的 excluded self-host scope，不构成本次新增差异。

## 第二次发行验证与短命子进程修复

[Issue #4 集成 PR #7](https://github.com/elliothux/open-compute/pull/7) 已合入
`5ed57bd9ec041cbdddb1aaa331b34fb7237c3fcb`，issue 已关闭。
[PR CI](https://github.com/elliothux/open-compute/actions/runs/33962041707) 与
[精确 main CI](https://github.com/elliothux/open-compute/actions/runs/33963262157) 通过；PR 的 Linux 单轮
workspace 为 49 个测试进程、1,134/1,134 用例，1,085.58 s。macOS 与 Linux 的 cfg 用例数不同，
不将 Linux 数量写成 macOS 的通过数量。

获授权后重建的 annotated tag object 为 `381a015583d5ed11085133b0de2b29ba0625f248`。
[第二次发行运行](https://github.com/elliothux/open-compute/actions/runs/33964367514) 的身份、MSRV 和
Linux/macOS 静态检查通过；coverage 的 R2、P2 exit Gate 通过，但 `p3-services-events` 在
`workerd --version` 验证阶段返回 `failed to read runtime process group`。coverage 未完成，最终 Gate、
打包及公开发布均未执行。保留原始 tag object 与失败 artifact，报告为
`.temp/gate-run/failed/20260905T115923-d2a36573/report.json`。

短命子进程可以已经退出但尚未回收，此时 macOS `getpgid` 返回 ESRCH，而 `kill(pid, 0)` 仍成功。
旧判断错误地把这种状态当作活进程校验失败。修复只在进程组读取失败时检查所拥有 `Child` 的真实
退出状态；活进程、进程组 leader 不匹配及 wait 失败继续 fail closed，版本退出码和输出继续校验。
删除旧的 PID 存在性辅助函数，不引入重试或跳过版本验证。

确定性回归通过 `waitid(EXITED | NOWAIT)` 保留已退出子进程，并在 macOS 断言上述两个系统调用的
结果；修复前同类回归失败，修复后成功并保留 stdout、成功退出码和回收保证。其余 89 个 runtime
单元用例通过，日志保留在 `.temp/release-exit-fix/`。修复后的真实 stock-workerd 插桩
`p3-services-events` 单轮通过，6.77 s；报告为
`.temp/gate-run/20260905T201927-86cc29bf/report.json`。新源码的完整发行资格仍待托管验证。

## 第三次发行验证与 S3 夹具的重复摘要计算

[短命子进程修复 PR #8](https://github.com/elliothux/open-compute/pull/8) 已合入
`2c36a0d52108bbf1f85f58f3fa305180057d5b71`，
[精确 main CI](https://github.com/elliothux/open-compute/actions/runs/33967204223) 的 49 个进程、
1,135/1,135 Linux 用例单轮通过，1,035.74 s。重建后的未公开 tag object 为
`072311df4e00d1d71ce9a09c9787598af2460ec8`。

[第三次发行运行](https://github.com/elliothux/open-compute/actions/runs/33968433021) 的静态检查和
完整 macOS coverage Gate 已通过：49 个测试进程、1,141/1,141 用例，884.70 s；
Rust 行覆盖率为 109,609 / 121,720（90.050115%）。报告为
`.temp/gate-run/20260905T132513-e9dab2d4/report.json`。这份证据属于该次固定输入，不代表随后修改后的
源码已经重新完成发行资格。

随后 Linux/macOS 非插桩最终 Gate 都在大文件 R2 回归返回 `R2_RESULT_UNKNOWN`，其他目标在失败后
按规则停止；未打包或公开发布。保留报告
`.temp/gate-run/failed/20260905T134120-9db263bf/report.json` 与
`.temp/gate-run/failed/20260905T134137-e7acbe07/report.json`。
旧日志统一在 `checked_json` 断言，没有标识失败请求阶段，不能据此宣称已直接观察到具体超时调用。

排查发现测试 S3 的 multipart complete 仍重新计算全部分片 MD5，虽然 uploadPart 已保存同一摘要。
同尺寸一次独立 debug 对比中，旧 assembly/hash 为 4.587 s，复用保存的分片摘要后为 0.711 s，最终
ETag 与对象 SHA-256 一致。旧耗时已接近夹具的 5 s S3 请求超时，存在随并发负载失败的成本问题。
修复仅复用夹具已生成的摘要；保留完整对象 SHA-256、真实分片 MD5 和 multipart ETag 算法，
不更改 production R2、5 s provider fixture timeout、30 s HTTP header deadline 或任何断言门槛。

补充两分片 ETag 回归及请求阶段诊断。修复后的本机非插桩 `p0-5` 两个注册用例单轮通过，56.81 s；
报告 `.temp/gate-run/20260905T221359-758a8564/report.json`。大文件 upload phase 为 3,756 ms，complete
为 721 ms，最慢分片 564 ms；52 次轻量请求探测中最慢 112 ms，读回摘要一致。
源码的托管最终资格与四平台打包仍待完成；旧通过记录和失败记录均保留。
