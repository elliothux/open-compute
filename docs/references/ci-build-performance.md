# CI 与 Rust 构建性能

2026-09-26 的 GitHub Actions run 复盘；以下数字是墙钟，不是 runner 分钟。历史 run 的数字和缓存记录保留。

2026-10-06 起，Rust 工具直接调用全局安装的 `mbx`，不同项目和 worktree 共用全局对象缓存；CI 安装 mbx 1.22.0。
CI 直接缓存可写 Cargo target，不导出 mbx objects bundle，也不使用 sccache；Cargo registry/git 下载仍独立缓存。
具体策略与安装命令见[测试说明](testing.md)。下文已观察到的成本与研究取舍记录当时的实现，不代表当前缓存配置。

## 已观察到的成本

0.2.2 的正式 run `36243010371` 墙钟为 82 分 06 秒、合计 240.55 runner-min；关键路径是
80 分 45 秒的 coverage。coverage 内部构建为 27 分 26 秒、Gate 为 44 分 57 秒，报告生成为
4 分 16 秒。macOS runner 有 3 CPU，但插桩构建被固定为单 job；正式 workflow 现在只把构建并行度
提高到 2，保留 Gate 的 `--jobs 2`、每个测试进程的 `--test-threads=1` 和独占目标边界。正式 artifact
只消费 LCOV 与 JSON，因此 tag CI 不再额外生成约 74 秒且不上传的 HTML；本地 coverage 默认仍生成 HTML。

同一 coverage Gate 中，service library 的 795 个 case 作为一个独占进程串行占用 1,061.67 秒。现在仍只
编译一个 test binary，但 discovery 后将 787 个已审计隔离的 case 交给普通两路调度，只把 8 个真实
workerd、进程级 shutdown 和 startup lifecycle case 留在独占 barrier。两个逻辑目标必须发现完全相同的
原生 inventory，随后验证分区无遗漏、无重叠；每个 case 仍恰好执行一次。按该 run 的调度数据预计缩短
关键路径；本地插桩验证发现 P0.5 的 241 MB 并发上传/回读不能与该分片争用资源，因此 P0.5 保持独占。
最终收益尚未经过下一次插桩 CI 实测，不能把估算写成已实现收益。

同日已退役的 GitHub dry-run `36241986887` 为 31 分 43 秒、32.42 runner-min；它尚未完成时正式 release
已经启动，而且其产物不会流入 tag workflow，因此没有提供发布前拦截或构建复用。远端 dry-run 已删除；
隔离 package 诊断改在本机 Docker 内完成，不再先消耗一轮 Actions runner。

本地最终 Gate 的一次 `p5-search` 失败发生在 05:35:08 macOS 入睡至 05:51:54 DarkWake 的窗口；
该目标报告只累计 22.40 秒 active monotonic time，并在唤醒同秒得到 `RUNTIME_UNAVAILABLE`。同一源码保持
唤醒后单目标 71.41 秒通过，因此不改产品 runtime、不加重试：本地 Gate 和 Docker dry-run 在 macOS
自动使用 `caffeinate -is`，并把 p5-search 移到 single-binary 后的 fail-fast 段。正式 GitHub runner 不变。

`main` 的旧轻量检查（`34015774164`）耗时 2 分 46 秒；加入生产 Clippy、no-default-features
和 production hygiene 后，健康缓存的 `34974064143` 耗时 8 分 04 秒，半成品缓存下的
`35006658887` 和 `35017803309` 分别耗时 18 分 37 秒和 19 分 39 秒。慢点不是 GitHub runner
本身：慢 run 的 `clippy` 为 519–539 秒、production hygiene 为 362–367 秒；健康缓存时分别为
131 秒和 81 秒。之前每个生产库单独调用 Cargo，还会在不同调用中重复依赖检查。

