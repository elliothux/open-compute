# Bindings

`env` 上的资源。在项目中用 `bindings` 对象、`services` 数组，以及 `assets` / `images` / `version_metadata` 声明。

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

`permissions` 可选 `{ "read": true, "write": false }`。Durable Object 与 Workflow 必须提供 `className`：只用于核对生成的 framework config 中的 class 语义，不作为资源 ID 发给平台。Workflow 可选 `schedules` 字符串数组。

更改配置后重新生成类型：

```sh
bun run oc types --config open-compute.json
```

运行时细节见 [Runtime APIs · bindings](/zh/workers/runtime-apis/bindings)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 租户只看见声明过的名字 | 是 | 是 |
| KV / R2 / D1 / DO / Queue / Workflow / Service / Assets / Images / Version Metadata 的 Worker 里用的 API | 是，见 [Cloudflare bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) | 同一套符号 |
| 配置形态 | Wrangler 分产品顶层数组（`kv_namespaces` 等） | `bindings` 对象，值为 `{type, id, permissions?}` |
| Service Bindings | 同账户 Worker | `services: [{binding, service, entrypoint?}]`；部署时解析同账户 Worker 名并冻结目标 ID；仅限本平台 |
| Workers AI、Vectorize、Hyperdrive、mTLS、Rate Limiting、Secrets Store、Analytics Engine、Browser Rendering | 是 | 不提供 |
| 写配置即创建资源 | Wrangler 可创建部分资源 | 资源必须已在平台上存在 |

