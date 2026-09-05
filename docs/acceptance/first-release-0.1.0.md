# 首次发行 0.1.0 验收

日期：2026-09-05。状态：准备中；尚未创建 `v0.1.0`，尚无公开 Release。

## 固定输入与范围

- 从 `main` 的 `62cb7a0c92059b9cc7a4c956915094d671d075b1` 创建 `codex/release-0.1.0`。
  当前独立 R2 修复分支不在本次发布准备变更内；最终源码以合入 main 后的 annotated tag 为准。
- Cargo workspace 与 lockfile 已是 `0.1.0`，不制造无意义的版本或 lockfile 变动。
- workerd 保持 `v1.20260830.1`，revision `e9dda5963aba7ee4323960db795690ec78fec118`，
  compatibility date `2026-08-30`；四平台摘要以正式 `packages/runtime/workerd.lock.json` 为准。
- 修复 Local artifacts 在 Linux 上的时间戳整数类型与条件 import 编译失败；保留已有
  stale partial grace-period、符号链接拒绝和 crash-recovery 测试。
- release 先完成静态检查与 90% coverage，再运行 Linux/macOS 各一轮完整 Gate；每个 job 显式构建
  runtime assets。四平台产物经只读聚合，再由 release environment 中唯一写权限 job 发布。
- tag 必须对应已经通过 main push CI 的精确 commit；不修改已推送 tag，不覆盖既有 assets。

## 发布内容与限制

仅发布 darwin-arm64、darwin-x64、linux-arm64、linux-x64 四个原生单文件 `ocd`，
以及 `release.json` 和 `SHA256SUMS`。生产启动离线，无 sidecar 或独立 workerd 下载。
下载和校验步骤见[发布参考](../references/releasing.md)，部署契约见
[单二进制指南](../references/single-binary.md)。

本次不改变 Cloudflare API 支持范围和 workerd pin。Workers Standard request/isolate 资源限制执行器
仍受 upstream 阻断，见 [P9 限制](../blocked/p9-workers-standard-limits.md)。长时 soak、签名/公证、
托管端差分与第三方应用重验不能由本次原生发行替代，其他缺口见[验收索引](README.md)。
完整 `bun run test:js` 的 vinext 冻结输入检查报告 `root lock digest drift`；215 个平台 JS 用例通过。
release 与普通 CI 使用相同的 `test:js:ci` 平台集合，不重写历史应用报告或冻结摘要来制造通过结果。

## 待收集的发行证据

- [x] 本地 build、generated assets、215/215 平台 JS 测试、Python Gate 工具测试、文档构建、
  fmt、Clippy、no-default-features、MSRV 1.98、metadata、dependency boundaries、production hygiene。
- [ ] coverage 和最终单轮 workspace Gate。coverage 首次尝试在准备阶段因文档继续编辑触发
  `source or verified inputs changed during the Gate`，尚未执行产品用例；保留失败报告
  `.temp/gate-run/failed/20260905T144303-2687d286/report.json`。后续冻结全部源码和文档再执行。
- [x] GitHub API 回读：main required `ci`（包含管理员，禁止 force push/delete）；release environment
  仅接受 `v*` tag；`Release tags` ruleset `22323316` 限制 tag 创建/更新/删除，只有 repository admin
  maintainer 可 bypass；默认 token read-only；immutable releases 已启用。
- [ ] 发布 PR、main CI、annotated tag commit 和 release workflow URL。
- [ ] Linux/macOS 单轮 Gate、90% coverage、Linux 特权 egress 及清理结果。
- [ ] 四平台 package report、单文件隔离/首启/重启/损坏拒绝、六个 assets 的回读校验。
- [ ] 正式 latest Release、逐目标文件大小/SHA-256、发布完成时间。

未完成上述验证前保留本文于 acceptance；完成后将实际结果归档到 implemented 并更新索引。
