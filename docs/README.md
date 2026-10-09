# 文档索引

| 内容                     | 权威入口                                                                                    |
| ------------------------ | ------------------------------------------------------------------------------------------- |
| 已实现架构与产品维护     | [完成索引](implemented/README.md)                                                           |
| 正式版本说明             | [Release notes](releases/README.md)                                                         |
| 当前 API 支持与偏差      | [兼容矩阵](references/cloudflare-compatibility.md)、[偏差清单](references/p1-deviations.md) |
| 开发测试、部署与运维     | [参考文档](references/README.md)                                                            |
| 原生运行时实施与后续工作 | [workerd 路线](workerd/README.md)；源码基于 `third_party/workerd/` submodule                |
| 其他待实现设计           | 下表；外部前置阻塞见 [blocked](blocked/README.md)                                           |

已完成文档保留实现职责、关键边界和实际验收结果；重复规则引用权威入口，不再保留实施过程、独立结果副本或废弃方案比较。
历史 PASS 不代表当前工作树已验收；必须原样保留的生成报告会单独标明。
当前应用配置、构建和 CLI 以 [P20](implemented/p20-cf-cli-migration.md) 为准；早期实现及 release notes 中的 Wrangler
版本、命令和 fixture 仅记录当时输入，不构成当前开发入口或兼容承诺。

## 待实施

| 文档                                                                               | 当前状态                                                                                                                                                                                                                                                                                |
| ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [W4 workerd fork 收薄](workerd/w4-thin-fork.md)                                    | planned：逐组复核整个 fork；保留必要执行器与宿主接口，优先替换 Service／DO 私有 RPC 和 Queue 重复路径；替代能力尚待验证                                                                                                                                                                 |
| [P23 Cloudflare Containers](p23-cloudflare-containers.md)                          | planned：两阶段 provider 方案短期依赖宿主 Docker + restricted Broker，长期以 BoxLite 或其他待 G0 的可嵌入 runtime + Docker 子集 shim 替换；已改用 cf/config、Build Output 与 cf Containers 合同；待 CT0 冻结 wire，仍受 dynamic DoHost/workerd attachment 与真实 engine/package G0 阻断 |
| [P24 macOS Developer ID 签名与 Apple 公证](p24-macos-code-signing-notarization.md) | Day 1 发行合同与 CI 方案完成；待配置受保护的 Apple/GitHub 凭据、签署最终 `ocd`、取得 Notary `Accepted` 并完成真实 tag 验收                                                                                                                                                              |
| [P25 平台后续能力](p25-platform-follow-ups.md)                                     | planned：instance 显式 `.env`、operator logger、lazy Worker startup、临时 Python 构建桥接移除及 Flask SDK 流式问题跟进                                                                                                                                                                  |

[P20 已实现 CLI/应用构建入口](implemented/p20-cf-cli-migration.md) 是应用入口的权威方案；[P21 Python Workers](implemented/p21-python-workers.md) 记录普通 Python 部署、prepared artifact 与真实 daemon 验收。Python 构建临时调用用户安装的 PyWrangler，不安装或校验版本；上传、部署、认证和资源管理保持 cf。桥接移除与已知 Flask SDK 流式问题继续由 [P25 活动待办](p25-platform-follow-ups.md) 跟进。

实现完成后移入 `implemented/`；仍需实施的限制、功能缺陷和 TODO 留在活动方案，真正无法继续的外部阻塞移入
`blocked/`。验证矩阵、Gate 和测试清单不作为独立活动文档保留。
