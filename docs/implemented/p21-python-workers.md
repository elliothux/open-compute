# P21：Cloudflare Python Workers

状态：已完成（2026-10-05）。普通 Python Worker 使用 workerd 官方 Python entrypoint、内置 Pyodide 和未修改的 `workers-runtime-sdk`。Python 与 JavaScript 共用 binding materializer、资源权限和持久化 authority。

## 用户入口与构建边界

维护配置为 `cloudflare.config.ts`，认证、上传、部署和资源管理使用 cf。当前 cf 缺少可用的 Python package builder，Python 开发构建临时由 `scripts/build-python.ts` 调用用户安装的 PyWrangler。优先选择项目 `.venv/bin/pywrangler`，再查找 PATH；未安装则明确报错，不安装或校验版本。开发者显式准备 PyWrangler 所需的依赖与构建解释器，cf 消费生成的 Build Output。

临时例外只限 Python 构建，等待 cf 原生 Python builder 后删除桥接代码的 TODO 留在活动后续方案和用户文档。正式 `ocd` 启动与请求不运行构建工具、不联网下载 runtime 或安装 package。

## 唯一运行链路

上传 admission 验证不可变模块、compatibility metadata 和 binding descriptor。部署前 coordinator 启动独立受监督的 preparation child；它复用现有进程组、就绪、停止、reap 和 orphan recovery 的 ownership，使用独立 lease，不开 DO storage。生成 snapshot 后先确认 child 退出，再加密并提交 prepared artifact。

`control.sqlite` 保存 Version、artifact identity 与资源 authority；对象存储保存加密不可变产物。加载时验证 Version/module/runtime identity、内容 digest 与绑定部署身份的 AEAD。promotion、rollback 和 restart 选择既有不可变内容；损坏、身份不匹配或过期 generation 失败关闭，不在读取时修复或重新构建。secret rotation 与 cancellation/deadline fencing 沿用平台 authority。

KV、R2、Queue、DO 和 Service 使用原生 workerd public objects；D1 使用上游 binding，其他声明产品使用现有 wrapped binding。窄协议 adapter 接入既有 SQLite、对象存储与资源后端，PRIVATE_POLICY、内部 transport/token 不进入租户 `env`。旧 public facade 和 Queue 旧序列化路径已删除。当前官方 compatibility date/flag 的行为直接由正式 runtime 合同决定。

## 当前输入

- 正式 workerd release：`v1.20260930.0-open-compute-r4.e98a3e843`；source `e98a3e8433979356047a202d3e0d1b0e2e2c4b8c`，upstream base `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`。
- Pyodide：`314.0.6_2026-08-17_6`；SDK：未修改的 `workers-runtime-sdk` 1.9.2。
- 开发配置与上传 CLI：cf `1.0.0-beta.12`；Vite plugin `2.0.0-beta.sha-52b0dc0e9`。固定租户类型仍为 `@cloudflare/workers-types@5.20260830.1`。
- 唯一 formal pin 为 [`workerd.lock.json`](../../packages/runtime/workerd.lock.json)。逐目标 archive/binary digest、source/build inputs、Python bundle/license 输入由 lock、release catalog 与构建校验拥有。

