# `ocd` Day1 命名改造完成记录

日期：2026-09-01

结论：**Implementation Go；本机完整单轮 Gate 通过。** 唯一生产 binary、CLI 与 daemon identity 已从
旧开发名直接切换为 `ocd`，没有 binary、参数、环境变量、脚本或文档兼容入口。项目与文档 origin
统一为 `https://open-compute.dev`，launchd reverse-DNS identity 统一为 `dev.open-compute.ocd`。

## 完成范围

- Cargo 只声明 `ocd` production bin，入口为 `crates/service/src/bin/ocd.rs`；Clap、version、真实进程
  helper、Gate registry、single-binary 与 release artifact 校验全部使用该名称。
- `oc` 工具链只接受 `--ocd` / `OPEN_COMPUTE_OCD`，conformance 与正式发行测试只使用
  `OPEN_COMPUTE_TEST_OCD` / `CARGO_BIN_EXE_ocd`；未保留旧 option/env fallback。
- 当前 README、runbook、package docs、container、systemd、launchd、开发及发行脚本均使用 `ocd`；
  旧 plist 被直接替换为 `examples/launchd/dev.open-compute.ocd.plist`。
- Cargo/Bun package homepage、VitePress sitemap、站点说明和 OCI image URL 均为
  `https://open-compute.dev`。launchd label 与文件名由该域名生成，不保留旧 reverse-DNS identity。
- P4 case matrix SHA 与 conformance source identity 由现有校验器刷新；没有修改既有 Cloudflare/P4
  verdict、远端运行证据或 workerd pin。

## 验收证据

以下检查均在同一工作树与 pinned `workerd v1.20260830.1` 输入上实际完成：

- `bun run build`、`bun run check:generated`、`bun run test:js`：通过，JS 198/198。
- `bun run docs:build`：通过，生成 sitemap 中 origin 为 `https://open-compute.dev`；VitePress/esbuild
  仍输出既有的 TypeScript `ES2024` target warning，不影响构建结果。
- service CLI subprocess tests：4/4；Gate runner self-tests：23/23；production artifact hygiene：通过，
  ordinary build 只产生一个 `ocd` production executable。
- `cargo fmt --all --check`、canonical Clippy、no-default-features、Rust 1.98 MSRV、Cargo metadata、
  dependency boundaries、plist lint、shell syntax、Markdown local-link audit 与 `git diff --check`：通过。
- `./test/coverage.sh`：40/40 targets、802/802 cases，通过；Rust line coverage
  `68407 / 75859 = 90.18%`。报告位于 `target/llvm-cov/summary.json`，Gate evidence 位于
  `.temp/gate-run/20260901T194008-19aeacff/report.json`。
- 最终未插桩 Gate 的完整 round 1：40/40 targets、802/802 cases，通过，耗时 614.80 秒。
  证据保存在
  `.temp/gate-run/failed/20260901T195159-60231662/report.json` 的 `results[0]`；该目录名与顶层
  `status=failed` 是用户要求在 round 2 开始后终止追加轮所致，不代表 round 1 失败。

用户在完整 round 1 通过后明确指定“Gate 跑一轮通过就行”。因此 round 2/3 不构成本次完成条件，
也不得将被中断的追加轮写成测试失败或三轮通过。中断遗留的一个 Gate 自有 `workerd` 已按精确 PID
终止，收尾审计未发现仍存活的 Gate/`workerd` 进程。

## 残留审计与限制

- live source、测试、脚本、examples、active docs、reference docs、package docs 与文件名中的旧 daemon
  token、旧 option/env、旧 Cargo target 和旧 reverse-DNS identity 均为零。
- `docs/implemented/**` 中此前 26 份历史资料仍有 157 个匹配行，记录当时实际 binary、命令与结果；
  按证据保留规则未改写，也不是 live compatibility surface。本方案本身同样保留旧值用于描述删除范围。
- 未执行 release packaging、正式发布、签名、公证、跨平台验证或 Cloudflare 部署；改名没有改变这些
  既有资格边界。
