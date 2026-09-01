# `ocd` Day1 命名改造方案

状态：已实现；本机完整单轮验收通过，追加时序轮按用户指示不作为完成条件

日期：2026-09-01

实施目标：把唯一生产可执行文件、CLI 与守护进程从 `platformd` 原子切换为 `ocd`

## 1. 决策

Open Compute 的产品名、仓库名、配置与环境变量命名空间继续使用 `open-compute`；唯一生产
daemon/CLI 使用 `ocd`，含义固定为 **Open Compute daemon**。现有开发工具 `oc` 保持不变，
由 `oc --ocd /absolute/path/to/ocd ...` 调用匹配版本的离线 bundle encoder。
项目与文档的权威 origin 为 `https://open-compute.dev`；需要 reverse-DNS identity 的运维表面使用
`dev.open-compute` 前缀。

`ocd` 足够短，也不会把产品名压缩成难辨认的随机缩写。已完成的命名调查没有发现足以阻止使用的
主流同名 CLI；小众项目的同名不作为阻塞项。名称一旦实施即作为 Day1 唯一权威入口，不再保留旧名。

## 2. Day1 边界

本改造是当前模型的直接重命名，不是迁移版本。实施完成后：

- 只有 `ocd` Cargo binary target、入口源码与发行文件；
- 不提供 `platformd` binary、软链接、wrapper、shell alias 或 PATH fallback；
- 不接受 `--platformd`、`OPEN_COMPUTE_PLATFORMD`、`OPEN_COMPUTE_TEST_PLATFORMD`；
- 不搜索 `/opt/open-compute/platformd`，不迁移旧 launchd label 或旧日志文件；
- 不引入 deprecated warning、双注册测试、双文档命令或 version selector；
- 不修改 SQLite schema、data-dir、artifact bytes、deployment identity、tenant API 或 workerd pin。

项目仍处于开发期，因此旧本地命令和旧发行文件没有兼容义务。改名不授权删除用户的本地数据；如果
实施前存在手工构建的旧二进制，由操作者自行处理，`ocd` 不在启动时发现、改写或删除它。

## 3. 权威命名契约

| 表面 | Day1 值 | 旧值处理 |
| --- | --- | --- |
| Cargo package | `open-compute-service` | 不改名 |
| Cargo binary / CLI / daemon | `ocd` | 删除 `platformd` target |
| binary entry | `crates/service/src/bin/ocd.rs` | 删除旧路径 |
| Clap program / version prefix | `ocd` / `ocd <version>` | 不接受旧 argv 名义入口 |
| Toolchain executable | `oc` | 不改名 |
| Toolchain binary option | `--ocd <absolute-file>` | `--platformd` 为 unknown option |
| Toolchain / conformance env | `OPEN_COMPUTE_OCD` | 不读取旧变量 |
| 正式发行测试覆盖 env | `OPEN_COMPUTE_TEST_OCD` | 不读取旧变量 |
| Cargo integration-test env | `CARGO_BIN_EXE_ocd` | 全部调用点直接更新 |
| 单文件发行名 | `ocd` | 不生成旧名副本 |
| 容器路径 | `/opt/open-compute/ocd` | 不创建旧路径 |
| systemd unit | `open-compute.service` | unit 名不改；只改 `ExecStart` 与描述 |
| project/docs origin | `https://open-compute.dev` | 不保留旧域名或无来源 reverse-DNS 标识 |
| launchd label / file | `dev.open-compute.ocd` / `dev.open-compute.ocd.plist` | 不保留旧 plist |
| launchd logs | `ocd.out.log` / `ocd.err.log` | 不轮转或迁移旧日志 |
| release 临时文件前缀 | `.ocd-<uuid>` | 不识别旧临时文件 |

`platform_version`、`PlatformConfig`、`PlatformError`、platform authority 等通用领域名称继续保留。
它们表达平台概念，不是旧 executable 的兼容表面，不应机械改成 `ocd_version` 或 daemon 专用类型。

## 4. 现状审计与改造面

### 4.1 Rust binary 与服务层

- `crates/service/Cargo.toml`：唯一 `[[bin]]` 改为 `ocd`，入口改为 `src/bin/ocd.rs`；package 名不变。
- `crates/service/src/bin/platformd.rs`：移动为 `ocd.rs`，继续保持 thin adapter，只负责 parse、logging
  和 exit-code adaptation。
- `crates/service/src/cli.rs`：Clap `name`、帮助文本和 rustdoc 改为 `ocd`。
- `crates/service/src/{lib.rs,exit.rs,run_p1.rs,runtime_bridge.rs,p2_3_promotion.rs}` 及相关 core/storage
  文档和错误文本：只替换 executable/daemon 含义明确的旧名，不改通用 platform 术语。
