# Runtime 布局的跨平台发行验收

日期：2026-08-29。状态：未执行；与本机核心实现及测试改造的验收分开记录。
实现和本地证据见 [Runtime 布局](implemented/runtime-and-test-layout.md)及[实测记录](implemented/runtime-and-test-layout-results.md)。

本计划只验证单机发行在不同 OS/CPU 上的构建、隔离启动和运行时资产契约，不证明 Cloudflare
Workers API conformance；平台契约与 portable differential 由
[P3.4](p3-4-cloudflare-conformance.md)单独验收。

## 待验证

- [ ] 在 CI 配置的 Linux/macOS 宿主上，从无 dist 的检出执行显式构建、静态检查、coverage
  和统一最终 workspace Gate（完整一轮、时序用例三轮），记录实际 CPU/架构、源码、工具链、
  输入摘要、逐轮用例与目标次数；规则见[测试节奏](references/testing.md)。
- [ ] 获得明确授权后，运行 Linux 受控 dual-stack/DNS/redirect egress 夹具；记录 loopback、
  hosts 和进程清理结果，不以 macOS 的普通 egress 测试代替。
- [ ] 对需要发行的目标逐一授权打包，使用唯一 [正式 pin](../packages/runtime/workerd.lock.json)，
  核对单文件版本、体积、SHA-256、无 JS 工具链的隔离离线首启/重启及损坏拒绝。
- [ ] 汇总支持宿主上的实际结果；未运行的目标保留未验证状态，不用 CI 配置存在代表 CI 已通过。

打包前需明确源码 revision、目标平台、workerd pin、绝对且未占用的输出路径，以及下载/权限
影响和排除的无关修改。此计划不是下载、sudo、打包、发布或部署授权，不触碰已有发行物和数据。
已有历史报告与失败证据保持原样；长时 soak 等仍由 [P1 剩余验收](p1-release-acceptance.md)跟踪。