本轮把生产库 lint 合并为一次 `cargo clippy --workspace --lib --no-default-features --no-deps`，
所有生产目标禁止 lint 第三方依赖；`--no-deps` 仍需让 Rust 编译依赖作为类型检查输入，
只是不会对第三方源码运行 Clippy 规则；本机同一源码的 canonical Clippy 从冷依赖到通过为 28 秒。
Rust target cache 不再保存失败的半成品，package 的 sccache 改为每个平台/锁定输入一个稳定 key，
不再把 commit、run 和 attempt 写进 key。第一次 v2 key 会冷启动，之后相同平台和锁图可复用。

`33977849336` 已完成 coverage 和 Linux/macOS 最终 workspace Gate，但 Linux x64/arm64 的
package 在编译后执行无 `--config` 的 capabilities 命令失败。两个 package 步骤分别消耗约
18 分 33 秒和 17 分 28 秒，错误是 CLI 契约不一致，不是编译错误。此前 package 必须等待整条
资格验证完成才启动，导致这一简单错误直到最后才出现。

远端 cache inventory 显示只有历史 dependency caches，没有本次三个正式平台 release Rust cache。
原 composite action 虽然设置 `cache-on-failure: true`，却以 `save-if: main` 排除了 tag 运行。
旧失败任务没有上传原生二进制，结束后的托管 VM 不能再取回；不能宣称能复用没有保存的 build。

确定性发布错误也曾被发现得过晚：`34939860032` 与 `35001991046` 在 macOS workspace/coverage 跑到末尾后
才由 `p3-contract` 报 source digest drift；`35001991046` 与 `35008672807` 又在三平台 package 完成后才由
assemble 报 SDK package report schema 不匹配。`35020012666` 到 publish 才发现 npm 认证缺失；后续 run 的
`npm publish` 已成功，但紧接着的 registry read-back 因传播延迟返回 E404，继续轮询没有增加发布正确性。
现在 main CI 与 tag release 都先执行秒级 `failfast`：source identity、release-tool/SDK report contract、
release environment、npm 认证与目标版本状态任一失败，都不会启动 Rust coverage、Gate 或三平台 package。

`35025974065` 是最近一次 7 分 06 秒的 full main CI，但输入只修改 CI/release workflow 与其分类器。
其中 setup 78 秒、Clippy 89 秒、production hygiene 80 秒；后两项以及 no-default-features/MSRV 都没有
读取这次改动的生产 Rust。main 现在把 change classification 与 source/release-tool fail-fast 合并为一个
job，并为 release workflow、release assembler/test 与随附文档设置 `release-tooling` scope；该 scope 只跑
TypeScript、format、文档和 release contract 检查。修改 `ci.yml`、共享 setup action、Rust/runtime 或未知路径
仍跑 full checks，避免改了检查本身却从未执行它。
full scope 保留全部命令，但拆成三个并行 matrix leg：core 负责 JS/Python、format、no-default-features、
MSRV、metadata 与 boundaries，Clippy 和 production executable hygiene 各自独立。首次成功实测
`35065714191` 总墙钟为 4 分 14 秒，较 `35025974065` 的 7 分 06 秒缩短 40%；failfast 13 秒，production、
Clippy、core 分别为 3 分 09 秒、3 分 27 秒、3 分 53 秒。此前两个 source identity 错误也都在 11 秒内停止，
没有启动 Rust legs。full run 的三个并行 leg 会增加总 runner 时间，因此不继续拆成更多 runner；路径分类
负责让这种成本只发生在真正需要 full 资格的改动上。

