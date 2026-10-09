# P22：Cloudflare Browser Run

状态：**implemented / verified（2026-10-08）**。

## 用户结果与支持面

Browser binding 支持 operator 配置的 external CDP 或 managed browser，两者互斥。
固定 cf 配置、Build Output/upload、session/browser/Live View，固定 Puppeteer/Playwright，
九项 Quick Actions、DevTools JSON/CDP WebSocket、Dashboard binding 与 Custom AI 已实现。
声明范围仍是固定客户端的实际调用路径和逐 route fixture，不把全部 Chrome CDP 或
Browser stable members 自动声明为支持。官方合同冲突与未资格化行为统一由
[Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md#browser-run)记录，
安装、配置与当前命令由[运维指南](../references/runbooks/install-and-first-start.md)拥有。

## 已接受的验收限制

以下两项于 2026-10-08 经用户明确接受，状态为 **acceptance / accepted limitation**，
不再阻塞 P22 交付；接受限制不代表这些行为已经与当前 CF 线上服务逐项对照一致。

- Worker Download 字节交付：固定 Playwright 的 `path()` 返回路径，但 Worker 无法读取
  Chrome 下载文件；`saveAs()` 报文件不存在，`createReadStream()` 返回空内容。
  下载策略、事件与元数据已通过，文件内容接口仍不支持。
- `connectionStartTime` 类型：binding 统一返回 string，符合固定 Puppeteer 声明，
  与固定 Playwright 的 number 声明存在差异；公开 v4 API 返回 number。
  接受这一类型差异，不增加客户端识别分支。

原因、固定版本与 CF 证据边界由[兼容矩阵](../references/cloudflare-compatibility.md#browser-run)维护。

## 架构与边界

- `core` 拥有显式 backend、容量、deadline、idle 与资源配置；`runtime` 拥有 verified
  browser 资源、private pipe/CDP、进程组与 session/context 生命周期；`storage` 拥有 SQLite
  session history；`service` 组合 admission、binding、public API、鉴权、Quick Actions 与 metrics。
- `ocd` 是唯一平台公开入口。instance、binding、session、target 与 generation 相互约束，
  内部 endpoint、token、profile、宿主路径不进入 tenant response 或日志。
- managed 每 instance 按需启动一个 browser process group，每 session 使用独立临时 context。
  single-start、取消、deadline、warm-idle、stop/admission、crash/restart、orphan 与 reap
  共用所属 runtime authority；关闭 session 不关闭其他 session。
- external CDP 保留原生浏览器行为，managed 的 idle stop 不接管其进程生命周期。
- managed 保留 Chrome 原生 sandbox；不增加 bubblewrap/Seatbelt 等外层 sandbox，也没有
  `--no-sandbox` fallback。CDP 边界拒绝宿主任意文件路径能力，下载使用 session-owned
  临时目录；间接本地文件访问有真实浏览器回归。Chrome 内部 sandbox 实现按用户要求不另行验收。
- 浏览器 IP 出站由 operator 的宿主网络策略管理，没有 public-only 地址限制。
  平台 listener 仍独立鉴权；不承诺 Chrome 主进程被攻破后的 OS 文件隔离。
- 生产启动离线：operator 提供明确 executable/resources/version/capability；没有 embedded
  browser、PATH discovery、自动下载、Miniflare 或旧实现 fallback。
- `/json` 的 `custom_ai` 复用现有 Provider 客户端与 generation alias catalog，最多三项候选，
  按顺序 fallback，并共享一个总 deadline；凭据和上游错误按现有秘密边界处理。
- bounded metrics 已实现；history 仅记录 terminal sessions，容量上限 100,000、TTL 上限一年，
  maintenance 与 restart 共同执行持久化预算。P7 Browser tail 与 P9 Browser 独立 CPU/memory
  accounting 仍为 planned，duration 不冒充 CPU。
- 当前 Day1 模型只有一个 authoritative implementation；没有旧 schema/config/runtime 兼容路径。
  Browser 能力复用正式 workerd pin，没有 Browser 专用 fork patch 或自定义客户端。

## 验收输入与已完成检查

基于 Git `17831f73a2a70f2fa990cdc3438dacf2143cf922` 的工作树，正式 workerd
`v1.20260930.0-open-compute-r4.e98a3e843`；chrome-headless-shell `153.0.8010.12`
覆盖 external CDP 与 managed 场景。固定 Puppeteer `1.4.0`、Playwright `1.3.6`、
cf `1.0.0-beta.12`、Cloudflare SDK `7.2.0`。Docker 仅作为 Linux 验证环境。

- Bun build、strict TS7、生成资产、source policy、依赖边界、Rust/JS 格式、Oxlint、Knip，
  canonical Clippy、no-default-features、Rust 1.98 all-targets 与 metadata 已通过。
- Dashboard 两个 unit tests（49 assertions）和一个真实 UI 场景通过。
- 10 分钟真实持续请求：600,216 ms、2,452 请求；live session 清零，历史预算和临时目录清理通过。
- Cloudflare differential：九项常见 Quick Actions 与一个 owned create/close session 完成，
  临时 session 已清理。它证明观察到的常见路径，不代表完整协议资格认证。
- 一个完整 coverage round：62 个目标、1,823 项、0 ignored；155,004/171,380 行，
  **90.44462597736025%**。原始 LLVM export 有 204 个 function mismatch warnings，
  已在报告核对；没有增加生产排除、覆盖率专用分支或放宽断言。
- source-28 与该 coverage 的 680 个实测生产 Rust 文件及 coverage 配置逐字节一致；
  复用 unchanged production/static configurations。仅 runtime TERM fixture 就绪顺序改变：
  先启动子进程，再发布 PID，再 builtin wait；原有 deadline、marker、进程组退出与 reap
  断言不变。该 crate 的 Clippy 与全部 130 个 library tests 通过，0 ignored，格式检查通过。

覆盖率、生产输入一致性与授权 differential 摘要：
`.temp/p22-linux/browser-source21-coverage-qualification.json`、
`.temp/p22-linux/browser-source28-production-identity-proof.json`、
`.temp/p22-cf-readonly/approved-differential-assessment-20261007-1.json`。

## 最终验收

`./test/gate.py --workspace --final --keep-going --jobs 4` 成功退出：
**62 个目标、1,823 项、0 ignored，一个完整 round，3,667.38 秒**。
Browser Run 三项真实场景 171.86 秒；runtime library 130 项和 service library 843 项全部通过。
该轮验证 external CDP、managed Puppeteer/Playwright、固定 cf/SDK、WebSocket、restart 与
现有平台回归；没有重复执行 unchanged coverage 或静态构建配置。

Gate 源码 SHA-256：`d4ad933020b5ea6ce810399fbf72357b747aa86a4d3b73949051486966adea6e`；
conformance source digest：`7d52a05fcccb35d49cba9135cd24d639d43cddde91bcfbdf05a2254cc676d576`。
当前宿主与只读 Docker 输入逐文件匹配，680 个实测生产 Rust 文件与 coverage 输入一致。
简短报告位于 `.temp/gate-run/20261007T214310-694958f6/report.json`；
原有 15 项完成标准的验收映射位于 `.temp/p22-linux/browser-source28-requirements-audit.json`。

全平台 conformance 汇总仍为 `no_go/not_qualified`：12 passed、0 failed、9 not_run、
1 unsupported、1 blocked。常见 Browser 路径通过不意味着完整 Cloudflare 协议资格认证；
该次验收后的边界修正见下节。Worker Download 字节交付、全部 CDP/options/errors 与
上游 timestamp 类型冲突由[兼容矩阵](../references/cloudflare-compatibility.md#browser-run)维护。

## 后续边界修正（2026-10-08）

- `browser.public_origin` 由 operator 配置并严格校验；public Browser create、DevTools JSON、
  CDP Live View 共享 URL 投影，HTTPS 生成 WSS。Live View URL 显式包含 account；
  Host/Forwarded 不参与 authority，TLS 反向代理配置由运维指南说明。
- Cloudflare 已认证只读 GraphQL schema 明确给出 `0 / Unknown`。lost history 采用该值，
  保留记录及 end time，整页不再返回 501；不猜测失联的具体原因。
- 两个固定客户端的 sessions 原始 JSON 已对照：共同收到 string `connectionStartTime`，
  Playwright 的 number 声明仍属于上游类型冲突，未新增客户端识别或转译分支。
- chrome-headless-shell 下验证 Console 实际日志及 Network/Elements 面板打开与选中。
  fixture 在 iframe 自己的 CDP execution context 使用固定 DevTools 模块，保持原生 CSP。
- CDP 和 managed 的 Playwright fixture 在一个 browser lifecycle 中检查默认/true/false
  下载策略与元数据，再关闭 browser。external CDP 保留 native close 语义，fixture 不在
  关闭外部进程之后重复连接，也不通过生产重启兜底掩盖这个生命周期。
- 下载事件与字节交付分别验证。固定 Playwright 的 Worker VFS 路径与宿主 Chrome 文件
  没有字节桥接：读文件与 saveAs 报不存在，createReadStream 为空。字节接口明确 unsupported，
  不把下载事件成功计为文件可读，也未引入定制客户端或私有传输协议。

定向验证收集了 3 项 core 配置测试、21 项 Browser service 测试和 3 项真实产品场景。
缺少 fixture 环境及 history 顺序断言已集中修正，4 项失败单元测试重跑通过；
managed Puppeteer 的通过结果保留，另外两个受影响产品场景最终共同通过（277.33 秒）。
生产变化限定在 4 个 Rust 文件，strict TS7、fixture build、格式、依赖边界及
no-default-features 检查已通过。该定向结果不冒充新的完整 workspace 验收。

验证流程、重复最终 Gate 防护、32 GiB 任务磁盘预算、共享 MBX 软预算与清理生命周期
已落实到 [AGENTS.md](../../AGENTS.md)、Gate runner 和[测试规范](../references/testing.md)。
已复盘的原始失败记录、profiles、旧快照与可执行文件副本直接清理，只保留必要摘要；
本任务的 Docker 容器和卷在验收后删除，不执行全局 prune。

原最终验收阶段的清理结果：本任务的源码、运行数据、构建和闲置 Linux MBX 缓存卷，以及验证镜像均已删除。
native GC 保留的 23.7 GiB recent state 在确认缓存卷无容器引用后，按用户授权一并清理。
其他任务的三个卷、三个镜像和运行中的 daemon 未触碰；当时 Docker 总卷约 6.444 GB、
镜像约 2.68 GB。本任务仅保留一套 verified host/Linux runtime inputs 和简短验证摘要。

## 后续修正的完整覆盖率与验收（2026-10-08）

完整插桩执行收集 62 个目标、1,835 项。60 个目标通过；两项失败集中处理：
旧 staging cleanup 测试将允许清理的普通资源文件误作拒绝对象，改为验证嵌套目录；
P0.2 单例诊断未复现原失败，其预算测试改用已有本地 HTTP fixture，移除外部 DNS 变量。
原有 CPU、邻居隔离、预算、generation restart 与 reaping 断言保留。
清理矩阵 1 项与 P0.2 整组 3 项定向通过，0 ignored；相关测试 Clippy 和格式检查通过。

没有重跑完整覆盖率执行。680 个实测生产 Rust 文件、Cargo.toml/Cargo.lock 和覆盖率规则
在测试修正前后逐字节一致，复用本轮冻结对象及 profiles，并执行原 canonical report phase。
覆盖率为 **155,423 / 171,887 行，90.42161420002675%**，门槛仍为 90%。
两次 export 各有 302 个 LLVM warnings；对全部冻结对象的 debug dump 逐项核对后，
均为缺少记录的零哈希依赖函数，未涉及 workspace/ocd 生产符号。编译器与报告工具均为
Rust 1.98.0 / LLVM 22.1.8，compiler commit 一致。

报告核对完成后删除约 12 GiB 插桩 target 与 12.23 GiB 原始对象/profiles，
已复盘的 DevTools profile 和旧诊断日志也已删除；保留标准 coverage summary/lcov 与精简证据。
未新增 Docker 容器或卷。阶段 GC 后共享 MBX 对象约 8.0 GiB，另有约 10.8 MiB
受保护状态；这是实测软预算结果，不宣称硬上限。

最终未插桩 `./test/gate.py --workspace --final --keep-going --jobs 4` 已通过：
**62 个目标、1,835 项、0 ignored，一个 round，5,529.45 秒**。
Browser Run 三个真实场景 322.36 秒，runtime library、P0.2 和全部平台回归通过。
精简报告：`.temp/gate-run/20261008T140304-80fc79f7/report.json`。
冻结 Gate source SHA-256：`374e49adf94b9d281ba3ac19b10c411b7e2e96c89975162e46ac81eba081dff1`；
conformance digest：`36a2907ce6c59d6c80d1ade17f14305ff29c793d1f187f1dc2796eaf265dc12f`。
覆盖率及 CF 范围摘要位于 `.temp/p22-completion/coverage-qualification.json` 和
`.temp/p22-completion/cf-review.json`；标准报告为 `target/llvm-cov/summary.json`。

最终收尾已复盘并清理本轮 Browser fixture、原始 Gate 日志和编译中间文件。
普通 target 约 8.6 GiB；共享 MBX 约 7.8 GiB，另有约 80.2 MiB 受保护状态。
本次没有新增 Docker 容器/卷，其他任务资源未触碰。最终状态与清理摘要：
`.temp/p22-completion/final-acceptance.json`。

## 上游同步（2026-10-08）

合并远端 main `26481829` 的 CI、容器示例与 Gate 工具修正；验收基线按合并后的源码重新计算。
固定 coverage 的 680 个生产 Rust 文件及三项配置逐字节不变，Live View HTML 保持已验收字节。
Gate runner 46 项单元测试通过；release tooling 收集的 20 项中，一条旧 Gate 命令断言
更新为包含 `--final` 后定向重跑通过。五项受影响 conformance 检查、strict TS7、格式、
文档检查和容器 shell 语法检查通过。复用上述完整 Gate 与覆盖率，不重复执行 workspace 验收。
本次同步未重新构建或运行上游容器镜像。
