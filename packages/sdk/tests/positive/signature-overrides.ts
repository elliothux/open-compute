import { createOpenComputeClient } from "../../src/index.ts";

declare const file: File;

const client = createOpenComputeClient({
  apiToken: "token",
  baseURL: "https://compute.example/client/v4",
});

void client.workers.scripts.update("app", {
  account_id: "account",
  metadata: {
    main_module: "index.js",
    bindings: [{ type: "worker_loader", name: "LOADER" }],
  },
  files: [file],
});

void client.workers.scripts.versions.create("app", {
  account_id: "account",
  metadata: {
    main_module: "index.js",
    bindings: [{ type: "worker_loader", name: "LOADER" }],
  },
  files: [file],
});

void client.workers.assets.upload.create({
  account_id: "account",
  base64: true,
  body: { "index.html": file, "plain.txt": "dGV4dA==" },
});
