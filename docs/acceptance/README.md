# 待验收资格文档

这里集中保存“核心实现已经完成，但仍缺外部账号、长时运行、跨平台或正式发行证据”的活动验收计划。
这些文档不是待实现设计，未完成资格也不会重新打开已经归档的 Day1 核心实现。

## 活动验收索引

| 文档 | 当前缺口 |
| --- | --- |
| [首次发行 0.1.0](first-release-0.1.0.md) | GitHub 分支策略已迁移；完整资格已有历史通过记录，打包修复与新输入发行仍待完成 |
| [P1 剩余验收](p1-release-acceptance.md) | 长时 soak 与正式发行演练尚无完成证据 |
| [Runtime 跨平台发行验收](runtime-layout-release-acceptance.md) | Linux/macOS Gate 与特权 egress 已有发行运行记录；四平台产物与公开发行待完成 |
| [Cloudflare Workflow 远端 differential](cloudflare-runtime-compatibility-acceptance.md) | 本地实现完成；托管端仍受 credential 条件阻塞 |
| [Static Assets / Service Binding 远端资格](p3-assets-service-bindings-acceptance.md) | 本地核心已归档；direct Cloudflare differential 尚未执行 |
| [P5 剩余发行验收](p5-release-acceptance.md) | benchmark report、四平台、parser process matrix、托管 rich-document differential 与正式 package 待完成 |
| [P6 Cloudflare v4 与固定客户端远端差分](p6-cloudflare-v4-differential-acceptance.md) | 仍需 Cloudflare credentials、hosted runner 与托管端证据 |
| [P7 observability 扩展差分与发行验收](p7-observability-extended-acceptance.md) | hosted 长尾、性能水位与跨平台发行资格待完成 |

## 维护规则

- 只有核心实现已经完成并归档、剩余工作纯属 qualification 的计划才放在本目录。
- 如果验收暴露新的实现缺口，应在 `docs/` 根目录建立或恢复明确的待实现设计，不能把实现工作隐藏在验收计划中。
- 外部 mutation、特权测试、发行打包和发布仍按仓库授权规则执行；缺少条件时保持待验收，不用本地 Gate 代替。
- 验收完成后，将计划和实际结果一起移入 [`docs/implemented/`](../implemented/README.md)，更新所有入站链接且不保留旧路径占位文件。

真正待实现的设计见 [`docs/` 索引](../README.md)，稳定契约和运维说明见
[`docs/references/`](../references/README.md)。
