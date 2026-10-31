# P20：Cloudflare CLI 单轨迁移

状态：implemented / Day1（2026-10-03）。正式入口为 `ocd cf`，应用使用官方 cf、Cloudflare Vite 插件 v2 与 Cloudflare Build Output。自有通用 Worker 应用工具链、Wrangler launcher 和旧配置消费路径已删除。

2026-10-04 的临时 Python 构建例外由 [P25 构建桥接移除待办](../p25-platform-follow-ups.md) 管理：
构建脚本调用用户安装的 PyWrangler，不恢复通用 Wrangler launcher；cf 继续负责上传、部署、认证和资源管理。

## 当前实现与边界

- 用户无需固定 cf CLI 版本。标准 SemVer `1.0.x`（含 prerelease）不警告；其他 major/minor 在 stderr warning 后仍执行。内部 CI 与 capability 基线固定 `cf@1.0.0-beta.12`、Vite `8.3.0`、Cloudflare Vite 插件 `2.0.0-beta.sha-52b0dc0e9`。
- `ocd cf` 解析 instance/target，定位项目或 workspace hoisted `node_modules/.bin/cf`，校验 Node `22.18+`，注入选定 account/token/API base，并透传原始 argv、TTY、信号、PID 与退出码。没有下载、全局 CLI、Wrangler 或版本专用 fallback。
- 上传命令做有限文件存在性预检，不执行 TypeScript 配置解析或猜测 binding。旧 Wrangler 配置给出 `cf migrate <exact-file> --bundler vite` 指引；真正 prebuilt 和 help/资源命令保持可用。布尔标志、选项值和 `--` 有独立回归。
- cf 和官方插件拥有配置、mode、类型生成、应用 bundling、assets 与模块上传。严格 TypeScript 检查由项目/CI 执行；Rolldown 仅继续拥有平台 runtime/system assets。daemon 启动保持离线。
- Hello Worker、Postgres、官网和 Vinext fixture 使用当前 `cloudflare.config.ts`。官网与 Vinext 分开检查浏览器和 Worker 类型；Worker 消费官方生成的 Env，不手写标准 binding 接口。
- 上传入口接收当前 cf 的 Workflow export 和同 Script 引用，并规范化 Vite 分片模块的 `./` 前缀；跨 Script 引用、路径穿越与重复模块名仍拒绝。
- cf 对已有 Worker 的 redeploy 携带非版本化 observability，随后 PATCH script-settings。上传入口验证该字段；纯 Version POST 保持现有 Script 策略与活动 Deployment。版本和 Service 元数据统一使用 `open-compute` producer。
- 现有 v4 产品接口继续复用 SQLite 与不可变 Version/Deployment authority。新版 Worker 删除与版本 GET/list 入口按 account 下的 name/public ID 解析到既有 authority，版本支持分页与校验后的模块读取，保留鉴权、引用、drain 与清理边界。
- API/capability/SDK、上游 pin/刷新工具、真实 cf 测试和 portable differential runner 同步切换。服务端不根据客户端品牌选择实现，不维护双 schema/read/write 或历史 alias。

## Containers producer 实施

[P23](p23-cloudflare-containers.md) 使用此处固定的 cf、官方 Vite 插件和 `@cloudflare/containers`。
Container application 和 rollout 的 producer fixture 已冻结；真实 cf deploy subprocess 与 runtime/provider 验收仍待完成。
应用使用标准 `cloudflare.config.ts`，不增加自有 Container 构建工具或 runtime pull 路径。

## 验收证据

验收输入为正式 workerd `v1.20260930.0-open-compute-r3.e3bdb07f5`（revision `e3bdb07f52affc6a618f02ed2b731a581b0b2f69`，maximum date `2026-10-07`）与正式 Caddy `2.11.4-open-compute.2`；未修改 runtime locks。所有 real-runtime Gates 使用校验后的 pinned binary。

- `bun run build`、严格 TypeScript、JS 测试（268 项）、format、dependency boundaries、Cargo metadata、no-default-features、Rust 1.98 MSRV 与 canonical Clippy 全部通过；Gate runner 的 30 项单元测试通过。
- `OPEN_COMPUTE_COVERAGE_HTML=0 ./test/coverage.sh`：54 个目标，1,718/1,718 用例通过，0 ignored，行覆盖率 **90.09%**（148,083 / 164,367，门槛 90.00%）。报告：`.temp/gate-run/20261003T034111-6d090389/report.json`，覆盖输出 `target/llvm-cov/{lcov.info,summary.json}`。LLVM 合并报告时输出 158 个 function data mismatch warning；正式 summary 与原有门槛均保留，未排除生产源或弱化断言。
- 最终未插桩 workspace Gate：`./test/gate.py --workspace` 单轮通过，54 个目标，1,718/1,718 用例通过，0 ignored，耗时 1639.06 s。报告：`.temp/gate-run/20261003T041727-4a130abd/report.json`；与覆盖率轮次的源码 SHA-256 相同：`b251a42d0fb21855a3a42dd0c717d347a4182f31a8e57101fa79837a5e7d8bb6`。

提交钩子随后格式化了文档；conformance baseline 的 sourceDigest 已同步到格式化后的工作树并核对一致。上述报告保留实际验收时的源码摘要，未因文档格式化重复运行 Gate。

真实 cf 测试保留资源 CRUD、secret 更新/删除/继承、首次部署、纯 Version 上传、激活/rollback、DO SQLite/Workflow/Loader、target/mode 隔离、错误 target/account 拒绝、活动部署保持和 daemon PID 稳定。非交互删除同时查询最终不存在状态，不以退出码 0 代替结果。CLI 进程测试覆盖原始 argv、中文/空参数、环境清理、hoisted 安装、退出码与信号。

失败/中止证据保留于 `.temp/gate-run/failed/` 与 `.temp/p20-inputs/`。验收发现的旧断言、官方 cf 参数/响应形状、分片模块命名、同 Script 引用与 redeploy 元数据问题均保留失败记录并在修复后复查；中止或失败轮次不计为验收通过。完整覆盖率运行还发现 cf/Node 编译缓存残留；Gate 统一关闭 native/typed 子进程的 Node/Bun 编译缓存，保持原有残留审计，修复后的 P0/P6 Gate 无残留通过。

## 维护入口

当前操作说明在 [应用示例](../../examples/hello-worker/cloudflare.config.ts)、[测试规则](../references/testing.md) 与 [上游刷新](../references/cloudflare-upstream-refresh.md)；后续语言/产品仍按各自阶段推进。旧入口移除不会修改历史验收报告或正式数据库 migration 字节。
