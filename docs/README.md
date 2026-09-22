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
| [macOS 解析进程内存限制](p5-8-macos-document-parser.md)                            | TODO：RSS 硬限制待实现；0.1.0 接受此限制并保留完整格式支持                                                                                                                                                             |
| [workerd W1/W2/W3](implemented/w3-user-extensible-native-bindings.md)              | Loader、Standard limits 与用户可扩展原生 Binding 已完成；三个正式产品平台及 macOS Intel 手动输入统一固定到当前 fork revision                                                                                           |
| [I42–67 GitHub open issues 剩余实施批次](implemented/i42-67-github-open-issues.md) | 已完成 `#66`、`#61`、`#62`、`#42`、`#58`，包含 Day1 实现、测试与 Cloudflare 兼容性检查                                                                                                                                 |
| [P18 单域名公网网关、DNS 与 TLS](p18-single-domain-public-gateway.md)              | implemented locally：固定双入口、标准多 Caddyfile、单 data-dir、私有 admin 热重载/恢复、`ocd caddy` 六命令和 Docker smoke 已落地；真实公网 DNS/ACME qualification 等待可公开 TCP 443、UDP/TCP 53 的主机                |
| [P19 Cloudflare Browser Run](p19-browser-run.md)                                   | Day 1 合同与单文件分发架构完成；`ocd` 内嵌压缩 Browser Runtime、首次使用时离线物化并完整监督；待 BR-G0 在 `chrome-headless-shell` 与 Obscura 中选择一个正式引擎                                                        |
| [P20 Cloudflare Containers](p20-cloudflare-containers.md)                          | Day 1 合同与两阶段 provider 路线完成；短期依赖宿主 Docker + restricted Broker，长期以 BoxLite 或其他待 G0 的可嵌入 runtime + Docker 子集 shim 替换；受 dynamic DoHost/workerd attachment 与真实 engine/package G0 阻断 |
| [P21 macOS Developer ID 签名与 Apple 公证](p21-macos-code-signing-notarization.md) | Day 1 发行合同与 CI 方案完成；待配置受保护的 Apple/GitHub 凭据、签署最终 `ocd`、取得 Notary `Accepted` 并完成真实 tag 验收                                                                                             |
| [I87 Git thin-pack push](blocked/i87-gitserver-thin-pack.md)                       | blocked（GitHub `#87`）：等待上游 gitserver [PR #18](https://github.com/WJQSERVER/gitserver/pull/18) 合并，再更新 immutable pin 并补齐真实 push 回归验收                                                               |
| [I99–100 Worker 上传与 R2 同名重建](i99-100-sdk-upload-r2-recreate.md)             | implemented：SDK 首次上传 `worker_loader` 类型与 multipart 修复；上传解析只选择 live ready 资源，同名重建 R2 已通过产品回归                                                                                            |
| [I102 Dynamic Worker Binding 转发](i102-dynamic-worker-binding-forwarding.md)      | implemented for first matrix：正式 fork/private env、KV/D1/R2/Queue 与 ordinary values 已通过真实产品 Gate；其余已装配 binding 根对象待各自 qualification                                                              |

设计完成并通过约定验收后移入 `implemented/`；只剩 qualification 时将剩余事项列入 `acceptance/`。
未实现设计不按完成文档精简，也不通过改状态标签宣称完成。
