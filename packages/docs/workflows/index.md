# Workflows

Workflows 是可从中断处恢复的多步工作流。已完成步骤的结果会持久化；进程退出后可以从检查点继续。步骤状态存储在本机 SQLite。

例如：

- 带持久化步骤的多步流程
- 休眠与等待事件
- 中断后从检查点恢复

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

在 `open-compute.json` 中绑定。Workflow 必须指定 `className`：

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  }
}
```

可选 `schedules`（字符串数组）。语法见 [绑定](/workers/configuration/bindings)。CLI：`oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 绑定 / 实例 API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | 相同：`create` / `get` / `createBatch` / `deleteBatch`、`step.do` / sleep / event、status / pause / resume / terminate / restart |
| 执行位置 | 可跨地区 | 本机 SQLite |
| 步骤回调 | — | 结果提交前可能重复执行；已持久化的步骤在重放时跳过 |
| 外部副作用 | — | 不随 Workflow 快照回滚 |
| 控制台 / 可观测性 | 提供 | 不提供 |
| 绑定 | wrangler | `{ type, id, className }`，必须指定 `className` |

## 本节

- [上手](/workflows/get-started/)
- [概念](/workflows/concepts/)
- [指南](/workflows/guides/)
- [示例](/workflows/examples/)
- [限制](/workflows/platform/limits)
- [行为差异](/workflows/platform/deviations)
