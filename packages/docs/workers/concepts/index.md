# 概念

Workers 在 isolate 里跑：V8 沙箱、按部署冻结的代码和 `env`，由本机这一个 `workerd` 子进程承载。一个 `ocd` 管一个 data-dir、一个 workerd child。不要在同一 data-dir 上起第二个 `ocd`。

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    ctx.waitUntil(Promise.resolve());
    return new Response(env.GREETING);
  },
} satisfies ExportedHandler<Env>;
```

`env` 只暴露部署里声明过的 vars、secrets 和 bindings。租户拿不到 SQLite 路径、S3 凭据或别人的资源。

## 与 Cloudflare 相同

[How Workers works](https://developers.cloudflare.com/workers/reference/how-workers-works/)：isolate 而不是容器；模块 Worker；binding 注入 `env`；handler 收到 `request` / `env` / `ctx`。兼容性日期决定 runtime 行为——见 Cloudflare [compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/)。

## 故意不同

compatibility date 由平台 runtime lock 冻结，当前 `effective_compatibility_date` 为 `2026-08-30`。项目 JSON 不能设 `compatibilityDate` 或 `compatibilityFlags`。没有全球边缘、没有 colo、没有 Smart Placement。出网是 stock workerd 的一份 `Network(allow = ["public"])`，见 [`OC-WKR-TCP-001`](/workers/runtime-apis/tcp-sockets)。CPU / subrequest 配额见 [`OC-WKR-LIMIT-001`](/workers/platform/limits)。

下一步：[配置](/workers/configuration/)、[Runtime APIs](/workers/runtime-apis/)。
