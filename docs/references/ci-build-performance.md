# CI 与 Rust 构建性能

2026-09-06。配置已调整，本轮托管命中率和净耗时尚待实测；不把理论收益写成已经达到的加速倍数。

## 已观察到的成本

`33977849336` 已完成 coverage 和 Linux/macOS 最终 workspace Gate，但 Linux x64/arm64 的
package 在编译后执行无 `--config` 的 capabilities 命令失败。两个 package 步骤分别消耗约
18 分 33 秒和 17 分 28 秒，错误是 CLI 契约不一致，不是编译错误。此前 package 必须等待整条
资格验证完成才启动，导致这一简单错误直到最后才出现。

远端 cache inventory 显示只有历史 dependency caches，没有本次四平台 release Rust cache。
原 composite action 虽然设置 `cache-on-failure: true`，却以 `save-if: main` 排除了 tag 运行。
旧失败任务没有上传原生二进制，结束后的托管 VM 不能再取回；不能宣称能复用没有保存的 build。

## 当前执行分工

- `main`、`release` 和普通 PR：同一 runner 完成 build/typecheck、快速工具测试、format、
  Rust 1.98 compile check、metadata 和边界检查。完整 workspace/coverage 不再重复排入每个开发提交。
- tag qualification：静态检查 → coverage → 一个完整最终 workspace Gate；保持 90% 门槛和受控 egress。
- 四平台 package：身份验证后即并行构建，和 qualification 重叠；publish 等待两条路径全部成功。
- package 与普通 production hygiene 使用同一 executable verifier，生成 mode 0600 临时配置再查询
  capabilities，同时核对 release identity、版本、licenses 和嵌入 docs；不初始化平台数据目录。
- `main` 不保护；`release` 要求 PR、最新 required `ci` 和讨论解决；tag 必须来自通过 CI 的 `release`。

## 缓存与证据

- Rust dependency cache 按工具链、OS/CPU、编译环境和 manifest/lock 分隔；release target 与 coverage
  各自使用 profile key。受信任的分支/tag 运行允许失败后保存，PR 不向这些缓存写入。
- MSRV job 只恢复依赖缓存，避免较小的 check 结果抢先占用其他 job 的不可覆盖缓存条目。
- package 使用固定 sccache 0.16.0，512 MiB 本地缓存位于 `.temp/sccache`，整目录通过 Actions
  cache restore/save 复用；主 key 包含源码/run/attempt，fallback 保持相同 OS/CPU、工具链和锁文件。
  成功和失败均保存。它是编译加速缓存，不是测试通过证据或可信发行物。
- 不启用逐 crate 的 GHA sccache backend：并行矩阵会增加缓存 API 请求，已存在上游限流与延迟报告。
  最终链接、bin/proc-macro 编译等仍有不可缓存部分；不承诺完全免编译。
- 保存 Cargo `--timings` 报告、cache statistics、失败时的未验收原生 binary 和现有失败 Gate evidence。
  一般日志显示子命令 stderr，避免长时间只看到一个无输出步骤。
- source、formal runtime pin、生成资产和 artifact SHA 校验仍执行；不得通过伪造 mtime 或复用不同
  revision 的发布二进制制造命中。输入发生变化，已有 Gate 结果只证明它原来的输入。

## 研究取舍

| 候选 | 当前决定与依据 |
| --- | --- |
| 同一 runner / 合并重复步骤 | 普通 CI 使用一台 runner、一轮 build，避免重复安装与 fresh-checkout 编译 |
| package 与 qualification 并行 | 已配置；publish 保留所有依赖，提前暴露打包问题 |
| Cargo target cache | 保留按 profile/平台区分的依赖缓存；不盲目上传整个几十 GiB workspace target 导致缓存驱逐 |
| sccache | 仅 native package 启用，限制容量并收集命中数据；coverage 保持现有插桩路径 |
| 容器 / cargo-chef | 当前四平台原生 runner 不增加一套容器构建；Linux 容器不能证明 macOS 原生行为，镜像不能直接复用所有架构的机器码 |
| Fat LTO → ThinLTO / 更多 codegen units | 尚未改 release profile；先用 timings 定位实际链接成本，避免未测量的大小/性能变化 |
| nightly 编译参数 / 替换 linker | 不引入 nightly 或未验证 linker；保持正式 Rust 1.98 和原生链接契约 |
| 增大 Gate 并发 | 保持审计后的 `--jobs 2` 和独占目标，不拿资源争抢换取新的时序失败 |

主要资料：

- [Cargo build cache](https://doc.rust-lang.org/cargo/reference/build-cache.html)：profile/target 布局与共享缓存。
- [Cargo timings](https://doc.rust-lang.org/cargo/reference/timings.html)：编译单元、并发与关键路径报告。
- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)：LTO、codegen units 和 incremental 的权衡。
- [rust-cache inputs](https://github.com/Swatinem/rust-cache)：save-if、cache-on-failure 与 workspace crate 缓存行为。
- [GitHub cache scope](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)：分支/tag 可见性与不可覆盖条目。
- [GitHub artifacts](https://docs.github.com/en/actions/concepts/workflows-and-actions/workflow-artifacts)：job 结束后的构建输出保留。
- [sccache Rust limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)：禁用 incremental、链接不可缓存与宏约束。
- [sccache cache API 请求问题](https://github.com/mozilla/sccache/issues/2730)：逐 crate 远端缓存的限流风险。
