# P3.2：Service Binding

状态：**implemented（2026-08-30）**。本地核心和最终 Gate 完成；Cloudflare direct differential 见
[P3 资格](../acceptance/p3-assets-service-bindings-acceptance.md)。

## 用户结果

- Worker 可以通过声明的 Service Binding 调用同账户 Worker 或自己，支持默认／命名 fetch 与原生 RPC。
- Deploy 时把目标名称解析为 Worker ID；每次新调用解析目标当前 active deployment 并固定本次调用。
- Request／Response、stream、WebSocket、callback 和 `RpcTarget` 保持 stock workerd 原生语义，不转换成自定义 HTTP JSON RPC。
- 目标使用自己的 env 和资源权限；caller 不会继承目标的 secret、内部 token 或控制面能力。
- 调用根共享深度 16、总调用 128、并发 32 和 30 秒 deadline；预算与身份不信任业务 header。
- Stream、WebSocket、`waitUntil` 和 capability 在实际 drain／close／dispose 或 workerd generation 退出前持有 deployment pin。
- 删除、generation 更换和进程崩溃会 fence 旧调用；无法证明结束时宁可保持 busy，也不用 TTL 猜测释放。

实现只使用现有 control authority、private loopback transport 和一个 Invocation Registry，不增加服务注册中心、额外网关或数据库。

## 历史验证与限制

`p3-services` hard／product／events／recovery、静态检查和 coverage 成功；当日完整历史 Gate 共 834/834 cases，
Rust line coverage 为 90.11%。事件源覆盖 Queue、Cron、Durable Object 和 Workflow；SIGKILL Gate 验证旧 handle／pin 清理。

完整 hosted Service fetch／RPC differential 尚未执行；固定 vinext qualification 明确未覆盖产品 Service Binding 组合。
