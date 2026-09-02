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

在 `wrangler.jsonc` 中使用 Wrangler 标准 Workflow 字段绑定：

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "workflows": [
    { "binding": "FLOW", "name": "flow", "class_name": "MyWorkflow" }
  ]
}
```

语法见[绑定](/zh/workers/configuration/bindings)。固定 Wrangler 负责 Workflow definition 部署；官方 SDK 负责 instance 与 lifecycle 操作。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 绑定 / 实例 API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | 相同：`create` / `get` / `createBatch` / `deleteBatch`、`step.do` / sleep / event、status / pause / resume / terminate / restart |
| 执行位置 | 可跨地区 | 本机 SQLite |
| 步骤回调 | — | 结果提交前可能重复执行；已持久化的步骤在重放时跳过 |
| 外部副作用 | — | 不随 Workflow 快照回滚 |
| 控制台 / 可观测性 | 提供 | 不提供 |
| 绑定 | Wrangler | 标准 `workflows[].binding/name/class_name`，必须指定 `class_name` |

## 本节

- [上手](/zh/workflows/get-started/)
- [概念](/zh/workflows/concepts/)
- [指南](/zh/workflows/guides/)
- [示例](/zh/workflows/examples/)
- [限制](/zh/workflows/platform/limits)
- [行为差异](/zh/workflows/platform/deviations)