`35026079295` 的单目标 dry-run 共 35 分 56 秒：正式 release profile 编译 26 分 25 秒，随后
`single-binary` Gate 的测试 harness 准备又耗时约 7 分 34 秒，而两个测试本身只有 7.25 秒。该 run 的
sccache 是 0 hits / 2,641 misses，保存又因 configured budget read-only 失败。dry-run 与正式 package
现在额外只读恢复可用的 main default Rust target cache，复用 debug/test 依赖；release profile 仍由独立
512 MiB sccache 加速，不把开发产物当发行物。单目标 dispatch 也只创建所选 runner，不再启动另外两个
立即 skip 的矩阵 job。后续实测 `35066106199` 总墙钟 31 分 14 秒，其中 SDK 50 秒、package 30 分 50 秒、
release profile 冷编译 27 分 09 秒；`single-binary` 整体准备从约 7 分 34 秒降到 98.96 秒，实际 case 仍为
7.56 秒。该优化已验收，剩余关键路径是 release 冷编译，不再把 Gate harness 误判为主要瓶颈。

本地完整构建明显更快并不矛盾：当前开发机是 12 核、32 GiB 的 Apple M2 Max，GitHub 标准 Linux
runner 是 4 vCPU、16 GiB；冷 release 又要处理 1,552 个 sccache 可缓存请求，而本地通常保留 Cargo
产物。当时正式 profile 还启用 fat LTO 和单 codegen unit，最后的全程序优化不能按 12:4 的核数比例完全
并行。因此应分别比较 CI 冷缓存、CI 暖缓存和本地已有 target 的重编译，不能拿后一种判断 runner 异常。

依赖审计还发现 `aws-sdk-s3` 默认启用了 SDK 自带 TLS/HTTP client 与 SigV4a，但生产路径始终注入
平台校验过的 Smithy HTTP client，并且声明的 S3-compatible 范围使用 SigV4。关闭未使用的默认 feature、
只保留 Tokio 后，lock graph 删除 30 个 package，包括整套 Hyper 0.14/Rustls 0.21 与旧 P-256 栈；不会
再为没有生产调用者的第二套网络栈付冷编译和 LTO 成本。

`35073942314` 首次保留的 Cargo timing 显示 952 个 dirty unit、4 个 CPU job、26 分 45 秒总编译时间；
最终 `ocd` binary 单元独占 524.74 秒。此前关键链还有 xberg-tesseract build 315.33 秒、xberg 261.60 秒、
service library 166.30 秒。最终单元已占全程约三分之一，fat LTO 是缓存无法消除的确定瓶颈；正式 profile
因此改用 Cargo 文档所述“显著更快且性能收益接近 fat”的 ThinLTO。`codegen-units=1` 暂时保留，避免在
没有应用 benchmark 时同时引入第二个运行时性能变量。

ThinLTO 后的首次换 key 冷跑 `35077620373` 有 922 个 dirty unit、0 fresh，release 编译为 21 分 45.7 秒，
比相同 runner 上 fat LTO 的 26 分 45.1 秒缩短 18.7%；最终 `ocd` 单元从 524.74 秒降到 250.88 秒。
整个 workflow 为 27 分 50 秒，package job 为 27 分 27 秒；该次因 profile 与依赖图都改变，sccache 为
0 hits / 1,514 misses，所以这是编译策略收益，不是暖缓存收益。

同一 SHA 紧接着的暖跑 `35080400786` 恢复了 release target 与 compiler cache：Cargo 有 904 个 fresh、
18 个 dirty unit，release 编译只需 4 分 25.9 秒，sccache 为 8 hits / 1 miss（88.89%）；整个 workflow
为 9 分 01 秒，package job 为 8 分 37 秒，`single-binary` Gate 为 99.80 秒。相对原始冷跑
`35066106199` 的 31 分 14 秒，完整 dry-run 墙钟缩短 71.2%；冷输入之间则从 31 分 14 秒降到
27 分 50 秒，缩短 10.9%。暖跑剩余的 245.58 秒几乎全部属于最终 `ocd` ThinLTO/link 单元，不能由
sccache 缓存；继续提速需要改变正式二进制的 codegen/LTO 性能权衡、付费换更大 runner，或缓存完整
workspace/final binary。当前没有应用 benchmark，且仓库 cache 已接近 20 GB，因此不做这三种高成本优化。

## 当前执行分工