四平台 workerd release 复用 [run 37175389986](https://github.com/elliothux/workerd/actions/runs/37175389986) 的已有产物，publish 修正没有重跑 native build。workflow-only commit `fdcb8c97f0b9f75f610b66e418d5175b9188ddad` 不冒充 binary source。10 项 release asset 已校验，workerd 的其他 CI 已停用，保留手动 publish。当前主机 single-binary Gate 证明内嵌输入和离线首次启动；此次没有额外发布 open-compute 平台安装包，也不据此宣称其他主机的 daemon 验收。

## 已接受的限制

公开上传仍拒绝 cross-Script DO 和 Workflow binding。Python/JavaScript 的资格测试分别调用各自 namespace/definition，不把它们写成相互绑定同一个对象。Workflow definition 的 current Version 独立于 HTTP deployment；rollback 后通过公开 `PUT /workflows/{name}` 重绑 definition，旧实例继续使用其固定 Version。Queue 的 force 只授权 backlog purge，不绕过活跃引用保护。

Cache API 保持 Worker 级共享可变状态；promote 和 rollback 不回退缓存内容。默认与命名空间、不同 Worker 继续隔离，自动 Workers Cache 的默认版本隔离独立验证。

Assets 使用现有 private wrapped binding，公开名称为 `Fetcher`，让官方 SDK 选择其既有 fetch 包装器并转换 Python 请求头。Workflow 内部 suspension/rollback 信号使用已有脱敏 Error 类型，跨 Python FFI 兑现 rejected promise，仍用私有 identity 和 controller verdict 决定中断。两处修复均复用现有运行链路。

自动缓存与原生 Cache API 共用 workerd 的 Response 序列化和同一个持久化后端。Cache HTTP adapter 在元数据入口删除 hop-by-hop 和 Connection 指定的字段，保留原样 encoded body、作用域和 purge/refresh fence；storage 继续严格拒绝传输头，不接受特殊修复或测试分支。

原生 R2 延续 upstream 的已知长度流上传合同：定长分块流可写入，未知长度流抛出 `TypeError` 且不产生对象。D1 binding 延续上游 WrappedBinding 的序列化能力，父 Worker 的持久数据可由显式接收该 binding 的子 Worker 读取，子 `env` 仍仅包含声明的键；原生 KV/R2/Queue 的直接 transfer 拒绝保持。测试使用当前原生 public objects，未恢复已删除的 facade。

Flask 带 request/app context 的流式响应受 [cloudflare/workers-py#287](https://github.com/cloudflare/workers-py/issues/287) 影响。用户于 2026-10-04 明确要求记录并暂缓。SDK 保持原版，普通 Flask 用例验证 HTTP、template、HEAD/status/query 与 secret。Main 用例集中验证 restart、rollback 和 prepared artifact 完整性。该流式路由不计为通过；Django 和 FastAPI 的流式验收保留。原始失败与诊断证据保留，待上游修复后重验 body、并发上下文和 cancellation cleanup。

Dynamic Python 的 fresh isolate 仍受 `OC-WKR-LIMIT-001` 的一秒 startup 限制，未声明支持；普通部署 prepared artifact 的通过不能推出 Dynamic Python 通过。完整支持面由 [兼容矩阵](../references/cloudflare-compatibility.md)、[偏差](../references/p1-deviations.md)、机器可读合同与用户文档拥有。

## 验收证据

2026-10-05 的完整 workspace coverage（`20261005T063253-76fd44a7`）执行 61 个进程、1771 个 case，零 ignored，全部通过；行覆盖率为 149835/165886（90.3241%）。精确 66 个对象、139 份 profiles 与报告保留在 `.temp/coverage/run-iDjTkgN5/`，未合并历史输入或改变阈值。174 条 LLVM mapping 警告经逐条诊断均为 hash=0 的依赖/标准库记录，没有项目符号。

修正测试 HTTP 服务的 TCP half-close 配置并完成五种 Clippy 配置、格式、source policy、baseline identity 和 CF 复核后，冻结源码运行最终普通 workspace Gate（`20261005T092248-6ab16d3d`）：61/61 进程、1771/1771 case、零 ignored、单轮全部通过。环境为 Rust 1.98，`RUSTFLAGS=-D warnings -C debuginfo=0`、`CARGO_INCREMENTAL=0`；保留默认调试断言、优化和全部案例，没有 profiling instrumentation。

覆盖率源码身份为 `f2be50333229f6bc84baef88d608907102b423737fccc0a64ddf3a14a269a97d`，最终 conformance 源码身份为 `2d208bffed6f587c1dfd8bfa0f3bbd2b97fe9b66e795e774910c4e46a0f0db78`；最终 Gate 的输入摘要为 `de24f357e34186a1183ec8fc9bfad135137779f5e61badf9a42ecbe4aa35a1d5`。覆盖率后仅更新文档、baseline 和两个 Rust 测试 fixture；生产代码未变化。新 TCP fixture 经 focused 和最终完整 Gate 验证，未把原覆盖率写成新 fixture 的覆盖率。

机器可读平台合同仍为 `incomplete`：12 项通过、9 项包含未运行的 hosted differential、1 项明确不支持、0 failed、0 blocked。此次完成当前声明范围的普通 Python 验收，不把本地通过写成完整 hosted Cloudflare 资格。

P21 注册七个 target、九个 case，覆盖普通 Main、三种 framework、Services、Queues、DO、Workflow 和 runtime/FFI/stdlib/outbound/observability，并保留已有产品权限、失败与恢复 Gate。编译、source review 或一次性 native probe 都不冒充普通 daemon 执行。

新增的 Python preparation、加密与 artifact authority 六个 Rust 模块在本轮完整覆盖率中分别达到 91.92%–97.53% 行覆盖率；coordinator 为 331/344（96.22%），native preparation owner 为 95/98（96.94%）。这些数字来自同一轮精确对象与 profiles，不合并历史输入。

CF 复核对照固定的官方 types、cf producer、SDK 与 formal workerd source，逐项记录 source、owning checks 和普通 Gate 的证据边界。此轮不声称完成 hosted Cloudflare differential、全球调度或全部上游 overload 资格。原始失败、修复验证与新 run 输入保留于 `.temp/p21-final-preflight/`、`.temp/gate-run/`、`.temp/coverage/` 和 `.temp/p21-workerd-release-r4/`。

## #129 Dynamic 基线与 #128 HTTP 子项

2026-10-05 scoped Gate `20261005T123536-25820dff`（`p21-python-runtime`）通过。

- **Dynamic Python（#129）**：raw Dynamic 路径在三个阶段（fresh process、普通 preparation 后、restart 后）各调用 4 次，12 次全部触发
  `Worker exceeded CPU time limit.`，1 秒 startup CPU 限额不变。失败初始化不会形成 warmed child。Dynamic Python 仍不可用于生产，
  后续实现见 [#126](https://github.com/elliothux/open-compute/issues/126)。
- **HTTP 客户端（#128 子项）**：原版 requests 2.33.1 和 httpx 0.28.1（sync/async）在首次部署、重启和 rollback 后通过 Unicode、错误码、
  流式读取、真实超时、连接拒绝、取消和 subrequest 限额断言。requests 的超时对外是 `ConnectionError`，不是 `requests.Timeout`。
  AI/API client、HTTP MCP、数据库驱动和 TLS 仍属于 [#128](https://github.com/elliothux/open-compute/issues/128)。
- Flask context streaming（workers-py#287）保持已接受的上游限制。
