# 示例

以下示例可在本机 workerd 上运行。Cloudflare 的 [geolocation / country-code](https://developers.cloudflare.com/workers/examples/geolocation-hello-world/) 示例依赖全球 Anycast 与 `request.cf.country`；本平台不提供该边缘地理产品。

## Hello JSON

与 [快速开始](/workers/get-started/) 使用同一份 hello-worker。

```ts
export default {
  fetch(request: Request, env: Env): Response {
    return Response.json({
      message: env.GREETING,
      pathname: new URL(request.url).pathname,
    });
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": { "GREETING": "Hello from TypeScript" }
}
```

## KV get / put

Worker 中的 KV API 与 [KV](/kv/) 对齐。数据位于本机一份 SQLite，不提供全球复制。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "PUT") {
      await env.KV.put("hello", await request.text());
      return new Response("ok");
    }
    return new Response((await env.KV.get("hello")) ?? "missing");
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "kv-demo",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-namespace-id>" }
  }
}
```

`id` 是平台上已存在的 KV namespace，不是名称占位符。

## Cron `scheduled` handler

Cron 产品见 [Cron Triggers](/workers/configuration/cron-triggers)。handler 与 [Cloudflare scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/) 对齐。

```ts
export default {
  fetch(): Response {
    return new Response("cron worker");
  },
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    console.log(controller.cron, controller.scheduledTime);
  },
} satisfies ExportedHandler<Env>;
```

表达式为 UTC 五字段，并支持已文档化的本机 Quartz-like 扩展。错过触发后，宽限时间内最多补最近一次。平台部署元数据字段为 `crons`（字符串数组）。`open-compute.json` 没有 Wrangler 的 `triggers` 键；写入该键将作为未知字段被拒绝。

## Service Binding `fetch`

同一平台账户内 Worker 之间的 fetch / RPC，见 [bindings](/workers/runtime-apis/bindings)。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return env.UPSTREAM.fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "front",
  "main": "src/index.ts",
  "services": [
    { "binding": "UPSTREAM", "service": "hello-typescript" }
  ]
}
```

目标 Worker 名在部署时解析并冻结为目标 ID。Service Bindings 仅限本平台，不提供跨地域发现。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 模块 handler、KV `get`/`put`、`scheduled`、Service Binding `fetch` | 是 | 是 |
| `request.cf.country` 边缘地理 / geolocation 示例 | 是 | 不提供 |
| workers.dev | 是 | 不提供 |
| Analytics Engine / Workers AI / Turnstile 示例 | 是 | 不提供 |
| 状态存储位置 | 全球复制产品 | 本机 |

下一步：[配置](/workers/configuration/)、[Runtime APIs](/workers/runtime-apis/)。
