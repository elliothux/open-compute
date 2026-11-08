# 文档索引

| 内容                   | 入口                                                                                        |
| ---------------------- | ------------------------------------------------------------------------------------------- |
| 版本说明               | [Release notes](releases/README.md)                                                         |
| 威胁模型               | [安全](security.md)                                                                         |
| 当前 API 支持与偏差    | [兼容矩阵](references/cloudflare-compatibility.md)、[偏差清单](references/p1-deviations.md) |
| 开发、测试、发布、运维 | [参考文档与文档规则](references/README.md)                                                  |
| workerd fork           | [workerd 路线](workerd/README.md)；源码在 `third_party/workerd/` submodule                  |
| 外部阻塞的方案         | [blocked](blocked/README.md)                                                                |
| 已完成方案（改动历史） | [implemented](implemented/README.md)                                                        |

用户文档在 `apps/website/src/content/docs/`。

## 待实施

| 文档                                                                               | 状态                                                                                                                         |
| ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| [R4 架构简化](r4-architecture-simplification.md)                                   | planned：调度库合并、按威胁模型删防御、成熟 crate 替换、删兼容负担、fork 瘦身、按产品组织代码，最后改测试和 CI               |
| [W4 workerd fork 收薄](workerd/w4-thin-fork.md)                                    | planned：逐组复核 fork，优先替换 Service／DO 私有 RPC 和 Queue 重复路径                                                      |
| [P24 macOS Developer ID 签名与 Apple 公证](p24-macos-code-signing-notarization.md) | 方案完成；待配置 Apple/GitHub 凭据、签署 `ocd`、取得 Notary `Accepted` 并完成真实 tag 验收                                   |
| [P25 平台后续能力](p25-platform-follow-ups.md)                                     | planned：instance `.env`、operator logger、lazy Worker startup、移除 Python 构建桥接、Flask SDK 流式问题、Container 日志采集 |
| [P26 外部反向代理下的 Worker 自定义域名](p26-external-proxy-worker-origins.md)     | planned：tenant-only external ingress、exact-host 授权、显式对外 origin；关联 #148                                           |
