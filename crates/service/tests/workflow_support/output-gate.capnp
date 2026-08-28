using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "probe", worker = (
      compatibilityDate = "2026-08-22",
      compatibilityFlags = ["rpc"],
      modules = [
        (name = "probe.js", esModule = embed "output-gate.js"),
        (name = "workflow-facade.js", esModule = embed "../../../../runtime/system-workers/workflows/facade.js"),
        (name = "json.js", esModule = embed "../../../../runtime/system-workers/workflows/json.js"),
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
