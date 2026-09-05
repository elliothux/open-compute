# 文档索引

| 内容 | 权威入口 |
| --- | --- |
| 已实现架构与产品维护 | [平台总览](implemented/open-compute-workerd-platform.md)、[完成索引](implemented/README.md) |
| 当前 API 支持与偏差 | [兼容矩阵](references/cloudflare-compatibility.md)、[偏差清单](references/p1-deviations.md) |
| 开发测试、部署与运维 | [参考文档](references/README.md) |
| 尚未取得的 qualification | [验收计划](acceptance/README.md) |
| 待实现的原生运行时 | [workerd P1/P2](workerd/README.md)；源码基于 `third_party/workerd/` submodule |
| 其他待实现设计 | 下表；外部前置阻塞见 [blocked](blocked/README.md) |

已完成文档保留实现职责、接口／数据不变量和实际验收依据；重复规则引用权威入口，不再保留实施过程与废弃方案比较。
原始结果报告保留对应日期、输入、命令和失败证据，历史 PASS 不代表当前工作树已验收。

## 待实施

| 文档 | 当前状态 |
| --- | --- |
| [macOS 解析进程内存限制](macos-document-parser.md) | TODO：RSS 硬限制待实现；0.1.0 接受此限制并保留完整格式支持 |
| [workerd 原生实现方案](workerd/native-limits-loader.md) | 路线已选定、待实施：用户 fork 上实现 native enforcer、Loader 委派、预算与生命周期；正式 pin 尚未切换 |
| [workerd P2 Workers Standard limits](workerd/p2-workers-standard-limits.md) | 原生 fork 路线待实施；局部改动未完成验收，`OC-WKR-LIMIT-001` 保持开放 |
| [workerd P1 Dynamic Workers / Worker Loader](workerd/p1-dynamic-workers-worker-loader.md) | 原生 fork 路线待实施；保留 [stock No-Go 证据](implemented/p10-worker-loader-feasibility.md)，DW1–DW5 未实施，Workers for Platforms 不在范围内 |
| [P11 Cloudflare Artifacts](p11-cloudflare-artifacts.md) | Day 1 合同与架构完成；标准 v4/Worker binding/Git Smart HTTP 受进程内 Git engine G0 阻断；不把现有内部 ArtifactStore 或 LynxOS 文件夹伪装成 Cloudflare Artifacts |
| [P12 Cloudflare Browser Run](p12-browser-run.md) | Day 1 合同与架构完成；标准 binding/Quick Actions/DevTools/CDP 通过 operator-owned 外部 Browser Provider 执行，受真实 stock-workerd/package/provider G0 阻断；正式 open-compute 发布仍是单个 `ocd` |

设计完成并通过约定验收后移入 `implemented/`；只剩 qualification 时将剩余事项列入 `acceptance/`。
未实现设计不按完成文档精简，也不通过改状态标签宣称完成。
