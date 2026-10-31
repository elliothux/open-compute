# P25：平台后续能力

状态：**planned**。本页保留尚未进入独立阶段的现有产品待办；开始实施时应冻结每项合同，若范围扩大则拆成独立编号。

## 待办

- [ ] cf 原生 Python builder 可用并通过资格检查后，删除 `scripts/build-python.ts` 的临时 PyWrangler/Wrangler 构建桥接，更新 Python 用户文档和构建测试。当前仅 Python 构建调用用户安装的 PyWrangler，不安装、不校验版本；上传、部署、开发与资源管理统一使用 cf。
- [ ] 跟进 [cloudflare/workers-py#287](https://github.com/cloudflare/workers-py/issues/287)。官方 SDK 发布修复后，重新验证 Flask 流式 body、并发 request/app context 和 cancellation cleanup，再恢复支持声明。当前按已知上游问题暂缓，原始 SDK 与失败证据保留。
- [ ] 支持从显式 `.env` 输入读取 instance 配置值，不隐式发现 cwd 或用户目录。
- [ ] 完成 operator-facing logger 合同，包括输出、级别、敏感信息清理和持久化边界。
- [ ] 评估并实现 lazy Worker startup，同时保持 readiness、首请求失败语义和受监督进程恢复合同。

## Containers 剩余范围

[P23 基础实现](implemented/p23-cloudflare-containers.md) 已验证用户当时指定的 13 个 Docker operation family。
2026-10-12，用户将 P23 目标扩展为完整 stable Container API 与 snapshot capture/restore。
当前固定清单的 65 个成员均声明为 `supported`。
2026-10-23（UTC），P23 的三项真实产品用例、coverage、最终 Gate 子进程及三宿主候选包已通过。
最终 Gate 外层守卫失败与独立摘要补证分别保留；正式 tag 工作流与公开发布尚未执行。
上述验收由 P23 记录，不作为 P25 的后续实现任务。
候选实现、正式 runtime、coverage、最终 Gate 与 package 的证据由 P23 结果页和 [Containers 运维](references/containers.md) 记录。
基础路径或专项检查通过不改变完整成员资格。

以下能力仍为 P25 的后续范围。

- [ ] 实现 application 主进程 stdout/stderr 采集。
      `observability.logs` 当前只保存请求意图；`enabled = true` 不证明实际采集。
- [ ] 在替换外部 Docker 前，验证可嵌入 provider 与同一私有子集的有效资源限制、离线分发和 lifecycle。
      未通过资格前继续使用 operator-owned Broker，不添加双路径或自动 fallback。
- [ ] 为 qualification probe 的 create 结果未知／Broker crash 增加有身份证明的恢复流程。
      当前 operator 核对实际名称、image digest 和 Docker ID 后清理；execution lease 的持久 fencing 保持独立。

## 已转交或撤销的方向

原 `wrangler.toml` 项目输入与 Wrangler transport 待办撤销，由 [P20：Cloudflare CLI 单轨迁移与官方应用构建链](implemented/p20-cf-cli-migration.md) 统一接管。OCD 不实现旧项目兼容层；检测旧配置后提示用户显式运行官方 `cf migrate <exact-file> --bundler vite`。

上面的 `.env` 待办只针对 instance 配置，不是应用项目 dotenv；应用配置与 CLI 凭据文件规则由官方 cf 负责，见 P20。

workerd compatibility authority 已拆入 R3。Q1 coverage 与 P24 macOS 签名已有各自的活动方案，不在这里重复跟踪。
