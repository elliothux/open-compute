# 文档索引

| 内容 | 权威入口 |
| --- | --- |
| 已实现架构与产品维护 | [平台总览](implemented/open-compute-workerd-platform.md)、[完成索引](implemented/README.md) |
| 当前 API 支持与偏差 | [兼容矩阵](references/cloudflare-compatibility.md)、[偏差清单](references/p1-deviations.md) |
| 开发测试、部署与运维 | [参考文档](references/README.md) |
| 尚未取得的 qualification | [验收计划](acceptance/README.md) |
| 原生运行时实施与后续工作 | [workerd P1/P2](workerd/README.md)；源码基于 `third_party/workerd/` submodule |
| 其他待实现设计 | 下表；外部前置阻塞见 [blocked](blocked/README.md) |

已完成文档保留实现职责、接口／数据不变量和实际验收依据；重复规则引用权威入口，不再保留实施过程与废弃方案比较。
原始结果报告保留对应日期、输入、命令和失败证据，历史 PASS 不代表当前工作树已验收。

## 待实施

| 文档 | 当前状态 |
| --- | --- |
| [代码质量提升专项](code-quality.md) | TODO：按 Day 1 收敛 Rust/TypeScript 领域、package/crate 边界、Rust 硬 lint、合同生成链、测试结构和仓库生成物 |
| [macOS 解析进程内存限制](macos-document-parser.md) | TODO：RSS 硬限制待实现；0.1.0 接受此限制并保留完整格式支持 |
| [workerd 原生实现方案](workerd/native-limits-loader.md) | P1 Loader 已完成并固定三个正式平台；macOS Intel 仅支持手动编译；P2 资源执行器与预算仍待实施 |
| [workerd P2 Workers Standard limits](workerd/p2-workers-standard-limits.md) | 原生 fork 路线待实施；局部改动未完成验收，`OC-WKR-LIMIT-001` 保持开放 |
| [P12 Wrangler 项目开发与部署体验](p12-wrangler-project-workflow.md) | Day 1 产品合同与架构完成；待实现 local instance/remote target 选择、`ocd wrangler` 透明 launcher、安全凭据注入和 dev/staging/production workflow |
| [P13 面向用户的文档站重构](p13-documentation-site-restructure.md) | Day 1 信息架构、内容合同与实施方案完成；按 P11/P12 已实现后的产品形态重写站点及英中根 README，用户优先，新增 Develop/CLI 专章并把架构与贡献内容降到第二优先级 |
| [P14 Cloudflare Artifacts](p14-cloudflare-artifacts.md) | Day 1 合同与架构完成；标准 v4/Worker binding/Git Smart HTTP 受进程内 Git engine G0 阻断；不把现有内部 ArtifactStore 或 LynxOS 文件夹伪装成 Cloudflare Artifacts |
| [P15 Cloudflare Browser Run](p15-browser-run.md) | Day 1 合同与单文件分发架构完成；`ocd` 内嵌压缩 Browser Runtime、首次使用时离线物化并完整监督；待 BR-G0 在 `chrome-headless-shell` 与 Obscura 中选择一个正式引擎 |
| [P16 Cloudflare Containers](p16-cloudflare-containers.md) | Day 1 合同与架构完成；固定 Worker `ctx.container` 与官方 package 复用 workerd 原生实现，通过 operator-owned 外部 Container Engine Broker 执行；受 dynamic DoHost/workerd attachment 与真实 engine/package G0 阻断 |

设计完成并通过约定验收后移入 `implemented/`；只剩 qualification 时将剩余事项列入 `acceptance/`。
未实现设计不按完成文档精简，也不通过改状态标签宣称完成。
