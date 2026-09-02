# 概念

Workers 在 isolate 中运行：V8 沙箱、按部署冻结的代码与 `env`，由本机一个 `workerd` 子进程承载。一个 `ocd` 对应一个 data-dir 和一个 workerd child。不可在同一 data-dir 上启动第二个 `ocd`。

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    ctx.waitUntil(Promise.resolve());
    return new Response(env.GREETING);
  },
} satisfies ExportedHandler<Env>;
```

`env` 只暴露部署中声明的 vars、secrets 和 bindings。租户无法访问 SQLite 路径、S3 凭据或其他账户的资源。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| isolate（而非容器） | 是，见 [How Workers works](https://developers.cloudflare.com/workers/reference/how-workers-works/) | 是 |
| 模块 Worker；binding 注入 `env`；handler 接收 `request` / `env` / `ctx` | 是 | 是 |
| compatibility date 决定 runtime 行为 | 是，见 [compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/) | 是；日期由平台 runtime lock 冻结 |
| 项目 JSON 中的 `compatibilityDate` / `compatibilityFlags` | 是 | 不允许；当前 `effective_compatibility_date` 为 `2026-08-30` |
| Cloudflare 边缘网络 / colo / Smart Placement | 是 | 不提供 |
| 出站网络 | Cloudflare 托管网络策略 | 租户通用出站共享 开源 workerd 的一份 `Network(allow = ["public"])`，见 [TCP sockets](/zh/workers/runtime-apis/tcp-sockets) |
| 请求级 CPU / subrequest 配额 | 是 | 不由 stock OSS workerd `LimitEnforcer` 执行；实时数值见 [限制](/zh/workers/platform/limits) |

下一步：[配置](/zh/workers/configuration/)、[Runtime APIs](/zh/workers/runtime-apis/)。
