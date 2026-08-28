# Runtime 包与测试流程整理

记录日期：2026-08-28。

状态：方案已确定，目录迁移、代码删除和测试入口改造尚未实施。本次只更新文档与
[AGENTS.md](../AGENTS.md)；以下目标布局和运行规则不能作为实现或测试通过的证据。
当前可用的单轮命令及尚未改造的入口见 [测试节奏](references/testing.md)。

## 决定

1. 将根目录 `runtime/` 移到 `packages/runtime/`，将生成目录 `system-workers/` 改名为
   `dist/`；`dist/` 不进入 Git。
2. POC 中已经完成任务的上游能力探测属于一次性代码，删除，不再作为日常或最终验收必跑项。
3. POC 中仍有产品回归价值的测试，以及它们实际依赖的 harness、fixtures，统一迁入根目录
   `test/`；已被现有测试等价覆盖的部分直接删除。
4. 清理重复用例、重复调度、重复构建和无效等待；优化耗时不能削弱安全、完整性、故障及恢复覆盖。
5. 开发、审查、修复期间，每次迭代只执行相关 Gate 的一轮；实现和审查完成、源码冻结后，
   才在最后一步验收执行三轮。完整检查和 coverage 不随三轮重复运行。

不保留旧目录别名、兼容入口或旧产物回退，不维护两套生产实现或两套相同的回归体系。

## Runtime 包

### 目标布局

```text
packages/runtime/
  package.json
  tsconfig.json
  tsconfig.build.json
  build.ts
  README.md
  workerd.lock.json          # 正式 pin，提交 Git
  config.capnp              # 配置源码，提交 Git
  src/<domain>/             # TypeScript 源码，提交 Git
  tests/<domain>/            # 包内行为测试，提交 Git
  dist/<domain>/            # 生成 JavaScript，不提交 Git
  dist/manifest.json        # 生成模块摘要，不提交 Git
```

`crates/runtime/` 继续负责 Rust 的 workerd 校验、配置编译和进程监督，与 TypeScript runtime
包不是同一个目录。包名保持 `@open-compute/runtime`，共享依赖继续由根 catalog 和唯一的
`bun.lock` 管理。源码、测试和生成模块均保留现有领域结构，不重新平铺文件。

### 构建约束

- `dist/` 全部可由已提交的源码和锁定工具链重建，禁止手工修改或 `git add -f`。
  现有 [.gitignore](../.gitignore) 的 `/packages/*/dist/` 已覆盖目标位置；迁移时仍须移除
  旧生成 JS 的 Git 跟踪，只有 ignore 规则并不会自动取消跟踪。
- TypeScript 7 负责严格类型检查，Rolldown 负责转换和构建。显式前置构建生成 `dist/`，
  然后 Rust 编译、测试和发行构建消费这些产物；缺少产物应失败，不能读取旧目录或隐式下载。
- CI 从不含 `dist/` 的检出开始构建。MSRV、lint、test、coverage、integration 等任何消费
  内嵌资源的 job，都必须先生成当前源码对应的 runtime 资产，不能依赖其他 job 的工作目录。
- `check:generated` 改为核对生成目录与当前源码、模块清单和摘要，不再以 Git 中存在一份 JS
  为前提。CI/最终验收使用隔离输出目录复现构建，比较完整文件集合和字节；重复构建仅用于这项
  可复现性验证，不在每个 Gate、每一轮重复执行。
- Rust 只内嵌经过前置构建和校验的资产。生产启动继续离线物化内嵌资源，不运行 Bun、Node、
  TypeScript 或 Rolldown；测试准备、构建准备和生产启动保持独立。

### 同步修改的调用方

不能只移动文件。实施时需要一起更新：

| 调用方 | 必须更新的内容 |
| --- | --- |
| 根 `package.json`、`bun.lock`、TS 配置 | workspace 路径、包定位、测试 glob、构建脚本的根依赖路径 |
| [runtime 构建脚本](../runtime/build.ts)与包内测试 | 输出目录、生成文件注释、fixture/模块加载路径及一致性检查 |
| [Rust 资源构建](../crates/runtime/build.rs)与 [embedded](../crates/runtime/src/embedded.rs) | 源码路径、内嵌资源映射、摘要、离线物化路径 |
| [Worker descriptor](../crates/workers/src/descriptor.rs)及相关测试 | `include_bytes!`、manifest 和 runtime-source 模块读取 |
| `config.capnp` | 指向 `dist/` 的模块路径，与嵌入及物化后的布局一致 |
| [构建输入准备](../scripts/workerd-archive.ts)、发行脚本、[CI](../.github/workflows/ci.yml) | 唯一正式 pin、资产前置构建及离线消费 |
| README、测试文档、AGENTS.md | 当前目录、命令、Git 跟踪规则及生成文件说明 |

