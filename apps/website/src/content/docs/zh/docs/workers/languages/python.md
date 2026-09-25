---
title: "Python"
description: "在 open-compute 中通过原生 Worker Loader child 运行 Python Worker。"
---

Cloudflare Python Workers 使用 `workers` SDK、`WorkerEntrypoint` class 和 `python_workers` compatibility flag。目前 Python Workers 仍为 beta。

open-compute 在认证日期 `2026-09-08` 下支持原生 [Worker Loader](/zh/docs/workers/runtime-apis/bindings/#dynamic-workers) child 中的 Python module。固定的 Pyodide bundle 内嵌在 `ocd` 中，生产启动不会下载 Python runtime。

## 配置 parent Worker

在 parent 项目的 `wrangler.jsonc` 中声明 Worker Loader binding：

```json
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "python-loader",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "worker_loaders": [{ "binding": "LOADER" }]
}
```

## 加载 Python child

Python child 使用 Cloudflare 的 `WorkerEntrypoint` 模型。parent 提供 Python module，并调用其默认 entrypoint：

```ts
interface Env {
  LOADER: WorkerLoader;
}

const pythonSource = `
from workers import WorkerEntrypoint, Response

class Default(WorkerEntrypoint):
    async def fetch(self, request):
        return Response(self.env.GREETING)
`;

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const child = env.LOADER.load({
      compatibilityDate: "2026-09-08",
      compatibilityFlags: ["python_workers"],
      mainModule: "main.py",
      globalOutbound: null,
      env: { GREETING: "Hello from Python!" },
      modules: {
        "main.py": { py: pythonSource },
      },
    });

    return child.getEntrypoint().fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

使用项目内经过认证的 Wrangler 部署 parent：

```sh
ocd wrangler deploy
```

目前 open-compute 的公开 upload contract 不支持通过 `pywrangler` 把 Python 文件直接作为普通 Worker 的 `main` 部署，请使用上面的 Worker Loader 路径。`env` 可直接传递 structured-clone 值和 Service Binding；KV、D1、R2 与 Queue 资源使用文档中的 [`open-compute:worker-loader` 转发 helper](/zh/docs/workers/runtime-apis/bindings/#dynamic-workers)。child 的 CPU、内存和 subrequest limit 会按本机配置上限校验。

Cloudflare 参考：[Python Workers](https://developers.cloudflare.com/workers/languages/python/)。