- 发行身份仍由现有 release identity 权威生成；`--version` 必须精确输出 `ocd <platform_version>`。

### 4.2 Rust 测试与 Gate case identity

- 所有 `env!("CARGO_BIN_EXE_platformd")` 改为 `env!("CARGO_BIN_EXE_ocd")`。
- 真实进程 helper、日志文件名、panic 文本和测试函数中的 daemon 名统一改为 `ocd`。
- timing registry 与 Rust case 一起原子改名，包括 P1 SIGKILL 和 Workflow process-crash case；禁止在 registry
  中同时登记新旧名称。
- `crates/service/tests/single_binary.rs` 使用 `OPEN_COMPUTE_TEST_OCD`，隔离目录内文件名为 `ocd`。
- CLI shape、P0.1、P0.2、P1、P2 Exit、Workflow 与 snapshot/restore 测试全部从新 target 启动，
  不以 mock 或 wrapper 替代真实进程路径。

### 4.3 TypeScript 工具链与 conformance

- `packages/toolchain/src/bundle-worker.ts` 的参数命名、绝对路径错误和失败提示改为 `ocd`。
- `packages/toolchain/src/cli.ts` 只声明 `--ocd` 并只读取 `OPEN_COMPUTE_OCD`；`types` 仍是离线命令，
  且明确拒绝 `--ocd`。
- 工具链 README 和测试同步更新。新 option 与新 env 的成功路径继续调用匹配的 absolute executable，
  `types` 不因 binary env 存在而加载 executable。旧 option/env 不作为受支持输入保留专用分支或测试；
  strict parser 的通用 unknown-option 行为和 live source 残留审计共同证明旧契约已删除。
- `test/conformance/differential.ts` 和 Gate 环境透传表只使用 `OPEN_COMPUTE_OCD` / `--ocd`。
- `packages/toolchain/dist/` 是生成物，不手改；通过根目录 `bun run build` 重建并保持 untracked。

### 4.4 开发、发行与运维入口

- `scripts/dev.sh` 使用 `cargo run ... --bin ocd`。
- `scripts/package-release.ts` 构建 `--bin ocd`、读取 `target/<target>/release/ocd`、校验新 version prefix，
  临时文件使用 `.ocd-<uuid>`；release 脚本仍只生成一个用户指定的绝对目标文件。
- `test/check-production.py` 只接受一个名为 `ocd` 的 production compiler artifact。
- `test/soak-p1.sh` 的新 Day1 fault-schedule 标识使用 `ocd_sigkill`；不解析旧记录作为新输入。
- 容器 build context、`COPY` 与 `ENTRYPOINT` 改为 `ocd`。
- systemd 保留 `open-compute.service`，只把 executable 路径改为 `/opt/open-compute/ocd`。
- launchd 文件、label、program path 与日志名一起改为 `ocd`，不留下旧 plist。
- Cargo 与 Bun workspace metadata 的 homepage、VitePress sitemap、文档站点说明和容器 OCI URL 统一为
  `https://open-compute.dev`；launchd reverse-DNS identity 统一为 `dev.open-compute.ocd`。

### 4.5 文档与生成基线

- 更新 `README.md`、`AGENTS.md`、维护中的 `docs/*.md`、`docs/references/**`、`packages/docs/**`
  和 examples，使所有可复制命令只出现 `ocd`。
- `docs/implemented/**` 中记录实际旧运行命令、旧文件名和旧结果的内容保持原文。这些是历史证据，
  不是当前 alias。维护索引里的当前状态描述可改为 `ocd`，但不得篡改历史命令、hash、结果或日期。
- 本方案实施完成后移动到 `docs/implemented/ocd-day1-rename.md`，另写实际验收记录并更新
  `docs/implemented/README.md`。
- conformance source identity 因维护源码变化而改变。只能由现有 check 计算新 revision 后更新
  `test/conformance/baseline.json`，不得猜测或手造 hash；应用资格结果中的既有历史 revision 不回写。

## 5. 实施顺序

1. 冻结当前工作树输入，记录既有未提交改动；只在上述命名范围内编辑，避免覆盖并行的 P4/CF 工作。
2. 先改 Cargo target 和 binary 文件名，再同步 Rust 调用点与测试 case identity，保持每个中间 commit
   至少没有双 target。