仓库源码位于 `packages/runtime/`，物化后的内部资产统一使用 `runtime/workerd.lock.json`、
`runtime/config.capnp` 和 `runtime/dist/`；两者建立明确映射，不把仓库的 `packages/` 层级带入数据目录。
所有现行消费者统一使用新布局，不能保留 `system-workers/` 回退。当前源码链接仅用于标记
待改位置，实施迁移时同步更新。

## POC 删除与测试迁移

### 按断言用途处理

[当前 POC](../poc/README.md) 使用独立测试宿主、静态部署注册表和模拟绑定后端。
已完成的上游能力调查不再维护为常驻套件，也不将整套 G0 原型换个目录继续运行。

| 内容 | 处理 |
| --- | --- |
| stock workerd 启动、workerLoader、JSRPC、原生 facets 等已完成的可行性探测 | 删除探测代码和只服务这些探测的宿主、fixtures |
| G0 的 `F10/F11` 等原生 facet 行为窗口调查 | 保留既有结论和证据；删除一次性执行代码，不要求每次改产品重跑 |
| `scheduled/queue/workflow-unimplemented` 等旧原型分支断言 | 删除；它们不代表当前产品支持范围 |
| 已被 P0/P1/P2 产品测试等价覆盖的行为 | 合并到现有权威测试，删除重复测试及失去调用方的辅助代码 |
| 尚无等价覆盖、仍约束当前产品的安全、隔离、事务和恢复断言 | 保留并迁移，改为验证当前生产路径；不能继续以模拟注册表代替持久化权威 |
| 存续测试依赖的进程/HTTP harness、测试 Worker、故障或非法模块 fixtures | 一起移入 `test/`，只保留实际被使用的内容 |
| POC 独立 pin、下载器、重复 Reporter、历史三轮 aggregate 和报告生成器 | 删除或合并到现有测试设施，不保留第二份正式 pin 或旧 G0 入口 |

POC 测试和 fixtures 的迁移位置按领域组织，例如：

```text
test/runtime/
  loader/
  bindings/
  durable-objects/
  recovery/
  support/                  # 确有多个测试共同使用的 harness
  fixtures/<domain>/        # 保留下来的测试宿主与 Worker
```

这是存续仓库级测试的归属，不要求提前创建空目录。已有 Rust 单元/集成测试继续留在所属 crate，
runtime 包内测试随包迁至 `packages/runtime/tests/`，不为集中目录而复制这些测试。
若存续 JS 测试需要独立 package，纳入根 Bun workspace，不能引入第二份 lockfile。
测试、mock、故意损坏的 JS fixture 可以保持 JS，不为迁移而将非法语法样本修成有效源码。

保留 [docs/implemented/g0-results.md](implemented/g0-results.md) 等历史证据及已知限制，不手改为新的验收结果，
不因删除执行代码而删除 `.temp/` 中保留的失败现场。`D-abort` 的调查结论仍然约束设计：
不能将客户端断开当作保证取消执行的原语；当前产品所需的取消、流中断和恢复断言应由产品测试
承担，不要求保留旧失败用例、旧 allowlist runner 或自动生成原报告的机制。
未来 workerd pin 变更需要针对变化验证所依赖的行为，不能直接用旧调查结果证明新 pin，
也不因此恢复整个历史 POC 套件。

## 测试去重与耗时治理

### 已确认的入口问题

| 现状与证据 | 改造要求 |
| --- | --- |
| [P0.2](../test/test-p0-2.sh) 和 [P2.3](../test/test-p2-3.sh) 都运行完整 `p0_2_runtime_gate` | 按实际目标去重；需要拆分时按 Worker 与 Queue/Cron 领域拆分，不能保留两份相同用例 |
| [P2.1](../test/test-p2-1.sh) 调用 P1 和 G0；P1 经 P0 exit 继续串联 P0 子 Gate | 取消子 Gate 间递归调用，由一个顶层入口选择所需目标，每个目标每轮只执行一次 |
| [P0.1](../crates/service/tests/p0_1_gate.rs) 在 Rust 测试内写死三轮；部分 shell 入口也写死三轮 | 测试本体执行一轮，轮数只由顶层调度控制，禁止内外两层循环相乘 |
| 多个 POC suite 复制 Reporter 和用例清单 | 删除随 POC 退役的代码；存续测试复用现有报告机制，不另建通用测试框架 |
| [coverage](../test/coverage.sh) 运行 workspace，其中包含内部多轮进程测试 | 覆盖率只运行一轮所需测试并生成报告，不隐式触发三轮 Gate 或历史 aggregate |

### 执行规则

- 所有日常入口默认一轮。统一沿用 `OPEN_COMPUTE_GATE_ROUNDS`，目标接口只接受 `1` 和 `3`，
  非法值在执行前报错；最终验收必须显式选择三轮。只让顶层入口读取和控制轮数。
- 开发阶段按改动选择相关目标，每个目标只执行一次。没有源码或相关输入变化，不因换了一个
  aggregate 名称再次执行同一目标；修复后重新运行受影响目标。
