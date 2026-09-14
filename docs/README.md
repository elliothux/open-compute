# 文档索引

| 内容                     | 权威入口                                                                                    |
| ------------------------ | ------------------------------------------------------------------------------------------- |
| 已实现架构与产品维护     | [完成索引](implemented/README.md)                                                           |
| 正式版本说明             | [Release notes](releases/README.md)                                                         |
| 当前 API 支持与偏差      | [兼容矩阵](references/cloudflare-compatibility.md)、[偏差清单](references/p1-deviations.md) |
| 开发测试、部署与运维     | [参考文档](references/README.md)                                                            |
| 尚未取得的 qualification | [验收计划](acceptance/README.md)                                                            |
| 原生运行时实施与后续工作 | [workerd 路线](workerd/README.md)；源码基于 `third_party/workerd/` submodule                |
| 其他待实现设计           | 下表；外部前置阻塞见 [blocked](blocked/README.md)                                           |

已完成文档保留实现职责、关键边界和实际验收结果；重复规则引用权威入口，不再保留实施过程、独立结果副本或废弃方案比较。
历史 PASS 不代表当前工作树已验收；必须原样保留的生成报告会单独标明。

## 待实施

| 文档                                                                               | 当前状态                                                                                                                                                                                                               |
| ---------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [代码质量提升专项-2026-09-08](q0-code-quality-2026-09-08.md)                       | TODO：按 Day 1 收敛 Rust/TypeScript 领域与 package/crate 边界；Dashboard 改为 kebab-case 文件名、Jotai 状态和 date-fns 日期边界，并建立 Prettier/Oxlint/Knip/typecheck/build/test 硬门                                 |
| [Q1 workspace 行覆盖长尾补齐](q1-coverage-long-tail.md)                            | TODO：从 90.02% 基线补齐既有覆盖长尾到 ≥91%；W2 新增代码已全覆盖，剩余缺口按 v4 产品后端/运行时进程/服务安装/存储引擎四组列出，结构性不可覆盖项（SIGKILL fixture、公网 clone）单列                                     |
| [W2 Workers Standard Resource Limits](w2-standard-limits.md)                       | 进行中：原生执行、isolate 摘除、supervisor 自恢复及四平台 formal pin 已完成；待接通 Wrangler snake_case、toolchain、v4 Settings、Dynamic Worker ceiling、完整 subrequest accounting 与产品级真实运行时验收       |
| [macOS 解析进程内存限制](p5-8-macos-document-parser.md)                            | TODO：RSS 硬限制待实现；0.1.0 接受此限制并保留完整格式支持                                                                                                                                                             |
| [workerd W1 Worker Loader](implemented/w1-native-limits-loader.md)                 | 已完成并固定三个正式平台；macOS Intel 仅支持手动编译                                                                                                                                                                   |
| [I42–67 GitHub open issues 剩余实施批次](i42-67-github-open-issues.md)             | P15/`#51` 与 `#67` 已关闭；W2 保留已验证的原生限额/恢复并继续 public limits 对齐；本批次剩余 `#66`、`#61`、`#62`、`#42`、`#58`                                                                                           |
| [P16 capability-scoped TypeScript SDK](p16-capability-scoped-typescript-sdk.md)    | 方案完成：生成只暴露 qualified Cloudflare subset 与 `client.openCompute` 的 `@open-compute/sdk`；待协调 pin review、实现、npm bootstrap、真实进程 Gate 与首次联合发布                                               |
| [P17 Cloudflare Browser Run](p17-browser-run.md)                                   | Day 1 合同与单文件分发架构完成；`ocd` 内嵌压缩 Browser Runtime、首次使用时离线物化并完整监督；待 BR-G0 在 `chrome-headless-shell` 与 Obscura 中选择一个正式引擎                                                        |
| [P18 Cloudflare Containers](p18-cloudflare-containers.md)                          | Day 1 合同与两阶段 provider 路线完成；短期依赖宿主 Docker + restricted Broker，长期以 BoxLite 或其他待 G0 的可嵌入 runtime + Docker 子集 shim 替换；受 dynamic DoHost/workerd attachment 与真实 engine/package G0 阻断 |
| [P19 macOS Developer ID 签名与 Apple 公证](p19-macos-code-signing-notarization.md) | Day 1 发行合同与 CI 方案完成；待配置受保护的 Apple/GitHub 凭据、签署最终 `ocd`、取得 Notary `Accepted` 并完成真实 tag 验收                                                                                             |

设计完成并通过约定验收后移入 `implemented/`；只剩 qualification 时将剩余事项列入 `acceptance/`。
未实现设计不按完成文档精简，也不通过改状态标签宣称完成。
