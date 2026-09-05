# GitHub Issues #1–#3 修复与验收

状态：Completed，2026-09-05。按 #1 → #2 → #3 顺序修复，前项提交后才修改下一项。
不保留旧实现、兼容分支或数据回填；不改变现有 schema，不重置持久数据。

## 修复与回归

| Issue | 根因与当前实现 | 回归证据 |
| --- | --- | --- |
| [#1](https://github.com/elliothux/open-compute/issues/1) | 通用 4 KiB 声明长度检查错误覆盖 tenant ingress；现只作用于已注册控制路由，tenant fallback（含自定义域名）继续由 `WorkerdTransport` 的配置预算流式限制 | HTTP public/merged fallback、header/control 限额单测；P0.2 真实 HTTP → workerd 的 16 KiB、32 KiB 边界与 32 KiB + 1 拒绝，分别覆盖 Content-Length 和 chunked |
| [#2](https://github.com/elliothux/open-compute/issues/2) | Assets bulk multipart 使用 Axum 默认 2 MiB 限额；现仅该 route 显式设置 64 MiB wire cap，保留 50 MiB base64 payload budget 和单文件 25 MiB 限额 | 大于 2 MiB 二进制上传、完成 token、持久 digest 和对象逐字节回读；无 Content-Length 的超预算请求拒绝后，同一 session 仍可完成合法上传 |
| [#3](https://github.com/elliothux/open-compute/issues/3) | promotion 只从 live activations 计算 generation，全部 tombstone 后重新使用 1，与保留行的唯一键冲突；现从全部持久 activation 取最大值，保留当前 staging/active 重试身份 | 完全关闭并重开 control/scheduler SQLite 后，回滚到相同 Cron 的 Version 得到新 activation/generation 2；重试及 reconcile 保持身份，旧代次不匹配；P0.2 清空后重新启用并执行真实 scheduled handler |

#1 提交为 `f4c831832`，#2 为 `6fa327876`；#3 与本完成记录一起提交。
具体单测分别为 `control_body_bounds_do_not_cap_tenant_fallbacks`、
`bulk_upload_accepts_the_exact_base64_multipart_contract`、
`cron_remove_all_restart_and_reenable_preserves_generation_and_retry_identity`。
新增 real-runtime 断言并入既有 P0.2 case，不另建重复 Gate。

## 本次验收

使用预置且经构建/Gate 验证的 `workerd v1.20260830.1`，revision
`e9dda5963aba7ee4323960db795690ec78fec118`，compatibility date `2026-08-30`；未下载 runtime。

- `bun run build`、`bun run check:generated`、`cargo fmt --all --check` 通过。
- canonical workspace/all-targets/all-features Clippy（含 `--keep-going`、`-D warnings`）通过；测试编译诊断修正后统一复查。
- no-default-features（`RUSTFLAGS='-D warnings'`）、Rust 1.98.0 all-targets、metadata、dependency boundaries 通过。
- 三项针对性回归及原有 HTTP bounds 测试通过。Assets 成功测试先暴露夹具的 64 KiB object cap；只为该测试配置 25 MiB 后，验证实际大文件写入和读取，不放宽生产预算。
- `./test/coverage.sh` 一轮通过：49 targets、1,131/1,131 cases，803.92 秒；行覆盖率 **90.014165%**（109,297 / 121,422），门槛保持 90.00%。报告：`.temp/gate-run/20260905T113618-7a834267/report.json`，汇总：`target/llvm-cov/summary.json`。
- 源码冻结后 `./test/gate.py --workspace` 一轮通过：49 targets、1,131/1,131 cases，773.97 秒。报告：`.temp/gate-run/20260905T115036-1ecc92b9/report.json`。
- 两轮分别为插桩 coverage 和最终未插桩验收，均 `inventory_verified=true`、无 ignored/skipped case；不是同一 Gate 的多轮采样。source identity 均为 `5cb7d5b9dd421d30801d79df257e3e18bb2119c1c5886375dc8ee10e82270478`。

完成记录属于 Gate 明确不消费的历史文档，在验收后写入；没有改变冻结的生产、测试、manifest 或 maintained reference 输入。

## cf-compatibility-check 结论与边界

复查范围为本次三个 issue 的修复，相对起点 `ad0be6e43`；不把此前整个分支或平台称为已重新审查。
技能旧设计路径已归档，采用其当前位置 `docs/implemented/cloudflare-runtime-compatibility.md`，并读取当前 maintained compatibility/deviation 合同及正式 pin。
本次范围无剩余 actionable runtime compatibility finding。

| 变化面 | 结论 | 依据 |
| --- | --- | --- |
| Worker Request body / streaming admission | aligned（声明的本地预算内） | pinned Request/Body declarations 未改写；P0.2 真实 HTTP/stock-workerd 边界测试通过；[Cloudflare limits](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits)的托管 account-plan quota 不被本机预算冒充 |
| Cron scheduled lifecycle | aligned（本地持久化合同） | [官方 Cron 创建/移除语义](https://developers.cloudflare.com/workers/configuration/cron-triggers/)、SQLite 完全重开回归与真实 scheduled dispatch；未修改 ScheduledController 类型、事件 shape 或 `noRetry()` |
| Wrangler bulk Assets management upload | 不在该 runtime 技能范围 | 按既有固定客户端 wire 合同验证；multipart 大文件与产品预算回归通过，不据此声称新完成 hosted management differential |

Excluded self-host scope：Cloudflare account-plan/fleet quotas 与 Cron 全球传播拓扑。本次没有复制这些托管属性。
真实 Cloudflare differential 未运行，既有外部 qualification 限制仍保留；没有部署、修改已有 Cloudflare 服务、推送 Git 或修改/关闭 GitHub issues。
当前实现说明同步维护在 [Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md)。
