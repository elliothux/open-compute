# 文档索引

| 内容                     | 权威入口                                                                                    |
| ------------------------ | ------------------------------------------------------------------------------------------- |
| 已实现架构与产品维护     | [完成索引](implemented/README.md)                                                           |
| 正式版本说明             | [Release notes](releases/README.md)                                                         |
| 当前 API 支持与偏差      | [兼容矩阵](references/cloudflare-compatibility.md)、[偏差清单](references/p1-deviations.md) |
| 开发测试、部署与运维     | [参考文档](references/README.md)                                                            |
| 尚未取得的 qualification | [验收计划](acceptance/README.md)                                                            |
| 原生运行时实施与后续工作 | [workerd W1/W2](workerd/README.md)；源码基于 `third_party/workerd/` submodule               |
| 其他待实现设计           | 下表；外部前置阻塞见 [blocked](blocked/README.md)                                           |

已完成文档保留实现职责、关键边界和实际验收结果；重复规则引用权威入口，不再保留实施过程、独立结果副本或废弃方案比较。
历史 PASS 不代表当前工作树已验收；必须原样保留的生成报告会单独标明。

## 待实施

| 文档                                                                               | 当前状态                                                                                                                                                                                                               |
| ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [代码质量提升专项-2026-09-08](q0-code-quality-2026-09-08.md)                       | TODO：按 Day 1 收敛 Rust/TypeScript 领域与 package/crate 边界；Dashboard 改为 kebab-case 文件名、Jotai 状态和 date-fns 日期边界，并建立 Prettier/Oxlint/Knip/typecheck/build/test 硬门                                 |
| [macOS 解析进程内存限制](p5-8-macos-document-parser.md)                            | TODO：RSS 硬限制待实现；0.1.0 接受此限制并保留完整格式支持                                                                                                                                                             |
| [workerd W1 Worker Loader](implemented/w1-native-limits-loader.md)                 | 已完成并固定三个正式平台；macOS Intel 仅支持手动编译                                                                                                                                                                   |
| [workerd W2 Workers Standard limits](workerd/w2-standard-limits.md)                | 原生执行器与预算待实施；局部改动未完成验收，`OC-WKR-LIMIT-001` 保持开放                                                                                                                                                |
| [P15 Cloudflare Browser Run](p15-browser-run.md)                                   | Day 1 合同与单文件分发架构完成；`ocd` 内嵌压缩 Browser Runtime、首次使用时离线物化并完整监督；待 BR-G0 在 `chrome-headless-shell` 与 Obscura 中选择一个正式引擎                                                        |
| [P16 Cloudflare Containers](p16-cloudflare-containers.md)                          | Day 1 合同与两阶段 provider 路线完成；短期依赖宿主 Docker + restricted Broker，长期以 BoxLite 或其他待 G0 的可嵌入 runtime + Docker 子集 shim 替换；受 dynamic DoHost/workerd attachment 与真实 engine/package G0 阻断 |
| [P17 macOS Developer ID 签名与 Apple 公证](p17-macos-code-signing-notarization.md) | Day 1 发行合同与 CI 方案完成；待配置受保护的 Apple/GitHub 凭据、签署最终 `ocd`、取得 Notary `Accepted` 并完成真实 tag 验收                                                                                             |

设计完成并通过约定验收后移入 `implemented/`；只剩 qualification 时将剩余事项列入 `acceptance/`。
未实现设计不按完成文档精简，也不通过改状态标签宣称完成。