- 最终阶段在同一冻结源码和输入上执行三轮独立的真实进程测试。每轮新进程、新隔离目录、
  无共享业务状态；同一场景内部的 restart/crash 必须复用该场景自己的持久化状态。
- format、lint、typecheck、workspace tests、构建、依赖边界和 coverage 各完成其所需检查，
  不塞进每轮 Gate。覆盖率插桩与未插桩进程验收用途不同，不能把 coverage 当三轮验收的替代。
- 首个失败停止无意义的后续轮次，保留实际执行记录与诊断；禁止自动重试直到变绿，不能把少于
  三轮成功写成最终验收通过。

以上是待实现的统一入口契约。当前不能把 `OPEN_COMPUTE_GATE_ROUNDS=1` 套到不读取它的脚本上
宣称单轮；实施前按 [现有单轮方法](references/testing.md) 直接运行可用目标。P0.1 尚无完整单轮入口，
该缺口必须明确记录，不能以相关单测冒充完整 Gate。

### 其他耗时来源

1. **构建和类型检查**：准备阶段一次安装锁定依赖、构建 runtime/toolchain、校验正式 runtime
   输入；复用同一源码、工具链、target/profile/features 的产物。消除 wrapper 与下层脚本的
   重复 typecheck/build，不在每个测试、每轮启动前重新编译整仓。
2. **缓存与重复下载**：复用有效依赖和编译缓存，不在日常检查前清空 `target/`、`node_modules/`
   或 runtime 缓存。输入缺失或摘要不符直接失败，不能隐式下载、自愈或把旧产物当新构建。
   coverage 自身所需的隔离或清理限于它拥有的产物，不扩大为全仓清理。
3. **重复进程和初始化**：在不改变隔离语义时复用只读夹具、已校验二进制和编译好的静态输入。
   数据目录锁、generation token、数据库、故障状态和需要冷启动的场景不能跨轮共享。
4. **等待**：用有截止时间的 readiness、事件或状态等待替代无依据的固定 sleep；记录超时所在
   阶段。真实超时、退避、过期和竞态测试保留所需时序，不靠缩短上限制造偶发通过。
5. **串行化**：无共享可变状态的单测可并行；进程、端口、环境变量、SQLite、网络夹具等测试
   先证明隔离再调整并发，不直接删除现有 `--test-threads=1`。
6. **矩阵和长任务**：按本次改动安排必要目标。soak、长 fuzz、load 和发行打包不作为每次
   开发迭代的前置条件；需要它们证明的改动仍应在对应验收中运行，不取消支持平台的必需覆盖。

实施时记录实际目标清单、调用次数、构建次数、进程启动次数、等待阶段及总耗时，再据此消除
瓶颈。优先使用现有日志，不为测耗时反复重跑完整三轮。未实测前不承诺提速比例。

## 实施顺序与验收

1. 迁移 runtime 包、生成目录和全部消费者，建立不依赖 Git 中 `dist/` 的构建链。
2. 逐项分类 POC 断言，迁移仍有价值且未被覆盖的测试及依赖，删除一次性探测、重复项和孤立代码。
3. 去掉 Gate 递归、固定三轮和重复目标，统一默认单轮/显式三轮入口及报告。
4. 同步 CI、workspace、开发文档和策略中的当前路径，验证实际执行次数与耗时。
5. 完成审查、冻结源码后，执行完整静态/单元/coverage 检查，最后执行相关三轮产品 Gate。

- [ ] `packages/runtime/dist/` 不受 Git 跟踪；无旧 runtime/system-workers 路径回退。
- [ ] 不含 `dist/` 的干净检出能完成显式构建；两次隔离生成的文件集合、字节和摘要一致。
- [ ] Rust/CI/发行构建均消费新位置的完整资产，生产启动仍离线且不依赖 JS 工具链。
- [ ] 一次性 POC 源码和入口移除；存续测试及 fixtures 归入 `test/`，无未使用辅助代码或第二份 pin。
- [ ] 每项删除都有“已完成的一次性探测”“被何处等价覆盖”或“旧模型已移除”的理由；
  当前安全、完整性、失败路径、restart/crash 覆盖不丢失。
- [ ] 默认 Gate 实际只跑一轮；显式最终验收恰好三轮，每轮各目标一次，没有递归或内部倍增。
- [ ] 完整静态检查、workspace tests 和 coverage 不随轮次重复；Rust 行覆盖率仍不低于 90.00%。
- [ ] 输出实际轮数、源码/输入身份、结果和失败诊断；历史报告与保留证据不被改写或删除。

本次文档变更仅执行 `git diff --check` 及路径、命令和状态核对，不运行 Rust/Gate、下载 runtime、
打包发行或清理现有数据。代码实施与验收完成情况记录到 [Day1 清理清单](day1-architecture-cleanup.md)。
