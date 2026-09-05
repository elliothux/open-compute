# 文档索引

`docs/` 根目录只保留真正仍需实施的设计文档和本索引。核心实现已经完成、但仍缺外部、长时、
跨平台或发行资格的计划统一放在 [acceptance](acceptance/README.md)；已经完成并有证据的设计放在
[implemented](implemented/README.md)；稳定接口、测试规则和运维手册放在
[references](references/README.md)。历史 PASS 只证明对应 revision 和输入，不能替代当前实现与 Gate。

## 待实施

| 文档 | 当前状态 |
| --- | --- |
| [P9 Workers Standard limits](p9-workers-standard-limits.md) | 设计完成；structural limits、Version settings 与 stock workerd runtime enforcer 尚未实施，CPU/subrequest/memory/startup/connection 当前受 `OC-WKR-LIMIT-001` 阻断 |
| [P10 Dynamic Workers / Worker Loader](p10-dynamic-workers-worker-loader.md) | 合同与架构完成；`worker_loaders` v4/Version 支持受 upstream stock workerd nested-loader、limits 与 bounded-cache G0 阻断；Workers for Platforms 不在范围内 |
| [P11 Cloudflare Artifacts](p11-cloudflare-artifacts.md) | Day 1 合同与架构完成；标准 v4/Worker binding/Git Smart HTTP 受进程内 Git engine G0 阻断；不把现有内部 ArtifactStore 或 LynxOS 文件夹伪装成 Cloudflare Artifacts |
| [P12 Cloudflare Browser Run](p12-browser-run.md) | Day 1 合同与架构完成；标准 binding/Quick Actions/DevTools/CDP 通过 operator-owned 外部 Browser Provider 执行，受真实 stock-workerd/package/provider G0 阻断；正式 open-compute 发布仍是单个 `ocd` |

## 其他文档目录

- [待验收资格](acceptance/README.md)：核心实现已完成，只追踪外部、长时、跨平台或发行证据。
- [已实现设计与结果](implemented/README.md)：包括当前单机平台总架构和各阶段实际验收记录。
- [维护中的参考资料](references/README.md)：当前接口、兼容矩阵、测试规则、部署指南和 runbook。

根目录设计完成后，核心设计和实际证据应移入 `implemented/`；若只剩 qualification，则把剩余事项拆入
`acceptance/`。不得用状态标签代替实际移动，也不得保留旧路径兼容占位文件。
