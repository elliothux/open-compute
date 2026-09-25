# I102：Dynamic Worker 显式资源 Binding 转发

状态：**implemented for the declared first matrix**（2026-09-22）。当前声明支持 KV、D1、R2、Queue producer 和 ordinary values；扩展产品资格见 [I102 验收计划](../acceptance/i102-dynamic-worker-binding-forwarding-acceptance.md)。对应 [#102](https://github.com/elliothux/open-compute/issues/102)。

## 用户结果

宿主 Worker 可以从 `open-compute:worker-loader` 调用 `getWorker`／`loadWorker`，显式选择自己持有的受支持根 binding，并让 Dynamic Worker 使用等价产品 API 和相同资源 authority。标准 Loader 合同没有被放宽：通过普通 `WorkerCode.env` 传递 D1/KV/R2/Queue 根对象仍抛 `DataCloneError`。

## 隔离与生命周期

正式 workerd fork 提供平台私有的 `openComputePrivateEnv` 和 host-issued Loader grant。原始 transport 只进入 handler-only capability table；`cloudflare:workers.env`、`node:process.env`、租户模块、argv 和日志都不可见。租户不能登记伪造根对象、覆盖 `open-compute:` 模块、跨 Loader/instance 借用 grant，或通过修改 JS 原型截获平台关联表。

`getWorker` ID 在同一来源版本内必须唯一对应代码、兼容配置、普通 env、binding 快照和权限。平台 namespace key 还包含来源 Worker version、route generation 与代码 digest。部署切换、设置变更和 Worker 删除会撤销旧 generation/prefix；不确定的撤销或后续提交失败会轮换 workerd generation，旧 key 不会重新开放。

子 Worker 只获得被选中的根 facade；底层 transport 继续执行资源归属、referrer、live version 和 Worker 删除授权。当前扩展只声明默认入口，不允许具名 entrypoint 绕过 wrapper。

## 验证记录

正式 fork revision `1c7b89bea323a39a8511271913820f9fcf39306d` 的四平台 workflow `35592596362` 通过。`p6-wrangler-resources` 在正式 `darwin-arm64` pin 上完成 KV、D1、R2、Queue 调用、标准 Loader `DataCloneError` 对照、重启读取和撤销回归。2026-09-22 的完整 53-target coverage Gate 与最终非插桩 Gate 通过，记录位于 `.temp/gate-run/20260922T085838-77f1bb0e/` 和 `.temp/gate-run/20260922T092511-751a59fb/`。

这项转发是 open-compute 扩展，不声明为 Cloudflare 标准 Loader 能力。没有真实产品矩阵证据的已装配 facade 仍不得加入公开支持面。
