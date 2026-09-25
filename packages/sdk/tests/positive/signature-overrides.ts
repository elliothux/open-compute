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
    bindings: [
      { type: "worker_loader", name: "LOADER" },
      {
        type: "service",
        name: "CATALOG",
        service: "catalog",
        props: { mode: "read", nested: [1, true] },
      },
      { type: "artifacts", name: "ARTIFACTS", namespace: "team" },
    ],
    migrations: {
      old_tag: "v0",
      new_tag: "v2",
      steps: [
        { new_classes: ["Counter"] },
        { renamed_classes: [{ from: "Counter", to: "Total" }] },
      ],
    },
  },
  files: [file],
});

void client.workers.scripts.versions.create("app", {
  account_id: "account",
  metadata: {
    main_module: "index.js",
    bindings: [
      { type: "worker_loader", name: "LOADER" },
      {
        type: "service",
        name: "CATALOG",
        service: "catalog",
        props: { mode: "read" },
      },
      { type: "artifacts", name: "ARTIFACTS", namespace: "team" },
    ],
  },
  files: [file],
});

void client.workers.scripts.scriptAndVersionSettings.edit("app", {
  account_id: "account",
  settings: { bindings: [{ type: "worker_loader", name: "LOADER" }] },
});

void client.workers.scripts.scriptAndVersionSettings.edit("app", {
  account_id: "account",
  settings: {
    bindings: [
      {
        type: "service",
        name: "TARGET",
        service: "worker-b",
        entrypoint: "NamedEntrypoint",
        props: { tenant: "example" },
      },
    ],
  },
});

void client.workers.assets.upload.create({
  account_id: "account",
  base64: true,
  body: { "index.html": file, "plain.txt": "dGV4dA==" },
});