3. 改 toolchain option/env 与 conformance runner，同步新输入测试并删除旧契约残留。
4. 改开发脚本、发行校验、container/systemd/launchd；检查所有复制安装命令的目标文件名一致。
5. 改维护中文档，构建 TypeScript/runtime assets，刷新由源码身份决定的 baseline。
6. 做旧名残留审计和代码 review；修复完成后冻结源码，再执行一次最终验收链。
7. 写入实际命令、case 数、coverage 和限制，归档本方案。验证失败时 fix-forward 后从受影响的开发检查
   开始；不以临时 alias 恢复绿色。

## 6. 验证计划

### 6.1 结构与负向契约

- `cargo metadata --no-deps --format-version 1` 中 `open-compute-service` 只有一个 production bin `ocd`。
- `ocd --version`、`ocd --help`、`ocd worker bundle` 与 CLI subprocess tests 通过。
- toolchain 新 option/env 成功；strict parser 没有旧 option，进程环境读取也没有旧 env fallback。
- production audit 只发现一个 `ocd` artifact；release script 的 version、workerd pin、revision 校验不变。
- live 文件名与内容残留审计为零。审计排除本迁移文档和不可改写的 `docs/implemented/**` 历史证据，
  但不排除源码、测试、脚本、examples、runbook、package docs 或 Gate registry。

建议的内容审计：

```sh
rg -n 'platformd|OPEN_COMPUTE_PLATFORMD|OPEN_COMPUTE_TEST_PLATFORMD|CARGO_BIN_EXE_platformd|--platformd' \
  AGENTS.md README.md crates packages scripts examples test docs/references docs/*.md \
  --glob '!packages/runtime/dist/**' --glob '!packages/toolchain/dist/**' \
  --glob '!docs/ocd-day1-rename.md'
find crates packages scripts examples test docs -iname '*platformd*' \
  -not -path '*/docs/implemented/*'
```

两条命令都应无输出。归档后第一条不扫描 `docs/implemented/**`，并在验收记录中说明历史残留数量和
用途，避免把历史文档误报成 live compatibility surface。

### 6.2 实现期检查

实现和修复阶段只对每个相关 Gate target 跑一轮，不执行三轮 aggregate 或 full coverage。至少覆盖：

- 根目录 `bun run build`、`bun run check:generated` 和 `bun run test:js`；
- service CLI、single-binary、P0.1 real process、P1 crash、Workflow recovery 的相关单轮 target；
- Gate registry 自测与 production audit；
- `git diff --check`。

Cargo 消费 runtime assets 前必须先 build，并显式设置已验证的绝对
`OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE`；真实 runtime tests 显式设置 `OPEN_COMPUTE_TEST_WORKERD`，
不得由测试隐式下载。

### 6.3 最终验收

源码、review 与修复全部冻结后，原计划按仓库顺序执行以下完整验收链：

```sh
bun run build
bun run check:generated
bun run test:js
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings
RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features
cargo +1.98.0 check --workspace --all-targets
cargo metadata --no-deps --format-version 1
./test/check-boundaries.sh
./test/coverage.sh
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace
```

需要 workerd 的命令附带同一组已验证绝对输入；最终 Gate 的 round 1 跑完整 workspace，round 2–3
只重复登记的 timing cases。任一失败后停止，不重试掩盖失败；修复后回到单轮开发检查，再重新开始
受影响的最终验收。

实际验收中，完整 round 1 已覆盖 40 个 target、802 个 case 并全部通过。用户随后明确接受单轮 Gate，
因此追加时序轮被终止，不作为本次命名改造的完成条件；详细命令、结果与限制见
[完成记录](ocd-day1-rename-results.md)。

## 7. 完成条件

- 唯一生产 binary、发行文件和 daemon identity 均为 `ocd`；旧入口在 live surface 中不存在。
- 新 option/env 行为有明确测试，旧 option/env 在 live source 中不存在，且没有 compatibility branch。
- 真实 process、crash/restart、single-binary 与 conformance coverage 没有因改名丢失或改成 mock。
- 当前 runbook、container、systemd、launchd 与 package docs 的命令可以互相复制使用。
- Rust line coverage 仍不低于 90.00%，最终三轮策略 Gate 通过。
- 完成报告只声明实际运行过的平台和检查；跨平台发行、签名、公证或部署未执行时继续列为限制。

## 8. 非目标

- 不把仓库、Cargo package、npm workspace、配置文件、data-dir 或 systemd unit 统统改名为 `ocd`。
- 不把通用 `platform_*` 领域词汇机械替换为 daemon 名。
- 不改变 Cloudflare API 兼容范围、workerd 配置、network capability、持久化或安全边界。
- 不在本改造中生成、发布、签名、部署正式发行物，也不清理用户已有文件。
