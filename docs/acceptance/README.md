# 待验收资格文档

这里集中保存“核心实现已经完成，但仍缺外部账号、长时运行、跨平台或正式发行证据”的活动验收计划。
这些文档不是待实现设计，未完成资格也不会重新打开已经归档的 Day1 核心实现。

## 活动验收索引

| 文档 | 当前缺口 |
| --- | --- |
| [P1 剩余验收](p1-release-acceptance.md) | 长时 soak 与正式发行演练尚无完成证据 |
| [Cloudflare Workflow 远端 differential](p3-0-cloudflare-runtime-compatibility-acceptance.md) | 本地实现完成；托管端仍受 credential 条件阻塞 |
| [Static Assets / Service Binding 远端资格](p3-assets-service-bindings-acceptance.md) | 本地核心已归档；direct Cloudflare differential 尚未执行 |
| [P5 剩余发行验收](p5-release-acceptance.md) | benchmark report、四平台、parser process matrix、托管 rich-document differential 与正式 package 待完成 |
| [P6 Cloudflare v4 与固定客户端远端差分](p6-cloudflare-v4-differential-acceptance.md) | 仍需 Cloudflare credentials、hosted runner 与托管端证据 |
| [P7 observability 扩展差分与发行验收](p7-observability-extended-acceptance.md) | hosted 长尾、性能水位与跨平台发行资格待完成 |
| [P11 正式 runner 安装与 OS service](p11-operator-experience-acceptance.md) | 本地实现已完成最终冻结；三目标正式 Release 安装冒烟、真实 systemd/launchd、全新主机 setup→readiness、双真实实例并行待收集 |

资格文档与需求一一对应。完成后只把关键结果并入对应的 [`implemented/`](../implemented/README.md) 文档并删除资格计划；
发现实现缺口则恢复活动方案，真实外部阻塞则移入 [`blocked/`](../blocked/README.md)。外部 mutation、特权操作和发布仍需明确授权。
