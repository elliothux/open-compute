---
title: "Python"
description: "使用 cf 和本机安装的 PyWrangler 构建 Python Worker。"
---

Cloudflare 于 2026 年 9 月 21 日宣布
[Python Workers 正式可用（GA）](https://blog.cloudflare.com/python-workers-ga/)。open-compute
只对本文列出的部署和库调用路径提供验收依据；上游 GA 不代表 open-compute 已实现全部兼容能力。

Cloudflare Python Workers 使用 `workers` SDK、`WorkerEntrypoint` class 和
`python_workers` compatibility flag。open-compute 已实现部署时的 Python 准备流程，固定的
Pyodide runtime 内嵌在 `ocd` 中；生产启动不下载 Python runtime，也不安装 package。

open-compute 支持普通 Python 部署，包括多模块 import、package data 和支持的
Pyodide/PyEmscripten 依赖。框架支持范围包括 Django、FastAPI 的 HTTP 与流式响应，以及
Flask 的普通 HTTP 和模板。准备后的部署在重启和 rollback 后保持身份与资源访问。

内嵌解释器 bundle 提供 Python runtime 字节；应用准备 snapshot 则另外保存部署初始化结果。
仅内嵌 Pyodide 并不消除应用 cold-start 工作。

公开上传接口当前只支持 Worker 自己拥有的 Durable Object namespace 和 Workflow binding。
跨 Script 绑定会被拒绝；Python 与 JavaScript Worker 分别使用各自拥有的 DO namespace 和 Workflow definition。

## HTTP 客户端

普通部署验收使用 PyWrangler 选择的原版 Pyodide `requests` 2.33.1 和 `httpx` 0.28.1 wheel。
requests、同步/异步 httpx 均通过受控 HTTP 服务验证 Unicode 请求和响应、status exception、
完整响应流、真实超时、连接拒绝、native subrequest 限额，以及失败、重启和 rollback 后的恢复。
所选 requests/urllib3 transport 将超时报为带 timeout cause 的 `ConnectionError`，
并非 `requests.Timeout`，调用方需处理这一异常形态。异步 httpx task cancellation 单独验证有界清理；取消 Python task 不保证底层 Fetch 立即 abort。
TLS、数据库驱动、上层 AI 客户端和 HTTP MCP 仍留在
[#128](https://github.com/elliothux/open-compute/issues/128)。

## 已知上游问题：Flask 流式响应

未修改的 runtime SDK 1.9.2 存在
[WSGI 上下文处理问题](https://github.com/cloudflare/workers-py/issues/287)：Flask 使用
`stream_with_context` 时，后续读取或清理可能丢失请求上下文并失败。官方原版 workerd 已独立复现，
不经过 open-compute daemon；尚未验证 Cloudflare 托管环境。依赖 request/app context 的 Flask
流式响应暂不在当前支持范围。普通 Flask HTTP 和模板在支持范围内，SDK 保持未修改；上游修复后重新验证此限制。

## Python 项目构建工具

在仓库根目录执行
`node scripts/build-python.ts /absolute/python-project`。项目需包含 `pyproject.toml`、声明
`type: module` 的 `package.json`，以及在 `cloudflare.config.ts` 中声明的 Python entrypoint。
entrypoint 与本地 package/data 需位于项目内部独立的应用目录，例如 `src/main.py`；
桥接拒绝根目录或项目外入口，避免将开发文件纳入 bundle。

构建脚本调用用户自己安装的 PyWrangler（`workers-py`），优先查找项目
`.venv/bin/pywrangler`，其次查找 PATH。未安装就报错并提示 `uv tool install workers-py`；
不内置、不自动安装、不校验其版本。用户将 Wrangler 安装为
Python 项目的本地开发依赖。PyWrangler 准备依赖，仅在本地构建时调用 Wrangler。它要求的构建解释器
也需由用户显式准备；桥接禁止隐式下载 Python 解释器或 Wrangler。将这个命令接入项目的 build
脚本，由用户或 CI 启动构建，脚本内部调用 PyWrangler。

`cloudflare.config.ts` 始终是唯一的 Worker 配置。桥接生成并删除两个临时 builder 输入，
不会覆盖已有 Wrangler 文件。上游工具生成 `.cloudflare/output/v0/`，包括未修改的 SDK
和本地 JSON/HTML package data。认证、资源管理、上传和部署仍使用 cf，上传/部署必须使用
`--prebuilt`。daemon 不安装或构建 Python 依赖。

静态资源在同一构建命令后添加 `--assets-directory public`；目录必须位于 Python 项目内部，
且不能直接使用项目根目录。这个参数只选择交给上游 builder 的文件；binding 仍通过
`cloudflare.config.ts` 的 `bindings.assets()` 声明，路由仍由 `worker.assets` 控制，
不会将 Worker 配置移入 Wrangler 文件。普通 runtime 用例已对照 Assets 的 SDK/FFI/JavaScript 响应，并验证 promotion、restart 和 rollback 后的不可变 asset 字节。

官方 cf Python builder 能生成等价且通过资格验证的 Build Output 后，将删除这项临时的
Python-only PyWrangler 桥接及临时 Wrangler 输入，切换时只保留一条构建路径。

## Dynamic Python 限制

在请求时通过 [Dynamic Workers](/zh/docs/workers/runtime-apis/bindings/#dynamic-workers)
创建的 Python child 尚未取得使用资格。新 isolate 的 Pyodide 初始化必须满足现有 1 秒 startup
CPU 限额；open-compute 不提高该限额，也不把普通部署的 prepared artifact 用于 Dynamic child。
缓存命中或普通部署准备成功都不能作为这条路径通过验收的证明。
限制 `OC-WKR-LIMIT-001` 见[行为差异](/zh/docs/platform/deviations/)。
当前正式 pin 基线与剩余实现工作见
[Python 实现记录](https://github.com/elliothux/open-compute/blob/main/docs/implemented/p21-python-workers.md)
和 [#126](https://github.com/elliothux/open-compute/issues/126)。

## TCP 验收边界

上游 Python socket bridge 使用既有的 Workers `cloudflare:sockets.connect()` capability。
普通 Python fixture 验证的是通过 JavaScript FFI 调用 native TCP；Python 数据库驱动的
socket/asyncio 和 TLS 验收仍留在 [#128](https://github.com/elliothux/open-compute/issues/128)。
这不提供 inbound TCP listener、Hyperdrive 或跨请求 connection pool。Outbound IP 访问遵循
宿主网络与 operator 负责的过滤规则，见[行为差异](/zh/docs/platform/deviations/)。

Cloudflare 参考：[Python Workers](https://developers.cloudflare.com/workers/languages/python/)。
