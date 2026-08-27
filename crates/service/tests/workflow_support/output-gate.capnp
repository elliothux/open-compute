using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "probe", worker = (
      compatibilityDate = "2026-08-22",
      compatibilityFlags = ["rpc"],
      modules = [
        (name = "probe.js", esModule = embed "output-gate.js"),
        (name = "workflow-facade.js", esModule = embed "../../../../runtime/system-workers/workflow-facade.js"),
        (name = "__open_compute_workflow_json__.js", esModule = embed "../../../../runtime/system-workers/workflow-json.js"),
      ],
      bindings = [
        (name = "PROBES", durableObjectNamespace = "Caller"),
        (name = "STORE", durableObjectNamespace = "Store"),
        (name = "BACKEND", service = (name = "probe", entrypoint = "Backend")),
      ],
      durableObjectNamespaces = [
        (className = "Caller", uniqueKey = "workflow-output-gate-caller"),
        (className = "Store", uniqueKey = "workflow-output-gate-store"),
      ],
      durableObjectStorage = (localDisk = "storage"),
    )),
    (name = "storage", disk = (path = ".", writable = true)),
  ],
);