- `main` 和普通 PR：`failfast` 同时完成变更分类与 source/release-tool contract；full scope 的 core、
  Clippy、production hygiene 三个职责并行执行，完整覆盖 build/typecheck、快速工具测试、format、
  no-default-features、Rust 1.98 compile check、metadata 和边界检查；这些任务统一依赖
  秒级 `failfast`。release-only tooling scope 不启动 Rust。release tag 校验精确 source commit 已通过该静态资格，不再重跑。
- CI 先按变更路径分类：纯 `docs/**`、README 和 release notes 只执行文档检查；纯 SDK、dashboard、
  website 或 toolchain 变更只执行对应 JavaScript 检查；release workflow/assembler/test 使用独立
  release-tooling 检查；文档与 frontend 混合时在同一个 frontend job 执行两类检查。只有 `sourceDigest`
  变化的独立修复提交回溯到上一次 baseline revision，按期间全部 owning files 选择检查；baseline 其他字段、
  Rust、runtime、workerd、`ci.yml`、共享 setup 或未知路径仍执行完整静态资格。汇总 job `ci` 保留不变，
  避免分支保护因跳过具体 job 失效。
- tag qualification：`failfast` 先验证 release environment、source/release identity、notes、SDK report
  contract、npm credential 和目标版本；随后 Linux coverage 与一个 Linux 完整最终 workspace Gate 并行启动。
  最终 Gate 在受控 egress fixture 中执行，包含 `p0-2`；工作流不再另跑一轮 `p0-2`。
- 三个正式平台 package：身份验证后即并行构建，和全部 qualification 重叠；publish 等待所有路径成功。macOS Intel 不再进入 package 矩阵。
- package 与普通 production hygiene 使用同一 executable verifier，生成 mode 0600 临时配置再查询
  capabilities，同时核对 release identity、版本、licenses 和嵌入 docs；不初始化平台数据目录。
- `main` 不保护；`release` 要求 PR、最新 required `ci` 和讨论解决；tag 必须来自通过 CI 的 `release`。

## 缓存与证据

