---
title: "Python"
description: "Run a Python Worker as a native Worker Loader child on open-compute."
---

Cloudflare Python Workers use the `workers` SDK, a `WorkerEntrypoint` class, and the `python_workers` compatibility flag. Python Workers are currently beta.

open-compute supports Python modules inside a native [Worker Loader](/docs/workers/runtime-apis/bindings/#dynamic-workers) child on the certified `2026-09-08` compatibility date. The pinned Pyodide bundle is embedded in `ocd`, so production startup does not download a Python runtime.

## Configure the parent Worker

Declare a Worker Loader binding in the parent project's `wrangler.jsonc`:

```json
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "python-loader",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "worker_loaders": [{ "binding": "LOADER" }]
}
```

## Load a Python child

The Python child follows Cloudflare's `WorkerEntrypoint` model. The parent supplies the Python module and invokes its default entrypoint:

```ts
interface Env {
  LOADER: WorkerLoader;
}

const pythonSource = `
from workers import WorkerEntrypoint, Response

class Default(WorkerEntrypoint):
    async def fetch(self, request):
        return Response("Hello from Python!")
`;

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const child = env.LOADER.load({
      compatibilityDate: "2026-09-08",
      compatibilityFlags: ["python_workers"],
      mainModule: "main.py",
      globalOutbound: null,
      modules: {
        "main.py": { py: pythonSource },
      },
    });

    return child.getEntrypoint().fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

Deploy the parent with the project-local certified Wrangler:

```sh
ocd wrangler deploy
```

Direct deployment of a Python file as an ordinary Worker's `main` through `pywrangler` is not currently part of open-compute's public upload contract. Use the Worker Loader path above. Explicit child `limits`, including an empty object, are also rejected until standard CPU, memory, and subrequest enforcement is available.

Cloudflare reference: [Python Workers](https://developers.cloudflare.com/workers/languages/python/).
