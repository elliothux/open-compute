# P4 Next.js / vinext 应用资格结果

状态：**Application Go；资格实现与仓库回归验收完成**，2026-09-01。

## 结论

固定 vinext `1.0.0-beta.8` / Next.js `16.2.7` workload 的同一 production artifact 已在真实
Cloudflare Workers 与 open-compute 上完成 differential qualification：20/20 selected mandatory
cases 通过，其中两端 application runner 各 15/15；0 optional，14 excluded。

本结论按 Cloudflare Worker Version/Deployment 语义判定。相同 source 的两次独立 vinext build 仍会因
上游生成的 build credential 改变 server chunks；该事实保留为非阻断 `toolchain-only` deviation，
不再错误充当 Runtime/Deployment Hard Gate。

## 冻结输入与产物

| 项目 | 证据 |
| --- | --- |
| root lock SHA-256 | `1486f18448f7220d640cbd1050d11922e49c89e390daea4dd37ea9128b5128b9` |
| fixture tree SHA-256 | `250045ab6e28eaa060aaef70728500880efacb077ba166f7ddab276cf962fbe5` |
| case matrix SHA-256 | `71866c92ee55775585efa8c58b06be2f92a848bc77541281a97c89cd4b84a6f2` |
| Wrangler/importer module inventory | 79 / 79；names SHA-256 均为 `8311c2918f094d6bbd435c9db94d4ced97d8da8c710e008d6dd925214a3e29d1` |
| open-compute canonical bundle | `a68f14ec859a463bbd4f946bff33cdbf57d32a32a4ebd3efd23ed2de2a02dd14` |
| Chromium executable | version `151.0.7922.34`；SHA-256 `a596b1cfc6353e987fcec8d71a23a28cd6a9e7a6b4e20b908e4c4fcffe51158e` |

## 实际结果

| 目标 | 结果 |
| --- | --- |
| offline manifest/registry checker | `verdict=go`；20 mandatory、0 optional、14 excluded |
| importer/project focused tests | 14/14 通过 |
| open-compute application runner | 15/15 通过 |
| Cloudflare application runner | 15/15 通过 |
| Cloudflare deployment | 1 Version、1 Deployment、100% traffic |
| Cloudflare Version ID | `890ee8c5-5608-40a7-9d85-891db837d8ec` |
| Cloudflare Deployment ID | `b45c5e85-e825-4b8a-83af-72cabc5362f3` |
| Cloudflare cleanup | 精确 Worker name 查询返回 `10007`，不存在 |
| open-compute cleanup | exact route/Worker tombstone；fresh list 为空；资格进程/listener 均停止 |

脱敏执行 evidence 保留于 `.temp/p4-*`。Tracked machine-readable verdict 位于
[`vinext.json`](../../test/conformance/applications/vinext.json)，详细范围、排除与语义见
[P4 资格文档](./p4-nextjs-vinext-qualification.md)。

## 实现影响

- 通用 framework importer 与 Wrangler `no_bundle` module traversal 对齐；
- generated binding reconciliation 保留本地 resource authority，同时核对 class/entrypoint 语义；
- staged deployment upload 的全局 middleware body ceiling 改为精确 route-aware，endpoint 上限保持；
- 固定 TypeScript application runner 覆盖 HTTP、streaming、Chromium、RSC、Action、Assets、env 和
  browser-context isolation。

实现没有增加 vinext 专用 runtime/schema/header/cache key，也没有把 Cloudflare provider ID 导入本地
authority。

## 仓库验收

| 检查 | 结果 |
| --- | --- |
| build / typecheck | `bun run build`、`bun run typecheck` 通过 |
| JavaScript/TypeScript tests | 197/197 通过 |
| Rust static checks | fmt、canonical Clippy、no-default-features、metadata、dependency boundaries 通过 |
| Rust 1.98 | workspace/all-targets/all-features 通过 |
| coverage | 40 targets、802/802 cases；75859 lines 中 68401 covered，90.17% |
| final workspace Gate | 80 processes、894/894 case executions；完整轮一次，46 个时序 case 追加两轮；1604.22 秒 |

Coverage 报告保留于 `.temp/gate-run/20260901T183941-4cc8d923/report.json`，最终三轮报告保留于
`.temp/gate-run/20260901T185216-7bb31c64/report.json`。

已知仓库级静态限制：不带 feature 的 canonical MSRV 命令会在既存 `p0_2_runtime_gate` 中调用仅由
`test-support` 导出的 helper；P4 未改动该 target。Rust 1.98 的 all-features check 与最终 workspace Gate
通过，因此该问题不改变本 Application verdict，但不能写成 canonical no-feature MSRV check 已通过。

## 限制

14 个 excluded cases 不进入结论：跨 source-build byte reproducibility、ISR/cache/invalidation、Images、
产品 bindings/Service 组合、promotion/rollback、workerd/platformd restart 和产品级双账户隔离。它们仍由
普通产品 Gate 所有。

本报告只给出 Application verdict；Platform、跨平台发行和 release qualification 仍保持各自真实状态。