当前配置以 [setup action](../../.github/actions/setup-open-compute/action.yml) 为准；全局 mbx 设置、磁盘预算与本地清理见[测试说明](testing.md#验证磁盘预算)。

- CI 直接缓存可写 Cargo target；不导出 mbx object bundle，不使用 sccache。key 区分工具链、平台、编译输入及 job/suite，防止并行任务争抢同一不可覆盖条目。
- 只有成功的 main push 写共享缓存，PR/tag 只恢复；Cargo registry/index/git 下载缓存与编译产物分开。
- 正式 package profile 使用 `.temp/release-target/`，普通开发使用 `target/`；不互相覆盖，不把开发产物或缓存当发行物。
- 保留 Cargo timings 和 cache statistics，结合实际 restore、fresh units 与墙钟判断收益；最终链接等不可缓存部分仍可能占主要耗时。下一次必需 package run 提供测量，不为性能分析单独重复构建。
- pin、source、生成资产和 artifact SHA 仍需校验。不得伪造 mtime、复用不同 revision 的二进制或把 cache hit 当成验证通过。诊断产物按[证据生命周期](testing.md#日志与保留证据)保留。

### 历史缓存测量

以下是当时的预算、sccache 和旧 key 测量记录，不要求恢复旧工具、调整当前账户预算或删除远端缓存。

- 0.2.2 发布时 inventory 为 22 个条目、约 10.43 GiB；11 个旧 `v0.1.10` tag-scope 条目占约
  6.06 GiB。它们既不能服务后续 tag，又使新保存因 configured budget 进入 read-only。删除这些可重建
  的旧 tag cache 或提高预算后，main/诊断 workflow 才能重新写入；不能把失败的 save 当成暖缓存证据。
- 2026-09-16 inventory 有 22 个条目、约 9.57 GiB，已经贴近 GitHub 每仓库 10 GiB 上限；其中
  8 个旧 package compiler key 含 run/attempt，约 3.9 GiB，几乎没有跨发布复用价值。v2 key
  目标是三个平台各 512 MiB，稳定占用约 1.5 GiB；旧条目由 GitHub 的 LRU 淘汰，不手工删除失败证据。
- 之后的 dry-run `35026079295` 首次 Linux x64 打包成功（35 分 56 秒，其中 package 34 分 42 秒），
  但 sccache 是冷缓存（0 hits、2,641 misses）。保存 512 MiB cache 时 GitHub 返回 configured budget
  read-only；Actions API 当时仍列出 23 个条目、11,866,896,013 bytes，且 storage-limit API 为 20 GB，
  所以这次没有产生可复用的 v2 compiler key。保存步骤是非阻断的，不能把这次成功误报为 warm-cache
  效果；待配额实际可写后再用下一次 package run 测量命中率。
- 2026-09-16 再查 API：repository storage limit 已显示 20 GB，但 23 个 cache 共 11,866,896,013 bytes，
  仍没有任何 `compiler-v2-*` key；因此不能把配额页面变化当作 cache 已可写的证据。GitHub 的仓库 cache
  limit 与 `Actions Cache Storage`（`actions_cache_storage`）预算是两个独立开关：预算为零或已触顶时，超过
  免费 10 GB 后 cache 会保持 read-only。20 GB 上限全部用满时只有额外 10 GB 计费，按当前 $0.07/GB-month
  最多约 $0.70/月；账户预算应至少设为 $1/月，或先删到 10 GB 以下。repo workflow 的 `cache-mode` 不能绕过
  这个 billing 限制。
- `35066106199` 在新增账户级 `Actions Cache Storage` $2/月预算后仍收到 configured budget read-only。
  预算页面同时存在更宽的账户级 `Actions` 产品预算 `$0`且`Stop usage: Yes`；产品预算会先阻断其下的 cache
SKU，单独增加 SKU 预算不能覆盖它。要允许 cache 写入，Actions 产品预算也必须非零（可同样设为 $2 并
保留 stop-usage 总上限），或删除该产品预算。该 run 的 492 MiB sccache 因此仍未保存，不能宣称 warm-cache
效果；日志中的通用 `another job may be creating this cache` 不是根因，前一行 budget warning 才是根因。
- Actions 产品与 Cache Storage SKU 都设为 $2 后，`35073942314` 首次成功保存 535,767,092-byte
  `compiler-v2-*` cache 和 824,260,351-byte `v3-release-*` dependency cache；两次上传合计约 16 秒。
  该 run 仍是 0 fresh / 952 dirty units 的冷编译，不能当暖缓存结果。独立 stats 步骤在 package 后移除
  wrapper、执行 Gate 后读到 0 request，与已保存的 535 MB cache 不一致；后续是否复用以 cache restore、
  Cargo fresh units 和墙钟共同判定，不以该步骤的单个 hit 计数下结论。
- ThinLTO 首跑 `35077620373` 成功保存 530,519,545-byte compiler cache 与 1,498,593,428-byte release
  target cache；相同 SHA 的 `35080400786` 精确命中且没有重复保存。此时仓库共 29 个 cache、
  16,963,407,475 bytes，仍低于 20 GB 上限；旧 fat-LTO key 交给 GitHub LRU 淘汰，不为了回收约 1.36 GB
  手工删除证据。

## 历史研究取舍（2026-09-26）

该表保留当时的决定与依据；现行缓存策略以上节和 setup action 为准。

| 候选                            | 当前决定与依据                                                                                                      |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| full CI 并行职责                | 三个 runner 把 89 秒 Clippy 与 80 秒 production link/scan 移出 core 关键路径；不再细拆，控制总 runner 成本          |
| package 与 qualification 并行   | 已配置；publish 保留所有依赖，提前暴露打包问题                                                                      |
| coverage 构建并行               | 3 CPU macOS runner 上由 1 提到保守的 2；不改变 Gate/test 并发，以下一次完整 coverage 验证实际收益与资源稳定性       |
| CI coverage HTML                | 正式 workflow 不上传或消费 HTML，跳过约 74 秒；本地默认继续生成                                                     |
| Cargo target cache              | 保留按 profile/平台区分的依赖缓存；不盲目上传整个几十 GiB workspace target 导致缓存驱逐                             |
| sccache                         | 仅 native package 启用，限制容量并收集命中数据；coverage 保持现有插桩路径                                           |
| S3 SDK 默认 feature             | 生产注入自有 verified HTTP client；只保留 `rt-tokio`，删除未使用的默认 TLS client 与 SigV4a 依赖                    |
| 容器 / cargo-chef               | 当前三个正式平台原生 runner 不增加一套容器构建；Linux 容器不能证明 macOS 原生行为，镜像不能直接复用所有架构的机器码 |
| Fat LTO → ThinLTO               | timing 证实最终 binary 单元占 524.74 秒；改 ThinLTO，保留单 codegen unit，暂不叠加未测的运行时权衡                  |
| 增加 codegen units              | 暖跑剩余 245.58 秒为最终 ThinLTO/link；没有应用 benchmark 前不拿未知运行时退化换几十秒构建时间                      |
| 缓存完整 workspace/final binary | 当前 target/compiler cache 已证明 9 分钟暖跑；不增加 source-keyed 全量 target 缓存挤占 20 GB 配额                   |
| nightly 编译参数 / 替换 linker  | 不引入 nightly 或未验证 linker；保持正式 Rust 1.98 和原生链接契约                                                   |
| Gate 调度                       | 保持 `--jobs 2`；service lib 的 787/8 case 分区并行调度，但 241 MB P0.5 矩阵继续独占，避免以资源争用换时序失败      |

## 测试与复用边界

本地选择、单轮策略与 evidence 复用统一见[测试说明](testing.md#按输入选择测试)。CI 各职责见上文；
release 的 coverage、未插桩 Gate、受控 egress、安装生命周期、stress、三平台 package 与公开字节回读互不替代。
变更后的输入不能继承旧 PASS，也不重复调度未变化且身份一致的检查。

## 失败后的选择性重跑

按[发布恢复](releasing.md#失败重跑与修复版本)区分源码缺陷、基础设施失败和 assemble/publish 失败。
recovery 使用原 tag 与 source run 的完整成功证据和产物，不重跑 coverage/Gate/package；
缺失或不一致的输入失败关闭，不覆盖 Draft asset。

## Dry-run release

需要隔离验证时，经授权从干净候选 `HEAD` 执行 `./scripts/release-dry-run.sh`。
它仅验证本机 Docker 中的 Linux ARM64 package 路径；前置输入、离线阶段和保留路径见
[发布流程](releasing.md#三条工作流)。不替代正式 tag 的其他平台与发布资格。

主要资料：

- [Cargo build cache](https://doc.rust-lang.org/cargo/reference/build-cache.html)：profile/target 布局与共享缓存。
- [Cargo timings](https://doc.rust-lang.org/cargo/reference/timings.html)：编译单元、并发与关键路径报告。
- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)：LTO、codegen units 和 incremental 的权衡。
- [GitHub-hosted runner reference](https://docs.github.com/actions/reference/runners/github-hosted-runners)：标准 runner 的 CPU、内存与磁盘规格。
- [rust-cache inputs](https://github.com/Swatinem/rust-cache)：save-if、cache-on-failure 与 workspace crate 缓存行为。
- [GitHub cache scope](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)：分支/tag 可见性与不可覆盖条目。
- [GitHub artifacts](https://docs.github.com/en/actions/concepts/workflows-and-actions/workflow-artifacts)：job 结束后的构建输出保留。
- [sccache Rust limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)：禁用 incremental、链接不可缓存与宏约束。
- [sccache cache API 请求问题](https://github.com/mozilla/sccache/issues/2730)：逐 crate 远端缓存的限流风险。
