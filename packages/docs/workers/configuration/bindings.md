# Bindings

`env` 上的资源。项目里用 `bindings` 对象、`services` 数组，以及 `assets` / `images` / `version_metadata`。

```json
{
  "name": "app",
  "main": "src/index.ts",
  "vars": { "LOG_LEVEL": "info" },
  "secrets": { "TOKEN": { "env": "MY_TOKEN" } },
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-id>" },
    "BUCKET": { "type": "r2_bucket", "id": "<r2-id>" },
    "DB": { "type": "d1_database", "id": "<d1-id>" },
    "COUNTER": { "type": "do_namespace", "id": "<do-id>", "className": "Counter" },
    "JOBS": { "type": "queue_producer", "id": "<queue-id>" },
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  },
  "services": [
    { "binding": "AUTH", "service": "auth-worker", "entrypoint": "AuthEntrypoint" }
  ],
  "images": { "binding": "IMAGES" },
  "version_metadata": { "binding": "VERSION", "tag": "release-1" }
}
```

`permissions` 可选 `{ "read": true, "write": false }`。Durable Object 与 Workflow 必须提供 `className`：只用于核对生成的 framework config 中 class 语义，不作为资源 ID 发给平台。Workflow 可选 `schedules` 字符串数组。

改完配置后重新生成类型：

```sh
bun run oc types --config open-compute.json
```

## 与 Cloudflare 相同

租户只看见声明过的名字。KV / R2 / D1 / DO / Queue / Workflow / Service / Assets / Images / Version Metadata 的 **Worker 侧 API** 跟 [Cloudflare bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) 同一套符号。运行时细节见 [Runtime APIs · bindings](/workers/runtime-apis/bindings)。

## 故意不同

`bindings` 是对象不是 Wrangler 的分产品顶层键（没有 `kv_namespaces` 数组那种完整 wrangler 形态）。Service Binding 是 `services: [{binding, service, entrypoint?}]`，部署时把同账户 Worker 名解析并冻结为目标 ID（`OC-SERVICE-001`）。没有 Workers AI、Vectorize、Hyperdrive、mTLS、Rate Limiting、Secrets Store、Analytics Engine、Browser Rendering。资源必须先在平台上存在；工具链不会因为写了配置就创建 KV。
