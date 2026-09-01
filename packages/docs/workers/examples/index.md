# 示例

下面几段在本机 workerd 上能跑。不要把 Cloudflare 的 [geolocation / country-code](https://developers.cloudflare.com/workers/examples/geolocation-hello-world/) 示例当成全球 Anycast：这里没有 `request.cf.country` 的边缘地理产品。

## Hello JSON

与 [上手](/workers/get-started/) 同一份 hello-worker。

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

KV API 与 Cloudflare 相同，见 [KV](/kv/)。权威是单节点 SQLite（`OC-KV-001`），不是全球复制。

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

`id` 是平台上已存在的 KV namespace，不是名字占位。

## Cron `scheduled` handler

Cron 产品见 [Cron Triggers](/workers/configuration/cron-triggers)。handler 与 [Cloudflare scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/) 相同。

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

表达式是 UTC 五字段，加上已文档化的本机 Quartz-like 扩展。恢复行为见 `OC-CRON-001`。平台部署元数据字段是 `crons`（字符串数组）。`open-compute.json` 没有 Wrangler 的 `triggers` 键，写进去会当未知字段拒绝。

## Service Binding `fetch`

同账户 Worker 之间的 fetch / RPC，见 [bindings](/workers/runtime-apis/bindings)。

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

目标 Worker 名在部署时解析并冻结成目标 ID。没有跨地域发现（`OC-SERVICE-001`）。

## 与 Cloudflare 相同

模块 handler、KV `get`/`put`、`scheduled`、Service Binding `fetch`。

## 故意不同

没有 geolocation 示例当全球产品；没有 `workers.dev`；所有状态落在这一台机器。更多 Cloudflare 示例里依赖边缘、Analytics Engine、AI、Turnstile 的，不要原样当本平台能力。
