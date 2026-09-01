# Workflows

Workflows 是可重放的多步应用。执行权威是该节点上的本地 SQLite。

例如：

- 带耐久 step 的多步应用
- sleep 与等待事件
- 中断后重放

```ts
export class MyWorkflow extends WorkflowEntrypoint<Env, { hello: string }> {
  async run(event: WorkflowEvent<{ hello: string }>, step: WorkflowStep) {
    const first = await step.do("first", async () => {
      return { ok: true, hello: event.payload.hello };
    });
    return first;
  }
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const instance = await env.FLOW.create({ params: { hello: "world" } });
    return Response.json({ id: instance.id, status: await instance.status() });
  },
} satisfies ExportedHandler<{ FLOW: Workflow }>;
```

在 `open-compute.json` 中绑定。Workflow 必须提供 `className`：

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  }
}
```

可选 `schedules` 字符串数组。语法见 [bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding / instance API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | 相同：`create` / `get` / `createBatch` / `deleteBatch`、`step.do` / sleep / event、status / pause / resume / terminate / restart |
| 执行 | 跨地域 | 该节点上的本地 SQLite |
| Callback | — | 结果提交前 at-least-once；replay 跳过已耐久完成的 callback |
| 外部副作用 | — | 不随 Workflow snapshot 回滚 |
| Dashboard / observability | 提供 | 不提供 |
| Binding | wrangler | `{ type, id, className }`；`className` 必填 |

## 本节

- [上手](/workflows/get-started/)
- [概念](/workflows/concepts/)
- [指南](/workflows/guides/)
- [示例](/workflows/examples/)
- [限制](/workflows/platform/limits)
- [行为差异](/workflows/platform/deviations)
